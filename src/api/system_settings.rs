//! Persistent system settings and RAGFlow-compatible variable APIs.
//!
//! RAGFlow seeds `system_settings` from `conf/system_settings.json`, inserting
//! only missing names. RayRAG keeps the same rows and wire values in an atomic
//! JSON snapshot, while the typed runtime mirror is refreshed after every
//! committed update. This store is not part of the generic PostgreSQL snapshot
//! mirror.

use crate::api::joint_services::SystemSetting;
use crate::server::{AppState, AuthContext};
use anyhow::Context;
use axum::{
    Json,
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PublicSystemSetting {
    pub data_type: String,
    pub name: String,
    pub setting_type: &'static str,
    pub value: String,
}

impl From<&SystemSetting> for PublicSystemSetting {
    fn from(setting: &SystemSetting) -> Self {
        Self {
            data_type: setting.data_type.clone(),
            name: setting.name.clone(),
            setting_type: "config",
            value: public_value(setting),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SystemSettingsSnapshot {
    system_settings: Vec<SystemSetting>,
}

pub struct SystemSettingsStore {
    settings: RwLock<BTreeMap<String, SystemSetting>>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
}

impl SystemSettingsStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let path = PathBuf::from(path);
        crate::persistence::restore_if_missing(&path)?;
        let existing = if path.exists() {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("Failed to read system settings '{}':", path.display()))?;
            let snapshot: SystemSettingsSnapshot =
                serde_json::from_slice(&bytes).with_context(|| {
                    format!("Failed to parse system settings '{}':", path.display())
                })?;
            snapshot.system_settings
        } else {
            Vec::new()
        };
        validate_rows(&existing)?;

        let now = now_ms();
        let (seeded, seeded_count) = crate::api::joint_services::seed_system_settings(
            &existing,
            &crate::api::runtime_config::system_settings_seed_records(),
            now,
        );
        let mut settings: BTreeMap<_, _> = existing
            .into_iter()
            .map(|setting| (setting.name.clone(), setting))
            .collect();
        settings.extend(
            seeded
                .into_iter()
                .map(|setting| (setting.name.clone(), setting)),
        );
        let store = Self {
            settings: RwLock::new(settings),
            path: Some(path),
            save_lock: Mutex::new(()),
        };
        if seeded_count > 0 {
            store.persist(&store.settings.read().unwrap())?;
        }
        store.refresh_runtime_mirror();
        Ok(store)
    }

    pub fn in_memory() -> Self {
        let now = now_ms();
        let settings = crate::api::runtime_config::system_settings_seed_records()
            .into_iter()
            .map(|mut setting| {
                setting.create_time = now;
                setting.update_time = now;
                (setting.name.clone(), setting)
            })
            .collect();
        Self {
            settings: RwLock::new(settings),
            path: None,
            save_lock: Mutex::new(()),
        }
    }

    pub fn list(&self) -> Vec<PublicSystemSetting> {
        self.settings
            .read()
            .unwrap()
            .values()
            .map(PublicSystemSetting::from)
            .collect()
    }

    /// Exact name wins; otherwise return the ordered name-prefix matches.
    pub fn find(&self, name: &str) -> Vec<PublicSystemSetting> {
        let settings = self.settings.read().unwrap();
        if let Some(setting) = settings.get(name) {
            return vec![PublicSystemSetting::from(setting)];
        }
        settings
            .range(name.to_owned()..)
            .take_while(|(candidate, _)| candidate.starts_with(name))
            .map(|(_, setting)| PublicSystemSetting::from(setting))
            .collect()
    }

    pub fn raw_value(&self, name: &str) -> Option<String> {
        self.settings
            .read()
            .unwrap()
            .get(name)
            .map(|setting| setting.value.clone())
    }

    pub fn typed_value(&self, name: &str) -> Option<Value> {
        self.settings.read().unwrap().get(name).map(|setting| {
            crate::api::runtime_config::coerce_setting_value(&setting.data_type, &setting.value)
        })
    }

    pub fn set(&self, name: &str, value: &str) -> anyhow::Result<PublicSystemSetting> {
        if name.is_empty() {
            anyhow::bail!("Var name is required");
        }
        let _save = self.save_lock.lock().unwrap();
        let mut next = self.settings.read().unwrap().clone();
        let now = now_ms();
        let setting = if let Some(setting) = next.get_mut(name) {
            let value = restore_redacted_value(setting, value);
            validate_value(setting, &value)?;
            setting.value = value;
            setting.update_time = now;
            setting.clone()
        } else {
            let data_type = infer_data_type(name).to_owned();
            let setting = SystemSetting {
                name: name.to_owned(),
                source: "admin".into(),
                data_type,
                value: value.to_owned(),
                create_time: now,
                update_time: now,
            };
            validate_value(&setting, value)?;
            next.insert(name.to_owned(), setting.clone());
            setting
        };
        self.persist(&next)?;
        *self.settings.write().unwrap() = next;
        self.refresh_runtime_mirror();
        Ok(PublicSystemSetting::from(&setting))
    }

    fn persist(&self, settings: &BTreeMap<String, SystemSetting>) -> anyhow::Result<()> {
        let Some(path) = self.path.as_deref() else {
            return Ok(());
        };
        let snapshot = SystemSettingsSnapshot {
            system_settings: settings.values().cloned().collect(),
        };
        let mut bytes = serde_json::to_vec_pretty(&snapshot)?;
        bytes.push(b'\n');
        crate::persistence::atomic_write(path, &bytes)
    }

    fn refresh_runtime_mirror(&self) {
        #[cfg(not(test))]
        crate::api::runtime_config::replace_system_settings(self.settings.read().unwrap().values());
        #[cfg(test)]
        let _ = &self.settings;
    }
}

const REDACTED: &str = "<redacted>";

fn public_value(setting: &SystemSetting) -> String {
    if is_secret_key(&setting.name) {
        return if setting.value.is_empty() {
            String::new()
        } else {
            REDACTED.into()
        };
    }
    if setting.data_type.eq_ignore_ascii_case("json")
        && let Ok(mut value) = serde_json::from_str::<Value>(&setting.value)
    {
        redact_json(&mut value);
        return value.to_string();
    }
    setting.value.clone()
}

fn restore_redacted_value(setting: &SystemSetting, submitted: &str) -> String {
    if is_secret_key(&setting.name) && submitted == REDACTED {
        return setting.value.clone();
    }
    if !setting.data_type.eq_ignore_ascii_case("json") {
        return submitted.to_owned();
    }
    let (Ok(mut submitted), Ok(existing)) = (
        serde_json::from_str::<Value>(submitted),
        serde_json::from_str::<Value>(&setting.value),
    ) else {
        return submitted.to_owned();
    };
    restore_redacted_json(&mut submitted, &existing);
    submitted.to_string()
}

fn redact_json(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if is_secret_key(key) && !value.is_null() && value.as_str() != Some("") {
                    *value = Value::String(REDACTED.into());
                } else {
                    redact_json(value);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(redact_json),
        _ => {}
    }
}

fn restore_redacted_json(submitted: &mut Value, existing: &Value) {
    match (submitted, existing) {
        (Value::Object(submitted), Value::Object(existing)) => {
            for (key, value) in submitted {
                let Some(old) = existing.get(key) else {
                    continue;
                };
                if value.as_str() == Some(REDACTED) {
                    *value = old.clone();
                } else {
                    restore_redacted_json(value, old);
                }
            }
        }
        (Value::Array(submitted), Value::Array(existing)) => {
            for (value, old) in submitted.iter_mut().zip(existing) {
                restore_redacted_json(value, old);
            }
        }
        _ => {}
    }
}

fn is_secret_key(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "password",
        "private_key",
        "passphrase",
        "access_key_secret",
        "api_key",
        "secret",
        "token",
    ]
    .iter()
    .any(|part| name.contains(part))
}

fn validate_rows(rows: &[SystemSetting]) -> anyhow::Result<()> {
    let mut names = HashSet::new();
    for setting in rows {
        if setting.name.is_empty() {
            anyhow::bail!("System setting name must not be empty");
        }
        if !names.insert(setting.name.as_str()) {
            anyhow::bail!("Duplicate system setting: {}", setting.name);
        }
        if !matches!(
            setting.data_type.to_ascii_lowercase().as_str(),
            "string" | "integer" | "int" | "bool" | "boolean" | "json"
        ) {
            anyhow::bail!(
                "Unsupported data type for {}: {}",
                setting.name,
                setting.data_type
            );
        }
    }
    Ok(())
}

fn validate_value(setting: &SystemSetting, value: &str) -> anyhow::Result<()> {
    match setting.data_type.to_ascii_lowercase().as_str() {
        "string" => Ok(()),
        "integer" | "int" => value
            .parse::<i64>()
            .map(|_| ())
            .with_context(|| format!("Invalid integer value for {}: {value}", setting.name)),
        "bool" | "boolean" if matches!(value, "true" | "false") => Ok(()),
        "bool" | "boolean" => anyhow::bail!(
            "Invalid bool value for {}: expected true or false",
            setting.name
        ),
        "json" => serde_json::from_str::<Value>(value)
            .map(|_| ())
            .with_context(|| format!("Invalid JSON value for {}", setting.name)),
        other => anyhow::bail!("Unsupported data type for {}: {other}", setting.name),
    }
}

fn infer_data_type(name: &str) -> &'static str {
    if name.starts_with("sandbox.") {
        "json"
    } else if name.ends_with(".enabled") {
        "bool"
    } else {
        "string"
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Deserialize)]
pub struct SetVariableRequest {
    pub var_name: String,
    pub var_value: String,
}

#[derive(Debug, Deserialize)]
pub struct VariableLookupRequest {
    pub var_name: String,
}

fn require_admin(auth: &AuthContext) -> Option<Response> {
    (!auth.is_admin).then(|| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "code": 403,
                "message": "Administrator access required"
            })),
        )
            .into_response()
    })
}

pub async fn list_variables(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    request: Option<Json<VariableLookupRequest>>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let settings = if let Some(Json(request)) = request {
        if request.var_name.is_empty() {
            return variable_error("Var name is required");
        }
        let settings = state.system_settings.find(&request.var_name);
        if settings.is_empty() {
            return variable_error(&format!("Can't get setting: {}", request.var_name));
        }
        settings
    } else {
        state.system_settings.list()
    };
    Json(serde_json::json!({
        "code": 0,
        "data": settings,
        "message": "SUCCESS"
    }))
    .into_response()
}

pub async fn show_variable(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(encoded_name): Path<String>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&encoded_name)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&encoded_name));
    let Ok(decoded) = decoded else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"code": 400, "message": "Invalid base64 variable name"})),
        )
            .into_response();
    };
    let Ok(name) = String::from_utf8(decoded) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"code": 400, "message": "Invalid UTF-8 variable name"})),
        )
            .into_response();
    };
    let settings = state.system_settings.find(&name);
    if settings.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "code": 404,
                "message": format!("Can't get setting: {name}")
            })),
        )
            .into_response();
    }
    Json(serde_json::json!({"code": 0, "data": settings, "message": "SUCCESS"})).into_response()
}

pub async fn set_variable(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<SetVariableRequest>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    if request.var_name.is_empty() {
        return variable_error("Var name is required");
    }
    if request.var_value.is_empty() {
        return variable_error("Var value is required");
    }
    match state
        .system_settings
        .set(&request.var_name, &request.var_value)
    {
        Ok(_) => Json(serde_json::json!({"code": 0, "message": "SUCCESS"})).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"code": 400, "message": error.to_string()})),
        )
            .into_response(),
    }
}

fn variable_error(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"code": 400, "message": message})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_exact_conf_rows_and_persists_updates() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("system_settings.json");
        let store = SystemSettingsStore::new(path.to_str().unwrap()).unwrap();
        assert_eq!(store.list().len(), 14);
        assert_eq!(
            store.raw_value("sandbox.provider_type").as_deref(),
            Some("self_managed")
        );
        assert_eq!(store.typed_value("mail.timeout"), Some(Value::from(10)));
        store.set("mail.timeout", "45").unwrap();
        drop(store);

        let reopened = SystemSettingsStore::new(path.to_str().unwrap()).unwrap();
        assert_eq!(reopened.raw_value("mail.timeout").as_deref(), Some("45"));
        assert_eq!(reopened.list().len(), 14);
    }

    #[test]
    fn exact_lookup_wins_and_prefix_lookup_is_sorted() {
        let store = SystemSettingsStore::in_memory();
        let exact = store.find("mail.server");
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].name, "mail.server");
        let prefix = store.find("mail.");
        assert_eq!(prefix.len(), 8);
        assert!(prefix.windows(2).all(|pair| pair[0].name < pair[1].name));
    }

    #[test]
    fn validates_existing_types_and_infers_new_names() {
        let store = SystemSettingsStore::in_memory();
        assert!(store.set("enable_whitelist", "yes").is_err());
        assert!(store.set("mail.timeout", "1.5").is_err());
        assert!(store.set("sandbox.custom", "not-json").is_err());
        let setting = store.set("feature.enabled", "true").unwrap();
        assert_eq!(setting.data_type, "bool");
        assert_eq!(setting.value, "true");
    }

    #[test]
    fn public_rows_redact_secrets_and_sentinel_preserves_them() {
        let store = SystemSettingsStore::in_memory();
        store.set("mail.password", "smtp-secret").unwrap();
        assert_eq!(store.find("mail.password")[0].value, REDACTED);
        store.set("mail.password", REDACTED).unwrap();
        assert_eq!(
            store.raw_value("mail.password").as_deref(),
            Some("smtp-secret")
        );

        store
            .set(
                "sandbox.self_managed",
                r#"{"endpoint":"http://sandbox:9385","password":"hidden"}"#,
            )
            .unwrap();
        let public: Value =
            serde_json::from_str(&store.find("sandbox.self_managed")[0].value).unwrap();
        assert_eq!(public["password"], REDACTED);
        store
            .set("sandbox.self_managed", &public.to_string())
            .unwrap();
        assert_eq!(
            store.typed_value("sandbox.self_managed").unwrap()["password"],
            "hidden"
        );
    }

    #[test]
    fn failed_persistence_keeps_memory_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let store =
            SystemSettingsStore::new(root.path().join("settings.json").to_str().unwrap()).unwrap();
        std::fs::remove_file(root.path().join("settings.json")).unwrap();
        std::fs::create_dir(root.path().join("settings.json")).unwrap();
        assert!(store.set("mail.timeout", "99").is_err());
        assert_eq!(store.raw_value("mail.timeout").as_deref(), Some("10"));
    }

    #[test]
    fn snapshot_rejects_duplicate_names() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        let row = SystemSetting {
            name: "x".into(),
            source: "variable".into(),
            data_type: "string".into(),
            value: "a".into(),
            create_time: 0,
            update_time: 0,
        };
        let bytes = serde_json::to_vec(&SystemSettingsSnapshot {
            system_settings: vec![row.clone(), row],
        })
        .unwrap();
        std::fs::write(&path, bytes).unwrap();
        let error = SystemSettingsStore::new(path.to_str().unwrap())
            .err()
            .unwrap();
        assert!(error.to_string().contains("Duplicate system setting"));
    }

    #[test]
    fn snapshot_requires_the_fixed_wrapper() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        std::fs::write(&path, br#"{"variables":[]}"#).unwrap();
        let error = SystemSettingsStore::new(path.to_str().unwrap())
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("Failed to parse system settings")
        );
    }
}
