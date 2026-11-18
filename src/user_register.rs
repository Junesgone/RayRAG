//! Upstream `api/apps/restful_apis/user_api.py::user_add` — the sign-up
//! endpoint — plus the validation helpers it calls.
//!
//! Two facts from the pinned source tree fix the contract:
//!
//! * `web/src/utils/api.ts` posts registrations to `` `${restAPIv1}/users` ``,
//!   and `api/apps/__init__.py::register_page` mounts
//!   `api/apps/restful_apis/user_api.py` at `/api/v1`, so the sign-up path is
//!   **`POST /api/v1/users`** — the same path whose `GET` lists users.
//! * every rejection keeps upstream's transport: `get_json_result` answers
//!   **HTTP 200** with a `RetCode` in the body. Registration being switched off
//!   is `RetCode.OPERATING_ERROR` (103) with the message
//!   `"User registration is disabled!"`, not an HTTP 403.
//!
//! The messages here are the upstream strings verbatim; the checks are the
//! upstream order: `@validate_request` → `REGISTER_ENABLED` → email shape →
//! duplicate email → `validate_nickname` → insert.

use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

/// `common/constants.py::RetCode.ARGUMENT_ERROR` — `validate_request` and
/// `nickname_validation.py` both report through it.
pub const ARGUMENT_ERROR: i32 = 101;
/// `common/constants.py::RetCode.EXCEPTION_ERROR` — a failed insert.
pub const EXCEPTION_ERROR: i32 = 100;
/// `common/constants.py::RetCode.OPERATING_ERROR` — disabled registration,
/// invalid email, duplicate email.
pub const OPERATING_ERROR: i32 = 103;

/// `user_api.user_add`: `message="User registration is disabled!"`.
pub const REGISTRATION_DISABLED_MESSAGE: &str = "User registration is disabled!";

/// `api/constants.py::NICKNAME_MAX_LENGTH`.
pub const NICKNAME_MAX_LENGTH: usize = 100;

/// `@validate_request("nickname", "email", "password")` only checks that the
/// keys are **present** in the parsed body, then joins the missing ones with a
/// bare comma and keeps the upstream trailing `"; "`.
///
/// `api/utils/api_utils.py::process_args`:
/// `"required argument are missing: {}; ".format(",".join(no_arguments))`.
pub fn missing_required_arguments(body: &Value, args: &[&str]) -> Option<String> {
    // A body that is not an object (`[]`, a bare string, no body at all) makes
    // upstream fall back to `{}` and report every argument as missing.
    let object = body.as_object();
    let missing: Vec<&str> = args
        .iter()
        .copied()
        .filter(|arg| !object.is_some_and(|map| map.contains_key(*arg)))
        .collect();
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "required argument are missing: {}; ",
        missing.join(",")
    ))
}

/// `re.compile(r"^[\w\._-]+@([\w_-]+\.)+[\w-]{2,}$")` from `user_api.user_add`.
///
/// Python's `\w` on `str` patterns is Unicode-aware and so is the `regex`
/// crate's by default, which keeps the two engines in step for international
/// addresses. Python's `$` also matches just before a trailing newline where
/// Rust's does not; an address with a trailing newline is rejected here and
/// accepted there, which is the safer of the two readings.
pub fn email_regex() -> &'static Regex {
    static EMAIL: OnceLock<Regex> = OnceLock::new();
    EMAIL.get_or_init(|| {
        Regex::new(r"^[\w\._-]+@([\w_-]+\.)+[\w-]{2,}$").expect("upstream email pattern compiles")
    })
}

/// `user_api.user_add`: `re.match(..., email_address)`.
pub fn email_is_valid(email: &str) -> bool {
    email_regex().is_match(email)
}

/// `f"Invalid email address: {email_address}!"`.
pub fn invalid_email_message(email: &str) -> String {
    format!("Invalid email address: {email}!")
}

/// `f"Email: {email_address} has already registered!"`.
pub fn duplicate_email_message(email: &str) -> String {
    format!("Email: {email} has already registered!")
}

/// `f"{nickname}, welcome aboard!"` — the success message of `user_add`.
pub fn welcome_message(nickname: &str) -> String {
    format!("{nickname}, welcome aboard!")
}

/// `f"User registration failure, error: {str(e)}"`.
pub fn registration_failure_message(error: &str) -> String {
    format!("User registration failure, error: {error}")
}

/// `api/utils/nickname_validation.py::validate_nickname`.
///
/// Returns the **stripped** nickname on success (`user_add` assigns
/// `nickname.strip()` right after the check) or the upstream message, which the
/// caller reports with [`ARGUMENT_ERROR`].
///
/// `NICKNAME_PATTERN = re.compile(r"^[\w ._'-]+$", re.UNICODE)` matches the
/// frontend constant in `pages/user-setting/profile/constants.ts`.
pub fn validate_nickname(value: Option<&Value>) -> Result<String, String> {
    let Some(value) = value else {
        // A present-but-null key reaches `validate_nickname(None)` upstream.
        return Err("Nickname is required.".to_string());
    };
    let Some(raw) = value.as_str() else {
        if value.is_null() {
            return Err("Nickname is required.".to_string());
        }
        return Err("Nickname must be a string.".to_string());
    };
    let nickname = raw.trim();
    if nickname.is_empty() {
        return Err("Nickname cannot be empty.".to_string());
    }
    // Python counts code points, not bytes.
    if nickname.chars().count() > NICKNAME_MAX_LENGTH {
        return Err(format!(
            "Nickname must be at most {NICKNAME_MAX_LENGTH} characters."
        ));
    }
    if !nickname_pattern().is_match(nickname) {
        return Err("Nickname contains invalid characters.".to_string());
    }
    Ok(nickname.to_string())
}

fn nickname_pattern() -> &'static Regex {
    static NICKNAME: OnceLock<Regex> = OnceLock::new();
    NICKNAME
        .get_or_init(|| Regex::new(r"^[\w ._'-]+$").expect("upstream nickname pattern compiles"))
}


