//! Runtime configuration with hot-reload semantics — ported from RAGFlow
//! `api/db/runtime_config.py` and `api/db/reload_config_base.py`.
//!
//! RAGFlow holds a process-wide `RuntimeConfig` class whose attributes are
//! re-seeded at boot (`init_config(**kwargs)`) and re-read on every request,
//! so config changes take effect without a restart. `ReloadConfigBase`
//! provides the `get_all()` / `get(name)` accessors that ignore methods and
//! dunder attributes. `RuntimeConfig` adds the `ENV` map (version etc.), the
//! `SERVICE_DB` handle and the `LOAD_CONFIG_MANAGER` flag.
//!
//! This module keeps the same shape as a thread-safe global: a single
//! `RwLock<RuntimeConfig>` behind a `OnceLock`, with `init_config` mutating
//! only *known* config fields (RAGFlow's `hasattr` gate) — unknown keys are
//! ignored instead of being stored.

use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

/// The config attribute names `RuntimeConfig` recognises. RAGFlow's
/// `init_config` sets an attribute only `if hasattr(cls, k)`.
pub const KNOWN_CONFIG_FIELDS: &[&str] = &[
    "DEBUG",
    "WORK_MODE",
    "HTTP_PORT",
    "JOB_SERVER_HOST",
    "JOB_SERVER_VIP",
    "SERVICE_DB",
    "LOAD_CONFIG_MANAGER",
];

/// `reload_config_base.py::ReloadConfigBase` semantics: enumerate every
/// non-callable, non-dunder class attribute as a name → value map, and read a
/// single attribute by name (None when absent).
pub trait ReloadConfig {
    fn get_all(&self) -> BTreeMap<String, Value>;
    fn get(&self, name: &str) -> Option<Value>;
}

/// `runtime_config.py::RuntimeConfig` — the hot-reloadable runtime settings.
#[derive(Debug, Clone, Default)]
pub struct RuntimeConfig {
    pub debug: Option<bool>,
    pub work_mode: Option<String>,
    pub http_port: Option<u16>,
    pub job_server_host: Option<String>,
    pub job_server_vip: Option<String>,
    pub service_db: Option<String>,
    pub load_config_manager: bool,
    /// `ENV` — process environment facts exposed through `get_env` /
    /// `get_all_env` (RAGFlow seeds it with the product version).
    pub env: BTreeMap<String, String>,
}

impl ReloadConfig for RuntimeConfig {
    fn get_all(&self) -> BTreeMap<String, Value> {
        let mut configs = BTreeMap::new();
        configs.insert("DEBUG".into(), Value::Bool(self.debug.unwrap_or(false)));
        if let Some(work_mode) = &self.work_mode {
            configs.insert("WORK_MODE".into(), Value::String(work_mode.clone()));
        }
        if let Some(http_port) = self.http_port {
            configs.insert("HTTP_PORT".into(), Value::Number(http_port.into()));
        }
        if let Some(job_server_host) = &self.job_server_host {
            configs.insert("JOB_SERVER_HOST".into(), Value::String(job_server_host.clone()));
        }
        if let Some(job_server_vip) = &self.job_server_vip {
            configs.insert("JOB_SERVER_VIP".into(), Value::String(job_server_vip.clone()));
        }
        if let Some(service_db) = &self.service_db {
            configs.insert("SERVICE_DB".into(), Value::String(service_db.clone()));
        }
        configs.insert(
            "LOAD_CONFIG_MANAGER".into(),
            Value::Bool(self.load_config_manager),
        );
        configs
    }

    fn get(&self, name: &str) -> Option<Value> {
        let string_value = |value: &String| Value::String(value.clone());
        match name {
            "DEBUG" => Some(Value::Bool(self.debug.unwrap_or(false))),
            "WORK_MODE" => self.work_mode.as_ref().map(string_value),
            "HTTP_PORT" => self.http_port.map(|port| Value::Number(port.into())),
            "JOB_SERVER_HOST" => self.job_server_host.as_ref().map(string_value),
            "JOB_SERVER_VIP" => self.job_server_vip.as_ref().map(string_value),
            "SERVICE_DB" => self.service_db.as_ref().map(string_value),
            "LOAD_CONFIG_MANAGER" => Some(Value::Bool(self.load_config_manager)),
            _ => None,
        }
    }
}

static GLOBAL: OnceLock<RwLock<RuntimeConfig>> = OnceLock::new();

fn global() -> &'static RwLock<RuntimeConfig> {
    GLOBAL.get_or_init(|| RwLock::new(RuntimeConfig::default()))
}

/// `RuntimeConfig.init_config(**kwargs)` — seed the process-wide runtime
/// config. Only known fields are written (RAGFlow `hasattr` gate); unknown
/// keys are ignored. `ENV` is never replaced here.
pub fn init_config(entries: &[(&str, Value)]) {
    let mut config = global().write().expect("runtime config lock poisoned");
    for (key, value) in entries {
        match *key {
            "DEBUG" => config.debug = value.as_bool(),
            "WORK_MODE" => config.work_mode = value.as_str().map(str::to_owned),
            "HTTP_PORT" => config.http_port = value.as_u64().map(|port| port as u16),
            "JOB_SERVER_HOST" => config.job_server_host = value.as_str().map(str::to_owned),
            "JOB_SERVER_VIP" => config.job_server_vip = value.as_str().map(str::to_owned),
            "SERVICE_DB" => config.service_db = value.as_str().map(str::to_owned),
            "LOAD_CONFIG_MANAGER" => config.load_config_manager = value.as_bool().unwrap_or(false),
            // Unknown keys: RAGFlow's `hasattr` check skips them silently.
            _ => {}
        }
    }
}

/// `ReloadConfigBase.get_all()` on the global instance.
pub fn get_all() -> BTreeMap<String, Value> {
    global().read().expect("runtime config lock poisoned").get_all()
}

/// `ReloadConfigBase.get(config_name)` on the global instance.
pub fn get(name: &str) -> Option<Value> {
    global().read().expect("runtime config lock poisoned").get(name)
}

/// `RuntimeConfig.init_env()` — seed the environment map (version etc.).
pub fn init_env(version: &str) {
    global()
        .write()
        .expect("runtime config lock poisoned")
        .env
        .insert("version".into(), version.into());
}

/// `RuntimeConfig.get_env(key)` — read one environment entry.
pub fn get_env(key: &str) -> Option<String> {
    global()
        .read()
        .expect("runtime config lock poisoned")
        .env
        .get(key)
        .cloned()
}

/// `RuntimeConfig.get_all_env()` — the whole environment map.
pub fn get_all_env() -> BTreeMap<String, String> {
    global()
        .read()
        .expect("runtime config lock poisoned")
        .env
        .clone()
}

/// `RuntimeConfig.set_env(key, value)` — upsert one environment entry.
pub fn set_env(key: &str, value: String) {
    global()
        .write()
        .expect("runtime config lock poisoned")
        .env
        .insert(key.into(), value);
}

/// `RuntimeConfig.load_config_manager()` — flip the config-manager flag.
pub fn load_config_manager() {
    global()
        .write()
        .expect("runtime config lock poisoned")
        .load_config_manager = true;
}

/// `RuntimeConfig.set_service_db(service_db)` — record the active service DB.
pub fn set_service_db(service_db: String) {
    global()
        .write()
        .expect("runtime config lock poisoned")
        .service_db = Some(service_db);
}

/// Test-only: reset the global to a clean default.
pub fn reset_for_tests() {
    *global().write().expect("runtime config lock poisoned") = RuntimeConfig::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_config_only_sets_known_fields() {
        reset_for_tests();
        init_config(&[
            ("HTTP_PORT", Value::Number(9380.into())),
            ("WORK_MODE", Value::String("rag".into())),
            ("NOT_A_FIELD", Value::String("ignored".into())),
        ]);
        assert_eq!(get("HTTP_PORT"), Some(Value::Number(9380.into())));
        assert_eq!(get("WORK_MODE"), Some(Value::String("rag".into())));
        // Unknown keys are silently dropped (RAGFlow `hasattr` gate).
        assert_eq!(get("NOT_A_FIELD"), None);
        assert_eq!(get("DEBUG"), Some(Value::Bool(false)));
    }

    #[test]
    fn get_all_returns_only_reloadable_config_attributes() {
        reset_for_tests();
        init_config(&[
            ("DEBUG", Value::Bool(true)),
            ("JOB_SERVER_HOST", Value::String("job.internal".into())),
            ("LOAD_CONFIG_MANAGER", Value::Bool(true)),
        ]);
        let all = get_all();
        assert_eq!(all.get("DEBUG"), Some(&Value::Bool(true)));
        assert_eq!(
            all.get("JOB_SERVER_HOST"),
            Some(&Value::String("job.internal".into()))
        );
        assert_eq!(all.get("LOAD_CONFIG_MANAGER"), Some(&Value::Bool(true)));
        // ENV is not part of the reloadable attribute surface.
        assert!(!all.contains_key("ENV"));
        assert!(!all.contains_key("__dict__"));
    }

    #[test]
    fn env_map_supports_version_seed_and_upsert() {
        reset_for_tests();
        init_env("v0.14.0");
        assert_eq!(get_env("version"), Some("v0.14.0".into()));
        set_env("region", "cn".into());
        assert_eq!(get_env("region"), Some("cn".into()));
        let all = get_all_env();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn service_db_and_config_manager_flags() {
        reset_for_tests();
        assert_eq!(get("SERVICE_DB"), None);
        set_service_db("postgres://snapshot".into());
        assert_eq!(
            get("SERVICE_DB"),
            Some(Value::String("postgres://snapshot".into()))
        );
        assert_eq!(get("LOAD_CONFIG_MANAGER"), Some(Value::Bool(false)));
        load_config_manager();
        assert_eq!(get("LOAD_CONFIG_MANAGER"), Some(Value::Bool(true)));
    }
}
