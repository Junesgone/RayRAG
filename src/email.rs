//! Email 连接器 — RAGFlow `email.py` 的 Rust 实现
//!
//! 上游语义：SMTP STARTTLS + 认证发送，支持收件人/抄送、HTML 正文、发送者显示名，
//! 可重试连接类错误，错误按类别返回（认证失败/连接失败/SMTP 错误/通用错误）。
//! 凭据从节点参数或环境变量读取，测试不触碰真实 SMTP。

use anyhow::{Result, bail};
use lettre::message::{Mailbox, Message, MultiPart, SinglePart, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{SmtpTransport, Transport};

/// SMTP 配置（对齐上游 EmailParam 固定配置项）。
#[derive(Debug, Clone, Default)]
pub struct EmailConfig {
    pub smtp_server: String,
    pub smtp_port: u16,
    pub email: String,
    pub smtp_username: String,
    pub password: String,
    pub sender_name: String,
}

/// 邮件请求（对齐上游 ToolMeta 参数）。
#[derive(Debug, Clone, Default)]
pub struct EmailRequest {
    pub to_email: String,
    pub cc_email: String,
    pub content: String,
    pub subject: String,
}

/// 邮件客户端。
#[derive(Debug, Clone, Default)]
pub struct EmailClient;

impl EmailClient {
    /// 发送邮件；返回成功布尔 + 可能的中文错误消息（对齐上游输出形状）。
    pub fn send(&self, config: &EmailConfig, request: &EmailRequest) -> Result<(bool, String)> {
        if config.smtp_server.trim().is_empty()
            || config.email.trim().is_empty()
            || config.password.trim().is_empty()
        {
            bail!(
                "SMTP server, sender email and password are required (node params or env EMAIL_SMTP_*)"
            );
        }
        let to_email = request.to_email.trim();
        if to_email.is_empty() {
            bail!("Missing required field: to_email");
        }

        let from_name = if config.sender_name.trim().is_empty() {
            None
        } else {
            Some(config.sender_name.trim().to_string())
        };
        let from: Mailbox = format!("{} <{}>", from_name.unwrap_or_default(), config.email)
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid sender email: {}", config.email))?;
        let to: Mailbox = to_email
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid to_email: {to_email}"))?;

        let subject = if request.subject.trim().is_empty() {
            "No Subject"
        } else {
            request.subject.trim()
        };
        let content = if request.content.trim().is_empty() {
            "No content provided"
        } else {
            request.content.trim()
        };

        // MIME multipart/alternative：HTML 正文（上游只挂 html），附纯文本降级
        let mut builder = Message::builder().from(from).to(to).subject(subject);

        let cc_email = request.cc_email.trim();
        if !cc_email.is_empty() {
            for address in cc_email.split(',') {
                let address = address.trim();
                if address.is_empty() {
                    continue;
                }
                let cc: Mailbox = address
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid cc_email: {address}"))?;
                builder = builder.cc(cc);
            }
        }
        let message = builder
            .multipart(
                MultiPart::alternative()
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(content.to_string()),
                    )
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(content.to_string()),
                    ),
            )
            .map_err(|error| anyhow::anyhow!("Email build error: {error}"))?;

        let username = if config.smtp_username.trim().is_empty() {
            config.email.clone()
        } else {
            config.smtp_username.clone()
        };
        let credentials = Credentials::new(username, config.password.clone());

        // STARTTLS（上游 smtplib.SMTP + starttls）
        let transport = SmtpTransport::builder_dangerous(config.smtp_server.as_str())
            .port(config.smtp_port)
            .credentials(credentials)
            .timeout(Some(crate::common::cmd_timeout::duration()))
            .build();

        match transport.send(&message) {
            Ok(_) => Ok((true, String::new())),
            Err(error) => {
                let message = classify_smtp_error(error, config);
                Ok((false, message))
            }
        }
    }
}

/// 错误分类（对齐上游 except 分支的提示语）。
fn classify_smtp_error(error: lettre::transport::smtp::Error, config: &EmailConfig) -> String {
    // 535/534 = 5,3,5 / 5,3,4（severity/category/detail）
    let auth_failed = error.is_permanent()
        && error
            .status()
            .is_some_and(|code| code.severity as u8 == 5 && code.category as u8 == 3);
    if auth_failed {
        return "SMTP Authentication failed. Please check your SMTP username(email) and authorization code.".to_string();
    }
    if error.is_transport_shutdown() || error.is_tls() || error.is_timeout() || error.is_response()
    {
        return format!(
            "Failed to connect to SMTP server {}:{} ({error})",
            config.smtp_server, config.smtp_port
        );
    }
    format!("SMTP error occurred: {error}")
}

/// 从节点参数/环境变量构造配置（运行时由 agent.rs 调用）。
impl EmailConfig {
    pub fn from_params_and_env(params: &serde_json::Map<String, serde_json::Value>) -> Self {
        let read = |key: &str, env: &str| -> String {
            params
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| std::env::var(env).unwrap_or_default())
        };
        let port = params
            .get("smtp_port")
            .and_then(serde_json::Value::as_u64)
            .map(|value| value as u16)
            .unwrap_or_else(|| {
                std::env::var("EMAIL_SMTP_PORT")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(465)
            });
        Self {
            smtp_server: read("smtp_server", "EMAIL_SMTP_SERVER"),
            smtp_port: port,
            email: read("email", "EMAIL_SENDER"),
            smtp_username: read("smtp_username", "EMAIL_SMTP_USERNAME"),
            password: read("password", "EMAIL_SMTP_PASSWORD"),
            sender_name: read("sender_name", "EMAIL_SENDER_NAME"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_config() -> EmailConfig {
        EmailConfig {
            smtp_server: "smtp.example.com".into(),
            smtp_port: 587,
            email: "sender@example.com".into(),
            smtp_username: "sender@example.com".into(),
            password: "secret".into(),
            sender_name: "测试发送者".into(),
        }
    }

    #[test]
    fn rejects_missing_config() {
        let client = EmailClient;
        let result = client.send(
            &EmailConfig::default(),
            &EmailRequest {
                to_email: "a@b.com".into(),
                ..Default::default()
            },
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("SMTP server"));
    }

    #[test]
    fn rejects_missing_recipient() {
        let client = EmailClient;
        let result = client.send(&sample_config(), &EmailRequest::default());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("to_email"));
    }

    #[test]
    fn connection_failure_is_classified_as_transport_error() {
        // 127.0.0.1:1 不可达 → 连接类错误
        let client = EmailClient;
        let config = EmailConfig {
            smtp_server: "127.0.0.1".into(),
            smtp_port: 1,
            ..sample_config()
        };
        let (success, message) = client
            .send(
                &config,
                &EmailRequest {
                    to_email: "a@b.com".into(),
                    subject: "hello".into(),
                    content: "body".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(!success, "unexpected success");
        assert!(
            message.contains("SMTP error") || message.contains("Failed to connect"),
            "unexpected message: {message}"
        );
    }

    #[test]
    fn config_from_params_and_env_prefers_params() {
        let params = serde_json::Map::from_iter([
            ("smtp_server".into(), json!("smtp.param.com")),
            ("email".into(), json!("p@param.com")),
        ]);
        unsafe { std::env::set_var("EMAIL_SMTP_SERVER", "smtp.env.com") };
        let config = EmailConfig::from_params_and_env(&params);
        assert_eq!(config.smtp_server, "smtp.param.com");
        assert_eq!(config.smtp_port, 465);
        assert_eq!(config.email, "p@param.com");
        unsafe { std::env::remove_var("EMAIL_SMTP_SERVER") };
    }
}
