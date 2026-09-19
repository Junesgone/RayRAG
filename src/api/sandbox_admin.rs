//! Admin sandbox-provider API — RAGFlow `admin/server/routes.py`
//! (`/sandbox/providers`, `/sandbox/providers/{id}/schema`, `/sandbox/config`,
//! `/sandbox/test`) over `admin/server/services.py::SandboxMgr` and
//! `agent/sandbox/providers/*.py`.
//!
//! The provider registry, the per-provider configuration schemas (field names,
//! labels, descriptions, defaults, bounds and secret flags) and the connection
//! probe are transcribed from those sources, so the admin console's provider
//! cards and schema-driven form match upstream field for field.
//!
//! Configuration is durable: the active provider lives in the
//! `sandbox.provider_type` system setting and each provider's values in
//! `sandbox.<provider_type>` (UTF-8 JSON), which is exactly where the code
//! executor already reads `sandbox.self_managed` from. Secrets round-trip
//! through the settings store's `<redacted>` sentinel.

use crate::server::{AppState, AuthContext};
use axum::{
    Json,
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Upstream `admin/server/services.py::SandboxMgr.PROVIDER_REGISTRY`, in
/// registry order.
pub const SANDBOX_PROVIDERS: [(&str, &str, &str, &[&str]); 5] = [
    (
        "local",
        "Local",
        "Execute code directly on the current host process.",
        &["local", "host", "minimal"],
    ),
    (
        "self_managed",
        "Self-Managed",
        "On-premise deployment using Daytona/Docker",
        &["self-hosted", "low-latency", "secure"],
    ),
    (
        "ssh",
        "SSH",
        "Execute code on a remote machine over SSH.",
        &["remote", "ssh", "custom-runtime"],
    ),
    (
        "aliyun_codeinterpreter",
        "Aliyun Code Interpreter",
        "Aliyun Function Compute Code Interpreter - Code execution in serverless microVMs",
        &["saas", "cloud", "scalable", "aliyun"],
    ),
    (
        "e2b",
        "E2B",
        "E2B Cloud - Code Execution Sandboxes",
        &["saas", "fast", "global"],
    ),
];

/// One field of a provider's configuration schema (`get_config_schema()`).
/// Every provider declares the same keys; the ones upstream omits stay `None`
/// and are left out of the serialized schema.
#[derive(Debug, Clone, Default)]
pub struct SandboxField {
    pub name: &'static str,
    /// `string` | `integer` | `boolean`.
    pub field_type: &'static str,
    pub required: bool,
    pub label: &'static str,
    pub description: &'static str,
    pub default: Option<Value>,
    pub placeholder: Option<&'static str>,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub secret: bool,
    pub multiline: bool,
    pub readonly: bool,
    /// `runtime` | `deployment` (self-managed only).
    pub scope: Option<&'static str>,
    pub options: &'static [&'static str],
}

impl SandboxField {
    fn string(
        name: &'static str,
        label: &'static str,
        description: &'static str,
        default: Option<&str>,
    ) -> Self {
        Self {
            name,
            field_type: "string",
            label,
            description,
            default: default.map(Value::from),
            ..Self::default()
        }
    }

    fn integer(
        name: &'static str,
        label: &'static str,
        description: &'static str,
        default: Option<i64>,
        min: i64,
        max: i64,
    ) -> Self {
        Self {
            name,
            field_type: "integer",
            label,
            description,
            default: default.map(Value::from),
            min: Some(min),
            max: Some(max),
            ..Self::default()
        }
    }

    fn required(mut self) -> Self {
        self.required = true;
        self
    }

    fn secret(mut self) -> Self {
        self.secret = true;
        self
    }

    fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    fn placeholder(mut self, placeholder: &'static str) -> Self {
        self.placeholder = Some(placeholder);
        self
    }

    fn readonly(mut self, scope: &'static str) -> Self {
        self.readonly = true;
        self.scope = Some(scope);
        self
    }

    fn scope(mut self, scope: &'static str) -> Self {
        self.scope = Some(scope);
        self
    }

    fn options(mut self, options: &'static [&'static str]) -> Self {
        self.options = options;
        self
    }

    fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("type".into(), Value::from(self.field_type));
        object.insert("required".into(), Value::from(self.required));
        object.insert("label".into(), Value::from(self.label));
        if let Some(default) = &self.default {
            object.insert("default".into(), default.clone());
        }
        if let Some(placeholder) = self.placeholder {
            object.insert("placeholder".into(), Value::from(placeholder));
        }
        object.insert("description".into(), Value::from(self.description));
        if self.secret {
            object.insert("secret".into(), Value::from(true));
        }
        if self.multiline {
            object.insert("multiline".into(), Value::from(true));
        }
        if let Some(min) = self.min {
            object.insert("min".into(), Value::from(min));
        }
        if let Some(max) = self.max {
            object.insert("max".into(), Value::from(max));
        }
        if let Some(scope) = self.scope {
            object.insert("scope".into(), Value::from(scope));
        }
        if self.readonly {
            object.insert("readonly".into(), Value::from(true));
        }
        if !self.options.is_empty() {
            object.insert(
                "options".into(),
                Value::Array(
                    self.options
                        .iter()
                        .map(|value| Value::from(*value))
                        .collect(),
                ),
            );
        }
        Value::Object(object)
    }
}

fn env_string(name: &str, fallback: &'static str) -> Value {
    Value::from(
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| fallback.to_string()),
    )
}

fn env_integer(name: &str, fallback: i64) -> Value {
    Value::from(
        std::env::var(name)
            .ok()
            .and_then(|value| value.trim().parse::<i64>().ok())
            .unwrap_or(fallback),
    )
}

fn env_boolean(name: &str, fallback: bool) -> Value {
    Value::from(
        std::env::var(name)
            .ok()
            .map(|value| value.eq_ignore_ascii_case("true"))
            .unwrap_or(fallback),
    )
}

/// Upstream `agent/sandbox/providers/*.py::get_config_schema()`.
pub fn sandbox_provider_schema(provider_id: &str) -> Option<Vec<SandboxField>> {
    let fields = match provider_id {
        "local" => vec![
            SandboxField::string(
                "python_bin",
                "Python Binary",
                "Python executable used for local code execution.",
                Some("python3"),
            ),
            SandboxField::string(
                "node_bin",
                "Node.js Binary",
                "Node.js executable used for local JavaScript execution.",
                Some("node"),
            ),
            SandboxField::string(
                "work_dir",
                "Working Directory",
                "Directory used to store temporary scripts and artifacts on the current host.",
                Some("/tmp/ragflow-codeexec"),
            ),
            SandboxField::integer(
                "timeout",
                "Timeout (seconds)",
                "Maximum execution time for each local run. Unit: seconds.",
                Some(30),
                1,
                600,
            ),
            SandboxField::integer(
                "max_memory_mb",
                "Max Memory (MB)",
                "Address-space memory limit for the local child process. Unit: MB.",
                Some(512),
                1,
                65536,
            ),
            SandboxField::integer(
                "max_output_bytes",
                "Max Output (bytes)",
                "Maximum combined stdout and stderr size. Unit: bytes.",
                Some(1048576),
                1024,
                10485760,
            ),
            SandboxField::integer(
                "max_artifacts",
                "Max Artifacts",
                "Maximum number of files collected from the artifacts directory.",
                Some(20),
                0,
                100,
            ),
            SandboxField::integer(
                "max_artifact_bytes",
                "Max Artifact Size (bytes)",
                "Maximum size of a single artifact file. Unit: bytes.",
                Some(10485760),
                1024,
                104857600,
            ),
        ],
        "ssh" => vec![
            SandboxField::string(
                "host",
                "SSH Host",
                "Remote host that will execute generated code.",
                None,
            )
            .required()
            .placeholder("192.168.1.10"),
            SandboxField::integer(
                "port",
                "SSH Port",
                "SSH port on the remote host.",
                Some(22),
                1,
                65535,
            )
            .required(),
            SandboxField::string(
                "username",
                "SSH Username",
                "Username used to connect to the remote host.",
                None,
            )
            .required()
            .placeholder("ragflow"),
            SandboxField::string(
                "password",
                "SSH Password",
                "Password-based SSH authentication.",
                None,
            )
            .secret()
            .placeholder("Optional when using a private key"),
            SandboxField::string(
                "private_key",
                "SSH Private Key",
                "Private key PEM content or a readable private key path on the RAGFlow host.",
                None,
            )
            .secret()
            .multiline()
            .placeholder("Paste PEM content or enter a local file path"),
            SandboxField::string(
                "passphrase",
                "Private Key Passphrase",
                "Passphrase for the private key if it is encrypted.",
                None,
            )
            .secret()
            .placeholder("Optional"),
            SandboxField::string(
                "known_hosts",
                "SSH known_hosts File",
                "Path to an OpenSSH-format known_hosts file used to verify the remote host's key. When set, the file is loaded on top of the system host keys (~/.ssh/known_hosts). When unset, only system keys are used and unknown hosts are rejected.",
                None,
            )
            .placeholder("/etc/ragflow/ssh_known_hosts"),
            SandboxField::string(
                "python_bin",
                "Python Binary",
                "Python executable used for remote code execution.",
                Some("python3"),
            ),
            SandboxField::string(
                "node_bin",
                "Node.js Binary",
                "Node.js executable used for remote JavaScript execution.",
                Some("node"),
            ),
            SandboxField::string(
                "work_dir",
                "Remote Workspace Root",
                "Writable remote directory used to create a temporary workspace.",
                Some("/tmp"),
            )
            .placeholder("/tmp"),
            SandboxField::integer(
                "timeout",
                "Timeout (seconds)",
                "Maximum SSH execution time for a single run.",
                Some(30),
                1,
                600,
            ),
            SandboxField::integer(
                "max_output_bytes",
                "Max Output Bytes",
                "Maximum combined stdout and stderr size.",
                Some(1048576),
                1024,
                10485760,
            ),
            SandboxField::integer(
                "max_artifacts",
                "Max Artifacts",
                "Maximum number of files collected from the remote artifacts directory.",
                Some(20),
                0,
                100,
            ),
            SandboxField::integer(
                "max_artifact_bytes",
                "Max Artifact Bytes",
                "Maximum size of a single artifact file in bytes.",
                Some(10485760),
                1024,
                104857600,
            ),
        ],
        "self_managed" => vec![
            SandboxField::string(
                "endpoint",
                "Executor Manager Endpoint",
                "HTTP endpoint used by RAGFlow to call sandbox-executor-manager.",
                Some("http://sandbox-executor-manager:9385"),
            )
            .required()
            .placeholder("http://sandbox-executor-manager:9385")
            .scope("runtime"),
            SandboxField::integer(
                "timeout",
                "Request Timeout (seconds)",
                "Maximum request time for a single code execution call. Unit: seconds.",
                Some(30),
                5,
                300,
            )
            .scope("runtime"),
            SandboxField {
                default: Some(env_string(
                    "SANDBOX_EXECUTOR_MANAGER_IMAGE",
                    "infiniflow/sandbox-executor-manager:latest",
                )),
                ..SandboxField::string(
                    "executor_manager_image",
                    "Executor Manager Image",
                    "Docker image used by sandbox-executor-manager.",
                    None,
                )
            }
            .readonly("deployment"),
            SandboxField {
                default: Some(env_integer("SANDBOX_EXECUTOR_MANAGER_POOL_SIZE", 3)),
                ..SandboxField::integer(
                    "executor_manager_pool_size",
                    "Container Pool Size",
                    "Container pool size used by sandbox-executor-manager.",
                    None,
                    1,
                    100,
                )
            }
            .readonly("deployment"),
            SandboxField {
                default: Some(env_string(
                    "SANDBOX_BASE_PYTHON_IMAGE",
                    "infiniflow/sandbox-base-python:latest",
                )),
                ..SandboxField::string(
                    "base_python_image",
                    "Base Python Image",
                    "Python runtime image used by executor-managed containers.",
                    None,
                )
            }
            .readonly("deployment"),
            SandboxField {
                default: Some(env_string(
                    "SANDBOX_BASE_NODEJS_IMAGE",
                    "infiniflow/sandbox-base-nodejs:latest",
                )),
                ..SandboxField::string(
                    "base_nodejs_image",
                    "Base Node.js Image",
                    "Node.js runtime image used by executor-managed containers.",
                    None,
                )
            }
            .readonly("deployment"),
            SandboxField {
                default: Some(env_integer("SANDBOX_EXECUTOR_MANAGER_PORT", 9385)),
                ..SandboxField::integer(
                    "executor_manager_port",
                    "Executor Manager Port",
                    "Host port exposed by sandbox-executor-manager.",
                    None,
                    1,
                    65535,
                )
            }
            .readonly("deployment"),
            SandboxField {
                default: Some(env_boolean("SANDBOX_ENABLE_SECCOMP", false)),
                ..SandboxField {
                    field_type: "boolean",
                    ..SandboxField::string(
                        "enable_seccomp",
                        "Enable Seccomp",
                        "Whether sandbox-executor-manager starts containers with seccomp enabled.",
                        None,
                    )
                }
            }
            .readonly("deployment"),
            SandboxField {
                default: Some(env_string("SANDBOX_MAX_MEMORY", "256m")),
                ..SandboxField::string(
                    "max_memory",
                    "Max Memory",
                    "Memory limit applied to each sandbox container. Common format: 256m or 1g.",
                    None,
                )
            }
            .readonly("deployment"),
            SandboxField {
                default: Some(env_string("SANDBOX_TIMEOUT", "10s")),
                ..SandboxField::string(
                    "sandbox_timeout",
                    "Sandbox Timeout",
                    "Executor-manager container timeout for each sandbox run. Common format: 10s or 1m.",
                    None,
                )
            }
            .readonly("deployment"),
        ],
        "aliyun_codeinterpreter" => vec![
            SandboxField::string(
                "access_key_id",
                "Access Key ID",
                "Aliyun AccessKey ID for authentication",
                None,
            )
            .required()
            .placeholder("LTAI5t..."),
            SandboxField::string(
                "access_key_secret",
                "Access Key Secret",
                "Aliyun AccessKey Secret for authentication",
                None,
            )
            .required()
            .secret()
            .placeholder("••••••••••••••••"),
            SandboxField::string(
                "account_id",
                "Account ID",
                "Aliyun primary account ID, required for API calls",
                None,
            )
            .required()
            .placeholder("1234567890..."),
            SandboxField::string(
                "region",
                "Region",
                "Aliyun region for Code Interpreter service",
                Some("cn-hangzhou"),
            )
            .options(&[
                "cn-hangzhou",
                "cn-beijing",
                "cn-shanghai",
                "cn-shenzhen",
                "cn-guangzhou",
            ]),
            SandboxField::string(
                "template_name",
                "Template Name",
                "Optional sandbox template name for pre-configured environments",
                None,
            )
            .placeholder("my-interpreter"),
            SandboxField::integer(
                "timeout",
                "Execution Timeout (seconds)",
                "Code execution timeout (max 30 seconds - hard limit)",
                Some(30),
                1,
                30,
            ),
        ],
        "e2b" => vec![
            SandboxField::string("api_key", "API Key", "E2B API key for authentication", None)
                .required()
                .secret()
                .placeholder("e2b_sk_..."),
            SandboxField::string(
                "region",
                "Region",
                "E2B service region (us or eu)",
                Some("us"),
            ),
            SandboxField::integer(
                "timeout",
                "Request Timeout (seconds)",
                "API request timeout for code execution",
                Some(30),
                5,
                300,
            ),
        ],
        _ => return None,
    };
    Some(fields)
}

fn schema_json(provider_id: &str) -> Option<Value> {
    let fields = sandbox_provider_schema(provider_id)?;
    let mut object = Map::new();
    for field in fields {
        object.insert(field.name.to_string(), field.to_json());
    }
    Some(Value::Object(object))
}

fn require_admin(auth: &AuthContext) -> Option<Response> {
    (!auth.is_admin).then(|| {
        (
            StatusCode::FORBIDDEN,
            Json(json!({ "code": 403, "message": "Administrator access required" })),
        )
            .into_response()
    })
}

/// Upstream `common.success_response(data)`.
fn ok(data: Value) -> Response {
    Json(json!({ "code": 0, "message": "success", "data": data })).into_response()
}

fn error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({ "code": status.as_u16(), "message": message })),
    )
        .into_response()
}

/// Mirrors `api::system_settings`' secret sentinel.
const REDACTED: &str = "<redacted>";

fn setting_name(provider_id: &str) -> String {
    format!("sandbox.{provider_id}")
}

fn active_provider(state: &AppState) -> String {
    state
        .system_settings
        .raw_value("sandbox.provider_type")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "self_managed".to_string())
}

fn stored_config(state: &AppState, provider_id: &str) -> Map<String, Value> {
    state
        .system_settings
        .typed_value(&setting_name(provider_id))
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

/// Upstream `SandboxMgr.get_config`: the stored provider and its values, with
/// the schema defaults filled in for everything that was never saved.
fn config_payload(state: &AppState) -> Value {
    let provider_type = active_provider(state);
    let stored = stored_config(state, &provider_type);
    let mut config = Map::new();
    let placeholder: Vec<SandboxField> = Vec::new();
    let fields = sandbox_provider_schema(&provider_type).unwrap_or(placeholder);
    if !fields.is_empty() {
        for field in &fields {
            if field.readonly {
                continue;
            }
            if let Some(default) = &field.default {
                config.insert(field.name.to_string(), default.clone());
            }
        }
    }
    for (key, value) in stored {
        let secret = fields.iter().any(|field| field.name == key && field.secret);
        if secret && !value.is_null() && value.as_str() != Some("") {
            // Same sentinel as the settings store: the console echoes it back
            // and `set_config` restores the stored secret.
            config.insert(key, Value::from(REDACTED));
        } else {
            config.insert(key, value);
        }
    }
    json!({ "provider_type": provider_type, "config": Value::Object(config) })
}

/// `GET /api/v1/admin/sandbox/providers`.
pub async fn list_providers(Extension(auth): Extension<AuthContext>) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let providers: Vec<Value> = SANDBOX_PROVIDERS
        .iter()
        .map(|(id, name, description, tags)| {
            json!({
                "id": id,
                "name": name,
                "description": description,
                "tags": tags,
            })
        })
        .collect();
    ok(Value::Array(providers))
}

/// `GET /api/v1/admin/sandbox/providers/{provider_id}/schema`.
pub async fn provider_schema(
    Extension(auth): Extension<AuthContext>,
    Path(provider_id): Path<String>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    match schema_json(&provider_id) {
        Some(schema) => ok(schema),
        None => error(
            StatusCode::BAD_REQUEST,
            &format!("Unknown provider: {provider_id}"),
        ),
    }
}

/// `GET /api/v1/admin/sandbox/config`.
pub async fn get_config(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    ok(config_payload(&state))
}

/// `POST /api/v1/admin/sandbox/config` — upstream body
/// `{provider_type, config, set_active}` (an older spelling sends `provider`).
pub async fn set_config(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<Value>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let provider_type = body
        .get("provider_type")
        .or_else(|| body.get("provider"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if provider_type.is_empty() {
        return error(StatusCode::BAD_REQUEST, "provider_type is required");
    }
    let Some(fields) = sandbox_provider_schema(&provider_type) else {
        return error(
            StatusCode::BAD_REQUEST,
            &format!("Unknown provider type: {provider_type}"),
        );
    };
    let submitted = body
        .get("config")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let set_active = body
        .get("set_active")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    // Upstream `SandboxMgr.set_config`: unknown keys are stored as submitted,
    // the schema drives the required/type/range checks and the wording of every
    // rejection, and the provider's own `validate_config` runs last.
    let stored = stored_config(&state, &provider_type);
    let mut merged = Map::new();
    for (key, value) in &submitted {
        let Some(field) = fields.iter().find(|field| field.name == key) else {
            // Not part of the schema (e.g. legacy keys such as the seeded
            // `max_retries`): upstream keeps them verbatim.
            merged.insert(key.clone(), value.clone());
            continue;
        };
        if field.secret
            && (value.as_str() == Some(REDACTED)
                || value.as_str().map(str::trim).unwrap_or_default().is_empty())
        {
            if let Some(existing) = stored.get(key) {
                merged.insert(key.clone(), existing.clone());
            }
            continue;
        }
        match field.field_type {
            "integer" => {
                let parsed = value
                    .as_i64()
                    .ok_or_else(|| format!("Field '{key}' must be an integer"));
                let Ok(parsed) = parsed else {
                    return error(
                        StatusCode::BAD_REQUEST,
                        &format!("Field '{key}' must be an integer"),
                    );
                };
                if let Some(min) = field.min
                    && parsed < min
                {
                    return error(
                        StatusCode::BAD_REQUEST,
                        &format!("Field '{key}' must be >= {min}"),
                    );
                }
                if let Some(max) = field.max
                    && parsed > max
                {
                    return error(
                        StatusCode::BAD_REQUEST,
                        &format!("Field '{key}' must be <= {max}"),
                    );
                }
                merged.insert(key.clone(), Value::from(parsed));
            }
            "boolean" => {
                if !value.is_boolean() {
                    return error(
                        StatusCode::BAD_REQUEST,
                        &format!("Field '{key}' must be a boolean"),
                    );
                }
                merged.insert(key.clone(), value.clone());
            }
            _ => {
                if !value.is_string() {
                    return error(
                        StatusCode::BAD_REQUEST,
                        &format!("Field '{key}' must be a string"),
                    );
                }
                merged.insert(key.clone(), value.clone());
            }
        }
    }
    for field in &fields {
        if field.readonly || !field.required {
            continue;
        }
        let missing = match merged.get(field.name) {
            None => true,
            Some(Value::String(text)) => text.trim().is_empty(),
            Some(Value::Null) => true,
            Some(_) => false,
        };
        if missing {
            return error(
                StatusCode::BAD_REQUEST,
                &format!("Required field '{}' is missing", field.name),
            );
        }
    }
    // Provider-specific `validate_config`: the option sets the schemas declare
    // are the only provider-side constraint this build enforces.
    for field in &fields {
        if field.options.is_empty() {
            continue;
        }
        let Some(Value::String(value)) = merged.get(field.name) else {
            continue;
        };
        if !value.trim().is_empty() && !field.options.contains(&value.trim()) {
            return error(
                StatusCode::BAD_REQUEST,
                &format!(
                    "Provider validation failed: Field '{}' must be one of: {}",
                    field.name,
                    field.options.join(", ")
                ),
            );
        }
    }

    let payload = Value::Object(merged);
    if let Err(err) = state
        .system_settings
        .set(&setting_name(&provider_type), &payload.to_string())
    {
        return error(
            StatusCode::BAD_REQUEST,
            &format!("Failed to set sandbox config: {err}"),
        );
    }
    if set_active
        && let Err(err) = state
            .system_settings
            .set("sandbox.provider_type", &provider_type)
    {
        return error(
            StatusCode::BAD_REQUEST,
            &format!("Failed to set sandbox config: {err}"),
        );
    }
    let mut response = config_payload(&state);
    if let Some(object) = response.as_object_mut() {
        object.insert(
            "message".into(),
            Value::from("Sandbox configuration updated successfully"),
        );
    }
    ok(response)
}

/// Upstream `SandboxMgr.test_connection` runs this exact probe.
const SANDBOX_TEST_CODE: &str = r#"import json
import math


def main() -> dict:
    left = 2
    right = 2
    print(f"2 + 2 = {left + right}")
    print(f"JSON dump: {json.dumps({'test': 'data', 'value': 123})}")
    print(f"Math.sqrt(16) = {math.sqrt(16)}")
    print("TEST_PASSED")
    return {"ok": True, "provider_test": "TEST_PASSED"}
"#;

/// Upstream builds `Test PASSED | Exit code: 0 | Execution time: 1.23s | …`.
fn test_message(
    success: bool,
    exit_code: i64,
    execution_time: f64,
    stdout: &str,
    stderr: &str,
) -> String {
    let mut parts = vec![
        format!("Test {}", if success { "PASSED" } else { "FAILED" }),
        format!("Exit code: {exit_code}"),
        format!("Execution time: {execution_time:.2}s"),
    ];
    let preview = |text: &str| text.trim().chars().take(200).collect::<String>();
    if !stdout.trim().is_empty() {
        parts.push(format!("Output: {}...", preview(stdout)));
    }
    if !stderr.trim().is_empty() {
        parts.push(format!("Errors: {}...", preview(stderr)));
    }
    parts.join(" | ")
}

fn test_payload(
    success: bool,
    exit_code: i64,
    execution_time: f64,
    stdout: &str,
    stderr: &str,
) -> Value {
    json!({
        "success": success,
        "message": test_message(success, exit_code, execution_time, stdout, stderr),
        "details": {
            "exit_code": exit_code,
            "execution_time": execution_time,
            "stdout": stdout,
            "stderr": stderr,
        },
    })
}

/// The local provider executes the source through its own wrapper, which calls
/// `main()`; a bare interpreter needs the entry point spelled out.
fn local_probe_source() -> String {
    format!(
        "{SANDBOX_TEST_CODE}
if __name__ == \"__main__\":
    main()
"
    )
}

/// Runs the probe with the configured interpreter, bounded by the configured
/// timeout, and reports the same fields upstream's `ExecutionResult` carries.
async fn run_local_probe(config: &Map<String, Value>) -> Result<Value, String> {
    let python_bin = config
        .get("python_bin")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("python3");
    let timeout_seconds = config
        .get("timeout")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(30) as u64;
    let started = Instant::now();
    let mut command = tokio::process::Command::new(python_bin);
    command
        .arg("-c")
        .arg(local_probe_source())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = tokio::time::timeout(Duration::from_secs(timeout_seconds), command.output())
        .await
        .map_err(|_| format!("Connection test timed out after {timeout_seconds} seconds"))?
        .map_err(|error| format!("Failed to start '{python_bin}': {error}"))?;
    let execution_time = started.elapsed().as_secs_f64();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1) as i64;
    let success = exit_code == 0 && stdout.contains("TEST_PASSED");
    Ok(test_payload(
        success,
        exit_code,
        execution_time,
        &stdout,
        &stderr,
    ))
}

/// Runs the probe through `sandbox-executor-manager` (`POST /run`), which is the
/// provider RayRAG's own code executor uses.
async fn run_self_managed_probe(
    state: &AppState,
    config: &Map<String, Value>,
) -> Result<Value, String> {
    let stored = stored_config(state, "self_managed");
    let endpoint = config
        .get("endpoint")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .or_else(|| stored.get("endpoint").and_then(Value::as_str))
        .map(str::to_string)
        .ok_or_else(|| "endpoint is required".to_string())?;
    let timeout = config
        .get("timeout")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(30) as u64;
    let client = crate::code_exec::SandboxClient::with_timeout(&endpoint, timeout);
    if !client.health_check().await {
        return Err(format!(
            "Connection test failed: {endpoint} is not reachable (GET /healthz)"
        ));
    }
    let mut arguments = Map::new();
    let request = crate::code_exec::CodeRequest {
        language: "python".into(),
        code: SANDBOX_TEST_CODE.into(),
        arguments: std::mem::take(&mut arguments),
    };
    let started = Instant::now();
    let result: crate::code_exec::SandboxResult = client
        .run(&request)
        .await
        .map_err(|error| format!("Connection test failed: {error}"))?;
    let execution_time = result
        .time_used_ms
        .map(|ms| ms / 1000.0)
        .unwrap_or_else(|| started.elapsed().as_secs_f64());
    let stdout = result.stdout.clone();
    let stderr = result.stderr.clone().unwrap_or_default();
    let exit_code = result.exit_code;
    let success = exit_code == 0 && stdout.contains("TEST_PASSED");
    Ok(test_payload(
        success,
        exit_code,
        execution_time,
        &stdout,
        &stderr,
    ))
}

/// Upstream opens a real SSH session; RayRAG has no SSH executor, so the probe
/// reports the reachability it can verify and says so instead of pretending the
/// code ran.
async fn run_ssh_probe(config: &Map<String, Value>) -> Result<Value, String> {
    let host = config
        .get("host")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "host is required".to_string())?;
    let port = config.get("port").and_then(Value::as_i64).unwrap_or(22);
    let started = Instant::now();
    let address = format!("{host}:{port}");
    let connected = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect(&address),
    )
    .await;
    let execution_time = started.elapsed().as_secs_f64();
    let reachable = matches!(connected, Ok(Ok(_)));
    let message = if reachable {
        format!(
            "Test FAILED | {address} accepted a TCP connection, but this build executes code in the local sandbox or through sandbox-executor-manager only, so the SSH probe was not run."
        )
    } else {
        format!("Test FAILED | Could not reach {address} within 5 seconds.")
    };
    Ok(json!({
        "success": false,
        "message": message,
        "details": {
            "exit_code": if reachable { 0 } else { 1 },
            "execution_time": execution_time,
            "stdout": if reachable { format!("TCP connection to {address} succeeded") } else { String::new() },
            "stderr": if reachable { String::new() } else { format!("Connection to {address} failed") },
        },
    }))
}

/// `POST /api/v1/admin/sandbox/test` — upstream body `{provider_type, config}`.
pub async fn test_connection(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<Value>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let provider_type = body
        .get("provider_type")
        .or_else(|| body.get("provider"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if provider_type.is_empty() {
        return error(StatusCode::BAD_REQUEST, "provider_type is required");
    }
    if sandbox_provider_schema(&provider_type).is_none() {
        return error(
            StatusCode::BAD_REQUEST,
            &format!("Unknown provider type: {provider_type}"),
        );
    }
    // Unsaved form values win, so "Test connection" reflects what is on screen.
    let mut config = stored_config(&state, &provider_type);
    if let Some(submitted) = body.get("config").and_then(Value::as_object) {
        for (key, value) in submitted {
            config.insert(key.clone(), value.clone());
        }
    }
    let result = match provider_type.as_str() {
        "local" => run_local_probe(&config).await,
        "self_managed" => run_self_managed_probe(&state, &config).await,
        "ssh" => run_ssh_probe(&config).await,
        other => Err(format!(
            "Connection test failed: the '{other}' provider is not implemented in this build. Its configuration is stored, but code execution is performed by the local sandbox or by sandbox-executor-manager."
        )),
    };
    match result {
        Ok(payload) => ok(payload),
        Err(message) => error(StatusCode::BAD_REQUEST, &message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_registry_matches_upstream() {
        let ids: Vec<&str> = SANDBOX_PROVIDERS.iter().map(|entry| entry.0).collect();
        assert_eq!(
            ids,
            vec![
                "local",
                "self_managed",
                "ssh",
                "aliyun_codeinterpreter",
                "e2b"
            ]
        );
        assert_eq!(SANDBOX_PROVIDERS[1].1, "Self-Managed");
        assert_eq!(
            SANDBOX_PROVIDERS[1].3,
            &["self-hosted", "low-latency", "secure"]
        );
        assert_eq!(SANDBOX_PROVIDERS[4].3, &["saas", "fast", "global"]);
    }

    #[test]
    fn schemas_match_the_upstream_field_sets() {
        let local = schema_json("local").unwrap();
        assert_eq!(local.as_object().unwrap().len(), 8);
        assert_eq!(local["python_bin"]["type"], "string");
        assert_eq!(local["python_bin"]["default"], "python3");
        assert_eq!(local["python_bin"]["required"], false);
        assert_eq!(local["timeout"]["min"], 1);
        assert_eq!(local["timeout"]["max"], 600);

        let ssh = schema_json("ssh").unwrap();
        assert_eq!(ssh.as_object().unwrap().len(), 14);
        assert_eq!(ssh["host"]["required"], true);
        assert_eq!(ssh["host"]["placeholder"], "192.168.1.10");
        assert_eq!(ssh["password"]["secret"], true);
        assert_eq!(ssh["private_key"]["multiline"], true);
        assert_eq!(ssh["port"]["default"], 22);
        assert!(ssh["host"].get("default").is_none());

        let self_managed = schema_json("self_managed").unwrap();
        assert_eq!(self_managed["endpoint"]["required"], true);
        assert_eq!(self_managed["endpoint"]["scope"], "runtime");
        assert_eq!(self_managed["executor_manager_image"]["readonly"], true);
        assert_eq!(
            self_managed["executor_manager_image"]["scope"],
            "deployment"
        );

        let aliyun = schema_json("aliyun_codeinterpreter").unwrap();
        assert_eq!(aliyun["access_key_secret"]["secret"], true);
        assert_eq!(
            aliyun["region"]["options"],
            json!([
                "cn-hangzhou",
                "cn-beijing",
                "cn-shanghai",
                "cn-shenzhen",
                "cn-guangzhou"
            ])
        );
        assert_eq!(aliyun["timeout"]["max"], 30);

        let e2b = schema_json("e2b").unwrap();
        assert_eq!(e2b.as_object().unwrap().len(), 3);
        assert_eq!(e2b["api_key"]["secret"], true);
        assert_eq!(e2b["region"]["default"], "us");

        assert!(schema_json("nope").is_none());
    }

    #[test]
    fn test_message_follows_the_upstream_shape() {
        let message = test_message(true, 0, 1.234, "TEST_PASSED\n", "");
        assert_eq!(
            message,
            "Test PASSED | Exit code: 0 | Execution time: 1.23s | Output: TEST_PASSED..."
        );
        let failure = test_message(false, 1, 0.5, "", "boom");
        assert_eq!(
            failure,
            "Test FAILED | Exit code: 1 | Execution time: 0.50s | Errors: boom..."
        );
    }
}
