//! Agent webhook surface, ported from RAGFlow v0.27.2
//! api/apps/restful_apis/agent_api.py:
//!
//! - ANY  /api/v1/agents/{agent_id}/webhook       - inbound trigger (public)
//! - ANY  /api/v1/agents/{agent_id}/webhook/test  - owner-only trigger with trace
//! - GET  /api/v1/agents/{agent_id}/webhook/logs  - poll the trace of a test run
//!
//! The upstream contract is an *inbound* trigger: the Begin component must use
//! mode "Webhook". Its params carry the allowed HTTP methods, content type,
//! request schema (query/headers/body), security (auth type, IP whitelist, body
//! size, token-bucket rate limit), execution mode (Immediately / Streaming) and
//! the response template. The request values become the Begin component's
//! query / headers / body outputs, exactly like upstream's
//! canvas.run(query="", user_id=..., webhook_payload=clean_request).
//!
//! Errors keep the upstream envelope: HTTP 200 with {"code": 400, "message": ...}
//! for validation failures, and a raw templated response for Immediately
//! success. Test-run traces live in a process-local TTL store (upstream uses
//! Redis); the id encoding matches upstream (webhook_id_secret HMAC).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path as AxumPath, RawQuery, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256, Sha384, Sha512};

use crate::server::{AppState, AuthContext};

const WEBHOOK_TRACE_TTL: Duration = Duration::from_secs(600);
const WEBHOOK_ID_SECRET: &[u8] = b"webhook_id_secret";
const DEFAULT_MAX_BODY_SIZE: &str = "10MB";
const HARD_MAX_BODY_SIZE: usize = 10 * 1024 * 1024;

/// ANY /api/v1/agents/{agent_id}/webhook - public inbound trigger.
pub async fn webhook_trigger(
    State(state): State<Arc<AppState>>,
    AxumPath(agent_id): AxumPath<String>,
    method: Method,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RawQuery(raw_query): RawQuery,
    body: Bytes,
) -> Response {
    dispatch(
        state, agent_id, method, headers, peer, raw_query, body, false, None,
    )
    .await
}

/// ANY /api/v1/agents/{agent_id}/webhook/test - owner-only trigger that also
/// records a trace for the webhook sheet.
pub async fn webhook_test(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    AxumPath(agent_id): AxumPath<String>,
    method: Method,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RawQuery(raw_query): RawQuery,
    body: Bytes,
) -> Response {
    dispatch(
        state,
        agent_id,
        method,
        headers,
        peer,
        raw_query,
        body,
        true,
        Some(auth),
    )
    .await
}

/// GET /api/v1/agents/{agent_id}/webhook/logs - poll test-run trace events.
pub async fn webhook_trace(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    AxumPath(agent_id): AxumPath<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let Some(agent) = state.agents.list().into_iter().find(|a| a.id == agent_id) else {
        return upstream_error(102, "Canvas not found.");
    };
    if agent.owner_id != auth.user_id && !auth.is_admin {
        return upstream_error(102, "Canvas not found.");
    }
    let params = parse_query(raw_query.as_deref().unwrap_or_default());
    let since_ts = params
        .get("since_ts")
        .and_then(|value| value.as_str())
        .and_then(|value| value.trim().parse::<f64>().ok());
    let webhook_id = params
        .get("webhook_id")
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    if since_ts.is_none() {
        return Json(json!({
            "code": 0,
            "data": {
                "webhook_id": Value::Null,
                "events": [],
                "next_since_ts": now_f64(),
                "finished": false,
            }
        }))
        .into_response();
    }
    let since_ts = since_ts.unwrap_or_default();
    let trace = trace_store().lock().unwrap().snapshot(&agent.id);
    let Some(trace) = trace else {
        return Json(json!({
            "code": 0,
            "data": {
                "webhook_id": Value::Null,
                "events": [],
                "next_since_ts": since_ts,
                "finished": false,
            }
        }))
        .into_response();
    };
    let webhooks = trace
        .get("webhooks")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if webhook_id.is_none() {
        let mut candidates: Vec<f64> = webhooks
            .keys()
            .filter_map(|key| key.parse::<f64>().ok())
            .filter(|ts| *ts > since_ts)
            .collect();
        candidates.sort_by(|left, right| left.total_cmp(right));
        let Some(start_ts) = candidates.first().copied() else {
            return Json(json!({
                "code": 0,
                "data": {
                    "webhook_id": Value::Null,
                    "events": [],
                    "next_since_ts": since_ts,
                    "finished": false,
                }
            }))
            .into_response();
        };
        return Json(json!({
            "code": 0,
            "data": {
                "webhook_id": encode_webhook_id(start_ts),
                "events": [],
                "next_since_ts": start_ts,
                "finished": false,
            }
        }))
        .into_response();
    }
    let webhook_id = webhook_id.unwrap_or_default();
    let Some(real_id) = webhooks
        .keys()
        .find(|key| encode_webhook_id_of(key) == webhook_id)
        .cloned()
    else {
        return Json(json!({
            "code": 0,
            "data": {
                "webhook_id": webhook_id,
                "events": [],
                "next_since_ts": since_ts,
                "finished": true,
            }
        }))
        .into_response();
    };
    let events = webhooks
        .get(&real_id)
        .and_then(|entry| entry.get("events"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut next_ts = since_ts;
    let mut finished = false;
    let mut new_events = Vec::new();
    for event in events {
        let ts = event.get("ts").and_then(Value::as_f64).unwrap_or_default();
        if ts <= since_ts {
            continue;
        }
        next_ts = next_ts.max(ts);
        if event.get("event").and_then(Value::as_str) == Some("finished") {
            finished = true;
        }
        new_events.push(event);
    }
    Json(json!({
        "code": 0,
        "data": {
            "webhook_id": webhook_id,
            "events": new_events,
            "next_since_ts": next_ts,
            "finished": finished,
        }
    }))
    .into_response()
}

async fn dispatch(
    state: Arc<AppState>,
    agent_id: String,
    method: Method,
    headers: HeaderMap,
    peer: SocketAddr,
    raw_query: Option<String>,
    body: Bytes,
    is_test: bool,
    auth: Option<AuthContext>,
) -> Response {
    let Some(agent) = state.agents.list().into_iter().find(|a| a.id == agent_id) else {
        return upstream_error(102, "Canvas not found.");
    };
    if is_test {
        let authorized = auth
            .as_ref()
            .is_some_and(|auth| auth.user_id == agent.owner_id || auth.is_admin);
        if !authorized {
            tracing::warn!(agent_id = %agent.id, "Webhook test denied: owner check failed");
            return upstream_error(
                103,
                "Only the owner of the agent is authorized for this operation.",
            );
        }
    }
    if agent.canvas_category == crate::api::db::CanvasCategory::DataFlow.as_str() {
        return upstream_error(400, "Dataflow can not be triggered by webhook.");
    }
    if !agent.dsl.is_object() {
        return upstream_error(400, "Invalid DSL format.");
    }
    let Some(params) = begin_webhook_params(&agent.dsl) else {
        return upstream_error(400, "Webhook not configured for this agent.");
    };
    let request_method = method.as_str().to_ascii_uppercase();
    let allowed_methods = params
        .get("methods")
        .and_then(Value::as_array)
        .map(|methods| {
            methods
                .iter()
                .filter_map(Value::as_str)
                .map(|method| method.trim().to_ascii_uppercase())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !allowed_methods.is_empty() && !allowed_methods.contains(&request_method) {
        return upstream_error(
            400,
            &format!("HTTP method '{request_method}' not allowed for this webhook."),
        );
    }
    let content_type = content_type_of(&headers);
    if let Err(message) = validate_security(
        &agent.id,
        &params,
        &request_method,
        &headers,
        peer.ip(),
        body.len(),
    ) {
        tracing::warn!(agent_id = %agent.id, %message, "Webhook denied");
        return upstream_error(400, &message);
    }
    let expected_content_type = params
        .get("content_types")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    if let Some(expected) = expected_content_type
        && !content_type.is_empty()
        && content_type != expected
    {
        return upstream_error(
            400,
            &format!("Invalid Content-Type: expect '{expected}', got '{content_type}'"),
        );
    }
    let parsed = match parse_webhook_request(&content_type, &headers, raw_query.as_deref(), &body) {
        Ok(parsed) => parsed,
        Err(message) => return upstream_error(400, &message),
    };
    let schema = params
        .get("schema")
        .cloned()
        .unwrap_or_else(|| json!({"query": {}, "headers": {}, "body": {}}));
    let (query_clean, headers_clean, body_clean) = match extract_request(&parsed, &schema) {
        Ok(cleaned) => cleaned,
        Err(message) => return upstream_error(400, &message),
    };
    let clean_request = json!({
        "query": query_clean,
        "headers": headers_clean,
        "body": body_clean,
        "input": parsed,
    });
    let execution_mode = params
        .get("execution_mode")
        .and_then(Value::as_str)
        .unwrap_or("Immediately");
    let response_cfg = params.get("response").cloned().unwrap_or_else(|| json!({}));
    let start_ts = now_f64();

    if execution_mode == "Immediately" {
        let status = response_cfg
            .get("status")
            .and_then(Value::as_i64)
            .unwrap_or(200);
        if !(200..=399).contains(&status) {
            return upstream_error(
                400,
                &format!("Invalid response status code: {status}, must be between 200 and 399"),
            );
        }
        let template = response_cfg
            .get("body_template")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let (rendered, rendered_type) = render_template(&template);
        let state_for_run = state.clone();
        let agent_for_run = agent.clone();
        let trace_agent = agent.id.clone();
        tokio::spawn(async move {
            run_background(
                state_for_run,
                agent_for_run,
                clean_request,
                is_test,
                trace_agent,
                start_ts,
            )
            .await;
        });
        let status = StatusCode::from_u16(status as u16).unwrap_or(StatusCode::OK);
        return (
            status,
            [(axum::http::header::CONTENT_TYPE, rendered_type)],
            rendered,
        )
            .into_response();
    }

    // Streaming: run synchronously and return the assistant text.
    let observer = TraceObserver::new(&agent.id, start_ts);
    let run = crate::api::features::run_agent_workflow_for_webhook(
        &state,
        &agent,
        &begin_inputs(&clean_request),
        if is_test { Some(&observer) } else { None },
    )
    .await;
    match run {
        Ok(result) => {
            if is_test {
                observer.finished(true, None);
            }
            (
                StatusCode::OK,
                Json(json!({
                    "message": result.answer,
                    "success": true,
                    "code": 200,
                })),
            )
                .into_response()
        }
        Err(error) => {
            if is_test {
                observer.finished(false, Some(error.to_string()));
            }
            tracing::warn!(agent_id = %agent.id, %error, "Webhook run failed");
            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "code": 400,
                    "message": error.to_string(),
                    "success": false,
                })),
            )
                .into_response()
        }
    }
}

async fn run_background(
    state: Arc<AppState>,
    agent: crate::api::features::Agent,
    clean_request: Value,
    is_test: bool,
    trace_agent: String,
    start_ts: f64,
) {
    let observer = TraceObserver::new(&trace_agent, start_ts);
    let result = crate::api::features::run_agent_workflow_for_webhook(
        &state,
        &agent,
        &begin_inputs(&clean_request),
        if is_test { Some(&observer) } else { None },
    )
    .await;
    if is_test {
        match result {
            Ok(_) => observer.finished(true, None),
            Err(error) => {
                tracing::warn!(agent_id = %trace_agent, %error, "Webhook background run failed");
                observer.finished(false, Some(error.to_string()));
            }
        }
    }
}

/// Upstream passes clean_request as webhook_payload; the canvas writes
/// query / headers / body onto the Begin component's outputs. RayRAG's Begin
/// executor consumes {value: ...} descriptors, so wrap each section.
pub(crate) fn begin_inputs(clean_request: &Value) -> Map<String, Value> {
    let mut inputs = Map::new();
    for key in ["query", "headers", "body"] {
        let value = clean_request.get(key).cloned().unwrap_or_else(|| json!({}));
        inputs.insert(key.to_string(), json!({"value": value}));
    }
    inputs
}

/// Find the Begin component with params.mode == "Webhook" and return its params.
///
/// The canvas normalizer rewrites authored components (obj.component_name /
/// obj.params) into a flat form (name / params), so both shapes are accepted.
pub(crate) fn begin_webhook_params(dsl: &Value) -> Option<Map<String, Value>> {
    let components = dsl.get("components")?.as_object()?;
    for (id, component) in components {
        let object = component.get("obj");
        let name = object
            .and_then(|object| object.get("component_name"))
            .and_then(Value::as_str)
            .or_else(|| component.get("name").and_then(Value::as_str))
            .unwrap_or(id);
        let params = object
            .and_then(|object| object.get("params"))
            .and_then(Value::as_object)
            .or_else(|| component.get("params").and_then(Value::as_object))?;
        if name.eq_ignore_ascii_case("begin")
            && params.get("mode").and_then(Value::as_str) == Some("Webhook")
        {
            return Some(params.clone());
        }
    }
    None
}

fn validate_security(
    agent_id: &str,
    params: &Map<String, Value>,
    method: &str,
    headers: &HeaderMap,
    peer_ip: IpAddr,
    body_len: usize,
) -> Result<(), String> {
    let _ = method;
    let security = params
        .get("security")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if security.is_empty() {
        return Err(
            "Webhook security is required. Set allow_anonymous to true to permit unauthenticated webhooks."
                .to_string(),
        );
    }
    let max_size = security
        .get("max_body_size")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_MAX_BODY_SIZE);
    let limit = parse_body_size(max_size)?;
    if limit > HARD_MAX_BODY_SIZE {
        return Err("max_body_size exceeds maximum allowed size (10MB)".to_string());
    }
    if body_len > limit {
        return Err(format!("Request body too large: {body_len} > {limit}"));
    }
    let whitelist = security
        .get("ip_whitelist")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !whitelist.is_empty() {
        let allowed = whitelist.iter().any(|rule| {
            rule.as_str()
                .is_some_and(|rule| ip_allowed(peer_ip, rule.trim()))
        });
        if !allowed {
            return Err(format!("IP {peer_ip} is not allowed by whitelist"));
        }
    }
    let rate_limit = security
        .get("rate_limit")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| {
            let mut map = Map::new();
            map.insert("limit".into(), json!(60));
            map.insert("per".into(), json!("minute"));
            map
        });
    let per = rate_limit
        .get("per")
        .and_then(Value::as_str)
        .unwrap_or("minute");
    let limit = rate_limit
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(60);
    if limit <= 0 {
        return Err("rate_limit.limit must be > 0".to_string());
    }
    let window = match per {
        "second" => 1.0,
        "minute" => 60.0,
        "hour" => 3600.0,
        "day" => 86400.0,
        other => return Err(format!("Invalid rate_limit.per: {other}")),
    };
    if !rate_limit_allow(agent_id, limit as f64, window) {
        return Err("Rate limit error: Too many requests (rate limit exceeded)".to_string());
    }
    let auth_type = security
        .get("auth_type")
        .and_then(Value::as_str)
        .unwrap_or("none");
    if auth_type == "none" {
        if security
            .get("allow_anonymous")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(());
        }
        return Err("Anonymous webhook access requires allow_anonymous to be true".to_string());
    }
    if auth_type == "token" {
        let token = security
            .get("token")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let header = token
            .get("token_header")
            .and_then(Value::as_str)
            .unwrap_or("");
        let expected = token
            .get("token_value")
            .and_then(Value::as_str)
            .unwrap_or("");
        let provided = header_value(headers, header).unwrap_or_default();
        if provided != expected {
            return Err("Invalid token authentication".to_string());
        }
        return Ok(());
    }
    if auth_type == "basic" {
        let basic = security
            .get("basic_auth")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let username = basic.get("username").and_then(Value::as_str).unwrap_or("");
        let password = basic.get("password").and_then(Value::as_str).unwrap_or("");
        let provided = header_value(headers, "authorization").unwrap_or_default();
        let Some(encoded) = provided.strip_prefix("Basic ") else {
            return Err("Invalid Basic Auth credentials".to_string());
        };
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .map_err(|_| "Invalid Basic Auth credentials".to_string())?;
        let decoded = String::from_utf8_lossy(&decoded);
        let Some((provided_user, provided_password)) = decoded.split_once(':') else {
            return Err("Invalid Basic Auth credentials".to_string());
        };
        if provided_user != username || provided_password != password {
            return Err("Invalid Basic Auth credentials".to_string());
        }
        return Ok(());
    }
    if auth_type == "jwt" {
        return validate_jwt_security(&security, headers);
    }
    Err(format!("Unsupported auth_type: {auth_type}"))
}

fn validate_jwt_security(security: &Map<String, Value>, headers: &HeaderMap) -> Result<(), String> {
    let jwt = security
        .get("jwt")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let secret = jwt.get("secret").and_then(Value::as_str).unwrap_or("");
    if secret.is_empty() {
        return Err("JWT secret not configured".to_string());
    }
    let authorization = header_value(headers, "authorization").unwrap_or_default();
    let Some(token) = authorization.strip_prefix("Bearer ") else {
        return Err("Missing Bearer token".to_string());
    };
    let token = token.trim();
    if token.is_empty() {
        return Err("Empty Bearer token".to_string());
    }
    let algorithm = jwt
        .get("algorithm")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("HS256")
        .to_ascii_uppercase();
    let claims = decode_jwt(token, secret.as_bytes(), &algorithm)?;
    if let Some(exp) = claims.get("exp").and_then(Value::as_f64)
        && now_f64() >= exp
    {
        return Err("Invalid JWT: token is expired".to_string());
    }
    if let Some(nbf) = claims.get("nbf").and_then(Value::as_f64)
        && now_f64() < nbf
    {
        return Err("Invalid JWT: token is not yet valid".to_string());
    }
    if let Some(issuer) = jwt
        .get("issuer")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        let actual = claims
            .get("iss")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if actual != issuer {
            return Err("Invalid JWT: invalid issuer".to_string());
        }
    }
    if let Some(audience) = jwt
        .get("audience")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        let matches = match claims.get("aud") {
            Some(Value::String(value)) => value == audience,
            Some(Value::Array(values)) => {
                values.iter().any(|value| value.as_str() == Some(audience))
            }
            _ => false,
        };
        if !matches {
            return Err("Invalid JWT: invalid audience".to_string());
        }
    }
    let required_claims: Vec<String> = jwt
        .get("required_claims")
        .map(|value| match value {
            Value::String(value) => vec![value.clone()],
            Value::Array(values) => values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default();
    for claim in required_claims {
        if claim.trim().is_empty() {
            continue;
        }
        if matches!(
            claim.as_str(),
            "exp" | "sub" | "aud" | "iss" | "nbf" | "iat"
        ) {
            return Err(format!("Reserved JWT claim cannot be required: {claim}"));
        }
        if !claims.contains_key(&claim) {
            return Err(format!("Missing JWT claim: {claim}"));
        }
    }
    Ok(())
}

fn decode_jwt(token: &str, secret: &[u8], algorithm: &str) -> Result<Map<String, Value>, String> {
    let mut segments = token.split('.');
    let (Some(header_b64), Some(payload_b64), Some(signature_b64), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err("Invalid JWT: malformed token".to_string());
    };
    let header: Value = decode_base64_json(header_b64)?;
    let header_alg = header
        .get("alg")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_uppercase();
    if header_alg != algorithm {
        return Err(format!(
            "Invalid JWT: algorithm mismatch (header '{header_alg}', expected '{algorithm}')"
        ));
    }
    let signing_input = format!("{header_b64}.{payload_b64}");
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature_b64.trim_end_matches('='))
        .map_err(|_| "Invalid JWT: malformed signature".to_string())?;
    let verified = match algorithm {
        "HS256" => {
            let mut mac = Hmac::<Sha256>::new_from_slice(secret)
                .map_err(|_| "Invalid JWT: bad secret".to_string())?;
            mac.update(signing_input.as_bytes());
            mac.verify_slice(&signature).is_ok()
        }
        "HS384" => {
            let mut mac = Hmac::<Sha384>::new_from_slice(secret)
                .map_err(|_| "Invalid JWT: bad secret".to_string())?;
            mac.update(signing_input.as_bytes());
            mac.verify_slice(&signature).is_ok()
        }
        "HS512" => {
            let mut mac = Hmac::<Sha512>::new_from_slice(secret)
                .map_err(|_| "Invalid JWT: bad secret".to_string())?;
            mac.update(signing_input.as_bytes());
            mac.verify_slice(&signature).is_ok()
        }
        other => return Err(format!("Invalid JWT: unsupported algorithm '{other}'")),
    };
    if !verified {
        return Err("Invalid JWT: signature verification failed".to_string());
    }
    let payload = decode_base64_json(payload_b64)?;
    payload
        .as_object()
        .cloned()
        .ok_or_else(|| "Invalid JWT: payload is not an object".to_string())
}

fn decode_base64_json(segment: &str) -> Result<Value, String> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segment.trim_end_matches('='))
        .map_err(|_| "Invalid JWT: malformed base64".to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| "Invalid JWT: malformed JSON".to_string())
}

fn parse_body_size(text: &str) -> Result<usize, String> {
    let lower = text.trim().to_ascii_lowercase();
    for (suffix, factor) in [("kb", 1024usize), ("mb", 1024 * 1024)] {
        if let Some(number) = lower.strip_suffix(suffix) {
            let value: f64 = number
                .trim()
                .parse()
                .map_err(|_| "Invalid max_body_size format".to_string())?;
            return Ok((value * factor as f64) as usize);
        }
    }
    lower
        .parse::<usize>()
        .map_err(|_| "Invalid max_body_size format".to_string())
}

fn ip_allowed(peer: IpAddr, rule: &str) -> bool {
    if let Some((network, prefix)) = rule.split_once('/') {
        let Ok(network) = network.parse::<IpAddr>() else {
            return false;
        };
        if network.is_ipv4() != peer.is_ipv4() {
            return false;
        }
        let Ok(prefix) = prefix.trim().parse::<u32>() else {
            return false;
        };
        match (peer, network) {
            (IpAddr::V4(peer), IpAddr::V4(network)) => {
                if prefix > 32 {
                    return false;
                }
                let mask = if prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - prefix)
                };
                (u32::from(peer) & mask) == (u32::from(network) & mask)
            }
            (IpAddr::V6(peer), IpAddr::V6(network)) => {
                if prefix > 128 {
                    return false;
                }
                let mask = if prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - prefix)
                };
                (u128::from(peer) & mask) == (u128::from(network) & mask)
            }
            _ => false,
        }
    } else {
        rule.parse::<Ipv4Addr>()
            .map(IpAddr::V4)
            .or_else(|_| rule.parse::<Ipv6Addr>().map(IpAddr::V6))
            .is_ok_and(|allowed| allowed == peer)
    }
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let name = axum::http::HeaderName::from_bytes(name.as_bytes()).ok()?;
    let value = headers.get(name)?.to_str().ok()?;
    Some(value.to_string())
}

fn content_type_of(headers: &HeaderMap) -> String {
    headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn parse_query(raw: &str) -> Map<String, Value> {
    let mut map = Map::new();
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(
            decode_component(key),
            Value::String(decode_component(value)),
        );
    }
    map
}

fn decode_component(text: &str) -> String {
    let replaced = text.replace('+', " ");
    let bytes = replaced.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn headers_to_map(headers: &HeaderMap) -> Map<String, Value> {
    let mut map = Map::new();
    for (name, value) in headers {
        let name = name.as_str().to_ascii_lowercase();
        let value = value.to_str().unwrap_or_default().to_string();
        map.entry(name)
            .and_modify(|existing| {
                if let Value::String(existing) = existing {
                    existing.push_str(", ");
                    existing.push_str(&value);
                }
            })
            .or_insert(Value::String(value));
    }
    map
}

fn parse_webhook_request(
    content_type: &str,
    headers: &HeaderMap,
    raw_query: Option<&str>,
    body: &[u8],
) -> Result<Value, String> {
    let query = parse_query(raw_query.unwrap_or_default());
    let header_map = headers_to_map(headers);
    let body_value = match content_type {
        "application/json" => {
            if body.is_empty() {
                json!({})
            } else {
                serde_json::from_slice(body)
                    .map_err(|error| format!("Invalid JSON body: {error}"))?
            }
        }
        "application/x-www-form-urlencoded" => {
            let text = String::from_utf8_lossy(body);
            Value::Object(parse_query(&text))
        }
        "multipart/form-data" => {
            let boundary = headers
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| {
                    value.split(';').find_map(|part| {
                        part.trim()
                            .strip_prefix("boundary=")
                            .map(|boundary| boundary.trim_matches('"').to_string())
                    })
                })
                .ok_or_else(|| "Invalid multipart body: missing boundary".to_string())?;
            parse_multipart_form(&boundary, body)?
        }
        _ => {
            if body.is_empty() {
                json!({})
            } else {
                serde_json::from_slice(body).unwrap_or_else(|_| json!({}))
            }
        }
    };
    Ok(json!({
        "query": query,
        "headers": header_map,
        "body": body_value,
        "content_type": content_type,
    }))
}

/// Minimal multipart/form-data parser: text fields are captured, file parts are
/// rejected explicitly (upstream parses them into canvas files; RayRAG does not
/// silently drop them).
fn parse_multipart_form(boundary: &str, body: &[u8]) -> Result<Value, String> {
    let marker = format!("--{boundary}");
    let mut map = Map::new();
    let text = String::from_utf8_lossy(body);
    let mut rest = text.as_ref();
    while let Some(start) = rest.find(&marker) {
        rest = &rest[start + marker.len()..];
        if rest.starts_with("--") {
            break;
        }
        let rest_trimmed = rest.strip_prefix("\r\n").unwrap_or(rest);
        let Some(header_end) = rest_trimmed.find("\r\n\r\n") else {
            break;
        };
        let header_block = &rest_trimmed[..header_end];
        let remainder = &rest_trimmed[header_end + 4..];
        let Some(value_end) = remainder.find(&format!("\r\n{marker}")) else {
            break;
        };
        let value = &remainder[..value_end];
        let mut name = None;
        let mut filename = None;
        for line in header_block.split("\r\n") {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("content-disposition:") {
                for part in line.split(';') {
                    let part = part.trim();
                    if let Some(field) = part.strip_prefix("name=") {
                        name = Some(field.trim_matches('"').to_string());
                    }
                    if let Some(field) = part.strip_prefix("filename=") {
                        filename = Some(field.trim_matches('"').to_string());
                    }
                }
            }
        }
        if filename.is_some_and(|filename| !filename.is_empty()) {
            return Err("Webhook file uploads are not supported by RayRAG yet".to_string());
        }
        if let Some(name) = name {
            map.insert(name, Value::String(value.to_string()));
        }
        rest = &remainder[value_end..];
    }
    Ok(Value::Object(map))
}

fn extract_request(parsed: &Value, schema: &Value) -> Result<(Value, Value, Value), String> {
    let query = extract_by_schema(
        parsed.get("query").cloned().unwrap_or_else(|| json!({})),
        schema.get("query").cloned().unwrap_or_else(|| json!({})),
        "query",
    )?;
    let headers = extract_by_schema(
        parsed.get("headers").cloned().unwrap_or_else(|| json!({})),
        schema.get("headers").cloned().unwrap_or_else(|| json!({})),
        "headers",
    )?;
    let body = extract_by_schema(
        parsed.get("body").cloned().unwrap_or_else(|| json!({})),
        schema.get("body").cloned().unwrap_or_else(|| json!({})),
        "body",
    )?;
    Ok((query, headers, body))
}

fn extract_by_schema(data: Value, schema: Value, name: &str) -> Result<Value, String> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Ok(json!({}));
    };
    let required: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let data = data.as_object().cloned().unwrap_or_default();
    let mut extracted = Map::new();
    for (field, field_schema) in properties {
        let field_type = field_schema
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("string")
            .to_string();
        if required.contains(field) && !data.contains_key(field) {
            return Err(format!("{name} missing required field: {field}"));
        }
        let Some(raw) = data.get(field) else {
            extracted.insert(field.clone(), default_for_type(&field_type));
            continue;
        };
        let value = auto_cast_value(raw.clone(), &field_type)
            .map_err(|error| format!("{name}.{field} auto-cast failed: {error}"))?;
        if !validate_type(&value, &field_type) {
            return Err(format!(
                "{name}.{field} type mismatch: expected {field_type}, got {}",
                type_name(&value)
            ));
        }
        extracted.insert(field.clone(), value);
    }
    Ok(Value::Object(extracted))
}

fn default_for_type(field_type: &str) -> Value {
    match field_type {
        "file" => json!([]),
        "object" => json!({}),
        "boolean" => json!(false),
        "number" => json!(0),
        "string" => json!(""),
        "null" => Value::Null,
        other if other.starts_with("array") => json!([]),
        _ => Value::Null,
    }
}

fn auto_cast_value(value: Value, expected: &str) -> Result<Value, String> {
    let Value::String(text) = value else {
        return Ok(value);
    };
    let trimmed = text.trim();
    match expected {
        "boolean" => match trimmed.to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(json!(true)),
            "false" | "0" => Ok(json!(false)),
            _ => Err(format!("Cannot convert '{text}' to boolean")),
        },
        "number" => {
            if let Ok(int) = trimmed.parse::<i64>() {
                return Ok(json!(int));
            }
            trimmed
                .parse::<f64>()
                .map(|float| json!(float))
                .map_err(|_| format!("Cannot convert '{text}' to number"))
        }
        "object" => {
            let parsed: Value = serde_json::from_str(trimmed)
                .map_err(|_| format!("Cannot convert '{text}' to object"))?;
            if parsed.is_object() {
                Ok(parsed)
            } else {
                Err("JSON is not an object".to_string())
            }
        }
        other if other.starts_with("array") => {
            let parsed: Value = serde_json::from_str(trimmed)
                .map_err(|_| format!("Cannot convert '{text}' to array"))?;
            if parsed.is_array() {
                Ok(parsed)
            } else {
                Err("JSON is not an array".to_string())
            }
        }
        "string" => Ok(json!(text)),
        "file" => Ok(json!(text)),
        _ => Ok(json!(text)),
    }
}

fn validate_type(value: &Value, expected: &str) -> bool {
    match expected {
        "file" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        other if other.starts_with("array") => {
            let Some(values) = value.as_array() else {
                return false;
            };
            if let Some(inner_start) = other.find('<')
                && let Some(inner_end) = other.find('>')
                && inner_end > inner_start
            {
                let inner = other[inner_start + 1..inner_end].to_string();
                return values.iter().all(|value| validate_type(value, &inner));
            }
            true
        }
        _ => true,
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn render_template(template: &str) -> (String, &'static str) {
    if template.is_empty() {
        return ("null".to_string(), "application/json");
    }
    match serde_json::from_str::<Value>(template) {
        Ok(value) => (
            serde_json::to_string(&value).unwrap_or_else(|_| "null".to_string()),
            "application/json",
        ),
        Err(_) => (template.to_string(), "text/plain"),
    }
}

struct TraceStore {
    traces: HashMap<String, Value>,
}

impl TraceStore {
    fn snapshot(&mut self, agent_id: &str) -> Option<Value> {
        self.prune();
        self.traces.get(agent_id).cloned()
    }

    fn append(&mut self, agent_id: &str, start_ts: f64, mut event: Value) {
        self.prune();
        if !event.is_object() {
            event = json!({"event": event.to_string()});
        }
        event["ts"] = json!(now_f64());
        let entry = self
            .traces
            .entry(agent_id.to_string())
            .or_insert_with(|| json!({"webhooks": {}}));
        let webhooks = entry
            .as_object_mut()
            .map(|object| object.entry("webhooks").or_insert_with(|| json!({})))
            .and_then(Value::as_object_mut);
        let Some(webhooks) = webhooks else {
            return;
        };
        let key = format!("{start_ts}");
        let webhook = webhooks
            .entry(key)
            .or_insert_with(|| json!({"start_ts": start_ts, "events": []}));
        if let Some(events) = webhook.get_mut("events").and_then(Value::as_array_mut) {
            events.push(event);
        }
    }

    fn prune(&mut self) {
        let cutoff = now_f64() - WEBHOOK_TRACE_TTL.as_secs_f64();
        self.traces.retain(|_, entry| {
            let Some(webhooks) = entry.get_mut("webhooks").and_then(Value::as_object_mut) else {
                return false;
            };
            webhooks.retain(|key, _| {
                key.parse::<f64>()
                    .map(|start_ts| start_ts >= cutoff)
                    .unwrap_or(false)
            });
            !webhooks.is_empty()
        });
    }
}

fn trace_store() -> &'static Mutex<TraceStore> {
    static STORE: OnceLock<Mutex<TraceStore>> = OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(TraceStore {
            traces: HashMap::new(),
        })
    })
}

struct TraceObserver {
    agent_id: String,
    start_ts: f64,
}

impl TraceObserver {
    fn new(agent_id: &str, start_ts: f64) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            start_ts,
        }
    }

    fn finished(&self, success: bool, error: Option<String>) {
        let mut event = json!({
            "event": "finished",
            "elapsed_time": now_f64() - self.start_ts,
            "success": success,
        });
        if let Some(error) = error {
            event["message"] = json!(error);
        }
        trace_store()
            .lock()
            .unwrap()
            .append(&self.agent_id, self.start_ts, event);
    }
}

impl crate::agent::WorkflowEventObserver for TraceObserver {
    fn emit(&self, event: crate::agent::WorkflowLifecycleEvent) {
        trace_store().lock().unwrap().append(
            &self.agent_id,
            self.start_ts,
            json!({
                "event": event.event,
                "data": event.data,
            }),
        );
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

fn rate_limit_allow(agent_id: &str, capacity: f64, window_secs: f64) -> bool {
    static BUCKETS: OnceLock<Mutex<HashMap<String, Bucket>>> = OnceLock::new();
    let buckets = BUCKETS.get_or_init(|| Mutex::new(HashMap::new()));
    let rate = capacity / window_secs;
    let mut buckets = buckets.lock().unwrap();
    let now = Instant::now();
    let bucket = buckets
        .entry(format!("rl:tb:{agent_id}"))
        .or_insert(Bucket {
            tokens: capacity,
            last: now,
        });
    let elapsed = now.duration_since(bucket.last).as_secs_f64();
    bucket.tokens = (bucket.tokens + elapsed * rate).min(capacity);
    bucket.last = now;
    if bucket.tokens >= 1.0 {
        bucket.tokens -= 1.0;
        true
    } else {
        false
    }
}

fn encode_webhook_id(start_ts: f64) -> String {
    encode_webhook_id_of(&format!("{start_ts}"))
}

fn encode_webhook_id_of(start_ts: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(WEBHOOK_ID_SECRET).expect("HMAC accepts any key");
    mac.update(start_ts.as_bytes());
    let digest = mac.finalize().into_bytes();
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(digest)
        .trim_end_matches('=')
        .to_string()
}

fn now_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn upstream_error(code: i64, message: &str) -> Response {
    tracing::warn!(code, %message, "Agent webhook request rejected");
    Json(json!({"code": code, "message": message})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn webhook_dsl(security: Value) -> Value {
        json!({
            "components": {
                "begin": {
                    "obj": {
                        "component_name": "Begin",
                        "params": {
                            "mode": "Webhook",
                            "methods": ["POST"],
                            "content_types": "application/json",
                            "execution_mode": "Immediately",
                            "security": security,
                            "response": {"status": 201, "body_template": "{\"ok\": true}"},
                            "schema": {
                                "body": {
                                    "properties": {
                                        "name": {"type": "string"},
                                        "count": {"type": "number"},
                                        "flag": {"type": "boolean"}
                                    },
                                    "required": ["name"]
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    #[test]
    fn finds_the_webhook_begin_params() {
        let dsl = webhook_dsl(json!({"auth_type": "none", "allow_anonymous": true}));
        let params = begin_webhook_params(&dsl).expect("webhook params");
        assert_eq!(params["methods"], json!(["POST"]));
        assert_eq!(params["response"]["status"], json!(201));
    }

    #[test]
    fn finds_webhook_params_in_the_normalized_canvas_shape() {
        let dsl = json!({
            "components": {
                "begin": {
                    "id": "begin",
                    "name": "Begin",
                    "params": {
                        "mode": "Webhook",
                        "methods": ["GET"],
                        "security": {"auth_type": "none", "allow_anonymous": true}
                    }
                }
            }
        });
        let params = begin_webhook_params(&dsl).expect("normalized webhook params");
        assert_eq!(params["methods"], json!(["GET"]));
    }

    #[test]
    fn ignores_a_begin_with_another_mode() {
        let mut dsl = webhook_dsl(json!({"auth_type": "none", "allow_anonymous": true}));
        dsl["components"]["begin"]["obj"]["params"]["mode"] = json!("conversational");
        assert!(begin_webhook_params(&dsl).is_none());
    }

    #[test]
    fn anonymous_access_needs_the_explicit_opt_in() {
        let params = begin_webhook_params(&webhook_dsl(json!({}))).unwrap();
        let denied = validate_security(
            "agent-1",
            &params,
            "POST",
            &HeaderMap::new(),
            "127.0.0.1".parse().unwrap(),
            10,
        )
        .unwrap_err();
        assert!(denied.contains("Webhook security is required"), "{denied}");
        let params = begin_webhook_params(&webhook_dsl(json!({"auth_type": "none"}))).unwrap();
        let denied = validate_security(
            "agent-2",
            &params,
            "POST",
            &HeaderMap::new(),
            "127.0.0.1".parse().unwrap(),
            10,
        )
        .unwrap_err();
        assert!(denied.contains("allow_anonymous"), "{denied}");
        let params = begin_webhook_params(&webhook_dsl(json!({
            "auth_type": "none",
            "allow_anonymous": true
        })))
        .unwrap();
        assert!(
            validate_security(
                "agent-3",
                &params,
                "POST",
                &HeaderMap::new(),
                "127.0.0.1".parse().unwrap(),
                10,
            )
            .is_ok()
        );
    }

    #[test]
    fn token_auth_compares_the_configured_header() {
        let params = begin_webhook_params(&webhook_dsl(json!({
            "auth_type": "token",
            "token": {"token_header": "X-Hook-Token", "token_value": "s3cret"}
        })))
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-hook-token", "s3cret".parse().unwrap());
        assert!(
            validate_security(
                "agent-token",
                &params,
                "POST",
                &headers,
                "127.0.0.1".parse().unwrap(),
                10,
            )
            .is_ok()
        );
        let mut wrong = HeaderMap::new();
        wrong.insert("x-hook-token", "nope".parse().unwrap());
        let denied = validate_security(
            "agent-token-2",
            &params,
            "POST",
            &wrong,
            "127.0.0.1".parse().unwrap(),
            10,
        )
        .unwrap_err();
        assert!(denied.contains("Invalid token authentication"), "{denied}");
    }

    #[test]
    fn basic_auth_decodes_standard_base64() {
        let params = begin_webhook_params(&webhook_dsl(json!({
            "auth_type": "basic",
            "basic_auth": {"username": "u", "password": "p"}
        })))
        .unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode("u:p");
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Basic {encoded}").parse().unwrap());
        assert!(
            validate_security(
                "agent-basic",
                &params,
                "POST",
                &headers,
                "127.0.0.1".parse().unwrap(),
                10,
            )
            .is_ok()
        );
    }

    fn sign_hs256(secret: &[u8], claims: Value) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"alg":"HS256","typ":"JWT"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims).unwrap());
        let signing_input = format!("{header}.{payload}");
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(signing_input.as_bytes());
        let signature =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        format!("{signing_input}.{signature}")
    }

    #[test]
    fn jwt_auth_verifies_signature_and_claims() {
        let params = begin_webhook_params(&webhook_dsl(json!({
            "auth_type": "jwt",
            "jwt": {"secret": "jwt-secret", "algorithm": "HS256", "required_claims": ["tenant"]}
        })))
        .unwrap();
        let token = sign_hs256(
            b"jwt-secret",
            json!({"tenant": "t1", "exp": now_f64() + 60.0}),
        );
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        assert!(
            validate_security(
                "agent-jwt",
                &params,
                "POST",
                &headers,
                "127.0.0.1".parse().unwrap(),
                10,
            )
            .is_ok()
        );
        let expired = sign_hs256(
            b"jwt-secret",
            json!({"tenant": "t1", "exp": now_f64() - 10.0}),
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            format!("Bearer {expired}").parse().unwrap(),
        );
        let denied = validate_security(
            "agent-jwt",
            &params,
            "POST",
            &headers,
            "127.0.0.1".parse().unwrap(),
            10,
        )
        .unwrap_err();
        assert!(denied.contains("expired"), "{denied}");
    }

    #[test]
    fn body_size_and_content_type_rules() {
        assert_eq!(parse_body_size("1MB").unwrap(), 1024 * 1024);
        assert_eq!(parse_body_size("10kb").unwrap(), 10 * 1024);
        assert!(parse_body_size("12MB").is_ok_and(|size| size > HARD_MAX_BODY_SIZE));
        let dsl = webhook_dsl(json!({
            "auth_type": "none",
            "allow_anonymous": true,
            "max_body_size": "1KB"
        }));
        let params = begin_webhook_params(&dsl).unwrap();
        let denied = validate_security(
            "agent-body",
            &params,
            "POST",
            &HeaderMap::new(),
            "127.0.0.1".parse().unwrap(),
            2048,
        )
        .unwrap_err();
        assert!(denied.contains("Request body too large"), "{denied}");
    }

    #[test]
    fn ip_whitelist_supports_cidr_and_single_addresses() {
        let peer: IpAddr = "192.168.1.42".parse().unwrap();
        assert!(ip_allowed(peer, "192.168.1.0/24"));
        assert!(ip_allowed(peer, "192.168.1.42"));
        assert!(!ip_allowed(peer, "10.0.0.0/8"));
        let v6: IpAddr = "2001:db8::1".parse().unwrap();
        assert!(ip_allowed(v6, "2001:db8::/32"));
    }

    #[test]
    fn schema_extraction_casts_and_validates() {
        let parsed = json!({
            "query": {},
            "headers": {},
            "body": {"name": "ray", "count": "7", "flag": "true"},
        });
        let schema = json!({
            "body": {
                "properties": {
                    "name": {"type": "string"},
                    "count": {"type": "number"},
                    "flag": {"type": "boolean"},
                    "missing": {"type": "array<string>"}
                },
                "required": ["name"]
            }
        });
        let (_, _, body) = extract_request(&parsed, &schema).unwrap();
        assert_eq!(body["name"], json!("ray"));
        assert_eq!(body["count"], json!(7));
        assert_eq!(body["flag"], json!(true));
        assert_eq!(body["missing"], json!([]));
        let missing = json!({"query": {}, "headers": {}, "body": {"count": "1"}});
        let error = extract_request(&missing, &schema).unwrap_err();
        assert!(error.contains("required field: name"), "{error}");
        let bad_number = json!({
            "query": {},
            "headers": {},
            "body": {"name": "x", "count": "abc"}
        });
        let error = extract_request(&bad_number, &schema).unwrap_err();
        assert!(error.contains("auto-cast failed"), "{error}");
    }

    #[test]
    fn templates_render_json_or_text() {
        assert_eq!(
            render_template("{\"ok\": true}"),
            ("{\"ok\":true}".to_string(), "application/json")
        );
        assert_eq!(render_template("done"), ("done".to_string(), "text/plain"));
        assert_eq!(
            render_template(""),
            ("null".to_string(), "application/json")
        );
    }

    #[test]
    fn rate_limit_bucket_drains_and_refills() {
        let agent = format!("agent-{}", uuid::Uuid::new_v4());
        assert!(rate_limit_allow(&agent, 2.0, 1.0));
        assert!(rate_limit_allow(&agent, 2.0, 1.0));
        assert!(!rate_limit_allow(&agent, 2.0, 1.0));
        std::thread::sleep(Duration::from_millis(600));
        assert!(rate_limit_allow(&agent, 2.0, 1.0));
    }

    #[test]
    fn webhook_id_round_trips_like_upstream() {
        let id = encode_webhook_id(1234.5);
        assert_eq!(id, encode_webhook_id_of("1234.5"));
        assert!(!id.contains('='));
    }

    #[test]
    fn multipart_text_fields_are_parsed_and_files_rejected() {
        let body = b"--X\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nray\r\n--X--\r\n";
        let parsed = parse_multipart_form("X", body).unwrap();
        assert_eq!(parsed["name"], json!("ray"));
        let file = b"--X\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nhi\r\n--X--\r\n";
        let error = parse_multipart_form("X", file).unwrap_err();
        assert!(error.contains("file uploads are not supported"), "{error}");
    }

    #[test]
    fn begin_inputs_wrap_sections_as_value_descriptors() {
        let clean = json!({
            "query": {"a": "1"},
            "headers": {"host": "example"},
            "body": {"name": "ray"},
            "input": {}
        });
        let inputs = begin_inputs(&clean);
        assert_eq!(inputs["query"], json!({"value": {"a": "1"}}));
        assert_eq!(inputs["body"], json!({"value": {"name": "ray"}}));
    }
}
