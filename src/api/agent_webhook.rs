//! `agent_api.py`'s webhook surface: one webhook per agent, its delivery log, and a real test
//! delivery. Every answer states what happened — including "no webhook is registered yet",
//! which upstream also treats as a normal state rather than an error.

use std::sync::Arc;

use axum::Extension;
use axum::Json;
use axum::extract::{Path as AxumPath, State};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::server::AppState;
use crate::server::AuthContext;

fn store() -> Result<&'static crate::webhooks::WebhookStore, Response> {
    crate::webhooks::WebhookStore::shared().map_err(|error| {
        crate::server::api_error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            &format!("the webhook store is unavailable: {error}"),
        )
    })
}

/// `GET /api/v1/agents/{id}/webhook`.
pub async fn get_webhook(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    AxumPath(agent_id): AxumPath<String>,
) -> Response {
    if state
        .agents
        .get_accessible(
            &agent_id,
            &auth.user_id,
            auth.is_admin,
            |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
        )
        .is_none()
    {
        return crate::server::api_error(
            axum::http::StatusCode::BAD_REQUEST,
            &format!("You don't own the agent {agent_id}."),
        );
    }
    let store = match store() {
        Ok(store) => store,
        Err(response) => return response,
    };
    match store.get(&agent_id) {
        Some(webhook) => Json(json!({ "code": 0, "data": webhook })).into_response(),
        None => Json(json!({
            "code": 0,
            "data": Value::Null,
            "message": "No webhook is registered for this agent yet."
        }))
        .into_response(),
    }
}

/// `POST /api/v1/agents/{id}/webhook` — create or update it.
pub async fn save_webhook(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    AxumPath(agent_id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Response {
    if state
        .agents
        .get_accessible(
            &agent_id,
            &auth.user_id,
            auth.is_admin,
            |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
        )
        .is_none()
    {
        return crate::server::api_error(
            axum::http::StatusCode::BAD_REQUEST,
            &format!("You don't own the agent {agent_id}."),
        );
    }
    let Some(url) = body.get("url").and_then(Value::as_str).map(str::trim) else {
        return crate::server::api_error(
            axum::http::StatusCode::BAD_REQUEST,
            "A webhook needs a url.",
        );
    };
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return crate::server::api_error(
            axum::http::StatusCode::BAD_REQUEST,
            "The webhook url must start with http:// or https://.",
        );
    }
    let token = body
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let enabled = body.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    let store = match store() {
        Ok(store) => store,
        Err(response) => return response,
    };
    match store.upsert(&agent_id, url, &token, enabled) {
        Ok(webhook) => Json(json!({ "code": 0, "data": webhook })).into_response(),
        Err(error) => crate::server::api_error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            &format!("the webhook could not be saved: {error}"),
        ),
    }
}

/// `DELETE /api/v1/agents/{id}/webhook`.
pub async fn delete_webhook(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    AxumPath(agent_id): AxumPath<String>,
) -> Response {
    if state
        .agents
        .get_accessible(
            &agent_id,
            &auth.user_id,
            auth.is_admin,
            |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
        )
        .is_none()
    {
        return crate::server::api_error(
            axum::http::StatusCode::BAD_REQUEST,
            &format!("You don't own the agent {agent_id}."),
        );
    }
    let store = match store() {
        Ok(store) => store,
        Err(response) => return response,
    };
    match store.delete(&agent_id) {
        Ok(removed) => Json(json!({
            "code": 0,
            "data": { "deleted": removed },
            "message": if removed { "Webhook removed." } else { "No webhook was registered for this agent." }
        }))
        .into_response(),
        Err(error) => crate::server::api_error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            &format!("the webhook could not be removed: {error}"),
        ),
    }
}

/// `GET /api/v1/agents/{id}/webhook/logs` — recent deliveries, newest first.
pub async fn webhook_logs(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    AxumPath(agent_id): AxumPath<String>,
) -> Response {
    if state
        .agents
        .get_accessible(
            &agent_id,
            &auth.user_id,
            auth.is_admin,
            |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
        )
        .is_none()
    {
        return crate::server::api_error(
            axum::http::StatusCode::BAD_REQUEST,
            &format!("You don't own the agent {agent_id}."),
        );
    }
    let store = match store() {
        Ok(store) => store,
        Err(response) => return response,
    };
    let items = store.deliveries(&agent_id, 50);
    Json(json!({
        "code": 0,
        "data": { "total": items.len(), "items": items },
        "message": if items.is_empty() {
            "No deliveries recorded yet. Use webhook/test to send one."
        } else {
            "OK"
        }
    }))
    .into_response()
}

/// `POST /api/v1/agents/{id}/webhook/test` — deliver a test payload and record the outcome.
pub async fn test_webhook(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    AxumPath(agent_id): AxumPath<String>,
    body: Option<Json<Value>>,
) -> Response {
    if state
        .agents
        .get_accessible(
            &agent_id,
            &auth.user_id,
            auth.is_admin,
            |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
        )
        .is_none()
    {
        return crate::server::api_error(
            axum::http::StatusCode::BAD_REQUEST,
            &format!("You don't own the agent {agent_id}."),
        );
    }
    let store = match store() {
        Ok(store) => store,
        Err(response) => return response,
    };
    let Some(webhook) = store.get(&agent_id) else {
        return crate::server::api_error(
            axum::http::StatusCode::BAD_REQUEST,
            "Register a webhook for this agent first.",
        );
    };
    if !webhook.enabled {
        return crate::server::api_error(
            axum::http::StatusCode::BAD_REQUEST,
            "This webhook is disabled; enable it before sending a test delivery.",
        );
    }
    let payload = body.map(|Json(value)| value).unwrap_or_else(|| json!({}));
    let delivery = crate::webhooks::deliver(&webhook, "test", payload).await;
    let recorded = store.record(delivery.clone());
    Json(json!({
        "code": 0,
        "data": { "delivery": delivery, "recorded": recorded.is_ok() },
        "message": if delivery.ok {
            format!("Delivered: {}", delivery.detail)
        } else {
            format!("Delivery failed: {}", delivery.detail)
        }
    }))
    .into_response()
}
