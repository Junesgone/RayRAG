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
    /// Runtime mirror of the `system_settings` table (seeded from
    /// `conf/system_settings.json`). Kept separate from the reloadable
    /// attribute surface above: in RAGFlow system settings live in a DB
    /// table, not on `RuntimeConfig` attributes.
    pub system_settings: BTreeMap<String, Value>,
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
            configs.insert(
                "JOB_SERVER_HOST".into(),
                Value::String(job_server_host.clone()),
            );
        }
        if let Some(job_server_vip) = &self.job_server_vip {
            configs.insert(
                "JOB_SERVER_VIP".into(),
                Value::String(job_server_vip.clone()),
            );
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
    global()
        .read()
        .expect("runtime config lock poisoned")
        .get_all()
}

/// `ReloadConfigBase.get(config_name)` on the global instance.
pub fn get(name: &str) -> Option<Value> {
    global()
        .read()
        .expect("runtime config lock poisoned")
        .get(name)
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

// ═══════════════════════════════════════════════════════════════════════════
// conf/system_settings.json coverage — system-settings seed.
//
// RAGFlow's `api/db/init_data.py::init_table()` seeds the `system_settings`
// table from `conf/system_settings.json`, inserting only rows whose `name`
// is missing. The DB-side upsert semantics are ported in
// `api::joint_services::seed_system_settings` (records + insert-only-missing
// decision). This module adds the *file shape*: the 14 seed rows verbatim
// from the conf file, a lenient parser for the `{"system_settings": [...]}`
// payload, typed value coercion (bool / integer / json / string), and a
// runtime mirror so in-process code can consult settings without a DB hit.
// ═══════════════════════════════════════════════════════════════════════════

/// The 14 seed rows of RAGFlow `conf/system_settings.json`, verbatim
/// `(name, source, data_type, value)`.
pub const SYSTEM_SETTINGS_UPSTREAM_BLOB: &str = "f546aa1436b0d262679a0567b7a61167bfcd43bb";
pub const SYSTEM_SETTINGS_UPSTREAM_SHA256: &str =
    "2d58065525cbba3e1e796b73be822547dd991dcab90cb71e342750b4867f3402";
pub const SYSTEM_SETTINGS_UPSTREAM_BYTES: usize = 1_843;
pub const SYSTEM_SETTINGS_UPSTREAM_LINES: usize = 88;

pub const SYSTEM_SETTINGS_SEED: &[(&str, &str, &str, &str)] = &[
    ("enable_whitelist", "variable", "bool", "true"),
    ("default_role", "variable", "string", ""),
    ("mail.server", "variable", "string", ""),
    ("mail.port", "variable", "integer", ""),
    ("mail.use_ssl", "variable", "bool", "false"),
    ("mail.use_tls", "variable", "bool", "false"),
    ("mail.username", "variable", "string", ""),
    ("mail.password", "variable", "string", ""),
    ("mail.timeout", "variable", "integer", "10"),
    ("mail.default_sender", "variable", "string", ""),
    (
        "sandbox.provider_type",
        "variable",
        "string",
        "self_managed",
    ),
    (
        "sandbox.self_managed",
        "variable",
        "json",
        r#"{"endpoint": "http://localhost:9385", "timeout": 30, "max_retries": 3, "pool_size": 10}"#,
    ),
    ("sandbox.aliyun_codeinterpreter", "variable", "json", "{}"),
    ("sandbox.e2b", "variable", "json", "{}"),
];

/// Build `SystemSetting` rows from the embedded seed (create/update times 0 —
/// the caller stamps them, matching `seed_system_settings`).
pub fn system_settings_seed_records() -> Vec<crate::api::joint_services::SystemSetting> {
    SYSTEM_SETTINGS_SEED
        .iter()
        .map(
            |(name, source, data_type, value)| crate::api::joint_services::SystemSetting {
                name: (*name).to_owned(),
                source: (*source).to_owned(),
                data_type: (*data_type).to_owned(),
                value: (*value).to_owned(),
                create_time: 0,
                update_time: 0,
            },
        )
        .collect()
}

/// Parse `conf/system_settings.json` (`{"system_settings": [...]}`) into
/// rows. Unknown keys and non-object entries are skipped leniently.
pub fn parse_system_settings_file(value: &Value) -> Vec<crate::api::joint_services::SystemSetting> {
    let mut records = Vec::new();
    let Some(list) = value.get("system_settings").and_then(Value::as_array) else {
        return records;
    };
    for entry in list {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let Some(name) = entry.get("name").and_then(Value::as_str) else {
            continue;
        };
        records.push(crate::api::joint_services::SystemSetting {
            name: name.to_owned(),
            source: entry
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("variable")
                .to_owned(),
            data_type: entry
                .get("data_type")
                .and_then(Value::as_str)
                .unwrap_or("string")
                .to_owned(),
            value: entry
                .get("value")
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default(),
            create_time: 0,
            update_time: 0,
        });
    }
    records
}

/// Coerce a raw seed value by its declared `data_type`
/// (`bool` / `integer` / `json` / anything else → string).
pub fn coerce_setting_value(data_type: &str, raw: &str) -> Value {
    match data_type {
        "bool" => Value::Bool(matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true"
        )),
        "integer" => raw
            .trim()
            .parse::<i64>()
            .map(|n| Value::Number(n.into()))
            .unwrap_or_else(|_| Value::String(raw.to_owned())),
        "json" => serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned())),
        _ => Value::String(raw.to_owned()),
    }
}

/// Pure planner for `init_table()` system-settings seeding: which typed
/// entries to add to an existing `name → value` map, inserting only names
/// that are missing. Deterministic and testable without the global.
pub fn plan_system_settings_seed(
    existing: &BTreeMap<String, Value>,
    records: &[crate::api::joint_services::SystemSetting],
) -> (BTreeMap<String, Value>, usize) {
    let mut additions = BTreeMap::new();
    let mut count = 0;
    for record in records {
        if existing.contains_key(&record.name) || additions.contains_key(&record.name) {
            continue;
        }
        additions.insert(
            record.name.clone(),
            coerce_setting_value(&record.data_type, &record.value),
        );
        count += 1;
    }
    (additions, count)
}

/// `init_table()` system-settings seeding on the runtime mirror: insert only
/// names that are missing, typed by `data_type`. Returns rows seeded.
pub fn seed_system_settings(records: &[crate::api::joint_services::SystemSetting]) -> usize {
    let mut config = global().write().expect("runtime config lock poisoned");
    let (additions, count) = plan_system_settings_seed(&config.system_settings, records);
    config.system_settings.extend(additions);
    count
}

/// All runtime system settings (`name` → typed value).
pub fn system_settings_all() -> BTreeMap<String, Value> {
    global()
        .read()
        .expect("runtime config lock poisoned")
        .system_settings
        .clone()
}

/// One runtime system setting by name.
pub fn system_settings_get(name: &str) -> Option<Value> {
    global()
        .read()
        .expect("runtime config lock poisoned")
        .system_settings
        .get(name)
        .cloned()
}

/// Replace the runtime mirror after a durable system-settings snapshot is
/// loaded or committed. Unlike boot seeding this removes stale names too.
pub fn replace_system_settings<'a>(
    records: impl IntoIterator<Item = &'a crate::api::joint_services::SystemSetting>,
) {
    let values = records
        .into_iter()
        .map(|record| {
            (
                record.name.clone(),
                coerce_setting_value(&record.data_type, &record.value),
            )
        })
        .collect();
    global()
        .write()
        .expect("runtime config lock poisoned")
        .system_settings = values;
}

/// Test-only: reset the global to a clean default.
pub fn reset_for_tests() {
    *global().write().expect("runtime config lock poisoned") = RuntimeConfig::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn global_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap()
    }

    #[test]
    fn init_config_only_sets_known_fields() {
        let _guard = global_test_guard();
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
        let _guard = global_test_guard();
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
        let _guard = global_test_guard();
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
        let _guard = global_test_guard();
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

    #[test]
    fn system_settings_seed_matches_conf_file_shape() {
        // The embedded seed mirrors the real conf/system_settings.json: 14
        // rows with the exact names / data types of the shipped file.
        assert_eq!(
            SYSTEM_SETTINGS_SEED,
            &[
                ("enable_whitelist", "variable", "bool", "true"),
                ("default_role", "variable", "string", ""),
                ("mail.server", "variable", "string", ""),
                ("mail.port", "variable", "integer", ""),
                ("mail.use_ssl", "variable", "bool", "false"),
                ("mail.use_tls", "variable", "bool", "false"),
                ("mail.username", "variable", "string", ""),
                ("mail.password", "variable", "string", ""),
                ("mail.timeout", "variable", "integer", "10"),
                ("mail.default_sender", "variable", "string", ""),
                (
                    "sandbox.provider_type",
                    "variable",
                    "string",
                    "self_managed"
                ),
                (
                    "sandbox.self_managed",
                    "variable",
                    "json",
                    r#"{"endpoint": "http://localhost:9385", "timeout": 30, "max_retries": 3, "pool_size": 10}"#
                ),
                ("sandbox.aliyun_codeinterpreter", "variable", "json", "{}"),
                ("sandbox.e2b", "variable", "json", "{}"),
            ]
        );
        assert_eq!(SYSTEM_SETTINGS_UPSTREAM_BYTES, 1_843);
        assert_eq!(SYSTEM_SETTINGS_UPSTREAM_LINES, 88);
        assert_eq!(SYSTEM_SETTINGS_UPSTREAM_BLOB.len(), 40);
        assert_eq!(SYSTEM_SETTINGS_UPSTREAM_SHA256.len(), 64);
        let records = system_settings_seed_records();
        assert_eq!(records.len(), 14);
        assert_eq!(records[0].name, "enable_whitelist");
        assert_eq!(records[0].data_type, "bool");
        assert_eq!(records[0].value, "true");
        assert_eq!(records[9].name, "mail.default_sender");
        assert_eq!(records[10].name, "sandbox.provider_type");
        assert_eq!(records[10].value, "self_managed");
        assert_eq!(records[11].name, "sandbox.self_managed");
        assert_eq!(records[11].data_type, "json");
        assert!(records[11].value.contains("localhost:9385"));
        // All rows carry the "variable" source, like the conf file.
        assert!(records.iter().all(|r| r.source == "variable"));
        // Parser round-trips the conf JSON shape.
        let parsed = parse_system_settings_file(&serde_json::json!({
            "system_settings": [
                {"name": "enable_whitelist", "source": "variable", "data_type": "bool", "value": "true"},
                {"name": "mail.timeout", "source": "variable", "data_type": "integer", "value": "10"}
            ]
        }));
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].name, "mail.timeout");
        // Lenient: missing wrapper or malformed entries yield empty / skip.
        assert!(parse_system_settings_file(&Value::Null).is_empty());
        assert!(
            parse_system_settings_file(&serde_json::json!({"system_settings": [42]})).is_empty()
        );
    }

    #[test]
    fn system_settings_seed_plans_inserts_only_missing_names_with_types() {
        // Pure planner: no global state involved, deterministic.
        let records = system_settings_seed_records();
        let (additions, count) = plan_system_settings_seed(&BTreeMap::new(), &records);
        assert_eq!(count, 14);
        assert_eq!(additions.len(), 14);
        // Typed coercion by data_type.
        assert_eq!(additions.get("enable_whitelist"), Some(&Value::Bool(true)));
        assert_eq!(
            additions.get("mail.timeout"),
            Some(&Value::Number(10.into()))
        );
        assert_eq!(
            additions.get("sandbox.provider_type"),
            Some(&Value::String("self_managed".into()))
        );
        // json-typed rows are parsed objects.
        assert_eq!(
            additions.get("sandbox.self_managed"),
            Some(&serde_json::json!({
                "endpoint": "http://localhost:9385",
                "timeout": 30,
                "max_retries": 3,
                "pool_size": 10
            }))
        );
        assert_eq!(additions.get("sandbox.e2b"), Some(&serde_json::json!({})));
        // mail.port has an empty value → stays a string (unparseable integer).
        assert_eq!(additions.get("mail.port"), Some(&Value::String("".into())));
        // Existing names are not re-planned (init_table semantics).
        let (additions2, count2) = plan_system_settings_seed(&additions, &records);
        assert_eq!(count2, 0);
        assert!(additions2.is_empty());
    }
}
