//! AIMLAPI agent-authorization — RAGFlow v0.27.2 `common/aimlapi_utils.py`
//! and `api/apps/restful_apis/aimlapi_api.py`.
//!
//! OAuth 2.0 Device Authorization Grant (RFC 8628): the browser opens the
//! AIMLAPI consent page in a popup, the backend polls the token endpoint, and
//! only the issued API key ever reaches the browser. The device code is held
//! server-side, scoped to the requesting user. Upstream keeps it in Redis with
//! a TTL; RayRAG is a single binary, so an in-process TTL store plays the same
//! role (bounded divergence: single-instance deployments only).

use crate::Result;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DEFAULT_SOURCE: &str = "agent/ragflow";
pub const DEFAULT_PARTNER_ID: &str = "part_yNkcOvbGLtgxWLjy4sRysaer";
/// RFC 8628 device-code grant type.
pub const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const DEFAULT_APP_URL: &str = "https://app.aimlapi.com";
const DEFAULT_PARTNER_NAME: &str = "RAGFlow";
const DEFAULT_VERIFICATION_BASE_URL: &str = "https://aimlapi.com";
const DEFAULT_REQUESTED_USD_LIMIT_MINOR: i64 = 1000;
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RESPONSE_BYTES: usize = 16 << 20;

/// Environment override with the upstream default; an empty value keeps the
/// default (upstream would send an empty header, RayRAG treats it as unset).
fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

pub fn source() -> String {
    env_or("AIMLAPI_SOURCE", DEFAULT_SOURCE)
}

pub fn partner_id() -> String {
    env_or("AIMLAPI_PARTNER_ID", DEFAULT_PARTNER_ID)
}

/// Headers carried by every AIMLAPI request (`attribution_headers`).
pub fn attribution_headers() -> Vec<(&'static str, String)> {
    let mut headers = vec![("X-AIMLAPI-Source", source())];
    let partner = partner_id();
    if !partner.is_empty() {
        headers.push(("X-AIMLAPI-Partner-ID", partner));
    }
    headers
}

pub fn app_url() -> String {
    env_or("AIMLAPI_APP_URL", DEFAULT_APP_URL)
        .trim_end_matches('/')
        .to_owned()
}

pub fn partner_name() -> String {
    env_or("AIMLAPI_PARTNER_NAME", DEFAULT_PARTNER_NAME)
}

pub fn verification_base_url() -> String {
    env_or(
        "AIMLAPI_VERIFICATION_BASE_URL",
        DEFAULT_VERIFICATION_BASE_URL,
    )
    .trim_end_matches('/')
    .to_owned()
}

pub fn requested_usd_limit_minor() -> i64 {
    std::env::var("AIMLAPI_REQUESTED_USD_LIMIT_MINOR")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(DEFAULT_REQUESTED_USD_LIMIT_MINOR)
}

/// Response of the `/authorize/start` flow (browser-facing fields only).
#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizationStart {
    pub request_id: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

/// Outcome of one `/authorize/poll` call.
#[derive(Debug, Clone, PartialEq)]
pub enum PollOutcome {
    Ready { api_key: String },
    Pending { status: String },
    Expired,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[derive(Debug, Clone)]
struct DeviceEntry {
    device_code: String,
    user_id: String,
    expires_at_ms: u64,
}

/// Server-side device-code store (`aimlapi_authz:<request_id>` upstream).
#[derive(Debug, Default)]
pub struct DeviceCodeStore {
    entries: Mutex<HashMap<String, DeviceEntry>>,
}

impl DeviceCodeStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn purge_expired(entries: &mut HashMap<String, DeviceEntry>) {
        let now = now_ms();
        entries.retain(|_, entry| entry.expires_at_ms > now);
    }

    pub fn set(&self, request_id: &str, device_code: &str, user_id: &str, ttl: Duration) {
        let mut entries = self.entries.lock().unwrap();
        Self::purge_expired(&mut entries);
        entries.insert(
            request_id.to_owned(),
            DeviceEntry {
                device_code: device_code.to_owned(),
                user_id: user_id.to_owned(),
                expires_at_ms: now_ms() + ttl.as_millis() as u64,
            },
        );
    }

    /// Returns the stored `(device_code, user_id)` when unexpired.
    pub fn get(&self, request_id: &str) -> Option<(String, String)> {
        let mut entries = self.entries.lock().unwrap();
        Self::purge_expired(&mut entries);
        entries
            .get(request_id)
            .map(|entry| (entry.device_code.clone(), entry.user_id.clone()))
    }

    pub fn delete(&self, request_id: &str) {
        self.entries.lock().unwrap().remove(request_id);
    }

    
}

/// Async AIMLAPI authorization client.
#[derive(Debug, Clone)]
pub struct AimlapiClient {
    client: reqwest::Client,
    app_url: String,
    verification_base_url: String,
    partner_id: String,
    partner_name: String,
    source: String,
    requested_usd_limit_minor: i64,
}

impl AimlapiClient {
    pub fn new() -> Result<Self> {
        Self::with_settings(
            app_url(),
            verification_base_url(),
            partner_id(),
            partner_name(),
            source(),
            requested_usd_limit_minor(),
        )
    }

    pub(crate) fn with_settings(
        app_url: String,
        verification_base_url: String,
        partner_id: String,
        partner_name: String,
        source: String,
        requested_usd_limit_minor: i64,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(crate::common::cmd_timeout::duration())
            .build()
            .map_err(|error| anyhow::anyhow!("could not build AIMLAPI HTTP client: {error}"))?;
        Ok(Self {
            client,
            app_url,
            verification_base_url,
            partner_id,
            partner_name,
            source,
            requested_usd_limit_minor,
        })
    }

    fn headers(&self) -> Vec<(&'static str, String)> {
        let mut headers = vec![("X-AIMLAPI-Source", self.source.clone())];
        if !self.partner_id.is_empty() {
            headers.push(("X-AIMLAPI-Partner-ID", self.partner_id.clone()));
        }
        headers
    }

    async fn post_json(&self, url: &str, payload: &Value) -> Result<(reqwest::StatusCode, String)> {
        let mut request = self
            .client
            .post(url)
            .timeout(HTTP_TIMEOUT)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(payload);
        for (name, value) in self.headers() {
            request = request.header(name, value);
        }
        let response = request
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("AIMLAPI request failed: {error}"))?;
        let status = response.status();
        let body = read_bounded_body(response).await?;
        Ok((status, String::from_utf8_lossy(&body).into_owned()))
    }

    /// `aimlapi_authorize_start`: create the device-authorization request and
    /// hold its code server-side for `expires_in` seconds.
    pub async fn start(
        &self,
        store: &DeviceCodeStore,
        user_id: &str,
    ) -> Result<AuthorizationStart> {
        if self.partner_id.is_empty() {
            anyhow::bail!("AIMLAPI partner id is not configured. Set AIMLAPI_PARTNER_ID.");
        }
        // returnUrl only controls where AIMLAPI sends the browser after
        // consent and never changes this flow, so the trusted verification
        // host is always used (no open-redirect surface).
        let payload = json!({
            "partnerId": self.partner_id,
            "partnerName": self.partner_name,
            "agentName": "RayRAG",
            "returnUrl": self.verification_base_url,
            "requestedUsdLimitMinor": self.requested_usd_limit_minor,
        });
        let (status, body) = self
            .post_json(
                &format!("{}/v3/agent-auth/authorizations", self.app_url),
                &payload,
            )
            .await?;
        if status != reqwest::StatusCode::OK && status != reqwest::StatusCode::CREATED {
            tracing::warn!(%status, body = %truncate(&body, 300), "AIMLAPI authorize start failed");
            anyhow::bail!("AIMLAPI authorization request failed (HTTP {status}).");
        }
        let data: Value = serde_json::from_str(&body)
            .map_err(|error| anyhow::anyhow!("AIMLAPI authorization response decode: {error}"))?;
        let request_id = data
            .get("requestId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let device_code = data
            .get("deviceCode")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if request_id.is_empty() || device_code.is_empty() {
            anyhow::bail!("AIMLAPI authorization response is missing requestId/deviceCode.");
        }
        let interval = int_or(&data, "interval", 5);
        let expires_in = int_or(&data, "expiresIn", 900);
        store.set(
            request_id,
            device_code,
            user_id,
            Duration::from_secs(expires_in.max(1)),
        );
        tracing::info!(request_id, "AIMLAPI authorize start ok");
        // Rebuild the consent URL so an env override applies; `source` carries
        // the same client identifier as the headers, since the sign-up happens
        // in a browser flow where a header cannot reach.
        let mut verification_uri =
            reqwest::Url::parse(&format!("{}/agent/authorize", self.verification_base_url))
                .map_err(|error| {
                    anyhow::anyhow!("invalid AIMLAPI verification base URL: {error}")
                })?;
        verification_uri
            .query_pairs_mut()
            .append_pair("request", request_id)
            .append_pair("source", &self.source);
        Ok(AuthorizationStart {
            request_id: request_id.to_owned(),
            verification_uri: verification_uri.into(),
            interval,
            expires_in,
        })
    }

    /// `aimlapi_authorize_poll`: poll the token endpoint until the key is
    /// issued or AIMLAPI reports a terminal state.
    pub async fn poll(
        &self,
        store: &DeviceCodeStore,
        user_id: &str,
        request_id: &str,
    ) -> Result<PollOutcome> {
        let Some((device_code, owner)) = store.get(request_id) else {
            return Ok(PollOutcome::Expired);
        };
        if owner != user_id {
            anyhow::bail!("Authorization request not found for the current user.");
        }
        let payload = json!({
            "partnerId": self.partner_id,
            "deviceCode": device_code,
            "grant_type": DEVICE_CODE_GRANT,
        });
        let (status, body) = self
            .post_json(&format!("{}/v3/agent-auth/token", self.app_url), &payload)
            .await?;
        if status != reqwest::StatusCode::OK && status != reqwest::StatusCode::CREATED {
            tracing::warn!(%status, body = %truncate(&body, 300), "AIMLAPI authorize poll failed");
            anyhow::bail!("AIMLAPI token poll failed (HTTP {status}).");
        }
        let data: Value = serde_json::from_str(&body)
            .map_err(|error| anyhow::anyhow!("AIMLAPI token response decode: {error}"))?;
        let status = data
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        // The success field name is not contractually fixed yet; accept the
        // common variants.
        let api_key = ["apiKey", "api_key", "access_token", "key"]
            .iter()
            .find_map(|field| data.get(*field).and_then(Value::as_str))
            .filter(|key| !key.trim().is_empty());
        if let Some(api_key) = api_key {
            store.delete(request_id);
            tracing::info!(request_id, "AIMLAPI authorize poll ready");
            return Ok(PollOutcome::Ready {
                api_key: api_key.to_owned(),
            });
        }
        if matches!(
            status.as_str(),
            "denied" | "expired" | "cancelled" | "canceled" | "rejected"
        ) {
            store.delete(request_id);
            return Ok(PollOutcome::Pending { status });
        }
        Ok(PollOutcome::Pending {
            status: if status.is_empty() {
                "pending".to_owned()
            } else {
                status
            },
        })
    }
}

fn int_or(data: &Value, field: &str, default: u64) -> u64 {
    data.get(field)
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
                .or_else(|| value.as_i64().map(|number| number.max(0) as u64))
        })
        .unwrap_or(default)
}

fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

async fn read_bounded_body(response: reqwest::Response) -> Result<Vec<u8>> {
    use futures_util::StreamExt;
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| anyhow::anyhow!("AIMLAPI response read: {error}"))?;
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            anyhow::bail!("AIMLAPI response exceeds {MAX_RESPONSE_BYTES} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}


