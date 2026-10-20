//! The password-reset endpoints, wired to [`crate::password_reset::OtpStore`] and the user store.
//!
//! Two things are deliberate: the request endpoint answers the same way whether or not the address
//! exists (so it cannot be used to enumerate accounts), and when no mail server is configured the
//! response says the code went to the server log instead of claiming an email was sent.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::password_reset::{OtpCheck, OtpStore};
use crate::server::{AppState, api_error, api_error_code};

fn normalise(body: &Value, key: &str) -> Option<String> {
    body.get(key)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// `GET`/`POST /api/v1/auth/password/forgot/captcha?email=...` — the image in front of the reset flow.
///
/// Upstream caches the answer under `captcha:{email}` for 60 seconds and refuses to send a reset code
/// until it comes back. RayRAG had the OTP half of that flow and not this half, so reset codes could be
/// requested for any address as fast as a caller liked. The answer is issued here, drawn into a real
/// PNG, and verified by [`request_otp`].
pub async fn forgot_captcha(Query(query): Query<CaptchaQuery>) -> Response {
    let Some(email) = query
        .email
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return api_error(StatusCode::BAD_REQUEST, "An email address is required.");
    };
    let code = crate::captcha::CaptchaStore::shared().issue(email);
    let png = match crate::captcha::render_png(&code) {
        Ok(png) => png,
        Err(error) => {
            // The answer must not be reported as issued when the image could not be drawn, or the
            // caller would be asked for something they were never shown.
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("the captcha image could not be rendered: {error}"),
            );
        }
    };
    (
        StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, "image/png"),
            (axum::http::header::CACHE_CONTROL, "no-store"),
            (
                axum::http::header::HeaderName::from_static("x-captcha-ttl"),
                "60",
            ),
        ],
        png,
    )
        .into_response()
}

#[derive(Debug, serde::Deserialize, Default)]
pub struct CaptchaQuery {
    #[serde(default)]
    pub email: Option<String>,
}

/// `POST /api/v1/auth/password/forgot/otp`.
pub async fn request_otp(Json(body): Json<Value>) -> Response {
    let Some(email) = normalise(&body, "email") else {
        return api_error(StatusCode::BAD_REQUEST, "An email address is required.");
    };
    // The captcha comes first, as upstream orders it: without it a caller can ask for reset codes for
    // arbitrary addresses in a loop. The message distinguishes "asked for nothing" from "answered
    // wrongly or too late", because those need different things from the user.
    let Some(answer) = normalise(&body, "captcha") else {
        return api_error_code(
            StatusCode::BAD_REQUEST,
            crate::server::code::INVALID_OR_MISSING_DATA,
            "A captcha answer is required; fetch the image from /api/v1/auth/password/forgot/captcha first.",
        );
    };
    if !crate::captcha::CaptchaStore::shared().verify(&email, &answer) {
        return api_error_code(
            StatusCode::BAD_REQUEST,
            crate::server::code::INVALID_OR_MISSING_DATA,
            "The captcha answer is wrong, or the image has expired. Fetch a new image and try again.",
        );
    }
    let store = OtpStore::shared();
    let code = match store.issue(&email) {
        Ok(code) => code,
        Err(error) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("the reset code could not be stored: {error}"),
            );
        }
    };
    // No mail transport in RayRAG today: the operator (who owns the logs) is the delivery channel,
    // and the caller is told so rather than being told to check an inbox.
    tracing::warn!(
        %email,
        code = %code,
        "password reset code issued; no mail server is configured so it is only in this log"
    );
    Json(json!({
        "code": 0,
        "data": {
            "email": email,
            "delivered_by": if crate::password_reset::mail_configured() { "email" } else { "server_log" },
            "expires_in_seconds": 600
        },
        "message": if crate::password_reset::mail_configured() {
            "A reset code has been sent.".to_string()
        } else {
            "No mail server is configured, so the reset code was written to the RayRAG server log. Ask your administrator for it.".to_string()
        }
    }))
    .into_response()
}

/// `POST /api/v1/auth/password/forgot/otp/verify`.
pub async fn verify_otp(Json(body): Json<Value>) -> Response {
    let (Some(email), Some(code)) = (normalise(&body, "email"), normalise(&body, "code")) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "An email address and a code are required.",
        );
    };
    match OtpStore::shared().verify(&email, &code) {
        Ok(OtpCheck::Ok) => Json(json!({
            "code": 0,
            "data": { "verified": true },
            "message": "Code accepted. You can now set a new password."
        }))
        .into_response(),
        Ok(OtpCheck::Missing) => api_error(
            StatusCode::BAD_REQUEST,
            "No reset code has been requested for this address, or it was already used.",
        ),
        Ok(OtpCheck::Expired) => api_error(
            StatusCode::BAD_REQUEST,
            "This reset code has expired. Request a new one.",
        ),
        Ok(OtpCheck::TooManyAttempts) => api_error(
            StatusCode::BAD_REQUEST,
            "Too many wrong attempts for this code. Request a new one.",
        ),
        Ok(OtpCheck::WrongCode { remaining }) => api_error(
            StatusCode::BAD_REQUEST,
            &format!("That code is not correct. {remaining} attempt(s) left."),
        ),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("the reset code could not be checked: {error}"),
        ),
    }
}

/// `POST /api/v1/auth/password/reset`.
pub async fn reset_password(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Response {
    let (Some(email), Some(code), Some(password)) = (
        normalise(&body, "email"),
        normalise(&body, "code"),
        normalise(&body, "password"),
    ) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "An email address, a code and a new password are required.",
        );
    };
    let otp = OtpStore::shared();
    // A reset requires a code that was verified; verifying again here keeps the two-step flow
    // usable by API clients that skip the verify call.
    match otp.verify(&email, &code) {
        Ok(OtpCheck::Ok) => {}
        Ok(OtpCheck::Missing) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "No reset code has been requested for this address, or it was already used.",
            );
        }
        Ok(OtpCheck::Expired) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "This reset code has expired. Request a new one.",
            );
        }
        Ok(OtpCheck::TooManyAttempts) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "Too many wrong attempts for this code. Request a new one.",
            );
        }
        Ok(OtpCheck::WrongCode { remaining }) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                &format!("That code is not correct. {remaining} attempt(s) left."),
            );
        }
        Err(error) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("the reset code could not be checked: {error}"),
            );
        }
    }
    let Some(user) = state.users.get_user(&email) else {
        return api_error(StatusCode::BAD_REQUEST, "No account uses that address.");
    };
    if let Err(error) = state.users.reset_password(&user.id, &password) {
        return api_error(
            StatusCode::BAD_REQUEST,
            &format!("the password was not changed: {error}"),
        );
    }
    match otp.consume_verified(&email) {
        Ok(true) => {}
        Ok(false) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "The reset code could not be consumed; request a new one.",
            );
        }
        Err(error) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("the reset code could not be consumed: {error}"),
            );
        }
    }
    Json(json!({
        "code": 0,
        "data": { "email": email },
        "message": "Password updated. Sign in with the new password."
    }))
    .into_response()
}
