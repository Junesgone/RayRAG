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

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::post,
    };
    use std::sync::{Arc, Mutex as StdMutex};

    #[derive(Clone, Default)]
    struct Seen {
        start: Arc<StdMutex<Vec<Value>>>,
        poll: Arc<StdMutex<Vec<Value>>>,
    }

    fn client(base: &str) -> AimlapiClient {
        AimlapiClient::with_settings(
            base.to_owned(),
            "https://verify.example".to_owned(),
            "part_test".to_owned(),
            "RAGFlow".to_owned(),
            "agent/ragflow".to_owned(),
            1000,
        )
        .unwrap()
    }

    async fn spawn_app(
        token_status: &'static str,
        api_key_body: Value,
    ) -> (String, Seen, tokio::task::JoinHandle<()>) {
        let seen = Seen::default();
        let start_seen = seen.start.clone();
        let poll_seen = seen.poll.clone();
        let app = Router::new()
            .route(
                "/v3/agent-auth/authorizations",
                post(move |headers: HeaderMap, body: String| {
                    let seen = start_seen.clone();
                    async move {
                        assert_eq!(headers["x-aimlapi-source"], "agent/ragflow");
                        assert_eq!(headers["x-aimlapi-partner-id"], "part_test");
                        seen.lock()
                            .unwrap()
                            .push(serde_json::from_str(&body).unwrap_or(Value::Null));
                        Json(json!({
                            "requestId": "req-1",
                            "deviceCode": "dev-1",
                            "interval": 5,
                            "expiresIn": 900
                        }))
                    }
                }),
            )
            .route(
                "/v3/agent-auth/token",
                post(move |body: String| {
                    let seen = poll_seen.clone();
                    async move {
                        seen.lock()
                            .unwrap()
                            .push(serde_json::from_str(&body).unwrap_or(Value::Null));
                        let status =
                            StatusCode::from_u16(if token_status == "ok" { 200 } else { 400 })
                                .unwrap();
                        (status, Json(api_key_body.clone()))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), seen, server)
    }

    #[test]
    fn device_store_scopes_users_and_expires() {
        let store = DeviceCodeStore::new();
        store.set("r1", "dev", "user-a", Duration::from_secs(60));
        assert_eq!(
            store.get("r1"),
            Some(("dev".to_owned(), "user-a".to_owned()))
        );
        store.set("r2", "dev2", "user-b", Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        assert!(store.get("r2").is_none());
        store.delete("r1");
        assert!(store.get("r1").is_none());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn attribution_defaults_come_from_constants() {
        // Env overrides are process-global; only assert the default path.
        if std::env::var("AIMLAPI_SOURCE").is_err() {
            assert_eq!(source(), DEFAULT_SOURCE);
            assert_eq!(partner_id(), DEFAULT_PARTNER_ID);
        }
        let headers = attribution_headers();
        assert!(headers.iter().any(|(name, _)| *name == "X-AIMLAPI-Source"));
    }

    #[tokio::test]
    async fn start_and_poll_return_the_issued_key() {
        let (base, seen, server) =
            spawn_app("ok", json!({"status": "ready", "apiKey": "sk-issued"})).await;
        let client = client(&base);
        let store = DeviceCodeStore::new();
        let start = client.start(&store, "user-a").await.unwrap();
        assert_eq!(start.request_id, "req-1");
        assert!(
            start
                .verification_uri
                .starts_with("https://verify.example/agent/authorize?")
        );
        assert!(start.verification_uri.contains("request=req-1"));
        assert!(start.verification_uri.contains("source=agent%2Fragflow"));
        assert_eq!(start.interval, 5);
        assert_eq!(start.expires_in, 900);
        assert_eq!(seen.start.lock().unwrap()[0]["agentName"], json!("RayRAG"));
        assert_eq!(
            seen.start.lock().unwrap()[0]["returnUrl"],
            json!("https://verify.example")
        );

        let outcome = client.poll(&store, "user-a", "req-1").await.unwrap();
        server.abort();
        assert_eq!(
            outcome,
            PollOutcome::Ready {
                api_key: "sk-issued".to_owned()
            }
        );
        assert_eq!(seen.poll.lock().unwrap()[0]["deviceCode"], json!("dev-1"));
        assert_eq!(
            seen.poll.lock().unwrap()[0]["grant_type"],
            json!(DEVICE_CODE_GRANT)
        );
    }

    #[tokio::test]
    async fn poll_reports_pending_expired_and_user_mismatch() {
        let (base, _, server) = spawn_app("ok", json!({"status": "pending"})).await;
        let client = client(&base);
        let store = DeviceCodeStore::new();
        assert_eq!(
            client.poll(&store, "user-a", "missing").await.unwrap(),
            PollOutcome::Expired
        );
        store.set("req-1", "dev-1", "user-b", Duration::from_secs(60));
        let error = client
            .poll(&store, "user-a", "req-1")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("not found for the current user"), "{error}");
        store.set("req-2", "dev-2", "user-a", Duration::from_secs(60));
        let outcome = client.poll(&store, "user-a", "req-2").await.unwrap();
        server.abort();
        assert_eq!(
            outcome,
            PollOutcome::Pending {
                status: "pending".to_owned()
            }
        );
    }

    #[tokio::test]
    async fn terminal_states_delete_the_cached_code() {
        let (base, _, server) = spawn_app("ok", json!({"status": "denied"})).await;
        let client = client(&base);
        let store = DeviceCodeStore::new();
        store.set("req-1", "dev-1", "user-a", Duration::from_secs(60));
        let outcome = client.poll(&store, "user-a", "req-1").await.unwrap();
        server.abort();
        assert_eq!(
            outcome,
            PollOutcome::Pending {
                status: "denied".to_owned()
            }
        );
        assert!(store.get("req-1").is_none());
    }

    #[tokio::test]
    async fn upstream_errors_surface_with_status() {
        let (base, _, server) = spawn_app("fail", json!({"error": "nope"})).await;
        let client = client(&base);
        let store = DeviceCodeStore::new();
        store.set("req-1", "dev-1", "user-a", Duration::from_secs(60));
        let error = client
            .poll(&store, "user-a", "req-1")
            .await
            .unwrap_err()
            .to_string();
        server.abort();
        assert!(error.contains("HTTP 400"), "{error}");
    }
}
