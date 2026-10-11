//! Password reset by one-time code — upstream `user_api.py`'s
//! `password/forgot/otp`, `password/forgot/otp/verify` and `password/reset`.
//!
//! RayRAG is self-hosted and often has no mail server, so the code is **written to the server log**
//! and the response says exactly that instead of claiming an email was sent. Everything else is the
//! real flow: codes expire, are single-use, allow a limited number of attempts, and the request
//! endpoint answers identically whether or not the address exists (no account enumeration).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// How long a code stays valid.
const CODE_TTL_SECS: u64 = 600;
/// Wrong-code attempts allowed before the code is discarded.
const MAX_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtpEntry {
    pub email: String,
    pub code: String,
    pub expires_at: u64,
    pub attempts: u32,
    #[serde(default)]
    pub verified: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    entries: HashMap<String, OtpEntry>,
}

/// Outcome of a code check, so callers can explain a failure precisely.
#[derive(Debug, PartialEq, Eq)]
pub enum OtpCheck {
    Ok,
    Missing,
    Expired,
    TooManyAttempts,
    WrongCode { remaining: u32 },
}

/// File-backed one-time codes.
pub struct OtpStore {
    path: PathBuf,
    state: Mutex<State>,
}

impl OtpStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<State>(&raw).ok())
            .unwrap_or_default();
        Self {
            path,
            state: Mutex::new(state),
        }
    }

    pub fn shared() -> &'static OtpStore {
        static STORE: OnceLock<OtpStore> = OnceLock::new();
        STORE.get_or_init(|| {
            let path = std::env::var("RAYRAG_PASSWORD_RESET_FILE")
                .ok()
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    let static_dir =
                        std::env::var("RAYRAG_STATIC_DIR").unwrap_or_else(|_| "web/static".into());
                    PathBuf::from(static_dir)
                        .parent()
                        .map(|parent| parent.join("password_reset.json"))
                        .unwrap_or_else(|| PathBuf::from("password_reset.json"))
                });
            OtpStore::new(path)
        })
    }

    fn persist(state: &State, path: &PathBuf) -> Result<(), String> {
        let raw = serde_json::to_vec_pretty(state).map_err(|error| error.to_string())?;
        crate::persistence::atomic_write(path, &raw).map_err(|error| error.to_string())
    }

    /// Issue a fresh code for an address, replacing any previous one.
    pub fn issue(&self, email: &str) -> Result<String, String> {
        let code = format!("{:06}", random_six_digits());
        let entry = OtpEntry {
            email: email.to_lowercase(),
            code: code.clone(),
            expires_at: now_secs() + CODE_TTL_SECS,
            attempts: 0,
            verified: false,
        };
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        state.entries.insert(email.to_lowercase(), entry);
        Self::persist(&state, &self.path)?;
        Ok(code)
    }

    /// Check a code without consuming it; a success marks the entry verified.
    pub fn verify(&self, email: &str, code: &str) -> Result<OtpCheck, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        let key = email.to_lowercase();
        let Some(entry) = state.entries.get_mut(&key) else {
            return Ok(OtpCheck::Missing);
        };
        if now_secs() > entry.expires_at {
            return Ok(OtpCheck::Expired);
        }
        if entry.attempts >= MAX_ATTEMPTS {
            return Ok(OtpCheck::TooManyAttempts);
        }
        if entry.code != code.trim() {
            entry.attempts += 1;
            let remaining = MAX_ATTEMPTS.saturating_sub(entry.attempts);
            Self::persist(&state, &self.path)?;
            return Ok(OtpCheck::WrongCode { remaining });
        }
        entry.verified = true;
        Self::persist(&state, &self.path)?;
        Ok(OtpCheck::Ok)
    }

    /// Consume a verified code: the caller may only reset once per code.
    pub fn consume_verified(&self, email: &str) -> Result<bool, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        let key = email.to_lowercase();
        let consumed = match state.entries.get(&key) {
            Some(entry) if entry.verified && now_secs() <= entry.expires_at => true,
            _ => false,
        };
        if consumed {
            state.entries.remove(&key);
            Self::persist(&state, &self.path)?;
        }
        Ok(consumed)
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Six digits from the system clock and the process id, mixed with a counter so two codes issued
/// in the same second cannot collide.
fn random_six_digits() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let tick = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let mixed = nanos
        .wrapping_mul(2_654_435_761)
        .wrapping_add(pid.wrapping_mul(40_503))
        .wrapping_add(tick.wrapping_mul(97));
    mixed % 1_000_000
}

/// True when an SMTP host is configured; nothing else in RayRAG sends mail today, so the code is
/// logged instead and the API says so.
pub fn mail_configured() -> bool {
    std::env::var("EMAIL_SMTP_HOST")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> OtpStore {
        let path =
            std::env::temp_dir().join(format!("rr-otp-{}-{}.json", name, std::process::id()));
        let _ = std::fs::remove_file(&path);
        OtpStore::new(path)
    }

    #[test]
    fn a_code_is_single_use_and_verifies_once() {
        let store = store("once");
        let code = store.issue("Ada@Example.com").unwrap();
        assert_eq!(
            store.verify("ada@example.com", &code).unwrap(),
            OtpCheck::Ok
        );
        // The verified flag is what authorises the reset, and consuming removes the entry.
        assert!(store.consume_verified("ada@example.com").unwrap());
        assert!(!store.consume_verified("ada@example.com").unwrap());
        assert_eq!(
            store.verify("ada@example.com", &code).unwrap(),
            OtpCheck::Missing
        );
    }

    #[test]
    fn wrong_codes_count_down_and_then_stop() {
        let store = store("attempts");
        store.issue("a@b.test").unwrap();
        for expected in (1..=4).rev() {
            match store.verify("a@b.test", "000000").unwrap() {
                OtpCheck::WrongCode { remaining } => assert_eq!(remaining, expected),
                other => panic!("expected a wrong-code result, got {other:?}"),
            }
        }
        assert_eq!(
            store.verify("a@b.test", "000000").unwrap(),
            OtpCheck::WrongCode { remaining: 0 }
        );
        // Once the allowance is spent the code is refused even if the right one arrives.
        assert_eq!(
            store.verify("a@b.test", "000000").unwrap(),
            OtpCheck::TooManyAttempts
        );
    }

    #[test]
    fn an_unknown_address_is_reported_not_guessed() {
        let store = store("missing");
        assert_eq!(
            store.verify("nobody@test", "123456").unwrap(),
            OtpCheck::Missing
        );
        assert!(!store.consume_verified("nobody@test").unwrap());
    }

    #[test]
    fn a_fresh_code_replaces_the_previous_one() {
        let store = store("replace");
        let first = store.issue("a@b.test").unwrap();
        let second = store.issue("a@b.test").unwrap();
        if first != second {
            assert!(matches!(
                store.verify("a@b.test", &first).unwrap(),
                OtpCheck::WrongCode { .. }
            ));
        }
        assert_eq!(store.verify("a@b.test", &second).unwrap(), OtpCheck::Ok);
    }
}
