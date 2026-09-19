//! Remote code execution over SSH — the Rust port of RAGFlow's
//! `agent/sandbox/providers/ssh.py` execution path.
//!
//! Upstream drives a Paramiko client (connect → SFTP workspace → `exec_command`
//! → structured-result marker → artifact collection). RayRAG keeps the same
//! *contract* — the provider config field set, the `python_bin`/`node_bin`
//! interpreters, the remote `work_dir`, the `timeout`, and the
//! `{exit_code, stdout, stderr, execution_time}` result — but talks to the
//! remote host through the system OpenSSH client (`ssh`), which keeps the
//! dependency surface small and lets operators reuse their existing keys,
//! `known_hosts` and `~/.ssh/config`. Password authentication goes through
//! `sshpass`, exactly like the key/password pair upstream accepts.
//!
//! Everything here is deliberately side-effect free outside the remote host: the
//! private key, when supplied inline, is written to a `0600` temporary file and
//! removed as soon as the command finishes.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Upstream `SSHProvider` defaults (`agent/sandbox/providers/ssh.py`).
pub const DEFAULT_SSH_PORT: u16 = 22;
pub const DEFAULT_PYTHON_BIN: &str = "python3";
pub const DEFAULT_NODE_BIN: &str = "node";
pub const DEFAULT_WORK_DIR: &str = "/tmp";
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// The executable languages upstream's SSH provider advertises.
pub const SSH_LANGUAGES: [&str; 3] = ["python", "javascript", "nodejs"];

/// What a single remote run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshOutcome {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub execution_time_ms: u128,
}

/// The parsed provider configuration (upstream `get_config_schema` field names).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    pub private_key: Option<String>,
    pub passphrase: Option<String>,
    pub known_hosts: Option<String>,
    pub python_bin: String,
    pub node_bin: String,
    pub work_dir: String,
    pub timeout_seconds: u64,
    pub max_output_bytes: usize,
}

impl SshConfig {
    /// Read the upstream field names out of a provider config map. Only `host`
    /// and `username` are mandatory, matching `validate_config`.
    pub fn from_config(config: &Map<String, Value>) -> Result<Self, String> {
        let text = |key: &str| -> Option<String> {
            config
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        // Credentials keep their exact bytes: `ssh -i` refuses a key whose PEM
        // trailer lost its newline, so they are only inspected for emptiness.
        let raw = |key: &str| -> Option<String> {
            config
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
        };
        let number = |key: &str| -> Option<i64> { config.get(key).and_then(Value::as_i64) };
        let host = text("host").ok_or_else(|| "host is required".to_string())?;
        let username = text("username").ok_or_else(|| "username is required".to_string())?;
        let port = match number("port") {
            Some(port) if (1..=65535).contains(&port) => port as u16,
            Some(port) => return Err(format!("port {port} is outside 1-65535")),
            None => DEFAULT_SSH_PORT,
        };
        let password = raw("password");
        let private_key = raw("private_key");
        if password.is_none() && private_key.is_none() {
            return Err("either password or private_key is required to authenticate".to_string());
        }
        let timeout_seconds = match number("timeout") {
            Some(value) if (1..=600).contains(&value) => value as u64,
            Some(value) => return Err(format!("timeout {value} is outside 1-600 seconds")),
            None => DEFAULT_TIMEOUT_SECONDS,
        };
        let max_output_bytes = match number("max_output_bytes") {
            Some(value) if value > 0 => value as usize,
            Some(_) => return Err("max_output_bytes must be positive".to_string()),
            None => DEFAULT_MAX_OUTPUT_BYTES,
        };
        Ok(Self {
            host,
            port,
            username,
            password,
            private_key,
            passphrase: raw("passphrase"),
            known_hosts: text("known_hosts"),
            python_bin: text("python_bin").unwrap_or_else(|| DEFAULT_PYTHON_BIN.to_string()),
            node_bin: text("node_bin").unwrap_or_else(|| DEFAULT_NODE_BIN.to_string()),
            work_dir: text("work_dir").unwrap_or_else(|| DEFAULT_WORK_DIR.to_string()),
            timeout_seconds,
            max_output_bytes,
        })
    }

    /// Interpreter for an upstream language name (`python` / `javascript` /
    /// `nodejs`), or `None` for an unsupported one.
    pub fn interpreter(&self, language: &str) -> Option<&str> {
        match language.trim().to_ascii_lowercase().as_str() {
            "python" | "python3" => Some(self.python_bin.as_str()),
            "javascript" | "js" | "nodejs" | "node" => Some(self.node_bin.as_str()),
            _ => None,
        }
    }

    /// The `ssh` argument vector, before the remote command. Inline keys become a
    /// `-i` file; `sshpass` wraps the whole call when a password is configured.
    pub fn ssh_arguments(&self, key_path: Option<&PathBuf>) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        args.push("-p".into());
        args.push(self.port.to_string());
        args.push("-l".into());
        args.push(self.username.clone());
        args.push("-o".into());
        args.push("BatchMode=yes".into());
        args.push("-o".into());
        args.push("ConnectTimeout=10".into());
        args.push("-o".into());
        match self.known_hosts.as_deref() {
            // Operators who pin a known_hosts file get strict verification; the
            // default matches `set_missing_host_key_policy(RejectPolicy())` only
            // when the host is already known to OpenSSH.
            Some(path) => {
                args.push(format!("UserKnownHostsFile={path}"));
                args.push("-o".into());
                args.push("StrictHostKeyChecking=yes".into());
            }
            None => {
                args.push("StrictHostKeyChecking=accept-new".into());
            }
        }
        if let Some(path) = key_path {
            args.push("-i".into());
            args.push(path.to_string_lossy().to_string());
        }
        args.push("--".into());
        args.push(self.host.clone());
        args
    }

    /// The full remote shell command for a snippet and language. The snippet itself is
    /// piped in over stdin (`_code` only documents the call), so no quoting rules apply.
    pub fn remote_command(&self, language: &str, _code: &str) -> Result<String, String> {
        let interpreter = self
            .interpreter(language)
            .ok_or_else(|| format!("Unsupported language for SSH execution: {language}"))?;
        let workspace = format!("{}/rayrag-ssh", self.work_dir.trim_end_matches('/'));
        let name = if interpreter.contains("node") {
            "main.js"
        } else {
            "main.py"
        };
        // The snippet travels over stdin, so nothing has to be uploaded first and
        // no quoting rules can corrupt it.
        Ok(format!(
            "set -e; mkdir -p {workspace} && cd {workspace} && cat > {name} && {interpreter} -I -B {name}",
            workspace = shell_quote(&workspace),
            name = name,
            interpreter = shell_quote(interpreter),
        ))
    }
}

/// Single-quote a token for the remote shell.
fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+:=@".contains(c))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Whether the tools this module shells out to are present.
pub fn tooling_available() -> (bool, bool) {
    let ssh = which("ssh");
    let sshpass = which("sshpass");
    (ssh, sshpass)
}

fn which(binary: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let candidate = dir.join(binary);
                candidate.is_file()
            })
        })
        .unwrap_or(false)
}

fn truncate_output(bytes: &[u8], limit: usize) -> String {
    let slice = if bytes.len() > limit {
        &bytes[..limit]
    } else {
        bytes
    };
    let mut text = String::from_utf8_lossy(slice).to_string();
    if bytes.len() > limit {
        text.push_str("\n… output truncated …");
    }
    text
}

/// Run `code` on the remote host through OpenSSH.
///
/// Mirrors upstream's `execute_code`: the snippet is piped to the configured
/// interpreter in a workspace under `work_dir`, stdout/stderr are capped by
/// `max_output_bytes`, and the whole call is bounded by `timeout` (a killed
/// client reports `-1`, the executor-manager timeout convention).
pub async fn execute(config: &SshConfig, language: &str, code: &str) -> Result<SshOutcome, String> {
    let (has_ssh, has_sshpass) = tooling_available();
    if !has_ssh {
        return Err(
            "the OpenSSH client (`ssh`) is not installed, so the SSH provider cannot run code"
                .to_string(),
        );
    }
    if config.password.is_some() && !has_sshpass {
        return Err(
            "password authentication needs `sshpass`, which is not installed; use a private key or install sshpass".to_string(),
        );
    }
    let remote = config.remote_command(language, code)?;

    // An inline key is materialised as a 0600 temporary file for the duration of
    // the call and removed afterwards.
    let mut key_file: Option<PathBuf> = None;
    let mut known_hosts_file: Option<PathBuf> = None;
    if let Some(key) = config.private_key.as_deref() {
        let path = std::env::temp_dir().join(format!(
            "rayrag-ssh-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        std::fs::write(&path, key).map_err(|error| format!("cannot write private key: {error}"))?;
        restrict_permissions(&path);
        key_file = Some(path);
    }
    // `sshpass` reads the password from a file when `-f` is used, which keeps it
    // out of the process table.
    let mut password_file: Option<PathBuf> = None;
    if let Some(password) = config.password.as_deref() {
        let path = std::env::temp_dir().join(format!(
            "rayrag-sshpass-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        std::fs::write(&path, password)
            .map_err(|error| format!("cannot write password file: {error}"))?;
        restrict_permissions(&path);
        password_file = Some(path);
    }
    if let Some(known_hosts) = config.known_hosts.as_deref() {
        if std::path::Path::new(known_hosts).is_file() {
            known_hosts_file = Some(PathBuf::from(known_hosts));
        }
    }

    let mut program = "ssh".to_string();
    let mut args: Vec<String> = Vec::new();
    if let Some(password_file) = password_file.as_ref() {
        program = "sshpass".to_string();
        args.push("-f".into());
        args.push(password_file.to_string_lossy().to_string());
        args.push("ssh".into());
    }
    args.extend(config.ssh_arguments(key_file.as_ref()));
    args.push(remote);

    let started = Instant::now();
    let mut child = Command::new(&program)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("cannot start {program}: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let payload = code.as_bytes().to_vec();
        tokio::spawn(async move {
            let _ = stdin.write_all(&payload).await;
            let _ = stdin.shutdown().await;
        });
    }
    let waited = tokio::time::timeout(
        Duration::from_secs(config.timeout_seconds),
        child.wait_with_output(),
    )
    .await;
    let execution_time_ms = started.elapsed().as_millis();

    for path in [key_file.as_ref(), password_file.as_ref()]
        .into_iter()
        .flatten()
    {
        let _ = std::fs::remove_file(path);
    }
    let _ = known_hosts_file;

    match waited {
        Ok(Ok(output)) => Ok(SshOutcome {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: truncate_output(&output.stdout, config.max_output_bytes),
            stderr: truncate_output(&output.stderr, config.max_output_bytes),
            execution_time_ms,
        }),
        Ok(Err(error)) => Err(format!("SSH execution failed: {error}")),
        Err(_) => Err(format!(
            "SSH execution timed out after {} seconds",
            config.timeout_seconds
        )),
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) {}

/// The admin console's "Test" payload, shaped exactly like the other providers
/// (`{success, message, details{exit_code, execution_time, stdout, stderr}}`).
pub async fn connection_test(config: &SshConfig) -> Value {
    let started = Instant::now();
    match execute(config, "python", "print('rayrag-ssh-ok')").await {
        Ok(outcome) => {
            let ok = outcome.exit_code == 0;
            let message = if ok {
                format!(
                    "Test PASSED | ran python on {}:{} in {:.3}s",
                    config.host,
                    config.port,
                    started.elapsed().as_secs_f64()
                )
            } else {
                format!(
                    "Test FAILED | remote python exited with code {}",
                    outcome.exit_code
                )
            };
            let mut details = BTreeMap::new();
            details.insert("exit_code".to_string(), json!(outcome.exit_code));
            details.insert(
                "execution_time".to_string(),
                json!(started.elapsed().as_secs_f64()),
            );
            details.insert("stdout".to_string(), json!(outcome.stdout));
            details.insert("stderr".to_string(), json!(outcome.stderr));
            json!({ "success": ok, "message": message, "details": details })
        }
        Err(message) => {
            let unreachable = message.contains("timed out")
                || message.contains("Permission denied")
                || message.contains("Connection");
            json!({
                "success": false,
                "message": format!("Test FAILED | {message}"),
                "details": {
                    "exit_code": if unreachable { 255 } else { 1 },
                    "execution_time": started.elapsed().as_secs_f64(),
                    "stdout": "",
                    "stderr": message,
                },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(pairs: &[(&str, Value)]) -> SshConfig {
        let mut map = Map::new();
        for (key, value) in pairs {
            map.insert((*key).to_string(), value.clone());
        }
        SshConfig::from_config(&map).unwrap()
    }

    #[test]
    fn config_matches_the_upstream_field_set_and_defaults() {
        let parsed = config(&[
            ("host", json!("192.168.1.10")),
            ("username", json!("ragflow")),
            ("password", json!("secret")),
        ]);
        assert_eq!(parsed.port, DEFAULT_SSH_PORT);
        assert_eq!(parsed.python_bin, DEFAULT_PYTHON_BIN);
        assert_eq!(parsed.node_bin, DEFAULT_NODE_BIN);
        assert_eq!(parsed.work_dir, DEFAULT_WORK_DIR);
        assert_eq!(parsed.timeout_seconds, DEFAULT_TIMEOUT_SECONDS);
        assert_eq!(parsed.max_output_bytes, DEFAULT_MAX_OUTPUT_BYTES);
        assert_eq!(parsed.interpreter("python"), Some("python3"));
        assert_eq!(parsed.interpreter("nodejs"), Some("node"));
        assert_eq!(parsed.interpreter("ruby"), None);
        assert_eq!(SSH_LANGUAGES, ["python", "javascript", "nodejs"]);
    }

    #[test]
    fn config_requires_host_username_and_one_credential() {
        let mut map = Map::new();
        assert!(SshConfig::from_config(&map).is_err());
        map.insert("host".into(), json!("host"));
        assert!(
            SshConfig::from_config(&map)
                .unwrap_err()
                .contains("username")
        );
        map.insert("username".into(), json!("user"));
        assert!(
            SshConfig::from_config(&map)
                .unwrap_err()
                .contains("password or private_key")
        );
        map.insert("private_key".into(), json!("-----BEGIN"));
        assert!(SshConfig::from_config(&map).is_ok());
    }

    #[test]
    fn config_rejects_out_of_range_port_and_timeout() {
        let bad_port = Map::from_iter([
            ("host".to_string(), json!("h")),
            ("username".to_string(), json!("u")),
            ("password".to_string(), json!("p")),
            ("port".to_string(), json!(70000)),
        ]);
        assert!(
            SshConfig::from_config(&bad_port)
                .unwrap_err()
                .contains("1-65535")
        );
        let bad_timeout = Map::from_iter([
            ("host".to_string(), json!("h")),
            ("username".to_string(), json!("u")),
            ("password".to_string(), json!("p")),
            ("timeout".to_string(), json!(9000)),
        ]);
        assert!(
            SshConfig::from_config(&bad_timeout)
                .unwrap_err()
                .contains("1-600")
        );
    }

    #[test]
    fn credentials_keep_their_exact_bytes() {
        // `ssh -i` rejects a key whose PEM trailer lost its newline ("error in
        // libcrypto"), so secrets must not be trimmed the way host names are.
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----\n";
        let parsed = config(&[
            ("host", json!("  host.example  ")),
            ("username", json!(" ragflow ")),
            ("private_key", json!(pem)),
            ("passphrase", json!(" pass phrase \n")),
        ]);
        assert_eq!(parsed.host, "host.example");
        assert_eq!(parsed.username, "ragflow");
        assert_eq!(parsed.private_key.as_deref(), Some(pem));
        assert_eq!(parsed.passphrase.as_deref(), Some(" pass phrase \n"));
        assert!(parsed.password.is_none());
    }

    #[test]
    fn ssh_arguments_carry_port_user_and_key() {
        let parsed = config(&[
            ("host", json!("example.com")),
            ("username", json!("ragflow")),
            ("port", json!(2222)),
            ("private_key", json!("k")),
        ]);
        let key = PathBuf::from("/tmp/key");
        let args = parsed.ssh_arguments(Some(&key));
        assert_eq!(args[0], "-p");
        assert_eq!(args[1], "2222");
        assert_eq!(args[2], "-l");
        assert_eq!(args[3], "ragflow");
        assert!(args.contains(&"BatchMode=yes".to_string()));
        assert!(args.contains(&"StrictHostKeyChecking=accept-new".to_string()));
        assert!(args.contains(&"/tmp/key".to_string()));
        assert_eq!(args[args.len() - 1], "example.com");
    }

    #[test]
    fn known_hosts_pins_strict_verification() {
        let parsed = config(&[
            ("host", json!("h")),
            ("username", json!("u")),
            ("password", json!("p")),
            ("known_hosts", json!("/etc/ragflow/ssh_known_hosts")),
        ]);
        let args = parsed.ssh_arguments(None);
        assert!(args.contains(&"UserKnownHostsFile=/etc/ragflow/ssh_known_hosts".to_string()));
        assert!(args.contains(&"StrictHostKeyChecking=yes".to_string()));
    }

    #[test]
    fn remote_command_pipes_the_snippet_through_the_configured_binary() {
        let parsed = config(&[
            ("host", json!("h")),
            ("username", json!("u")),
            ("password", json!("p")),
            ("work_dir", json!("/srv/work")),
            ("python_bin", json!("/usr/bin/python3.11")),
        ]);
        let command = parsed.remote_command("python", "print(1)").unwrap();
        assert!(command.contains("mkdir -p /srv/work/rayrag-ssh"));
        assert!(command.contains("cat > main.py"));
        assert!(command.contains("/usr/bin/python3.11 -I -B main.py"));
        let node = parsed.remote_command("nodejs", "console.log(1)").unwrap();
        assert!(node.contains("cat > main.js"));
        assert!(node.contains("node -I -B main.js"));
        assert!(parsed.remote_command("ruby", "puts 1").is_err());
    }

    #[test]
    fn shell_quoting_survives_spaces_and_quotes() {
        assert_eq!(shell_quote("/tmp/plain"), "/tmp/plain");
        assert_eq!(shell_quote("/tmp/with space"), "'/tmp/with space'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn output_is_capped_at_max_output_bytes() {
        let payload = vec![b'x'; 64];
        assert_eq!(truncate_output(&payload, 1024).len(), 64);
        let capped = truncate_output(&payload, 16);
        assert!(capped.starts_with(&"x".repeat(16)));
        assert!(capped.ends_with("… output truncated …"));
    }

    #[tokio::test]
    async fn execute_rejects_unsupported_languages_before_spawning() {
        let parsed = config(&[
            ("host", json!("127.0.0.1")),
            ("username", json!("nobody")),
            ("password", json!("nope")),
        ]);
        let error = execute(&parsed, "ruby", "puts 1").await.unwrap_err();
        assert!(error.contains("Unsupported language"), "{error}");
    }
}
