//! Chat channel framework — Rust port of RAGFlow `api/channels`.
//!
//! Mirrors the RAGFlow abstraction:
//!
//! - [`Channel`]: one bot identity on one messaging platform with `start`/`stop`
//!   lifecycle and `send_text`/`send_markdown` outbound (RAGFlow
//!   `Channel.start` / `Channel.stop` / `Channel.send(OutgoingMessage)`).
//! - [`IncomingMessage`] / [`OutgoingMessage`]: the RAGFlow message contracts.
//! - [`ChannelRegistry`]: builder registry + running-instance table with
//!   RAGFlow `registry.py` semantics (`register_channel`, `registered_channel_ids`,
//!   `build_channels` walking `channels.<name>.accounts.<id>` config, `enabled`
//!   flags, shared + account config merge) and `bootstrap.py` semantics
//!   (`start_all`/`stop_all` start/stop hooks, per-channel error isolation).
//!
//! Bundled channels (plain HTTP, no external SDK; parameter contracts aligned
//! with the RAGFlow channel implementations):
//!
//! - `webhook`: generic webhook callback channel — outbound POSTs the
//!   `OutgoingMessage` contract to a configured `callback_url`, inbound is fed
//!   by `POST /api/v1/channels/webhook/{account_id}/inbound`.
//! - `feishu`: Feishu Open Platform channel — `tenant_access_token` auth +
//!   `im/v1/messages` send (the contract RAGFlow's `FeishuChannel.send` uses),
//!   inbound fed by `POST /api/v1/channels/feishu/{account_id}/event`
//!   (event + URL-verification challenge). This is the channel the Boss
//!   environment actually uses.

use async_trait::async_trait;
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Inbound message — RAGFlow `IncomingMessage` contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessage {
    /// Platform name, e.g. `"feishu"`.
    pub channel: String,
    /// Bot identity (RAGFlow `account_id`, == chat_channel.id).
    pub account_id: String,
    /// Chat / conversation id on the platform.
    pub chat_id: String,
    /// `"p2p"` | `"group"` (RAGFlow `chat_type`).
    pub chat_type: String,
    /// Platform message id (used for reply_to).
    pub message_id: String,
    /// Platform sender id (e.g. Feishu open_id).
    pub sender_id: String,
    /// Message text.
    pub text: String,
    /// Raw platform payload (RAGFlow `raw`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,
}

/// Outbound message — RAGFlow `OutgoingMessage` contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessage {
    /// Chat / conversation id to deliver to.
    pub chat_id: String,
    /// Message text.
    pub text: String,
    /// When set, deliver as a reply to this platform message id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<String>,
}

/// Inbound message handler (RAGFlow `MessageHandler`). Errors are contained by
/// the framework boundary (`Channel::dispatch`), never the caller.
pub type MessageHandler = Arc<dyn Fn(IncomingMessage) -> BoxFuture<'static, ()> + Send + Sync>;

/// Channel builder (RAGFlow `ChannelBuilder`): `(account_id, config) -> Channel`.
pub type ChannelBuilder =
    Arc<dyn Fn(&str, &serde_json::Value) -> anyhow::Result<Arc<dyn Channel>> + Send + Sync>;

/// Interior handler slot shared by every channel implementation
/// (RAGFlow `Channel._handler`).
#[derive(Default)]
pub struct HandlerSlot(Mutex<Option<MessageHandler>>);

impl HandlerSlot {
    pub fn set(&self, handler: MessageHandler) {
        *self.0.lock().unwrap() = Some(handler);
    }

    pub fn get(&self) -> Option<MessageHandler> {
        self.0.lock().unwrap().clone()
    }

    /// Framework boundary: run one inbound message through the handler,
    /// isolating handler failures (RAGFlow `Channel._dispatch`).
    pub async fn dispatch(&self, message: IncomingMessage) {
        let Some(handler) = self.get() else {
            return;
        };
        if let Err(error) = tokio::task::spawn_blocking(move || handler(message))
            .await
            .map_err(|join| anyhow::anyhow!("handler task panicked: {join}"))
        {
            error!("channel handler error: {error:#}");
        }
    }
}

/// One configured bot identity on one messaging platform (RAGFlow `Channel`).
#[async_trait]
pub trait Channel: Send + Sync {
    /// Platform name, e.g. `"feishu"` (RAGFlow `channel_id` ClassVar).
    fn channel_id(&self) -> &str;
    /// Bot identity (RAGFlow `account_id`).
    fn account_id(&self) -> &str;

    /// Set the inbound message handler (RAGFlow `set_message_handler`).
    fn set_message_handler(&self, handler: MessageHandler);
    /// Current handler slot (used by the default `dispatch`).
    fn handler(&self) -> Option<MessageHandler>;

    /// Start receiving / connecting (RAGFlow `start`).
    async fn start(&self) -> anyhow::Result<()>;
    /// Stop receiving / disconnect (RAGFlow `stop`).
    async fn stop(&self) -> anyhow::Result<()>;

    /// Send plain text (RAGFlow `send(OutgoingMessage)` with msg_type text).
    async fn send_text(&self, message: &OutgoingMessage) -> anyhow::Result<()>;
    /// Send markdown-formatted text.
    async fn send_markdown(&self, message: &OutgoingMessage) -> anyhow::Result<()>;

    /// Framework boundary: deliver one inbound message to the handler,
    /// isolating handler failures (RAGFlow `Channel._dispatch`).
    async fn dispatch(&self, message: IncomingMessage) {
        self.handler().map(|_| ()).unwrap_or(());
        if let Some(handler) = self.handler()
            && let Err(error) = tokio::task::spawn_blocking(move || handler(message))
                .await
                .map_err(|join| anyhow::anyhow!("handler task panicked: {join}"))
        {
            error!(
                "[{}:{}] handler error: {error:#}",
                self.channel_id(),
                self.account_id()
            );
        }
    }

    /// Normalize a platform-specific raw inbound payload and dispatch it.
    /// Returns an optional synchronous response (e.g. the Feishu URL
    /// verification challenge). The default implementation handles the generic
    /// webhook callback contract; channels override it for their own event
    /// shapes (RAGFlow `_normalize` + `_dispatch`).
    async fn handle_raw(
        &self,
        payload: &serde_json::Value,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        let incoming = IncomingMessage {
            channel: self.channel_id().to_string(),
            account_id: self.account_id().to_string(),
            chat_id: payload
                .get("chat_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            chat_type: payload
                .get("chat_type")
                .and_then(|v| v.as_str())
                .unwrap_or("p2p")
                .to_string(),
            message_id: payload
                .get("message_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            sender_id: payload
                .get("sender_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            text: payload
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            raw: Some(payload.clone()),
        };
        self.dispatch(incoming).await;
        Ok(None)
    }
}

// ── Registry ────────────────────────────────────────────────────
//
// RAGFlow `api/channels/core/registry.py` + `bootstrap.py` port: builders are
// registered by platform name, instances are built from `channels.<name>.
// accounts.<id>` config and tracked while running so start/stop hooks and
// inbound endpoints can find them by account id.

/// Channel registry with RAGFlow `registry.py` / `bootstrap.py` semantics.
#[derive(Default)]
pub struct ChannelRegistry {
    builders: RwLock<HashMap<String, ChannelBuilder>>,
    running: RwLock<HashMap<String, Arc<dyn Channel>>>,
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a channel builder by platform name (RAGFlow `register_channel`).
    pub async fn register(&self, name: &str, builder: ChannelBuilder) {
        self.builders
            .write()
            .await
            .insert(name.to_string(), builder);
    }

    /// Sorted platform names with a registered builder
    /// (RAGFlow `registered_channel_ids`).
    pub async fn registered_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.builders.read().await.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Whether a builder is registered for `name`.
    pub async fn has(&self, name: &str) -> bool {
        self.builders.read().await.contains_key(name)
    }

    /// Walk `config.channels.<name>.accounts.<id>` and construct one Channel per
    /// enabled account (RAGFlow `build_channels`). Shared (non-`accounts`,
    /// non-`default_account`) keys are merged into each account config; a flat
    /// single-account config without an `accounts:` block is allowed.
    pub async fn build_channels(&self, config: &serde_json::Value) -> Vec<Arc<dyn Channel>> {
        let mut instances: Vec<Arc<dyn Channel>> = Vec::new();
        let Some(channels_cfg) = config.get("channels").and_then(|v| v.as_object()) else {
            return instances;
        };
        for (name, raw) in channels_cfg {
            let Some(raw_obj) = raw.as_object() else {
                continue;
            };
            if raw_obj.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
                continue;
            }
            let Some(builder) = self.builders.read().await.get(name).cloned() else {
                warn!("no builder registered for channel '{name}'; skipping");
                continue;
            };
            let mut accounts: HashMap<String, serde_json::Value> = raw_obj
                .get("accounts")
                .and_then(|v| v.as_object())
                .map(|map| {
                    map.iter()
                        .map(|(id, cfg)| (id.clone(), cfg.clone()))
                        .collect()
                })
                .unwrap_or_default();
            if accounts.is_empty() {
                // Allow a flat single-account config without an `accounts:` block.
                let mut flat = raw_obj.clone();
                flat.remove("accounts");
                flat.remove("default_account");
                accounts.insert("default".to_string(), serde_json::Value::Object(flat));
            }
            let shared: serde_json::Map<String, serde_json::Value> = raw_obj
                .iter()
                .filter(|(key, _)| *key != "accounts" && *key != "default_account")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            for (account_id, account_cfg) in accounts {
                let Some(account_obj) = account_cfg.as_object() else {
                    continue;
                };
                if account_obj.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
                    continue;
                }
                let mut merged = shared.clone();
                for (key, value) in account_obj {
                    merged.insert(key.clone(), value.clone());
                }
                match builder(&account_id, &serde_json::Value::Object(merged)) {
                    Ok(channel) => instances.push(channel),
                    Err(error) => {
                        error!("failed to build channel '{name}' account '{account_id}': {error:#}")
                    }
                }
            }
        }
        instances
    }

    /// Running instance by account id (bootstrap bookkeeping).
    pub async fn get_running(&self, account_id: &str) -> Option<Arc<dyn Channel>> {
        self.running.read().await.get(account_id).cloned()
    }

    /// Bootstrap start hook: build + start every enabled channel from config.
    /// Errors are contained per channel so one bad bot never aborts the rest
    /// (RAGFlow `_start_channel`). Returns `(platform, account_id)` pairs.
    pub async fn start_all(&self, config: &serde_json::Value) -> Vec<(String, String)> {
        let mut started = Vec::new();
        for channel in self.build_channels(config).await {
            let platform = channel.channel_id().to_string();
            let account = channel.account_id().to_string();
            match channel.start().await {
                Ok(()) => {
                    self.running
                        .write()
                        .await
                        .insert(account.clone(), channel.clone());
                    info!("started chat channel {platform}:{account}");
                    started.push((platform, account));
                }
                Err(error) => {
                    error!("failed to start chat channel {platform}:{account}: {error:#}")
                }
            }
        }
        started
    }

    /// Bootstrap stop hook: stop every running channel (RAGFlow `_stop_channel`).
    pub async fn stop_all(&self) {
        let running: Vec<Arc<dyn Channel>> = self
            .running
            .write()
            .await
            .drain()
            .map(|(_, channel)| channel)
            .collect();
        for channel in running {
            let platform = channel.channel_id().to_string();
            let account = channel.account_id().to_string();
            match channel.stop().await {
                Ok(()) => info!("stopped chat channel {platform}:{account}"),
                Err(error) => error!("failed to stop chat channel {account}: {error:#}"),
            }
        }
    }

    /// Dispatch an inbound message to the running channel for `account_id`.
    /// Returns `false` when no channel is running for that account
    /// (RAGFlow `_dispatch` routing by account_id).
    pub async fn dispatch_inbound(&self, account_id: &str, message: IncomingMessage) -> bool {
        match self.get_running(account_id).await {
            Some(channel) => {
                channel.dispatch(message).await;
                true
            }
            None => {
                warn!("no running channel for account '{account_id}'");
                false
            }
        }
    }
}

// ── Bootstrap ───────────────────────────────────────────────────

/// Bootstrap the bundled channel builders (RAGFlow `_register_channels`).
/// Each bundled channel registers itself here; a missing channel only disables
/// that one channel instead of taking down the registry.
pub async fn bootstrap_registry() -> Arc<ChannelRegistry> {
    let registry = Arc::new(ChannelRegistry::new());
    registry
        .register(
            "webhook",
            Arc::new(|account_id, cfg| {
                Ok(Arc::new(WebhookChannel::new(account_id, cfg)?) as Arc<dyn Channel>)
            }),
        )
        .await;
    registry
        .register(
            "feishu",
            Arc::new(|account_id, cfg| {
                Ok(Arc::new(FeishuChannel::new(account_id, cfg)?) as Arc<dyn Channel>)
            }),
        )
        .await;
    registry
}

/// Read the channel config from the environment, mirroring the RAGFlow
/// `config.channels` shape:
///
/// - `RAYRAG_CHANNELS_CONFIG`: full JSON config, e.g.
///   `{"channels": {"feishu": {"enabled": true, "app_id": "...", "app_secret": "..."}}}`
/// - fallback: per-channel env vars (`RAYRAG_FEISHU_APP_ID`,
///   `RAYRAG_FEISHU_APP_SECRET`, `RAYRAG_WEBHOOK_CALLBACK_URL`, ...).
pub fn channels_config_from_env() -> serde_json::Value {
    if let Ok(raw) = std::env::var("RAYRAG_CHANNELS_CONFIG") {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
            return value;
        }
        warn!("invalid RAYRAG_CHANNELS_CONFIG JSON; falling back to per-channel env vars");
    }
    let mut channels = serde_json::Map::new();

    // webhook channel: outbound callback url (+ optional bearer token).
    if let Ok(url) = std::env::var("RAYRAG_WEBHOOK_CALLBACK_URL")
        && !url.trim().is_empty()
    {
        let mut cfg = serde_json::Map::new();
        cfg.insert("enabled".into(), serde_json::json!(true));
        cfg.insert("callback_url".into(), serde_json::json!(url));
        if let Ok(token) = std::env::var("RAYRAG_WEBHOOK_TOKEN")
            && !token.is_empty()
        {
            cfg.insert("token".into(), serde_json::json!(token));
        }
        channels.insert("webhook".into(), serde_json::Value::Object(cfg));
    }

    // feishu channel: app credentials (Boss environment's actual channel).
    let app_id = std::env::var("RAYRAG_FEISHU_APP_ID").ok();
    let app_secret = std::env::var("RAYRAG_FEISHU_APP_SECRET").ok();
    if let (Some(app_id), Some(app_secret)) = (app_id, app_secret)
        && !app_id.trim().is_empty()
        && !app_secret.trim().is_empty()
    {
        let mut cfg = serde_json::Map::new();
        cfg.insert("enabled".into(), serde_json::json!(true));
        cfg.insert("app_id".into(), serde_json::json!(app_id));
        cfg.insert("app_secret".into(), serde_json::json!(app_secret));
        if let Ok(domain) = std::env::var("RAYRAG_FEISHU_DOMAIN")
            && !domain.is_empty()
        {
            cfg.insert("domain".into(), serde_json::json!(domain));
        }
        channels.insert("feishu".into(), serde_json::Value::Object(cfg));
    }
    serde_json::json!({ "channels": channels })
}

// ── Webhook channel ─────────────────────────────────────────────

/// Generic webhook callback channel.
///
/// Outbound: POSTs the RAGFlow `OutgoingMessage` contract
/// (`{chat_id, text, reply_to_message_id, msg_type}`) to the configured
/// `callback_url` (with an optional `Authorization: Bearer <token>` header).
/// Inbound: fed by `POST /api/v1/channels/webhook/{account_id}/inbound`
/// via the default [`Channel::handle_raw`] (platform-agnostic callback shape).
pub struct WebhookChannel {
    account_id: String,
    callback_url: String,
    token: Option<String>,
    slot: HandlerSlot,
    started: AtomicBool,
}

impl WebhookChannel {
    pub fn new(account_id: &str, cfg: &serde_json::Value) -> anyhow::Result<Self> {
        let callback_url = cfg
            .get("callback_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if callback_url.is_empty() {
            anyhow::bail!("webhook account '{account_id}' is missing callback_url");
        }
        let token = cfg
            .get("token")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        Ok(Self {
            account_id: account_id.to_string(),
            callback_url,
            token,
            slot: HandlerSlot::default(),
            started: AtomicBool::new(false),
        })
    }

    async fn post(&self, msg_type: &str, message: &OutgoingMessage) -> anyhow::Result<()> {
        let mut body = serde_json::json!({
            "chat_id": message.chat_id,
            "text": message.text,
            "msg_type": msg_type,
        });
        if let Some(reply_to) = &message.reply_to_message_id {
            body["reply_to_message_id"] = serde_json::json!(reply_to);
        }
        let mut request = crate::common::cmd_timeout::model_client()
            .post(&self.callback_url)
            .json(&body);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            anyhow::bail!(
                "webhook callback {} -> HTTP {}",
                self.callback_url,
                response.status()
            );
        }
        Ok(())
    }
}

#[async_trait]
impl Channel for WebhookChannel {
    fn channel_id(&self) -> &str {
        "webhook"
    }

    fn account_id(&self) -> &str {
        &self.account_id
    }

    fn set_message_handler(&self, handler: MessageHandler) {
        self.slot.set(handler);
    }

    fn handler(&self) -> Option<MessageHandler> {
        self.slot.get()
    }

    async fn start(&self) -> anyhow::Result<()> {
        self.started.store(true, Ordering::SeqCst);
        info!(
            "[webhook:{}] started (callback_url={})",
            self.account_id, self.callback_url
        );
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.started.store(false, Ordering::SeqCst);
        info!("[webhook:{}] stopped", self.account_id);
        Ok(())
    }

    async fn send_text(&self, message: &OutgoingMessage) -> anyhow::Result<()> {
        self.post("text", message).await
    }

    async fn send_markdown(&self, message: &OutgoingMessage) -> anyhow::Result<()> {
        self.post("markdown", message).await
    }
}

// ── Feishu channel ──────────────────────────────────────────────

/// Feishu Open Platform channel (plain HTTP, no `lark_oapi` SDK).
///
/// Outbound contract matches RAGFlow's `FeishuChannel.send`:
/// `im/v1/messages?receive_id_type=chat_id` with
/// `{"receive_id": chat_id, "msg_type": "text", "content": "{\"text\": ...}"}`;
/// replies go to `im/v1/messages/{message_id}/reply`. Auth uses the
/// `tenant_access_token` from `auth/v3/tenant_access_token/internal`
/// (`app_id` + `app_secret`), cached for its ~2h validity. Markdown is sent as
/// an interactive card (Feishu's markdown-capable message type).
pub struct FeishuChannel {
    account_id: String,
    app_id: String,
    app_secret: String,
    base_url: String,
    token_cache: Mutex<Option<(String, std::time::Instant)>>,
    slot: HandlerSlot,
    started: AtomicBool,
}

/// Refresh the cached tenant_access_token slightly before Feishu's 2h expiry.
const FEISHU_TOKEN_TTL: std::time::Duration = std::time::Duration::from_secs(7_000);

impl FeishuChannel {
    pub fn new(account_id: &str, cfg: &serde_json::Value) -> anyhow::Result<Self> {
        let app_id = cfg
            .get("app_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let app_secret = cfg
            .get("app_secret")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if app_id.is_empty() || app_secret.is_empty() {
            anyhow::bail!("feishu account '{account_id}' is missing app_id or app_secret");
        }
        let domain = cfg
            .get("domain")
            .and_then(|v| v.as_str())
            .unwrap_or("feishu");
        let base_url = if domain == "lark" {
            "https://open.larksuite.com"
        } else {
            "https://open.feishu.cn"
        }
        .to_string();
        Ok(Self {
            account_id: account_id.to_string(),
            app_id,
            app_secret,
            base_url,
            token_cache: Mutex::new(None),
            slot: HandlerSlot::default(),
            started: AtomicBool::new(false),
        })
    }

    async fn tenant_access_token(&self) -> anyhow::Result<String> {
        {
            let cache = self.token_cache.lock().unwrap();
            if let Some((token, issued_at)) = &*cache
                && issued_at.elapsed() < FEISHU_TOKEN_TTL
            {
                return Ok(token.clone());
            }
        }
        let response: serde_json::Value = crate::common::cmd_timeout::model_client()
            .post(format!(
                "{}/open-apis/auth/v3/tenant_access_token/internal",
                self.base_url
            ))
            .json(&serde_json::json!({
                "app_id": self.app_id,
                "app_secret": self.app_secret,
            }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let code = response.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            anyhow::bail!(
                "feishu token error: code={code} msg={}",
                response
                    .get("msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
            );
        }
        let token = response
            .get("tenant_access_token")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if token.is_empty() {
            anyhow::bail!("feishu returned an empty tenant_access_token");
        }
        *self.token_cache.lock().unwrap() = Some((token.clone(), std::time::Instant::now()));
        Ok(token)
    }

    /// One send call: reply path when `reply_to_message_id` is set, otherwise
    /// create-message to `chat_id` (both match RAGFlow's Feishu send contract).
    async fn send_content(
        &self,
        msg_type: &str,
        content: &str,
        message: &OutgoingMessage,
    ) -> anyhow::Result<()> {
        let token = self.tenant_access_token().await?;
        let client = crate::common::cmd_timeout::model_client();
        let response: serde_json::Value = if let Some(reply_to) = &message.reply_to_message_id {
            client
                .post(format!(
                    "{}/open-apis/im/v1/messages/{}/reply",
                    self.base_url, reply_to
                ))
                .bearer_auth(&token)
                .json(&serde_json::json!({ "msg_type": msg_type, "content": content }))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?
        } else {
            client
                .post(format!(
                    "{}/open-apis/im/v1/messages?receive_id_type=chat_id",
                    self.base_url
                ))
                .bearer_auth(&token)
                .json(&serde_json::json!({
                    "receive_id": message.chat_id,
                    "msg_type": msg_type,
                    "content": content,
                }))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?
        };
        let code = response.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            anyhow::bail!(
                "feishu send failed: code={code} msg={}",
                response
                    .get("msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
            );
        }
        Ok(())
    }

    /// Build the Feishu `content` JSON string for a text message.
    fn text_content(text: &str) -> String {
        serde_json::json!({ "text": text }).to_string()
    }

    /// Build the Feishu `content` JSON string for a markdown interactive card.
    fn markdown_content(text: &str) -> String {
        serde_json::json!({
            "config": { "wide_screen_mode": true },
            "elements": [ { "tag": "markdown", "content": text } ],
        })
        .to_string()
    }
}

#[async_trait]
impl Channel for FeishuChannel {
    fn channel_id(&self) -> &str {
        "feishu"
    }

    fn account_id(&self) -> &str {
        &self.account_id
    }

    fn set_message_handler(&self, handler: MessageHandler) {
        self.slot.set(handler);
    }

    fn handler(&self) -> Option<MessageHandler> {
        self.slot.get()
    }

    async fn start(&self) -> anyhow::Result<()> {
        self.started.store(true, Ordering::SeqCst);
        info!(
            "[feishu:{}] started (app_id={}, domain={})",
            self.account_id, self.app_id, self.base_url
        );
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.started.store(false, Ordering::SeqCst);
        *self.token_cache.lock().unwrap() = None;
        info!("[feishu:{}] stopped", self.account_id);
        Ok(())
    }

    async fn send_text(&self, message: &OutgoingMessage) -> anyhow::Result<()> {
        self.send_content("text", &Self::text_content(&message.text), message)
            .await
    }

    async fn send_markdown(&self, message: &OutgoingMessage) -> anyhow::Result<()> {
        self.send_content(
            "interactive",
            &Self::markdown_content(&message.text),
            message,
        )
        .await
    }

    /// Feishu inbound: URL-verification challenge + v2.0 message event
    /// normalization (RAGFlow `FeishuChannel._normalize`).
    async fn handle_raw(
        &self,
        payload: &serde_json::Value,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        // URL verification: `{"challenge": "...", "token": "..."}` → echo.
        if let Some(challenge) = payload.get("challenge").and_then(|v| v.as_str()) {
            return Ok(Some(serde_json::json!({ "challenge": challenge })));
        }
        let Some(event) = payload.get("event") else {
            return Ok(None);
        };
        let message = event.get("message");
        let text = message
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .and_then(|content| {
                content
                    .get("text")
                    .and_then(|t| t.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let incoming = IncomingMessage {
            channel: self.channel_id().to_string(),
            account_id: self.account_id.clone(),
            chat_id: message
                .and_then(|m| m.get("chat_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            chat_type: message
                .and_then(|m| m.get("chat_type"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            message_id: message
                .and_then(|m| m.get("message_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            sender_id: event
                .get("sender")
                .and_then(|s| s.get("sender_id"))
                .and_then(|s| s.get("open_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            text,
            raw: Some(payload.clone()),
        };
        self.dispatch(incoming).await;
        Ok(None)
    }
}

// ── Inbound HTTP endpoints ──────────────────────────────────────
//
// New routes only — the existing route table is untouched. The endpoints feed
// raw platform payloads into the running channel instances (RAGFlow wires
// inbound via WS/webhook servers per channel; here the axum router plays that
// role). Paths are whitelisted in `server::public_api_path` so platform
// callbacks do not need the RayRAG API token.

/// Router fragment exposing the bundled channels' inbound endpoints.
pub fn channel_router() -> axum::Router<Arc<crate::server::AppState>> {
    axum::Router::new()
        .route(
            "/api/v1/channels/webhook/{account_id}/inbound",
            axum::routing::post(webhook_inbound),
        )
        .route(
            "/api/v1/channels/feishu/{account_id}/event",
            axum::routing::post(feishu_event),
        )
}

/// `POST /api/v1/channels/webhook/{account_id}/inbound`
async fn webhook_inbound(
    State(state): State<Arc<crate::server::AppState>>,
    axum::extract::Path(account_id): axum::extract::Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    match state.channels.get_running(&account_id).await {
        Some(channel) => match channel.handle_raw(&payload).await {
            Ok(_) => (
                StatusCode::OK,
                Json(serde_json::json!({ "code": 0, "message": "ok" })),
            )
                .into_response(),
            Err(error) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
            )
                .into_response(),
        },
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "code": 404,
                "message": format!("no running channel for account '{account_id}'")
            })),
        )
            .into_response(),
    }
}

/// `POST /api/v1/channels/feishu/{account_id}/event`
/// Handles the Feishu URL-verification challenge and v2.0 message events.
async fn feishu_event(
    State(state): State<Arc<crate::server::AppState>>,
    axum::extract::Path(account_id): axum::extract::Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    match state.channels.get_running(&account_id).await {
        Some(channel) => match channel.handle_raw(&payload).await {
            Ok(Some(reply)) => Json(reply).into_response(),
            Ok(None) => (
                StatusCode::OK,
                Json(serde_json::json!({ "code": 0, "message": "ok" })),
            )
                .into_response(),
            Err(error) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
            )
                .into_response(),
        },
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "code": 404,
                "message": format!("no running channel for account '{account_id}'")
            })),
        )
            .into_response(),
    }
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Minimal in-memory channel used to exercise the registry.
    struct EchoChannel {
        account_id: String,
        slot: HandlerSlot,
    }

    impl EchoChannel {
        fn new(account_id: &str, _cfg: &serde_json::Value) -> Self {
            Self {
                account_id: account_id.to_string(),
                slot: HandlerSlot::default(),
            }
        }
    }

    #[async_trait]
    impl Channel for EchoChannel {
        fn channel_id(&self) -> &str {
            "echo"
        }
        fn account_id(&self) -> &str {
            &self.account_id
        }
        fn set_message_handler(&self, handler: MessageHandler) {
            self.slot.set(handler);
        }
        fn handler(&self) -> Option<MessageHandler> {
            self.slot.get()
        }
        async fn start(&self) -> anyhow::Result<()> {
            Ok(())
        }
        async fn stop(&self) -> anyhow::Result<()> {
            Ok(())
        }
        async fn send_text(&self, _message: &OutgoingMessage) -> anyhow::Result<()> {
            Ok(())
        }
        async fn send_markdown(&self, _message: &OutgoingMessage) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// Registry bootstrap: bundled channels self-register (RAGFlow
    /// `registered_channel_ids`), unknown platforms are rejected.
    #[tokio::test]
    async fn registry_bootstrap_lists_bundled_channels() {
        let registry = bootstrap_registry().await;
        assert_eq!(registry.registered_ids().await, vec!["feishu", "webhook"]);
        assert!(registry.has("feishu").await);
        assert!(registry.has("webhook").await);
        assert!(!registry.has("slack").await);
    }

    /// Config walking: `channels.<name>.accounts.<id>` with `enabled` flags,
    /// shared + account config merge, disabled accounts skipped, unknown
    /// channel skipped (RAGFlow `build_channels`).
    #[tokio::test]
    async fn build_channels_walks_accounts_and_merges_config() {
        let registry = ChannelRegistry::new();
        let seen = Arc::new(StdMutex::new(Vec::<(String, serde_json::Value)>::new()));
        let seen_clone = seen.clone();
        registry
            .register(
                "echo",
                Arc::new(move |account_id, cfg| {
                    seen_clone
                        .lock()
                        .unwrap()
                        .push((account_id.to_string(), cfg.clone()));
                    Ok(Arc::new(EchoChannel::new(account_id, cfg)) as Arc<dyn Channel>)
                }),
            )
            .await;
        let config = serde_json::json!({
            "channels": {
                "echo": {
                    "enabled": true,
                    "shared_key": "shared-value",
                    "accounts": {
                        "a1": {"secret": "s1"},
                        "a2": {"secret": "s2", "enabled": true},
                        "a3": {"enabled": false}
                    }
                },
                "ghost": {"enabled": true}
            }
        });
        let instances = registry.build_channels(&config).await;
        assert_eq!(instances.len(), 2);
        // JSON map 遍历顺序不稳定 → 集合匹配，不依赖顺序。
        let mut ids: Vec<&str> = instances.iter().map(|c| c.account_id()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["a1", "a2"]);
        let seen = seen.lock().unwrap();
        let a1 = seen.iter().find(|(id, _)| id == "a1").unwrap();
        assert_eq!(a1.1["shared_key"], "shared-value");
        assert_eq!(a1.1["secret"], "s1");
        let a2 = seen.iter().find(|(id, _)| id == "a2").unwrap();
        assert_eq!(a2.1["shared_key"], "shared-value");
        assert_eq!(a2.1["secret"], "s2");
        assert!(seen.iter().all(|(id, _)| id != "a3"));
    }

    /// Bootstrap start/stop hooks + inbound dispatch routing by account id
    /// (RAGFlow `_start_channel` / `_stop_channel` / `_dispatch`).
    #[tokio::test]
    async fn start_all_tracks_running_and_dispatch_inbound_routes_to_handler() {
        let registry = bootstrap_registry().await;
        let config = serde_json::json!({
            "channels": {
                "webhook": {
                    "enabled": true,
                    "callback_url": "http://127.0.0.1:1/unused",
                    "accounts": { "default": {} }
                }
            }
        });
        let started = registry.start_all(&config).await;
        assert_eq!(
            started,
            vec![("webhook".to_string(), "default".to_string())]
        );
        assert!(registry.get_running("default").await.is_some());

        let received = Arc::new(StdMutex::new(Vec::<IncomingMessage>::new()));
        let received_clone = received.clone();
        registry
            .get_running("default")
            .await
            .unwrap()
            .set_message_handler(Arc::new(
                move |message: IncomingMessage| -> BoxFuture<'static, ()> {
                    received_clone.lock().unwrap().push(message);
                    Box::pin(async {})
                },
            ));

        let delivered = registry
            .dispatch_inbound(
                "default",
                IncomingMessage {
                    channel: "webhook".into(),
                    account_id: "default".into(),
                    chat_id: "chat-1".into(),
                    chat_type: "p2p".into(),
                    message_id: "m-1".into(),
                    sender_id: "u-1".into(),
                    text: "hello".into(),
                    raw: None,
                },
            )
            .await;
        assert!(delivered);
        // Unknown account is not delivered.
        assert!(
            !registry
                .dispatch_inbound(
                    "missing",
                    IncomingMessage {
                        channel: "webhook".into(),
                        account_id: "missing".into(),
                        chat_id: "c".into(),
                        chat_type: "p2p".into(),
                        message_id: "m".into(),
                        sender_id: "u".into(),
                        text: "x".into(),
                        raw: None,
                    },
                )
                .await
        );
        assert_eq!(received.lock().unwrap().len(), 1);
        assert_eq!(received.lock().unwrap()[0].text, "hello");
        assert_eq!(received.lock().unwrap()[0].chat_id, "chat-1");

        registry.stop_all().await;
        assert!(registry.get_running("default").await.is_none());
    }

    /// Webhook channel outbound: POSTs the RAGFlow `OutgoingMessage` contract
    /// (`chat_id`, `text`, `reply_to_message_id`, `msg_type`) to callback_url.
    #[tokio::test]
    async fn webhook_channel_posts_outgoing_message_contract() {
        let captured = Arc::new(StdMutex::new(Vec::<serde_json::Value>::new()));
        let captured_clone = captured.clone();
        let app = axum::Router::new().route(
            "/cb",
            axum::routing::post(move |Json(payload): Json<serde_json::Value>| {
                captured_clone.lock().unwrap().push(payload);
                async { StatusCode::OK }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let channel = WebhookChannel::new(
            "default",
            &serde_json::json!({ "callback_url": format!("http://{addr}/cb") }),
        )
        .unwrap();
        channel
            .send_text(&OutgoingMessage {
                chat_id: "chat-1".into(),
                text: "hello".into(),
                reply_to_message_id: None,
            })
            .await
            .unwrap();
        channel
            .send_markdown(&OutgoingMessage {
                chat_id: "chat-1".into(),
                text: "**hi**".into(),
                reply_to_message_id: Some("m-9".into()),
            })
            .await
            .unwrap();

        // Let the capture server drain both requests.
        for _ in 0..50 {
            if captured.lock().unwrap().len() == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        server.abort();

        let got = captured.lock().unwrap().clone();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["chat_id"], "chat-1");
        assert_eq!(got[0]["text"], "hello");
        assert_eq!(got[0]["msg_type"], "text");
        assert_eq!(got[1]["msg_type"], "markdown");
        assert_eq!(got[1]["reply_to_message_id"], "m-9");
    }

    /// Feishu inbound: URL-verification challenge echo + v2.0 message event
    /// normalization (RAGFlow `FeishuChannel._normalize`).
    #[tokio::test]
    async fn feishu_channel_handles_challenge_and_message_event() {
        let channel = FeishuChannel::new(
            "default",
            &serde_json::json!({ "app_id": "cli_test", "app_secret": "secret" }),
        )
        .unwrap();
        assert_eq!(channel.channel_id(), "feishu");

        // URL verification challenge is echoed synchronously.
        let reply = channel
            .handle_raw(&serde_json::json!({ "challenge": "abc123", "token": "t" }))
            .await
            .unwrap();
        assert_eq!(reply.unwrap()["challenge"], "abc123");

        // Inbound message event is normalized + dispatched.
        let received = Arc::new(StdMutex::new(Vec::<IncomingMessage>::new()));
        let received_clone = received.clone();
        channel.set_message_handler(Arc::new(
            move |message: IncomingMessage| -> BoxFuture<'static, ()> {
                received_clone.lock().unwrap().push(message);
                Box::pin(async {})
            },
        ));
        let payload = serde_json::json!({
            "schema": "2.0",
            "header": { "event_type": "im.message.receive_v1" },
            "event": {
                "message": {
                    "message_id": "om_123",
                    "chat_id": "oc_456",
                    "chat_type": "p2p",
                    "content": "{\"text\":\"你好，飞书\"}"
                },
                "sender": { "sender_id": { "open_id": "ou_789" } }
            }
        });
        let reply = channel.handle_raw(&payload).await.unwrap();
        assert!(reply.is_none());

        let messages = received.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].channel, "feishu");
        assert_eq!(messages[0].account_id, "default");
        assert_eq!(messages[0].chat_id, "oc_456");
        assert_eq!(messages[0].chat_type, "p2p");
        assert_eq!(messages[0].message_id, "om_123");
        assert_eq!(messages[0].sender_id, "ou_789");
        assert_eq!(messages[0].text, "你好，飞书");
    }
}
