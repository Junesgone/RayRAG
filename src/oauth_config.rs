//! OAuth/OIDC login channels (upstream `settings.OAUTH_CONFIG`).
//!
//! RAGFlow reads the `oauth:` block from `conf/service_conf.yaml` (optionally
//! overridden by an `OAUTH` environment value via `get_base_config("oauth", {})`).
//! RayRAG is environment-driven, so the same shape is accepted as JSON in
//! `RAYRAG_OAUTH_CONFIG` (falling back to `OAUTH`), e.g.
//!
//! ```json
//! {"github": {"type": "github", "icon": "github", "display_name": "Github",
//!             "client_id": "…", "client_secret": "…",
//!             "redirect_uri": "https://app/v1/user/oauth/callback/github",
//!             "authorization_url": "https://github.com/login/oauth/authorize",
//!             "scope": "read:user"}}
//! ```
//!
//! `GET /api/v1/auth/login/channels` lists the configured channels and
//! `GET /api/v1/auth/login/{channel}` redirects to the authorization endpoint,
//! mirroring `user_api.py` (`oauth_login` builds the URL from the auth client).

use serde_json::{Map, Value};

/// One configured login channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthChannel {
    /// Channel key (`github`, `oidc`, `oauth2`, …).
    pub channel: String,
    /// `display_name`, defaulting to the capitalised channel key.
    pub display_name: String,
    /// `icon`, defaulting to `sso` like the React page.
    pub icon: String,
    /// Authorization endpoint (`authorization_url`, or `{issuer}/authorize` for OIDC).
    pub authorization_url: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub scope: Option<String>,
    /// `type` (e.g. `github`); kept for the callback/dialect selection.
    pub kind: Option<String>,
    /// Client secret used by the code → token exchange.
    pub client_secret: Option<String>,
    /// Token endpoint (`token_url`).
    pub token_url: Option<String>,
    /// User-info endpoint (`userinfo_url`).
    pub userinfo_url: Option<String>,
    /// HTTP timeout for the exchange/userinfo calls (upstream `http_request_timeout`).
    pub timeout_secs: u64,
}

impl OAuthChannel {
    /// Build from one `oauth:` entry, mirroring the upstream defaults.
    pub fn from_entry(channel: &str, value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let text = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let display_name = text("display_name").unwrap_or_else(|| title_case(channel));
        // OIDC entries only carry `issuer`; upstream derives the endpoints from it.
        let authorization_url = text("authorization_url").or_else(|| {
            text("issuer").map(|issuer| {
                let base = issuer.trim_end_matches('/');
                if base.ends_with("/protocol/openid-connect") {
                    format!("{base}/auth")
                } else {
                    format!("{base}/protocol/openid-connect/auth")
                }
            })
        });
        let kind = text("type");
        // `GithubOAuthClient` fills in the fixed GitHub endpoints/scope.
        let is_github = kind.as_deref() == Some("github") || channel == "github";
        let (authorization_url, token_url, userinfo_url, scope) = if is_github {
            (
                Some(
                    text("authorization_url")
                        .unwrap_or_else(|| "https://github.com/login/oauth/authorize".to_string()),
                ),
                Some(
                    text("token_url").unwrap_or_else(|| {
                        "https://github.com/login/oauth/access_token".to_string()
                    }),
                ),
                Some(
                    text("userinfo_url")
                        .unwrap_or_else(|| "https://api.github.com/user".to_string()),
                ),
                Some(text("scope").unwrap_or_else(|| "user:email".to_string())),
            )
        } else {
            (
                authorization_url,
                text("token_url"),
                text("userinfo_url"),
                text("scope"),
            )
        };
        Some(Self {
            channel: channel.to_string(),
            display_name,
            icon: text("icon").unwrap_or_else(|| "sso".to_string()),
            authorization_url,
            client_id: text("client_id"),
            redirect_uri: text("redirect_uri"),
            scope,
            kind,
            client_secret: text("client_secret"),
            token_url,
            userinfo_url,
            timeout_secs: text("timeout")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(7),
        })
    }

    /// Authorisation URL with the standard query parameters, or `None` when the
    /// entry has no usable endpoint.
    pub fn authorization_request(&self, state: &str) -> Option<String> {
        let endpoint = self.authorization_url.as_deref()?;
        let mut query: Vec<(&str, String)> = vec![("response_type", "code".to_string())];
        if let Some(client_id) = self.client_id.as_deref() {
            query.push(("client_id", client_id.to_string()));
        }
        if let Some(redirect_uri) = self.redirect_uri.as_deref() {
            query.push(("redirect_uri", redirect_uri.to_string()));
        }
        if let Some(scope) = self.scope.as_deref() {
            query.push(("scope", scope.to_string()));
        }
        query.push(("state", state.to_string()));
        let separator = if endpoint.contains('?') { '&' } else { '?' };
        let encoded = query
            .into_iter()
            .map(|(key, value)| format!("{key}={}", url_encode(&value)))
            .collect::<Vec<_>>()
            .join("&");
        Some(format!("{endpoint}{separator}{encoded}"))
    }

    /// Public shape returned by `GET /api/v1/auth/login/channels`.
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "channel": self.channel,
            "display_name": self.display_name,
            "icon": self.icon,
        })
    }
}

fn title_case(channel: &str) -> String {
    let mut chars = channel.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Parse an `oauth:` block (upstream `OAUTH_CONFIG` shape).
pub fn parse(json: &str) -> Vec<OAuthChannel> {
    let Ok(value) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    let mut channels: Vec<OAuthChannel> = object
        .iter()
        .filter_map(|(channel, entry)| OAuthChannel::from_entry(channel, entry))
        .collect();
    // Stable presentation order: configuration order is not preserved by JSON
    // objects in every producer, so sort by key like the upstream mapping does.
    channels.sort_by(|left, right| left.channel.cmp(&right.channel));
    channels
}

/// Channels configured through `RAYRAG_OAUTH_CONFIG` / `OAUTH`.
pub fn from_env() -> Vec<OAuthChannel> {
    for key in ["RAYRAG_OAUTH_CONFIG", "OAUTH"] {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                return parse(&value);
            }
        }
    }
    Vec::new()
}

/// Look up one channel by name (case-sensitive, like upstream `OAUTH_CONFIG.get`).
pub fn find<'a>(channels: &'a [OAuthChannel], channel: &str) -> Option<&'a OAuthChannel> {
    channels.iter().find(|entry| entry.channel == channel)
}

/// `{}`-free helper for tests: the raw JSON object keys are exposed so the
/// ledger's "fixed channel registry" claim stays checkable.
pub fn channel_names(channels: &[OAuthChannel]) -> Vec<String> {
    channels.iter().map(|entry| entry.channel.clone()).collect()
}

/// Unused placeholder kept public for future callback handling.
pub type OAuthConfig = Map<String, Value>;

// ── Login flow (upstream `user_api.oauth_login` / `oauth_callback` +
//    `api/apps/auth/oauth.py`) ────────────────────────────────────────────

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Normalised provider user info (upstream `UserInfo`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserInfo {
    pub email: String,
    pub username: String,
    pub nickname: String,
    pub avatar_url: String,
}

/// `auth_cli.normalize_user_info`: nickname falls back to the username, which
/// falls back to the email local part; the avatar accepts `avatar_url` or
/// `picture`.
pub fn normalize_user_info(value: &Value) -> UserInfo {
    let email = value
        .get("email")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let username = value
        .get("username")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| email.split('@').next().unwrap_or_default().to_string());
    let nickname = value
        .get("nickname")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| username.clone());
    let avatar_url = value
        .get("avatar_url")
        .and_then(Value::as_str)
        .or_else(|| value.get("picture").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();
    UserInfo {
        email,
        username,
        nickname,
        avatar_url,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Single-use OAuth `state` store. Upstream keeps the state in the Flask
/// session (`session["oauth_state"]`); RayRAG is stateless, so states live in
/// memory with a TTL and are consumed on first callback.
#[derive(Debug, Default)]
pub struct StateStore {
    states: Mutex<HashMap<String, u64>>,
}

impl StateStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Issue a fresh opaque state valid for `ttl`.
    pub fn issue_with_ttl(&self, ttl: Duration) -> String {
        let state = uuid::Uuid::new_v4().simple().to_string();
        let expires_at = now_ms() + ttl.as_millis() as u64;
        let mut states = self.states.lock().unwrap();
        states.retain(|_, expiry| *expiry > now_ms());
        states.insert(state.clone(), expires_at);
        state
    }

    /// Issue with the upstream session default (10 minutes).
    pub fn issue(&self) -> String {
        self.issue_with_ttl(Duration::from_secs(600))
    }

    /// Validate **and** consume a state; unknown/expired/replayed states fail.
    pub fn consume(&self, state: &str) -> bool {
        if state.is_empty() {
            return false;
        }
        let mut states = self.states.lock().unwrap();
        let now = now_ms();
        states.retain(|_, expiry| *expiry > now);
        states.remove(state).is_some()
    }

    pub fn len(&self) -> usize {
        self.states.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Result of the authorization-code exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenExchange {
    pub access_token: String,
    pub id_token: Option<String>,
}

/// Build the `exchange_code_for_token` form body (upstream payload order).
pub fn token_exchange_form(channel: &OAuthChannel, code: &str) -> Vec<(String, String)> {
    let mut form = Vec::new();
    if let Some(client_id) = channel.client_id.as_deref() {
        form.push(("client_id".to_string(), client_id.to_string()));
    }
    if let Some(client_secret) = channel.client_secret.as_deref() {
        form.push(("client_secret".to_string(), client_secret.to_string()));
    }
    form.push(("code".to_string(), code.to_string()));
    if let Some(redirect_uri) = channel.redirect_uri.as_deref() {
        form.push(("redirect_uri".to_string(), redirect_uri.to_string()));
    }
    form.push(("grant_type".to_string(), "authorization_code".to_string()));
    form
}

/// POST the code to `token_url` and read `access_token`/`id_token`
/// (upstream `OAuthClient.exchange_code_for_token`).
pub async fn exchange_code(channel: &OAuthChannel, code: &str) -> anyhow::Result<TokenExchange> {
    let token_url = channel
        .token_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("channel {} has no token_url", channel.channel))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(channel.timeout_secs.max(1)))
        .build()?;
    let response = client
        .post(token_url)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&token_exchange_form(channel, code))
        .send()
        .await
        .map_err(|error| {
            anyhow::anyhow!("Failed to exchange authorization code for token: {error}")
        })?;
    let status = response.status();
    let payload: Value = response.json().await.map_err(|error| {
        anyhow::anyhow!("Failed to exchange authorization code for token: {error}")
    })?;
    if !status.is_success() {
        anyhow::bail!("Failed to exchange authorization code for token: HTTP {status}");
    }
    let access_token = payload
        .get("access_token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if access_token.is_empty() {
        anyhow::bail!("Failed to exchange authorization code for token: no access_token");
    }
    Ok(TokenExchange {
        access_token,
        id_token: payload
            .get("id_token")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// GET `userinfo_url` with the bearer token (upstream `fetch_user_info`); the
/// fixed GitHub client additionally reads `/emails` and keeps the primary one.
pub async fn fetch_user_info(
    channel: &OAuthChannel,
    access_token: &str,
) -> anyhow::Result<UserInfo> {
    let userinfo_url = channel
        .userinfo_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("channel {} has no userinfo_url", channel.channel))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(channel.timeout_secs.max(1)))
        .build()?;
    let response = client
        .get(userinfo_url)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {access_token}"),
        )
        .send()
        .await
        .map_err(|error| anyhow::anyhow!("Failed to fetch user info: {error}"))?;
    let status = response.status();
    let mut payload: Value = response
        .json()
        .await
        .map_err(|error| anyhow::anyhow!("Failed to fetch user info: {error}"))?;
    if !status.is_success() {
        anyhow::bail!("Failed to fetch user info: HTTP {status}");
    }
    if channel.kind.as_deref() == Some("github") || channel.channel == "github" {
        if let Ok(emails) = client
            .get(format!("{userinfo_url}/emails"))
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {access_token}"),
            )
            .send()
            .await
        {
            if let Ok(list) = emails.json::<Value>().await {
                if let Some(primary) = list.as_array().and_then(|entries| {
                    entries
                        .iter()
                        .find(|entry| entry.get("primary").and_then(Value::as_bool) == Some(true))
                        .or_else(|| entries.first())
                }) {
                    if let Some(email) = primary.get("email").and_then(Value::as_str) {
                        if let Some(object) = payload.as_object_mut() {
                            object.insert("email".to_string(), Value::String(email.to_string()));
                        }
                    }
                }
            }
        }
    }
    Ok(normalize_user_info(&payload))
}
