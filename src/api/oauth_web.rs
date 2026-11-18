//! Web OAuth flow endpoints for connector token fields.
//!
//! Mirrors the fixed RAGFlow v0.26.4 `connector_api.py` Box web-OAuth start /
//! callback / result endpoints. The real Box token exchange runs only when
//! `BOX_OAUTH_TOKEN_URL` is configured (mainland deployments can point it at a
//! mirror or leave it unset to persist the authorization code verbatim).

use axum::Json;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, SystemTime};

const FLOW_TTL_SECS: u64 = 600;

#[derive(Debug, Clone)]
struct BoxFlow {
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    created_at: SystemTime,
    /// Set by the callback; exchanged to tokens when a token URL is configured.
    code: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

#[derive(Debug, Default)]
struct OAuthFlowRegistry {
    flows: HashMap<String, BoxFlow>,
}

fn registry() -> &'static RwLock<OAuthFlowRegistry> {
    static REGISTRY: OnceLock<RwLock<OAuthFlowRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(OAuthFlowRegistry::default()))
}

#[derive(Debug, Clone)]
struct GoogleFlow {
    source: String,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    /// The full uploaded client-config JSON for rebuilding result credentials.
    raw_config: String,
    created_at: SystemTime,
    code: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

#[derive(Debug, Default)]
struct GoogleFlowRegistry {
    flows: HashMap<String, GoogleFlow>,
}

fn google_registry() -> &'static RwLock<GoogleFlowRegistry> {
    static REGISTRY: OnceLock<RwLock<GoogleFlowRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(GoogleFlowRegistry::default()))
}

/// POST the authorization code to the configured token endpoint and return the
/// access/refresh token pair. None means "no endpoint configured" (the flow
/// stores the code verbatim) or the exchange failed; the callback keeps the
/// code either way so the result endpoint still answers.
async fn exchange_authorization_code(
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
    code: &str,
    token_url: Option<&str>,
) -> Option<(Option<String>, Option<String>)> {
    let token_url = token_url?;
    let client = reqwest::Client::builder()
        .timeout(crate::common::cmd_timeout::duration())
        .build()
        .ok()?;
    let response = client
        .post(token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let json = response.json::<serde_json::Value>().await.ok()?;
    Some((
        json.get("access_token")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        json.get("refresh_token")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    ))
}

async fn box_token_exchange(flow: &mut BoxFlow, code: &str, token_url: Option<&str>) {
    if let Some((access, refresh)) = exchange_authorization_code(
        &flow.client_id,
        &flow.client_secret,
        &flow.redirect_uri,
        code,
        token_url,
    )
    .await
    {
        flow.access_token = access;
        flow.refresh_token = refresh;
    }
}

async fn google_token_exchange(flow: &mut GoogleFlow, code: &str, token_url: Option<&str>) {
    if let Some((access, refresh)) = exchange_authorization_code(
        &flow.client_id,
        &flow.client_secret,
        &flow.redirect_uri,
        code,
        token_url,
    )
    .await
    {
        flow.access_token = access;
        flow.refresh_token = refresh;
    }
}

const GOOGLE_DRIVE_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/drive.readonly",
    "https://www.googleapis.com/auth/drive.metadata.readonly",
    "https://www.googleapis.com/auth/admin.directory.group.readonly",
    "https://www.googleapis.com/auth/admin.directory.user.readonly",
];

const GMAIL_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/gmail.readonly",
    "https://www.googleapis.com/auth/admin.directory.user.readonly",
    "https://www.googleapis.com/auth/admin.directory.group.readonly",
];

fn expired(flow: &BoxFlow) -> bool {
    flow.created_at
        .elapsed()
        .map(|age| age > Duration::from_secs(FLOW_TTL_SECS))
        .unwrap_or(true)
}

fn google_expired(flow: &GoogleFlow) -> bool {
    flow.created_at
        .elapsed()
        .map(|age| age > Duration::from_secs(FLOW_TTL_SECS))
        .unwrap_or(true)
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[derive(Debug, Deserialize)]
pub struct BoxStartRequest {
    pub client_id: String,
    pub client_secret: String,
    #[serde(default)]
    pub redirect_uri: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BoxResultRequest {
    pub flow_id: String,
}

fn oauth_popup(
    flow_id: &str,
    success: bool,
    message: &str,
    payload_type: &str,
    title: &str,
) -> Response {
    let status = if success { "success" } else { "error" };
    let auto_close = if success { "window.close();" } else { "" };
    let payload = serde_json::json!({
        "type": payload_type,
        "status": status,
        "flowId": flow_id,
        "message": message,
    });
    let html = format!(
        "<!DOCTYPE html><html lang='en'><head><meta charset='utf-8'/><title>{title}</title>\
         <style>body{{font-family:Arial,sans-serif;background:#f8fafc;color:#0f172a;display:flex;flex-direction:column;align-items:center;justify-content:center;min-height:100vh;margin:0}}.card{{background:white;padding:32px;border-radius:12px;box-shadow:0 8px 30px rgba(15,23,42,.1);max-width:420px;text-align:center}}h1{{font-size:1.5rem;margin-bottom:12px}}p{{font-size:.95rem;line-height:1.5}}</style></head>\
         <body><div class='card'><h1>{heading}</h1><p>{message}</p><p>You can close this window.</p></div>\
         <script>(function(){{if(window.opener){{window.opener.postMessage({payload},\"*\")}}{auto_close}}})();</script></body></html>",
        heading = if success {
            "Authorization complete"
        } else {
            "Authorization failed"
        },
        message = escape_html(message),
        payload = payload,
        auto_close = auto_close,
    );
    Html(html).into_response()
}

fn box_popup(flow_id: &str, success: bool, message: &str) -> Response {
    oauth_popup(
        flow_id,
        success,
        message,
        "ragflow-box-oauth",
        "Box Authorization",
    )
}

/// POST /api/v1/connectors/box/oauth/web/start
pub async fn box_oauth_start(
    Json(body): Json<BoxStartRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let client_id = body.client_id.trim().to_string();
    let client_secret = body.client_secret.trim().to_string();
    let redirect_uri = body.redirect_uri.unwrap_or_default().trim().to_string();
    if client_id.is_empty() || client_secret.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Box client_id and client_secret are required.".into(),
        ));
    }
    if !redirect_uri.is_empty()
        && !redirect_uri.starts_with("https://")
        && !redirect_uri.starts_with("http://")
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Box redirect_uri must be an http(s) URL.".into(),
        ));
    }
    let flow_id = uuid::Uuid::new_v4().to_string();
    let authorization_url = format!(
        "https://account.box.com/api/oauth2/authorize?response_type=code&client_id={client_id}&redirect_uri={redirect_uri}&state={flow_id}"
    );
    let flow = BoxFlow {
        client_id,
        client_secret,
        redirect_uri,
        created_at: SystemTime::now(),
        code: None,
        access_token: None,
        refresh_token: None,
    };
    registry()
        .write()
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "flow lock poisoned".into(),
            )
        })?
        .flows
        .insert(flow_id.clone(), flow);
    Ok(Json(serde_json::json!({
        "code": 0,
        "data": { "flow_id": flow_id, "authorization_url": authorization_url, "expires_in": FLOW_TTL_SECS }
    })))
}

/// GET /api/v1/connectors/box/oauth/web/callback
pub async fn box_oauth_callback(Query(params): Query<HashMap<String, String>>) -> Response {
    let flow_id = params.get("state").cloned().unwrap_or_default();
    if flow_id.is_empty() {
        return box_popup("", false, "Missing OAuth parameters.");
    }
    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .cloned()
            .unwrap_or_else(|| error.clone());
        if let Ok(mut registry) = registry().write() {
            registry.flows.remove(&flow_id);
        }
        return box_popup(&flow_id, false, &description);
    }
    let code = params.get("code").cloned().unwrap_or_default();
    if code.is_empty() {
        return box_popup(&flow_id, false, "Missing authorization code from Box.");
    }
    let mut flow = {
        let mut registry = registry().write().expect("flow lock poisoned");
        let Some(flow) = registry.flows.remove(&flow_id) else {
            return box_popup(&flow_id, false, "Box OAuth session expired or invalid.");
        };
        flow
    };
    if expired(&flow) {
        return box_popup(&flow_id, false, "Box OAuth session expired or invalid.");
    }
    flow.code = Some(code.clone());
    let token_url = std::env::var("BOX_OAUTH_TOKEN_URL").ok();
    box_token_exchange(&mut flow, &code, token_url.as_deref()).await;
    // Store the completed result under the same flow id for the poll endpoint.
    registry()
        .write()
        .expect("flow lock poisoned")
        .flows
        .insert(flow_id.clone(), flow);
    box_popup(&flow_id, true, "Authorization completed successfully.")
}

/// POST /api/v1/connectors/box/oauth/web/result
pub async fn box_oauth_result(Json(body): Json<BoxResultRequest>) -> Json<serde_json::Value> {
    let flow_id = body.flow_id.trim().to_string();
    let flow = {
        let mut registry = registry().write().expect("flow lock poisoned");
        let Some(flow) = registry.flows.get(&flow_id) else {
            return Json(serde_json::json!({
                "code": 106,
                "message": "Authorization is still pending.",
                "data": null
            }));
        };
        let completed = flow.code.is_some();
        let flow = flow.clone();
        if completed {
            registry.flows.remove(&flow_id);
        }
        (completed, flow)
    };
    if !flow.0 {
        return Json(serde_json::json!({
            "code": 106,
            "message": "Authorization is still pending.",
            "data": null
        }));
    }
    Json(serde_json::json!({
        "code": 0,
        "data": { "credentials": {
            "client_id": flow.1.client_id,
            "client_secret": flow.1.client_secret,
            "redirect_uri": flow.1.redirect_uri,
            "code": flow.1.code,
            "access_token": flow.1.access_token,
            "refresh_token": flow.1.refresh_token,
        } }
    }))
}
#[derive(Debug, Deserialize)]
pub struct GoogleStartRequest {
    pub credentials: serde_json::Value,
    #[serde(default)]
    pub redirect_uri: Option<String>,
}

fn google_client_config(credentials: &serde_json::Value) -> Option<(String, String)> {
    for key in ["web", "installed"] {
        if let Some(config) = credentials.get(key) {
            let client_id = config
                .get("client_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let client_secret = config
                .get("client_secret")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !client_id.is_empty() && !client_secret.is_empty() {
                return Some((client_id, client_secret));
            }
        }
    }
    None
}

/// POST /api/v1/connectors/google/oauth/web/start?type=google-drive|gmail
pub async fn google_oauth_start(
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<GoogleStartRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let source = params.get("type").cloned().unwrap_or_default();
    if source != "google-drive" && source != "gmail" {
        return Err((StatusCode::BAD_REQUEST, "Invalid Google OAuth type.".into()));
    }
    let credentials = match &body.credentials {
        serde_json::Value::String(text) => serde_json::from_str::<serde_json::Value>(text)
            .map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    "Invalid credentials JSON.".to_string(),
                )
            })?,
        other => other.clone(),
    };
    if credentials
        .get("refresh_token")
        .is_some_and(|value| !value.is_null())
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Uploaded credentials already include a refresh token.".into(),
        ));
    }
    let (client_id, client_secret) = google_client_config(&credentials).ok_or((
        StatusCode::BAD_REQUEST,
        "Uploaded credentials do not contain client_id/client_secret.".to_string(),
    ))?;
    let default_redirect_env = if source == "gmail" {
        "GMAIL_WEB_OAUTH_REDIRECT_URI"
    } else {
        "GOOGLE_DRIVE_WEB_OAUTH_REDIRECT_URI"
    };
    let redirect_uri = body
        .redirect_uri
        .clone()
        .or_else(|| std::env::var(default_redirect_env).ok())
        .unwrap_or_default()
        .trim()
        .to_string();
    if redirect_uri.is_empty() {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Google OAuth redirect URI is not configured on the server.".into(),
        ));
    }
    let scopes = if source == "gmail" {
        GMAIL_SCOPES
    } else {
        GOOGLE_DRIVE_SCOPES
    };
    let flow_id = uuid::Uuid::new_v4().to_string();
    let authorization_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?response_type=code&client_id={client_id}&redirect_uri={redirect_uri}&scope={}&access_type=offline&include_granted_scopes=true&prompt=consent&state={flow_id}",
        scopes.join(" ")
    );
    let flow = GoogleFlow {
        source,
        client_id,
        client_secret,
        redirect_uri,
        raw_config: credentials.to_string(),
        created_at: SystemTime::now(),
        code: None,
        access_token: None,
        refresh_token: None,
    };
    google_registry()
        .write()
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "flow lock poisoned".into(),
            )
        })?
        .flows
        .insert(flow_id.clone(), flow);
    Ok(Json(serde_json::json!({
        "code": 0,
        "data": { "flow_id": flow_id, "authorization_url": authorization_url, "expires_in": FLOW_TTL_SECS }
    })))
}

async fn google_callback_impl(params: HashMap<String, String>, source: &str) -> Response {
    let payload_type = if source == "gmail" {
        "ragflow-gmail-oauth"
    } else {
        "ragflow-google-drive-oauth"
    };
    let title = if source == "gmail" {
        "Google Gmail Authorization"
    } else {
        "Google Drive Authorization"
    };
    let flow_id = params.get("state").cloned().unwrap_or_default();
    if flow_id.is_empty() {
        return oauth_popup(
            "",
            false,
            "Missing OAuth state parameter.",
            payload_type,
            title,
        );
    }
    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .cloned()
            .unwrap_or_else(|| error.clone());
        google_registry()
            .write()
            .expect("flow lock poisoned")
            .flows
            .remove(&flow_id);
        return oauth_popup(&flow_id, false, &description, payload_type, title);
    }
    let code = params.get("code").cloned().unwrap_or_default();
    if code.is_empty() {
        return oauth_popup(
            &flow_id,
            false,
            "Missing authorization code from Google.",
            payload_type,
            title,
        );
    }
    let mut flow = {
        let mut registry = google_registry().write().expect("flow lock poisoned");
        let Some(flow) = registry.flows.remove(&flow_id) else {
            return oauth_popup(
                &flow_id,
                false,
                "Authorization session expired. Please restart from the main window.",
                payload_type,
                title,
            );
        };
        flow
    };
    if flow.source != source || google_expired(&flow) {
        return oauth_popup(
            &flow_id,
            false,
            "Authorization session was invalid. Please retry.",
            payload_type,
            title,
        );
    }
    flow.code = Some(code.clone());
    let token_url = std::env::var("GOOGLE_OAUTH_TOKEN_URL").ok();
    google_token_exchange(&mut flow, &code, token_url.as_deref()).await;
    google_registry()
        .write()
        .expect("flow lock poisoned")
        .flows
        .insert(flow_id.clone(), flow);
    oauth_popup(
        &flow_id,
        true,
        "Authorization completed successfully.",
        payload_type,
        title,
    )
}

/// GET /api/v1/connectors/gmail/oauth/web/callback
pub async fn gmail_oauth_callback(Query(params): Query<HashMap<String, String>>) -> Response {
    google_callback_impl(params, "gmail").await
}

/// GET /api/v1/connectors/google-drive/oauth/web/callback
pub async fn google_drive_oauth_callback(
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    google_callback_impl(params, "google-drive").await
}

/// POST /api/v1/connectors/google/oauth/web/result?type=google-drive|gmail
pub async fn google_oauth_result(
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<BoxResultRequest>,
) -> Json<serde_json::Value> {
    let source = params.get("type").cloned().unwrap_or_default();
    let flow = {
        let mut registry = google_registry().write().expect("flow lock poisoned");
        let Some(flow) = registry.flows.get(&body.flow_id).cloned() else {
            return Json(serde_json::json!({
                "code": 106,
                "message": "Authorization is still pending.",
                "data": null
            }));
        };
        if flow.source != source {
            return Json(serde_json::json!({
                "code": 106,
                "message": "Authorization is still pending.",
                "data": null
            }));
        }
        let completed = flow.code.is_some();
        if completed {
            registry.flows.remove(&body.flow_id);
        }
        (completed, flow)
    };
    if !flow.0 {
        return Json(serde_json::json!({
            "code": 106,
            "message": "Authorization is still pending.",
            "data": null
        }));
    }
    let mut credentials: serde_json::Value =
        serde_json::from_str(&flow.1.raw_config).unwrap_or(serde_json::json!({}));
    if let Some(object) = credentials.as_object_mut() {
        object.insert(
            "redirect_uri".to_string(),
            serde_json::json!(flow.1.redirect_uri),
        );
        if let Some(access) = flow.1.access_token {
            object.insert("access_token".to_string(), serde_json::json!(access));
        }
        if let Some(refresh) = flow.1.refresh_token {
            object.insert("refresh_token".to_string(), serde_json::json!(refresh));
        }
        if let Some(code) = flow.1.code {
            object.insert("code".to_string(), serde_json::json!(code));
        }
    }
    Json(serde_json::json!({
        "code": 0,
        "data": { "credentials": credentials.to_string() }
    }))
}


