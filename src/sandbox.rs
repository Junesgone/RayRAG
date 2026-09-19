//! 本地代码沙箱 — RAGFlow `agent/sandbox/providers/local.py` 的 Rust 实现
//!
//! 子进程隔离执行（对齐上游 LocalProvider）：
//!   - Python/JS wrapper：`main(**args)` → `__RAGFLOW_RESULT__:` base64 JSON 结构化结果
//!   - 临时实例目录作 cwd + env 隔离（HOME/TMPDIR/MPLBACKEND/PYTHONUNBUFFERED）
//!   - 超时 kill（进程组）+ 输出大小上限 + 最大内存/CPU 软限制（ulimit 命令）
//!   - 结构化结果提取：从 stdout 移除标记行并解析（对齐 extract_structured_result）
//!
//! 远程契约层（executor_manager 对齐，全部只新增、不改动既有 pub API）：
//!   - 枚举：SupportLanguage / ResultStatus / ResourceLimitType /
//!     UnauthorizedAccessType / RuntimeErrorType（对齐 models/enums.py）
//!   - 失败协议：负值退出码约定（exit_codes）、classify_execution_failure /
//!     classify_stderr_failure（对齐 execution.py analyze_error_result）、
//!     安全分析 analyze_code_security（对齐 handlers.py 的 -999 分支）
//!   - 运行时环境约定：容器池命名 / 基础镜像 / 容器创建参数（gVisor runsc、
//!     tmpfs、--user nobody、--memory、seccomp）/ 执行命令（docker exec …
//!     timeout N python -I -B runner.py）/ 超时字符串解析 / 执行包布局
//!     （main.py|js + runner.py|js + args.json + artifacts/）
//!   - artifact 收集契约：允许扩展名 → MIME、数量/大小上限、文件名净化

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
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
            python_bin: std::env::var("RAYRAG_SANDBOX_PYTHON")
                .unwrap_or_else(|_| "python3".to_string()),
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

        let started = wrapped
            .spawn()
            .map_err(|error| anyhow::anyhow!("Spawn failed: {error}"))?;
        let timeout = Duration::from_secs(self.limits.timeout_seconds);
        let output = tokio::time::timeout(timeout, started.wait_with_output())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Execution timed out after {} seconds",
                    self.limits.timeout_seconds
                )
            })?
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

// ═══════════════════════════════════════════════════════════════════════════
// 契约层（新增，对齐 RAGFlow agent/sandbox 远程契约；不改动既有 pub API）
// ═══════════════════════════════════════════════════════════════════════════

// ─────────────────────────── 枚举（对齐 models/enums.py） ───────────────────────────

/// 支持的沙箱语言（对齐 `SupportLanguage`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportLanguage {
    Python,
    Nodejs,
}

impl SupportLanguage {
    /// 上游字符串值 `"python"`。
    pub const PYTHON: &'static str = "python";
    /// 上游字符串值 `"nodejs"`。
    pub const NODEJS: &'static str = "nodejs";

    /// 规范字符串值（对齐 SupportLanguage 枚举值）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Python => Self::PYTHON,
            Self::Nodejs => Self::NODEJS,
        }
    }

    /// 解析语言标识（容忍 python3/py/javascript/js/node 别名，对齐
    /// `SelfManagedProvider._normalize_language` 与 `LocalProvider._normalize_language`）。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "python" | "python3" | "py" | "python3.11" | "python3.12" => Some(Self::Python),
            "nodejs" | "javascript" | "js" | "node" => Some(Self::Nodejs),
            _ => None,
        }
    }
}

/// 执行结果状态（对齐 `ResultStatus`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultStatus {
    Success,
    ProgramError,
    ResourceLimitExceeded,
    UnauthorizedAccess,
    RuntimeError,
    ProgramRunnerError,
}

impl ResultStatus {
    /// 规范字符串值（对齐 ResultStatus 枚举值）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::ProgramError => "program_error",
            Self::ResourceLimitExceeded => "resource_limit_exceeded",
            Self::UnauthorizedAccess => "unauthorized_access",
            Self::RuntimeError => "runtime_error",
            Self::ProgramRunnerError => "program_runner_error",
        }
    }

    /// 从上游字符串值解析（大小写不敏感）。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "success" => Some(Self::Success),
            "program_error" => Some(Self::ProgramError),
            "resource_limit_exceeded" => Some(Self::ResourceLimitExceeded),
            "unauthorized_access" => Some(Self::UnauthorizedAccess),
            "runtime_error" => Some(Self::RuntimeError),
            "program_runner_error" => Some(Self::ProgramRunnerError),
            _ => None,
        }
    }
}

/// 资源限制类型（对齐 `ResourceLimitType`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceLimitType {
    Time,
    Memory,
    Output,
}

impl ResourceLimitType {
    /// 规范字符串值（对齐 ResourceLimitType 枚举值）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Memory => "memory",
            Self::Output => "output",
        }
    }
}

/// 未授权访问类型（对齐 `UnauthorizedAccessType`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnauthorizedAccessType {
    DisallowedSyscall,
    FileAccess,
    NetworkAccess,
}

impl UnauthorizedAccessType {
    /// 规范字符串值（对齐 UnauthorizedAccessType 枚举值）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DisallowedSyscall => "disallowed_syscall",
            Self::FileAccess => "file_access",
            Self::NetworkAccess => "network_access",
        }
    }
}

/// 运行时错误类型（对齐 `RuntimeErrorType`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeErrorType {
    Signalled,
    NonzeroExit,
}

impl RuntimeErrorType {
    /// 规范字符串值（对齐 RuntimeErrorType 枚举值）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Signalled => "signalled",
            Self::NonzeroExit => "nonzero_exit",
        }
    }
}

// ─────────────────────────── 失败协议（退出码 + stderr 分类） ───────────────────────────

/// 退出码约定（对齐 executor_manager：0 成功；负值编码失败类别）。
///
/// 注：上游 result_protocol.py 未定义 `FAILED` / `SANDBOX_RESULT_FILE` 常量；
/// 实际的失败协议是 `ResultStatus` 枚举 + 负值退出码 + stderr 关键字分类
/// （execution.py / handlers.py / limiter.py）。
pub mod exit_codes {
    /// 成功（returncode == 0）。
    pub const SUCCESS: i64 = 0;
    /// 代码未通过安全检查（handlers.py：status=program_runner_error,
    /// detail="Code is unsafe", stderr="Line {lineno}: {issue}"）。
    pub const UNSAFE_CODE: i64 = -999;
    /// 请求限流（limiter.py：5/second，status=program_runner_error,
    /// detail=stderr="Too many requests, please try again later"）。
    pub const RATE_LIMITED: i64 = -429;
    /// GNU `timeout` 触发（容器内 `timeout N python -I -B runner.py` 返回 124；
    /// 结果侧记为 -124，status=resource_limit_exceeded, type=time）。
    pub const TIMEOUT: i64 = -124;
    /// OOM kill（容器被 --memory 限制杀死，docker 返回 137；结果侧记为 -137，
    /// status=resource_limit_exceeded, type=memory）。
    pub const OUT_OF_MEMORY: i64 = -137;
    /// 容器池繁忙（execution.py：status=program_runner_error,
    /// stderr="Container pool is busy", detail="no_available_container"）。
    pub const POOL_BUSY: i64 = -10;
    /// 内部异常（execution.py：status=program_runner_error, detail="internal_error"）。
    pub const INTERNAL_ERROR: i64 = -3;
    /// 异步超时 kill 后（execution.py TimeoutError 分支：pkill -9 后
    /// status=resource_limit_exceeded, type=time）。
    pub const TIMEOUT_AFTER_KILL: i64 = -1;
}

/// 失败分类结果（对齐 CodeExecutionResult 的错误字段子集）。
#[derive(Debug, Clone, PartialEq)]
pub struct FailureClassification {
    pub status: ResultStatus,
    pub resource_limit_type: Option<ResourceLimitType>,
    pub unauthorized_access_type: Option<UnauthorizedAccessType>,
    pub runtime_error_type: Option<RuntimeErrorType>,
}

impl Default for FailureClassification {
    fn default() -> Self {
        Self {
            status: ResultStatus::ProgramError,
            resource_limit_type: None,
            unauthorized_access_type: None,
            runtime_error_type: None,
        }
    }
}

/// 按退出码 + stderr 分类执行失败（对齐 execution.py execute_code 的非零分支 +
/// analyze_error_result）。
///
/// 上游映射：
///   - returncode 124 → resource_limit_exceeded / time（exit_code 记为 -124）
///   - returncode 137 → resource_limit_exceeded / memory（exit_code 记为 -137）
///   - stderr 含 "Permission denied"      → unauthorized_access / file_access
///   - stderr 含 "Operation not permitted"→ unauthorized_access / disallowed_syscall
///   - stderr 含 "MemoryError"            → resource_limit_exceeded / memory
///   - 其他                               → program_error / nonzero_exit
pub fn classify_execution_failure(exit_code: i64, stderr: &str) -> FailureClassification {
    match exit_code {
        124 => FailureClassification {
            status: ResultStatus::ResourceLimitExceeded,
            resource_limit_type: Some(ResourceLimitType::Time),
            ..Default::default()
        },
        137 => FailureClassification {
            status: ResultStatus::ResourceLimitExceeded,
            resource_limit_type: Some(ResourceLimitType::Memory),
            ..Default::default()
        },
        _ => classify_stderr_failure(stderr),
    }
}

/// 按 stderr 关键字分类（对齐 analyze_error_result 的字符串分支）。
pub fn classify_stderr_failure(stderr: &str) -> FailureClassification {
    if stderr.contains("Permission denied") {
        FailureClassification {
            status: ResultStatus::UnauthorizedAccess,
            unauthorized_access_type: Some(UnauthorizedAccessType::FileAccess),
            ..Default::default()
        }
    } else if stderr.contains("Operation not permitted") {
        FailureClassification {
            status: ResultStatus::UnauthorizedAccess,
            unauthorized_access_type: Some(UnauthorizedAccessType::DisallowedSyscall),
            ..Default::default()
        }
    } else if stderr.contains("MemoryError") {
        FailureClassification {
            status: ResultStatus::ResourceLimitExceeded,
            resource_limit_type: Some(ResourceLimitType::Memory),
            ..Default::default()
        }
    } else {
        FailureClassification {
            status: ResultStatus::ProgramError,
            runtime_error_type: Some(RuntimeErrorType::NonzeroExit),
            ..Default::default()
        }
    }
}

/// 安全分析 issue 列表渲染（对齐 handlers.py：`"\n".join([f"Line {lineno}: {issue}" ...])`）。
pub fn format_security_issues(issues: &[(String, i64)]) -> String {
    issues
        .iter()
        .map(|(issue, lineno)| format!("Line {lineno}: {issue}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ─────────────────────────── 运行时环境约定（providers/ + executor_manager 镜像语义） ───────────────────────────

/// 默认执行超时秒数（对齐 config.py `SANDBOX_TIMEOUT` 默认 "10s"）。
pub const DEFAULT_SANDBOX_TIMEOUT_SECONDS: u64 = 10;
/// 默认容器内存上限（对齐 container.py 默认 "256m"）。
pub const DEFAULT_SANDBOX_MAX_MEMORY: &str = "256m";
/// gVisor 运行时（对齐 container.py `--runtime=runsc`）。
pub const SANDBOX_CONTAINER_RUNTIME: &str = "runsc";
/// 容器内工作区 tmpfs（对齐 container.py `/workspace:rw,exec,size=100M,uid=65534,gid=65534`）。
pub const SANDBOX_WORKSPACE_TMPFS: &str = "/workspace:rw,exec,size=100M,uid=65534,gid=65534";
/// 容器内 /tmp tmpfs（对齐 container.py `/tmp:rw,exec,size=50M`）。
pub const SANDBOX_TMP_TMPFS: &str = "/tmp:rw,exec,size=50M";
/// 容器运行用户（对齐 container.py `--user nobody`）。
pub const SANDBOX_CONTAINER_USER: &str = "nobody";
/// Python 基础镜像默认名（对齐 `SANDBOX_BASE_PYTHON_IMAGE` 默认值）。
pub const DEFAULT_PYTHON_SANDBOX_IMAGE: &str = "sandbox-base-python:latest";
/// Node.js 基础镜像默认名（对齐 `SANDBOX_BASE_NODEJS_IMAGE` 默认值）。
pub const DEFAULT_NODEJS_SANDBOX_IMAGE: &str = "sandbox-base-nodejs:latest";
/// 结构化结果标记前缀（对齐 RESULT_MARKER_PREFIX，与既有常量一致）。
const RESULT_MARKER: &str = "__RAGFLOW_RESULT__:";

/// 解析超时字符串为秒（对齐 util.py parse_timeout_duration）。
///
/// 支持 "90s"、"2m"、"1m30s"（s/m 大小写不敏感）；非法或 0 → 返回默认值。
pub fn parse_timeout_duration(timeout: &str, default_seconds: u64) -> u64 {
    let trimmed = timeout.trim().to_lowercase();
    let pattern = regex::Regex::new(r"^(?:(\d+)m)?(?:(\d+)s)?$").expect("static regex");
    let captures = match pattern.captures(&trimmed) {
        Some(captures) if captures.get(1).is_some() || captures.get(2).is_some() => captures,
        _ => return default_seconds,
    };
    let minutes: u64 = captures
        .get(1)
        .and_then(|group| group.as_str().parse().ok())
        .unwrap_or(0);
    let seconds: u64 = captures
        .get(2)
        .and_then(|group| group.as_str().parse().ok())
        .unwrap_or(0);
    let total = minutes * 60 + seconds;
    if total > 0 { total } else { default_seconds }
}

/// 格式化秒数为上游风格字符串（对齐 util.py format_timeout_duration）。
pub fn format_timeout_duration(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    let remaining = seconds % 60;
    if remaining == 0 {
        format!("{minutes}m")
    } else {
        format!("{minutes}m{remaining}s")
    }
}

/// 校验 Docker 内存限制字符串（对齐 util.py is_valid_memory_limit：`[1-9]\d*(b|k|m|g)`）。
pub fn is_valid_memory_limit(memory: &str) -> bool {
    let pattern = regex::Regex::new(r"^[1-9]\d*(b|k|m|g)$").expect("static regex");
    pattern.is_match(memory.trim().to_lowercase().as_str())
}

/// 容器池命名（对齐 container.py：`sandbox_python_{i}` / `sandbox_nodejs_{i}`）。
pub fn container_pool_name(language: &str, index: usize) -> String {
    let normalized = SupportLanguage::parse(language)
        .map(|lang| lang.as_str())
        .unwrap_or(language);
    format!("sandbox_{normalized}_{index}")
}

/// 语言 → 基础镜像（对齐 container.py：SANDBOX_BASE_PYTHON_IMAGE /
/// SANDBOX_BASE_NODEJS_IMAGE 环境变量覆盖，默认 sandbox-base-{lang}:latest）。
pub fn sandbox_base_image_from_env(language: &str) -> String {
    match SupportLanguage::parse(language) {
        Some(SupportLanguage::Python) => std::env::var("SANDBOX_BASE_PYTHON_IMAGE")
            .unwrap_or_else(|_| DEFAULT_PYTHON_SANDBOX_IMAGE.to_string()),
        Some(SupportLanguage::Nodejs) => std::env::var("SANDBOX_BASE_NODEJS_IMAGE")
            .unwrap_or_else(|_| DEFAULT_NODEJS_SANDBOX_IMAGE.to_string()),
        None => format!("sandbox-base-{language}:latest"),
    }
}

/// 构建容器创建参数（对齐 container.py create_container）。
///
/// 语义：gVisor runsc + 只读根文件系统 + /workspace 与 /tmp tmpfs +
/// `--user nobody` + `--workdir /workspace` + 内存上限（默认 256m，可用
/// SANDBOX_MAX_MEMORY 覆盖）+ 可选 seccomp profile。
pub fn build_container_create_args(
    name: &str,
    _language: &str,
    image: &str,
    max_memory: Option<&str>,
    enable_seccomp: bool,
) -> Vec<String> {
    let mut args = vec![
        "docker".to_string(),
        "run".to_string(),
        "-d".to_string(),
        format!("--runtime={SANDBOX_CONTAINER_RUNTIME}"),
        "--name".to_string(),
        name.to_string(),
        "--read-only".to_string(),
        "--tmpfs".to_string(),
        SANDBOX_WORKSPACE_TMPFS.to_string(),
        "--tmpfs".to_string(),
        SANDBOX_TMP_TMPFS.to_string(),
        "--user".to_string(),
        SANDBOX_CONTAINER_USER.to_string(),
        "--workdir".to_string(),
        "/workspace".to_string(),
    ];
    let memory = max_memory
        .filter(|value| is_valid_memory_limit(value))
        .unwrap_or(DEFAULT_SANDBOX_MAX_MEMORY);
    args.extend(["--memory".to_string(), memory.to_string()]);
    if enable_seccomp {
        args.extend([
            "--security-opt".to_string(),
            "seccomp=/app/seccomp-profile-default.json".to_string(),
        ]);
    }
    args.push(image.to_string());
    args
}

/// 构建容器内执行命令（对齐 execution.py _build_container_run_args）。
///
/// python：`docker exec --workdir /workspace/{task_id} {container} timeout {N} python -I -B {runner}`
/// nodejs：同上但无 `-I -B`。
pub fn build_container_run_args(
    language: &str,
    task_id: &str,
    container: &str,
    timeout_seconds: u64,
    runner: &str,
) -> Vec<String> {
    let mut args = vec![
        "docker".to_string(),
        "exec".to_string(),
        "--workdir".to_string(),
        format!("/workspace/{task_id}"),
        container.to_string(),
        "timeout".to_string(),
        timeout_seconds.to_string(),
        SupportLanguage::parse(language)
            .map(|lang| lang.as_str())
            .unwrap_or(language)
            .to_string(),
    ];
    if SupportLanguage::parse(language) == Some(SupportLanguage::Python) {
        args.extend(["-I".to_string(), "-B".to_string()]);
    }
    args.push(runner.to_string());
    args
}

/// Node.js 兼容导出追加（对齐 handlers.py：NODEJS 时 `code += "\n\nmodule.exports = { main };"`）。
pub fn append_nodejs_module_export(code: &str) -> String {
    format!("{code}\n\nmodule.exports = {{ main }};")
}

/// 执行包（对齐 execution.py _build_execution_bundle 产物）。
#[derive(Debug, Clone)]
pub struct ExecutionBundle {
    pub code_name: String,
    pub code_bytes: Vec<u8>,
    pub runner_name: String,
    pub runner_source: String,
    pub args_name: String,
    pub args_source: String,
}

/// 构建执行包（对齐 _build_execution_bundle：main.py|js + runner.py|js + args.json）。
pub fn build_execution_bundle(
    language: &str,
    code_bytes: &[u8],
    arguments: &Map<String, Value>,
) -> Result<ExecutionBundle> {
    let args_source = serde_json::to_string(arguments)?;
    match SupportLanguage::parse(language) {
        Some(SupportLanguage::Python) => Ok(ExecutionBundle {
            code_name: "main.py".to_string(),
            code_bytes: code_bytes.to_vec(),
            runner_name: "runner.py".to_string(),
            runner_source: PYTHON_RUNNER_SOURCE.to_string(),
            args_name: "args.json".to_string(),
            args_source,
        }),
        Some(SupportLanguage::Nodejs) => Ok(ExecutionBundle {
            code_name: "main.js".to_string(),
            code_bytes: code_bytes.to_vec(),
            runner_name: "runner.js".to_string(),
            runner_source: NODEJS_RUNNER_SOURCE.to_string(),
            args_name: "args.json".to_string(),
            args_source,
        }),
        None => bail!("Unsupported language for sandbox execution: {language}"),
    }
}

/// Python runner 模板（对齐 execution.py 的 runner_source；`-I` 隔离模式不加载
/// 脚本目录，故先 `sys.path.insert(0, dirname(__file__))` 再 `from main import main`）。
const PYTHON_RUNNER_SOURCE: &str = r#"import base64
import json
import os
import sys

os.makedirs(os.path.join(os.getcwd(), "artifacts"), exist_ok=True)

sys.path.insert(0, os.path.dirname(__file__))
from main import main

RESULT_MARKER_PREFIX = "__RAGFLOW_RESULT__:"


def emit_result(value):
    payload = json.dumps(
        {
            "present": True,
            "value": value,
            "type": "json",
        },
        ensure_ascii=False,
        separators=(",", ":"),
    )
    print(RESULT_MARKER_PREFIX + base64.b64encode(payload.encode("utf-8")).decode("ascii"))


if __name__ == "__main__":
    with open(os.path.join(os.path.dirname(__file__), "args.json"), encoding="utf-8") as f:
        args = json.load(f)
    result = main(**args)
    emit_result(result)
"#;

/// Node.js runner 模板（对齐 execution.py 的 runner.js：读 args.json、require main.js、
/// 支持 promise 结果，标记行输出结构化结果）。
const NODEJS_RUNNER_SOURCE: &str = r#"const fs = require('fs');
const path = require('path');

const args = JSON.parse(fs.readFileSync(path.join(__dirname, 'args.json'), 'utf8'));
const mainPath = path.join(__dirname, 'main.js');
const RESULT_MARKER_PREFIX = '__RAGFLOW_RESULT__:';

function isPromise(value) {
    return Boolean(value && typeof value.then === 'function');
}

function emitResult(value) {
    if (typeof value === 'undefined') {
        console.error('Error: main() must return a value. Use null for an empty result.');
        process.exit(1);
    }

    const payload = JSON.stringify({ present: true, value, type: 'json' });
    if (typeof payload === 'undefined') {
        console.error('Error: main() returned a non-JSON-serializable value.');
        process.exit(1);
    }

    console.log(RESULT_MARKER_PREFIX + Buffer.from(payload, 'utf8').toString('base64'));
}

if (fs.existsSync(mainPath)) {
    const mod = require(mainPath);
    const main = typeof mod === 'function' ? mod : mod.main;

    if (typeof main !== 'function') {
        console.error('Error: main is not a function');
        process.exit(1);
    }

    if (typeof args === 'object' && args !== null) {
        try {
            const result = Promise.resolve(main(args));
            if (isPromise(result)) {
                result.then(output => {
                    emitResult(output);
                }).catch(err => {
                    console.error('Error in async main function:', err);
                    process.exit(1);
                });
            } else {
                emitResult(result);
            }
        } catch (err) {
            console.error('Error when executing main:', err);
            process.exit(1);
        }
    } else {
        console.error('Error: args is not a valid object:', args);
        process.exit(1);
    }
} else {
    console.error('main.js not found in the current directory');
    process.exit(1);
}
"#;

// ─────────────────────────── artifact 收集契约 ───────────────────────────

/// 允许的 artifact 扩展名 → MIME 类型（对齐 execution.py ALLOWED_ARTIFACT_EXTENSIONS）。
pub const ALLOWED_ARTIFACT_EXTENSIONS: &[(&str, &str)] = &[
    (".png", "image/png"),
    (".jpg", "image/jpeg"),
    (".jpeg", "image/jpeg"),
    (".svg", "image/svg+xml"),
    (".pdf", "application/pdf"),
    (".csv", "text/csv"),
    (".json", "application/json"),
    (".html", "text/html"),
];

/// 单次执行最大 artifact 数量（对齐 MAX_ARTIFACT_COUNT = 10）。
pub const MAX_ARTIFACT_COUNT: usize = 10;
/// 单个 artifact 最大字节数（对齐 MAX_ARTIFACT_SIZE = 10MB）。
pub const MAX_ARTIFACT_SIZE_BYTES: u64 = 10 * 1024 * 1024;

/// artifact 条目（对齐 schemas.py ArtifactItem）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactItem {
    pub name: String,
    pub mime_type: String,
    pub size: u64,
    pub content_b64: String,
}

/// 扩展名 → MIME 类型（小写扩展名匹配，对齐 ALLOWED_ARTIFACT_EXTENSIONS）。
pub fn mime_type_for_artifact(name: &str) -> Option<&'static str> {
    let ext = name
        .rsplit('.')
        .next()
        .map(|ext| format!(".{}", ext.to_lowercase()))
        .unwrap_or_default();
    ALLOWED_ARTIFACT_EXTENSIONS
        .iter()
        .find(|(allowed, _)| *allowed == ext)
        .map(|(_, mime)| *mime)
}

/// artifact 文件名净化校验（对齐 execution.py：拒绝 `/`、`\`、`..`、点开头）。
pub fn artifact_name_is_safe(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && !name.starts_with('.')
}

// ─────────────────────────── 安全分析（对齐 handlers.py / services/security.py） ───────────────────────────

/// Python 危险导入根模块（对齐 SecurePythonAnalyzer.DANGEROUS_IMPORTS）。
pub const DANGEROUS_PYTHON_IMPORTS: &[&str] = &[
    "os",
    "subprocess",
    "sys",
    "shutil",
    "socket",
    "ctypes",
    "pickle",
    "threading",
    "multiprocessing",
    "asyncio",
    "http.client",
    "ftplib",
    "telnetlib",
    "builtins",
];

/// Python 危险调用名（对齐 SecurePythonAnalyzer.DANGEROUS_CALLS 核心集合）。
pub const DANGEROUS_PYTHON_CALLS: &[&str] = &[
    "eval",
    "exec",
    "open",
    "__import__",
    "compile",
    "input",
    "system",
    "popen",
    "remove",
    "rename",
    "rmdir",
    "chdir",
    "chmod",
    "chown",
    "getattr",
    "setattr",
    "globals",
    "locals",
];

/// Node.js 危险模式（对齐 SecureJavaScriptAnalyzer.DANGEROUS_PATTERNS）。
pub const DANGEROUS_NODEJS_PATTERNS: &[(&str, &str)] = &[
    (
        r#"require\s*\(\s*['"]child_process['"]\s*\)"#,
        "Require: child_process",
    ),
    (r#"require\s*\(\s*['"]fs['"]\s*\)"#, "Require: fs"),
    (
        r#"require\s*\(\s*['"]worker_threads['"]\s*\)"#,
        "Require: worker_threads",
    ),
    (r#"\beval\s*\("#, "Call: eval"),
    (r#"\bFunction\s*\("#, "Call: Function"),
    (r#"\bprocess\s*\.\s*binding\s*\("#, "Call: process.binding"),
];

/// 代码安全检查（对齐 analyze_code_security）。
///
/// 返回 `(is_safe, issues)`，issues 为 `(描述, 行号)` 列表。行级扫描启发式镜像
/// 上游 AST / 正则分析：Python 覆盖危险 import / from-import / 裸调用 /
/// 危险模块属性访问；Node.js 覆盖 require(child_process|fs|worker_threads)、
/// eval(、Function(、process.binding(。重复的 (描述, 行号) 会去重。
pub fn analyze_code_security(code: &str, language: &str) -> (bool, Vec<(String, i64)>) {
    let issues = match SupportLanguage::parse(language) {
        Some(SupportLanguage::Python) => analyze_python_security(code),
        Some(SupportLanguage::Nodejs) => analyze_nodejs_security(code),
        None => vec![(
            format!("Unsupported language for security analysis: {language}"),
            -1,
        )],
    };
    (issues.is_empty(), issues)
}

/// Python 行级安全扫描（启发式，对齐 SecurePythonAnalyzer 的 issue 文案）。
fn analyze_python_security(code: &str) -> Vec<(String, i64)> {
    let import_pattern = regex::Regex::new(
        r"(?m)^\s*(?:import\s+([A-Za-z_][A-Za-z0-9_.]*)|from\s+([A-Za-z_][A-Za-z0-9_.]*)\s+import)",
    )
    .expect("static regex");
    let call_alternation = DANGEROUS_PYTHON_CALLS.join("|");
    let call_pattern =
        regex::Regex::new(&format!(r"\b(?:{call_alternation})\s*\(")).expect("static regex");
    let import_alternation = DANGEROUS_PYTHON_IMPORTS.join("|");
    let attribute_pattern = regex::Regex::new(&format!(
        r"\b(?:{import_alternation})\.[A-Za-z_][A-Za-z0-9_]*"
    ))
    .expect("static regex");

    let mut issues: Vec<(String, i64)> = Vec::new();
    let record = |description: String, line: i64, issues: &mut Vec<(String, i64)>| {
        if !issues
            .iter()
            .any(|(existing, lineno)| *existing == description && *lineno == line)
        {
            issues.push((description, line));
        }
    };

    for captures in import_pattern.captures_iter(code) {
        if let Some(module) = captures.get(1) {
            let root = module.as_str().split('.').next().unwrap_or("");
            if DANGEROUS_PYTHON_IMPORTS.contains(&root) {
                let line = code[..module.start()].matches('\n').count() as i64 + 1;
                record(format!("Import: {}", module.as_str()), line, &mut issues);
            }
        }
        if let Some(module) = captures.get(2) {
            let root = module.as_str().split('.').next().unwrap_or("");
            if DANGEROUS_PYTHON_IMPORTS.contains(&root) {
                let line = code[..module.start()].matches('\n').count() as i64 + 1;
                record(
                    format!("From Import: {}", module.as_str()),
                    line,
                    &mut issues,
                );
            }
        }
    }
    for captures in call_pattern.captures_iter(code) {
        let call = captures.get(0).expect("whole match");
        let name = call
            .as_str()
            .trim_end_matches('(')
            .trim()
            .split('.')
            .next_back()
            .unwrap_or("")
            .to_string();
        let line = code[..call.start()].matches('\n').count() as i64 + 1;
        record(format!("Call: {name}"), line, &mut issues);
    }
    for captures in attribute_pattern.captures_iter(code) {
        let matched = captures.get(0).expect("whole match");
        let line = code[..matched.start()].matches('\n').count() as i64 + 1;
        record(
            format!("Attribute Access: {}", matched.as_str()),
            line,
            &mut issues,
        );
    }
    issues
}

/// Node.js 正则安全扫描（对齐 SecureJavaScriptAnalyzer.analyze）。
fn analyze_nodejs_security(code: &str) -> Vec<(String, i64)> {
    let mut issues: Vec<(String, i64)> = Vec::new();
    for (pattern, description) in DANGEROUS_NODEJS_PATTERNS {
        let regex = regex::Regex::new(pattern).expect("static regex");
        for matched in regex.find_iter(code) {
            let line = code[..matched.start()].matches('\n').count() as i64 + 1;
            if !issues
                .iter()
                .any(|(existing, lineno)| *existing == *description && *lineno == line)
            {
                issues.push((description.to_string(), line));
            }
        }
    }
    issues
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
            .execute(
                code,
                "python3",
                &Map::from_iter([("name".into(), json!("世界"))]),
            )
            .await
            .unwrap();
        eprintln!(
            "PROBE stdout={:?} stderr={:?}",
            result.stdout, result.stderr
        );
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
        let error = sandbox
            .execute(code, "python", &Map::new())
            .await
            .unwrap_err();
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

    // ── 契约层测试（executor_manager 远程契约） ──────────────────────────

    #[test]
    fn enums_and_exit_codes_match_executor_manager() {
        // 枚举字符串值（对齐 models/enums.py）
        assert_eq!(SupportLanguage::Python.as_str(), "python");
        assert_eq!(SupportLanguage::Nodejs.as_str(), "nodejs");
        assert_eq!(
            SupportLanguage::parse("python3"),
            Some(SupportLanguage::Python)
        );
        assert_eq!(
            SupportLanguage::parse("javascript"),
            Some(SupportLanguage::Nodejs)
        );
        assert_eq!(SupportLanguage::parse("ruby"), None);
        assert_eq!(ResultStatus::Success.as_str(), "success");
        assert_eq!(ResultStatus::ProgramError.as_str(), "program_error");
        assert_eq!(
            ResultStatus::ResourceLimitExceeded.as_str(),
            "resource_limit_exceeded"
        );
        assert_eq!(
            ResultStatus::UnauthorizedAccess.as_str(),
            "unauthorized_access"
        );
        assert_eq!(ResultStatus::RuntimeError.as_str(), "runtime_error");
        assert_eq!(
            ResultStatus::ProgramRunnerError.as_str(),
            "program_runner_error"
        );
        assert_eq!(
            ResultStatus::parse("RESOURCE_LIMIT_EXCEEDED"),
            Some(ResultStatus::ResourceLimitExceeded)
        );
        assert_eq!(ResultStatus::parse("bogus"), None);
        assert_eq!(ResourceLimitType::Time.as_str(), "time");
        assert_eq!(ResourceLimitType::Memory.as_str(), "memory");
        assert_eq!(ResourceLimitType::Output.as_str(), "output");
        assert_eq!(
            UnauthorizedAccessType::DisallowedSyscall.as_str(),
            "disallowed_syscall"
        );
        assert_eq!(UnauthorizedAccessType::FileAccess.as_str(), "file_access");
        assert_eq!(
            UnauthorizedAccessType::NetworkAccess.as_str(),
            "network_access"
        );
        assert_eq!(RuntimeErrorType::Signalled.as_str(), "signalled");
        assert_eq!(RuntimeErrorType::NonzeroExit.as_str(), "nonzero_exit");
        // 负值退出码约定（对齐 execution.py / handlers.py / limiter.py）
        assert_eq!(exit_codes::SUCCESS, 0);
        assert_eq!(exit_codes::UNSAFE_CODE, -999);
        assert_eq!(exit_codes::RATE_LIMITED, -429);
        assert_eq!(exit_codes::TIMEOUT, -124);
        assert_eq!(exit_codes::OUT_OF_MEMORY, -137);
        assert_eq!(exit_codes::POOL_BUSY, -10);
        assert_eq!(exit_codes::INTERNAL_ERROR, -3);
        assert_eq!(exit_codes::TIMEOUT_AFTER_KILL, -1);
    }

    #[test]
    fn classify_execution_failure_matches_executor_manager() {
        // returncode 124 → resource_limit_exceeded / time
        let failure = classify_execution_failure(124, "");
        assert_eq!(failure.status, ResultStatus::ResourceLimitExceeded);
        assert_eq!(failure.resource_limit_type, Some(ResourceLimitType::Time));
        assert_eq!(failure.unauthorized_access_type, None);
        // returncode 137 → resource_limit_exceeded / memory
        let failure = classify_execution_failure(137, "");
        assert_eq!(failure.status, ResultStatus::ResourceLimitExceeded);
        assert_eq!(failure.resource_limit_type, Some(ResourceLimitType::Memory));
        // "Permission denied" → unauthorized_access / file_access
        let failure = classify_execution_failure(1, "open('/etc/passwd'): Permission denied");
        assert_eq!(failure.status, ResultStatus::UnauthorizedAccess);
        assert_eq!(
            failure.unauthorized_access_type,
            Some(UnauthorizedAccessType::FileAccess)
        );
        // "Operation not permitted" → unauthorized_access / disallowed_syscall
        let failure = classify_execution_failure(1, "socket(): Operation not permitted");
        assert_eq!(
            failure.unauthorized_access_type,
            Some(UnauthorizedAccessType::DisallowedSyscall)
        );
        // "MemoryError" → resource_limit_exceeded / memory
        let failure = classify_execution_failure(1, "MemoryError: out of memory");
        assert_eq!(failure.status, ResultStatus::ResourceLimitExceeded);
        assert_eq!(failure.resource_limit_type, Some(ResourceLimitType::Memory));
        // 其他 → program_error / nonzero_exit
        let failure = classify_execution_failure(2, "Traceback: boom");
        assert_eq!(failure.status, ResultStatus::ProgramError);
        assert_eq!(
            failure.runtime_error_type,
            Some(RuntimeErrorType::NonzeroExit)
        );
    }

    #[test]
    fn timeout_duration_parsing_and_formatting() {
        // 对齐 util.py parse_timeout_duration
        assert_eq!(parse_timeout_duration("90s", 10), 90);
        assert_eq!(parse_timeout_duration("2m", 10), 120);
        assert_eq!(parse_timeout_duration("1m30s", 10), 90);
        assert_eq!(parse_timeout_duration("1M30S", 10), 90);
        assert_eq!(parse_timeout_duration("bogus", 10), 10);
        assert_eq!(parse_timeout_duration("", 10), 10);
        assert_eq!(parse_timeout_duration("0s", 10), 10);
        // 对齐 util.py format_timeout_duration
        assert_eq!(format_timeout_duration(59), "59s");
        assert_eq!(format_timeout_duration(90), "1m30s");
        assert_eq!(format_timeout_duration(120), "2m");
        // 对齐 util.py is_valid_memory_limit
        assert!(is_valid_memory_limit("256m"));
        assert!(is_valid_memory_limit("1g"));
        assert!(is_valid_memory_limit("512M"));
        assert!(!is_valid_memory_limit("0m"));
        assert!(!is_valid_memory_limit("256"));
        assert!(!is_valid_memory_limit(""));
    }

    #[test]
    fn runtime_env_container_contract() {
        // 容器池命名（对齐 container.py）
        assert_eq!(container_pool_name("python", 0), "sandbox_python_0");
        assert_eq!(container_pool_name("nodejs", 2), "sandbox_nodejs_2");
        assert_eq!(container_pool_name("javascript", 1), "sandbox_nodejs_1");
        // python 执行命令：timeout + -I -B（对齐 _build_container_run_args）
        let args =
            build_container_run_args("python", "task-1", "sandbox_python_0", 10, "runner.py");
        assert_eq!(
            args,
            [
                "docker",
                "exec",
                "--workdir",
                "/workspace/task-1",
                "sandbox_python_0",
                "timeout",
                "10",
                "python",
                "-I",
                "-B",
                "runner.py",
            ]
        );
        // nodejs 执行命令：无 -I -B
        let args =
            build_container_run_args("nodejs", "task-1", "sandbox_nodejs_0", 10, "runner.js");
        assert_eq!(
            args,
            [
                "docker",
                "exec",
                "--workdir",
                "/workspace/task-1",
                "sandbox_nodejs_0",
                "timeout",
                "10",
                "nodejs",
                "runner.js",
            ]
        );
        // 容器创建参数（对齐 create_container：runsc/read-only/tmpfs/user/memory/seccomp）
        let args = build_container_create_args(
            "sandbox_python_0",
            "python",
            "sandbox-base-python:latest",
            Some("512m"),
            true,
        );
        assert_eq!(args[0], "docker");
        assert!(args.iter().any(|arg| arg == "--runtime=runsc"));
        assert!(args.iter().any(|arg| arg == "--read-only"));
        assert!(args.iter().any(|arg| arg == "--tmpfs"));
        assert!(
            args.iter()
                .any(|arg| arg == "/workspace:rw,exec,size=100M,uid=65534,gid=65534")
        );
        assert!(args.iter().any(|arg| arg == "/tmp:rw,exec,size=50M"));
        assert!(args.iter().any(|arg| arg == "--user"));
        assert!(args.iter().any(|arg| arg == "nobody"));
        assert!(args.iter().any(|arg| arg == "--memory"));
        assert!(args.iter().any(|arg| arg == "512m"));
        assert!(
            args.iter()
                .any(|arg| arg == "seccomp=/app/seccomp-profile-default.json")
        );
        assert!(args.iter().any(|arg| arg == "sandbox-base-python:latest"));
        // 非法内存值回退默认 256m
        let args =
            build_container_create_args("sandbox_python_0", "python", "img", Some("oops"), false);
        assert!(args.iter().any(|arg| arg == "256m"));
        assert!(!args.iter().any(|arg| arg.contains("seccomp")));
        // nodejs module.exports 追加（对齐 handlers.py）
        assert_eq!(
            append_nodejs_module_export("function main() {}"),
            "function main() {}\n\nmodule.exports = { main };"
        );
        // 基础镜像默认值（对齐 SANDBOX_BASE_*_IMAGE 默认）
        assert_eq!(
            sandbox_base_image_from_env("python"),
            "sandbox-base-python:latest"
        );
        assert_eq!(
            sandbox_base_image_from_env("nodejs"),
            "sandbox-base-nodejs:latest"
        );
    }

    #[test]
    fn artifact_contract_mime_and_sanitize() {
        // MIME 映射（对齐 ALLOWED_ARTIFACT_EXTENSIONS）
        assert_eq!(mime_type_for_artifact("a.png"), Some("image/png"));
        assert_eq!(mime_type_for_artifact("a.JPG"), Some("image/jpeg"));
        assert_eq!(mime_type_for_artifact("a.svg"), Some("image/svg+xml"));
        assert_eq!(mime_type_for_artifact("a.pdf"), Some("application/pdf"));
        assert_eq!(mime_type_for_artifact("a.csv"), Some("text/csv"));
        assert_eq!(mime_type_for_artifact("a.json"), Some("application/json"));
        assert_eq!(mime_type_for_artifact("a.html"), Some("text/html"));
        assert_eq!(mime_type_for_artifact("a.exe"), None);
        assert_eq!(mime_type_for_artifact("noext"), None);
        // 文件名净化（对齐 execution.py：拒绝 /、\、..、点开头）
        assert!(artifact_name_is_safe("chart.csv"));
        assert!(!artifact_name_is_safe("../evil.csv"));
        assert!(!artifact_name_is_safe("a/b.csv"));
        assert!(!artifact_name_is_safe("a\\b.csv"));
        assert!(!artifact_name_is_safe(".hidden"));
        assert!(!artifact_name_is_safe(""));
        // 常量
        assert_eq!(MAX_ARTIFACT_COUNT, 10);
        assert_eq!(MAX_ARTIFACT_SIZE_BYTES, 10 * 1024 * 1024);
    }

    #[test]
    fn execution_bundle_builds_runner_and_args() {
        // python 包布局（对齐 _build_execution_bundle）
        let bundle = build_execution_bundle(
            "python",
            b"def main():\n    return 1\n",
            &Map::from_iter([("x".into(), json!(1))]),
        )
        .unwrap();
        assert_eq!(bundle.code_name, "main.py");
        assert_eq!(bundle.runner_name, "runner.py");
        assert_eq!(bundle.args_name, "args.json");
        assert_eq!(bundle.args_source, "{\"x\":1}");
        assert!(bundle.runner_source.contains("from main import main"));
        assert!(bundle.runner_source.contains("main(**args)"));
        assert!(bundle.runner_source.contains("__RAGFLOW_RESULT__:"));
        assert!(bundle.runner_source.contains("artifacts"));
        // nodejs 包布局
        let bundle = build_execution_bundle("nodejs", b"function main() {}", &Map::new()).unwrap();
        assert_eq!(bundle.code_name, "main.js");
        assert_eq!(bundle.runner_name, "runner.js");
        assert!(bundle.runner_source.contains("require(mainPath)"));
        assert!(bundle.runner_source.contains("__RAGFLOW_RESULT__:"));
        // 不支持的语言 → Err
        assert!(build_execution_bundle("ruby", b"x", &Map::new()).is_err());
    }

    #[test]
    fn security_analysis_flags_dangerous_code() {
        // python 危险 import（对齐 SecurePythonAnalyzer）
        let (safe, issues) = analyze_code_security("import os\nx = 1\n", "python");
        assert!(!safe);
        assert!(
            issues
                .iter()
                .any(|(description, line)| { description == "Import: os" && *line == 1 })
        );
        let (safe, issues) =
            analyze_code_security("import json\nfrom subprocess import run\n", "python");
        assert!(!safe);
        assert!(
            issues.iter().any(|(description, line)| {
                description == "From Import: subprocess" && *line == 2
            })
        );
        // python 危险调用 + 属性访问
        let (safe, issues) =
            analyze_code_security("def main():\n    return eval('1+1')\n", "python");
        assert!(!safe);
        assert!(
            issues
                .iter()
                .any(|(description, line)| { description == "Call: eval" && *line == 2 })
        );
        let (safe, _) =
            analyze_code_security("def main():\n    return os.path.join('a', 'b')\n", "python");
        assert!(!safe);
        // 干净代码 → safe
        let (safe, issues) =
            analyze_code_security("def main():\n    return {'ok': True}\n", "python");
        assert!(safe);
        assert!(issues.is_empty());
        // nodejs require（对齐 SecureJavaScriptAnalyzer）
        let (safe, issues) =
            analyze_code_security("const { exec } = require('child_process');\n", "nodejs");
        assert!(!safe);
        assert!(
            issues
                .iter()
                .any(|(description, _)| description == "Require: child_process")
        );
        let (safe, issues) = analyze_code_security("process.binding('natives');\n", "nodejs");
        assert!(!safe);
        assert!(
            issues
                .iter()
                .any(|(description, _)| description == "Call: process.binding")
        );
        let (safe, _) = analyze_code_security("function main() { return 1; }\n", "nodejs");
        assert!(safe);
        // 不支持的语言 → unsafe（对齐 analyze_code_security 默认分支）
        let (safe, _) = analyze_code_security("x", "ruby");
        assert!(!safe);
        // issue 渲染（对齐 handlers.py："Line {lineno}: {issue}" 换行连接）
        let formatted =
            format_security_issues(&[("Import: os".to_string(), 1), ("Call: eval".to_string(), 3)]);
        assert_eq!(formatted, "Line 1: Import: os\nLine 3: Call: eval");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 远程调用契约层（新增，对齐 client.py / providers/self_managed.py +
// executor_manager api 层 routes.py / handlers.py；不改动既有 pub API）
// ═══════════════════════════════════════════════════════════════════════════
//
// 任务提交/结果拉取语义（对齐 SelfManagedProvider.execute_code）：
//   - POST {endpoint}/run，请求体 {"code_b64": b64(code), "language", "arguments"}
//   - 同步返回（无轮询）：200 → CodeExecutionResult JSON；非 200 → Err("HTTP …")
//   - 请求超时 = 执行超时（对齐 requests timeout=exec_timeout）
//   - 请求级瞬时错误按 max_retries 重试（默认 3，指数退避）
//   - GET {endpoint}/healthz → 200 即健康（对齐 health_check）
//
// executor_manager api 层语义（对齐 handlers.py run_code_handler）：
//   - nodejs 执行前追加 "\n\nmodule.exports = { main };"（既有 append_nodejs_module_export）
//   - 安全检查不过 → status=program_runner_error / exit_code=-999 /
//     stderr=issue 行列表 / detail="Code is unsafe"
//   - 未处理异常 → status=program_runner_error / exit_code=-999 /
//     stderr=str(e) / detail="unhandled_exception"

/// 执行结构化结果（对齐 schemas.py ExecutionStructuredResult）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionStructuredResult {
    pub present: bool,
    #[serde(default)]
    pub value: Option<Value>,
    #[serde(default = "default_structured_type")]
    pub r#type: String,
}

fn default_structured_type() -> String {
    "json".to_string()
}

/// 代码执行结果（对齐 schemas.py CodeExecutionResult，/run 响应体）。
///
/// `status` 保留为上游字符串值，由 [`ResultStatus::parse`] 解析为类型化枚举。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CodeExecutionResult {
    pub status: String,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    pub exit_code: i64,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub time_used_ms: Option<f64>,
    #[serde(default)]
    pub memory_used_kb: Option<f64>,
    #[serde(default)]
    pub resource_limit_type: Option<String>,
    #[serde(default)]
    pub unauthorized_access_type: Option<String>,
    #[serde(default)]
    pub runtime_error_type: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<ArtifactItem>,
    #[serde(default)]
    pub result: Option<ExecutionStructuredResult>,
}

impl CodeExecutionResult {
    /// 转换为远程执行结果（对齐 SelfManagedProvider 的 ExecutionResult 组装：
    /// stdout/stderr/exit_code + metadata{status,time_used_ms,memory_used_kb,detail,
    /// artifacts,result_present,result_value,result_type}）。
    pub fn into_remote(self, execution_time: f64) -> RemoteExecutionResult {
        let structured = self.result.clone();
        RemoteExecutionResult {
            stdout: self.stdout,
            stderr: self.stderr,
            exit_code: self.exit_code,
            execution_time,
            status: ResultStatus::parse(self.status.as_str()),
            time_used_ms: self.time_used_ms,
            memory_used_kb: self.memory_used_kb,
            detail: self.detail,
            artifacts: self.artifacts,
            result_present: structured
                .as_ref()
                .map(|result| result.present)
                .unwrap_or(false),
            result_value: structured.as_ref().and_then(|result| result.value.clone()),
            result_type: structured.map(|result| result.r#type),
        }
    }
}

/// 不安全代码响应（对齐 handlers.py run_code_handler 安全检查分支：
/// status=program_runner_error, exit_code=-999, stderr=issue 行列表,
/// detail="Code is unsafe"）。
pub fn unsafe_code_result(issue_details: &str) -> CodeExecutionResult {
    CodeExecutionResult {
        status: ResultStatus::ProgramRunnerError.as_str().to_string(),
        stdout: String::new(),
        stderr: issue_details.to_string(),
        exit_code: exit_codes::UNSAFE_CODE,
        detail: Some("Code is unsafe".to_string()),
        ..Default::default()
    }
}

/// 未处理异常响应（对齐 handlers.py run_code_handler 异常分支：
/// status=program_runner_error, exit_code=-999, stderr=str(e),
/// detail="unhandled_exception"）。
pub fn unhandled_exception_result(error: &str) -> CodeExecutionResult {
    CodeExecutionResult {
        status: ResultStatus::ProgramRunnerError.as_str().to_string(),
        stdout: String::new(),
        stderr: error.to_string(),
        exit_code: exit_codes::UNSAFE_CODE,
        detail: Some("unhandled_exception".to_string()),
        ..Default::default()
    }
}

/// 远程执行结果（对齐 providers/base.py ExecutionResult + metadata 子集）。
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i64,
    pub execution_time: f64,
    pub status: Option<ResultStatus>,
    pub time_used_ms: Option<f64>,
    pub memory_used_kb: Option<f64>,
    pub detail: Option<String>,
    pub artifacts: Vec<ArtifactItem>,
    pub result_present: bool,
    pub result_value: Option<Value>,
    pub result_type: Option<String>,
}

/// 远程沙箱配置（对齐 SelfManagedProvider 配置项）。
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteSandboxConfig {
    /// executor_manager HTTP endpoint（默认 http://localhost:9385）。
    pub endpoint: String,
    /// 请求超时秒数（默认 30）。
    pub timeout_seconds: u64,
    /// 最大重试次数（默认 3）。
    pub max_retries: u32,
    /// 容器池大小（默认 10，仅信息用途）。
    pub pool_size: u32,
}

impl Default for RemoteSandboxConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://localhost:9385".to_string(),
            timeout_seconds: 30,
            max_retries: 3,
            pool_size: 10,
        }
    }
}

impl RemoteSandboxConfig {
    /// 从环境变量覆盖（对齐 client.py _load_self_managed_provider_config_from_env：
    /// SANDBOX_HOST + SANDBOX_EXECUTOR_MANAGER_PORT → endpoint（端口默认 9385）、
    /// SANDBOX_EXECUTOR_MANAGER_POOL_SIZE → pool_size）。
    pub fn from_env() -> Self {
        let mut config = Self::default();
        let host = std::env::var("SANDBOX_HOST")
            .ok()
            .filter(|value| !value.trim().is_empty());
        if let Some(host) = host {
            let port = std::env::var("SANDBOX_EXECUTOR_MANAGER_PORT")
                .unwrap_or_else(|_| "9385".to_string());
            config.endpoint = format!("http://{host}:{port}");
        }
        if let Ok(pool) = std::env::var("SANDBOX_EXECUTOR_MANAGER_POOL_SIZE")
            && let Ok(parsed) = pool.trim().parse() {
                config.pool_size = parsed;
            }
        config
    }
}

/// 远程语言规范化（对齐 SelfManagedProvider._normalize_language）。
pub fn normalize_remote_language(language: &str) -> String {
    match language.trim().to_lowercase().as_str() {
        "" => "python".to_string(),
        "python" | "python3" => "python".to_string(),
        "javascript" | "nodejs" => "nodejs".to_string(),
        other => other.to_string(),
    }
}

/// 构建 /run 请求体（对齐 SelfManagedProvider.execute_code 的 payload：
/// code_b64 + language + arguments）。
pub fn build_run_payload(code: &str, language: &str, arguments: &Map<String, Value>) -> Value {
    use base64::Engine;
    let code_b64 = base64::engine::general_purpose::STANDARD.encode(code.as_bytes());
    json!({
        "code_b64": code_b64,
        "language": normalize_remote_language(language),
        "arguments": arguments,
    })
}

/// 自管理沙箱远程客户端（对齐 SelfManagedProvider）。
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct RemoteSandboxClient {
    pub config: RemoteSandboxConfig,
}


impl RemoteSandboxClient {
    pub fn new(config: RemoteSandboxConfig) -> Self {
        Self { config }
    }

    /// 从环境变量构造（对齐 client.py 的 env 覆盖路径）。
    pub fn from_env() -> Self {
        Self::new(RemoteSandboxConfig::from_env())
    }

    /// 健康检查（对齐 health_check：GET {endpoint}/healthz，200 = healthy）。
    pub async fn health_check(&self) -> bool {
        let url = format!("{}/healthz", self.config.endpoint.trim_end_matches('/'));
        let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
        else {
            return false;
        };
        match client.get(&url).send().await {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }

    /// 提交代码执行任务并同步拉取结果（对齐 execute_code：POST {endpoint}/run）。
    ///
    /// 语义：请求体 code_b64/language/arguments；请求超时 = 执行超时；
    /// 非 200 → Err("HTTP {status}: {body}")；请求级瞬时错误按 max_retries 重试
    /// （线性退避）；超时 → Err("Execution timed out after {N} seconds")。
    pub async fn run(
        &self,
        code: &str,
        language: &str,
        timeout: Option<u64>,
        arguments: &Map<String, Value>,
    ) -> Result<RemoteExecutionResult> {
        let exec_timeout = timeout.unwrap_or(self.config.timeout_seconds).max(1);
        let payload = build_run_payload(code, language, arguments);
        let url = format!("{}/run", self.config.endpoint.trim_end_matches('/'));
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(exec_timeout))
            .build()?;

        let start = std::time::Instant::now();
        let attempts = self.config.max_retries.max(1);
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 0..attempts {
            match client.post(&url).json(&payload).send().await {
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().await?;
                    if !status.is_success() {
                        return Err(anyhow::anyhow!("HTTP {status}: {body}"));
                    }
                    let wire: CodeExecutionResult = serde_json::from_str(&body)?;
                    return Ok(wire.into_remote(start.elapsed().as_secs_f64()));
                }
                Err(error) => {
                    if error.is_timeout() {
                        return Err(anyhow::anyhow!(
                            "Execution timed out after {exec_timeout} seconds"
                        ));
                    }
                    last_error = Some(anyhow::anyhow!("HTTP request failed: {error}"));
                    if attempt + 1 < attempts {
                        tokio::time::sleep(Duration::from_millis(100 * (attempt as u64 + 1))).await;
                    }
                }
            }
        }
        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("HTTP request failed after {attempts} attempts")))
    }
}

#[cfg(test)]
mod remote_tests {
    use super::*;
    use serde_json::json;

    /// 启动一个一次性 HTTP 测试服务，返回监听端口。
    async fn serve_once(status_line: &str, body: &str) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let status_line = status_line.to_string();
        let body = body.to_string();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 8192];
            let _ = socket.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        });
        port
    }

    #[test]
    fn remote_language_normalization_matches_provider() {
        assert_eq!(normalize_remote_language("python"), "python");
        assert_eq!(normalize_remote_language("python3"), "python");
        assert_eq!(normalize_remote_language("javascript"), "nodejs");
        assert_eq!(normalize_remote_language("nodejs"), "nodejs");
        assert_eq!(normalize_remote_language(""), "python");
        assert_eq!(normalize_remote_language("ruby"), "ruby");
    }

    #[test]
    fn run_payload_base64_encodes_code_and_arguments() {
        use base64::Engine;
        let payload = build_run_payload(
            "def main():\n    return 1\n",
            "python3",
            &Map::from_iter([("x".into(), json!(1))]),
        );
        assert_eq!(payload["language"].as_str(), Some("python"));
        assert_eq!(payload["arguments"]["x"].as_i64(), Some(1));
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(payload["code_b64"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            String::from_utf8(decoded).unwrap(),
            "def main():\n    return 1\n"
        );
    }

    #[test]
    fn remote_response_parses_into_typed_result() {
        let wire: CodeExecutionResult = serde_json::from_str(
            r#"{
                "status": "success", "stdout": "hello", "stderr": "",
                "exit_code": 0, "detail": null,
                "time_used_ms": 12.5, "memory_used_kb": 2048.0,
                "artifacts": [{"name": "chart.png", "mime_type": "image/png", "size": 10, "content_b64": "AAAA"}],
                "result": {"present": true, "value": {"sum": 103}, "type": "json"}
            }"#,
        )
        .unwrap();
        assert_eq!(wire.status, "success");
        assert_eq!(wire.artifacts.len(), 1);
        assert_eq!(wire.artifacts[0].mime_type, "image/png");
        let remote = wire.into_remote(0.5);
        assert_eq!(remote.status, Some(ResultStatus::Success));
        assert_eq!(remote.exit_code, 0);
        assert!(remote.stdout.contains("hello"));
        assert_eq!(remote.time_used_ms, Some(12.5));
        assert_eq!(remote.memory_used_kb, Some(2048.0));
        assert!(remote.result_present);
        assert_eq!(remote.result_type.as_deref(), Some("json"));
        assert_eq!(
            remote.result_value.as_ref().unwrap().get("sum"),
            Some(&json!(103))
        );
    }

    #[test]
    fn handler_failure_results_match_run_code_handler() {
        // 安全检查分支（对齐 handlers.py：-999 / program_runner_error / "Code is unsafe"）
        let unsafe_result = unsafe_code_result("Line 1: Import: os");
        assert_eq!(
            unsafe_result.status,
            ResultStatus::ProgramRunnerError.as_str()
        );
        assert_eq!(unsafe_result.exit_code, exit_codes::UNSAFE_CODE);
        assert_eq!(unsafe_result.detail.as_deref(), Some("Code is unsafe"));
        assert_eq!(unsafe_result.stderr, "Line 1: Import: os");
        // 未处理异常分支（对齐 handlers.py：-999 / "unhandled_exception"）
        let exception_result = unhandled_exception_result("boom");
        assert_eq!(exception_result.exit_code, exit_codes::UNSAFE_CODE);
        assert_eq!(
            exception_result.detail.as_deref(),
            Some("unhandled_exception")
        );
        assert_eq!(exception_result.stderr, "boom");
    }

    #[tokio::test]
    async fn remote_client_run_roundtrip_over_http() {
        let body = json!({
            "status": "success",
            "stdout": "plain output",
            "stderr": "",
            "exit_code": 0,
            "detail": null,
            "time_used_ms": 12.5,
            "memory_used_kb": 2048.0,
            "artifacts": [],
            "result": {"present": true, "value": {"greeting": "hello 世界"}, "type": "json"}
        })
        .to_string();
        let port = serve_once("200 OK", &body).await;
        let client = RemoteSandboxClient::new(RemoteSandboxConfig {
            endpoint: format!("http://127.0.0.1:{port}"),
            ..Default::default()
        });
        let result = client
            .run(
                "def main(name):\n    return {'greeting': f'hello {name}'}\n",
                "python3",
                Some(10),
                &Map::from_iter([("name".into(), json!("世界"))]),
            )
            .await
            .unwrap();
        assert_eq!(result.status, Some(ResultStatus::Success));
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("plain output"));
        assert_eq!(
            result.result_value.as_ref().unwrap().get("greeting"),
            Some(&json!("hello 世界"))
        );
    }

    #[tokio::test]
    async fn remote_client_non_200_returns_http_error() {
        let port = serve_once("500 Internal Server Error", "boom").await;
        let client = RemoteSandboxClient::new(RemoteSandboxConfig {
            endpoint: format!("http://127.0.0.1:{port}"),
            ..Default::default()
        });
        let error = client
            .run("x", "python", Some(5), &Map::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("HTTP 500"));
        assert!(error.contains("boom"));
    }

    #[tokio::test]
    async fn remote_client_health_check_reports_liveness() {
        let port = serve_once("200 OK", r#"{"status":"ok"}"#).await;
        let healthy = RemoteSandboxClient::new(RemoteSandboxConfig {
            endpoint: format!("http://127.0.0.1:{port}"),
            ..Default::default()
        });
        assert!(healthy.health_check().await);
        let dead = RemoteSandboxClient::new(RemoteSandboxConfig {
            endpoint: "http://127.0.0.1:1".to_string(),
            ..Default::default()
        });
        assert!(!dead.health_check().await);
    }
}
