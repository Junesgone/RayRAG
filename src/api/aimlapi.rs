//! AIMLAPI agent-authorization HTTP endpoints — RAGFlow v0.27.2
//! `api/apps/restful_apis/aimlapi_api.py`.
//!
//! `POST /api/v1/llm/aimlapi/authorize/start` creates a device-authorization
//! request and returns the consent URL; `POST /api/v1/llm/aimlapi/authorize/poll`
//! polls the token endpoint and returns the issued API key once the user
//! approves. The device code never reaches the browser. Upstream keeps it in
//! Redis; RayRAG uses an in-process TTL store (single-instance deployments).

use crate::aimlapi::{AimlapiClient, DeviceCodeStore, PollOutcome};
use crate::server::AuthContext;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::{Value, json};
use std::sync::LazyLock;

/// Same role as upstream's Redis-backed `aimlapi_authz:*` keys.
static DEVICE_CODES: LazyLock<DeviceCodeStore> = LazyLock::new(DeviceCodeStore::new);

fn data_error(status: StatusCode, message: String) -> Response {
    (
        status,
        Json(json!({"code": 102, "message": message, "data": null})),
    )
        .into_response()
}

fn upstream_error(error: anyhow::Error) -> Response {
    let message = error.to_string();
    let status = if message.contains("is not configured") {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::BAD_GATEWAY
    };
    data_error(status, message)
}

/// `aimlapi_authorize_start`.
pub async fn aimlapi_authorize_start(Extension(auth): Extension<AuthContext>) -> Response {
    let client = match AimlapiClient::new() {
        Ok(client) => client,
        Err(error) => return data_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    match client.start(&DEVICE_CODES, &auth.user_id).await {
        Ok(start) => Json(json!({
            "code": 0,
            "data": {
                "request_id": start.request_id,
                "verification_uri": start.verification_uri,
                "interval": start.interval,
                "expires_in": start.expires_in,
            }
        }))
        .into_response(),
        Err(error) => upstream_error(error),
    }
}

/// `aimlapi_authorize_poll` (`validate_request("request_id")`).
pub async fn aimlapi_authorize_poll(
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<Value>,
) -> Response {
    let request_id = body
        .get("request_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if request_id.is_empty() {
        return data_error(StatusCode::BAD_REQUEST, "request_id is required".into());
    }
    let client = match AimlapiClient::new() {
        Ok(client) => client,
        Err(error) => return data_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    match client.poll(&DEVICE_CODES, &auth.user_id, request_id).await {
        Ok(PollOutcome::Ready { api_key }) => Json(json!({
            "code": 0,
            "data": {"status": "ready", "api_key": api_key}
        }))
        .into_response(),
        Ok(PollOutcome::Pending { status }) => {
            Json(json!({"code": 0, "data": {"status": status}})).into_response()
        }
        Ok(PollOutcome::Expired) => {
            Json(json!({"code": 0, "data": {"status": "expired"}})).into_response()
        }
        Err(error) => upstream_error(error),
    }
}
