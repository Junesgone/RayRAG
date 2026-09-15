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
        Some(Self {
            channel: channel.to_string(),
            display_name,
            icon: text("icon").unwrap_or_else(|| "sso".to_string()),
            authorization_url,
            client_id: text("client_id"),
            redirect_uri: text("redirect_uri"),
            scope: text("scope"),
            kind: text("type"),
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
