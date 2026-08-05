//! RAGFlow-compatible settings layer.
//!
//! Mirrors the environment-driven configuration semantics of:
//! - `rag/settings.py` (deployment settings: storage impl, doc engine, sizes)
//! - `common/settings.py` (runtime settings: auth switches, LLM defaults,
//!   sandbox, crypto, secret key, SMTP)
//! - `agent/settings.py` (canvas constants: `FLOAT_ZERO`, `PARAM_MAXDEPTH`)
//!
//! Everything is loaded from environment variables with the same defaults and
//! type-conversion rules as the Python side. Reading is pure: `Settings::from_env`
//! snapshots the process environment once; tests inject values through the
//! internal `from_reader` without touching `std::env`.

use serde_json::Value;
use std::collections::BTreeMap;

/// Canvas float comparison epsilon (`agent/settings.py::FLOAT_ZERO`).
pub const FLOAT_ZERO: f64 = 1e-8;
/// Maximum nesting depth for canvas parameter expansion
/// (`agent/settings.py::PARAM_MAXDEPTH`).
pub const PARAM_MAXDEPTH: usize = 5;

/// Server task queue base name (`rag/settings.py::SVR_QUEUE_NAME`).
pub const SVR_QUEUE_NAME: &str = "rag_flow_svr_queue";
/// Server task consumer group (`rag/settings.py::SVR_CONSUMER_GROUP_NAME`).
pub const SVR_CONSUMER_GROUP_NAME: &str = "rag_flow_svr_task_broker";
/// Pagerank field name (`rag/settings.py::PAGERANK_FLD`).
pub const PAGERANK_FLD: &str = "pagerank_fea";
/// Tag feature field name (`rag/settings.py::TAG_FLD`).
pub const TAG_FLD: &str = "tag_feas";

/// Default maximum document upload size (`MAX_CONTENT_LENGTH`, 128 MiB).
pub const DEFAULT_DOC_MAXIMUM_SIZE: u64 = 128 * 1024 * 1024;
/// Default documents per bulk write (`DOC_BULK_SIZE`).
pub const DEFAULT_DOC_BULK_SIZE: u32 = 4;
/// Default embeddings per batch (`EMBEDDING_BATCH_SIZE`).
pub const DEFAULT_EMBEDDING_BATCH_SIZE: u32 = 16;
/// Default sandbox strong-test count (`STRONG_TEST_COUNT`).
pub const DEFAULT_STRONG_TEST_COUNT: u32 = 8;
/// Default timezone (`TZ`).
pub const DEFAULT_TZ: &str = "Asia/Shanghai";
/// Default storage implementation (`STORAGE_IMPL`).
pub const DEFAULT_STORAGE_IMPL: &str = "MINIO";
/// Default document engine (`DOC_ENGINE`).
pub const DEFAULT_DOC_ENGINE: &str = "elasticsearch";
/// Default sandbox executor manager host (`SANDBOX_HOST`).
pub const DEFAULT_SANDBOX_HOST: &str = "sandbox-executor-manager";
/// Default crypto algorithm (`RAGFLOW_CRYPTO_ALGORITHM`).
pub const DEFAULT_CRYPTO_ALGORITHM: &str = "aes-256-cbc";
/// Default parsers catalog (`common/settings.py::PARSERS`).
pub const DEFAULT_PARSERS: &str = "naive:General,qa:Q&A,resume:Resume,manual:Manual,table:Table,paper:Paper,book:Book,laws:Laws,presentation:Presentation,picture:Picture,one:One,audio:Audio,email:Email,tag:Tag";

/// Supported storage implementations, mirroring `Storage` enum usage in
/// `common/settings.py::StorageFactory.storage_mapping`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageImpl {
    Minio,
    AzureSpn,
    AzureSas,
    AwsS3,
    Oss,
    Opendal,
    Gcs,
}

impl StorageImpl {
    /// Parse a `STORAGE_IMPL` value (case-insensitive). Unknown values fall
    /// back to `Minio` exactly like the Python side's dict lookup raising is
    /// avoided at import time (storage is only constructed in `init_settings`).
    pub fn from_env_value(value: &str) -> StorageImpl {
        match value.to_ascii_uppercase().as_str() {
            "AZURE_SPN" => StorageImpl::AzureSpn,
            "AZURE_SAS" => StorageImpl::AzureSas,
            "AWS_S3" => StorageImpl::AwsS3,
            "OSS" => StorageImpl::Oss,
            "OPENDAL" => StorageImpl::Opendal,
            "GCS" => StorageImpl::Gcs,
            _ => StorageImpl::Minio,
        }
    }
}

/// Supported document engines, mirroring `DOC_ENGINE` dispatch in
/// `common/settings.py::init_settings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocEngine {
    Elasticsearch,
    Infinity,
    Opensearch,
    Oceanbase,
    Seekdb,
}

impl DocEngine {
    pub fn from_env_value(value: &str) -> DocEngine {
        match value.to_ascii_lowercase().as_str() {
            "infinity" => DocEngine::Infinity,
            "opensearch" => DocEngine::Opensearch,
            "oceanbase" => DocEngine::Oceanbase,
            "seekdb" => DocEngine::Seekdb,
            _ => DocEngine::Elasticsearch,
        }
    }

    /// `DOC_ENGINE_INFINITY` flag (`common/settings.py`).
    pub fn is_infinity(self) -> bool {
        matches!(self, DocEngine::Infinity)
    }

    /// `DOC_ENGINE_OCEANBASE` flag (`common/settings.py`).
    pub fn is_oceanbase(self) -> bool {
        matches!(self, DocEngine::Oceanbase)
    }

    /// Oceanbase-family engines also route to `OBConnection`
    /// (`common/settings.py` treats `seekdb` like `oceanbase`).
    pub fn is_oceanbase_family(self) -> bool {
        matches!(self, DocEngine::Oceanbase | DocEngine::Seekdb)
    }
}

/// A resolved default-model entry, mirroring `common/settings.py`'s
/// `_parse_model_entry` / `_resolve_per_model_config` shape.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelEntry {
    pub name: String,
    pub factory: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
}

/// `common/settings.py::_parse_model_entry` — a plain string entry is a bare
/// model name; a dict entry may carry `name`/`model`, `factory`, `api_key`,
/// `base_url`; anything else yields an empty entry.
pub fn parse_model_entry(entry: &Value) -> ModelEntry {
    match entry {
        Value::String(name) => ModelEntry {
            name: name.clone(),
            ..Default::default()
        },
        Value::Object(map) => {
            let name = map
                .get("name")
                .or_else(|| map.get("model"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            ModelEntry {
                name,
                factory: map
                    .get("factory")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                api_key: map
                    .get("api_key")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                base_url: map
                    .get("base_url")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }
        }
        _ => ModelEntry::default(),
    }
}

/// `common/settings.py::_resolve_per_model_config` — fall back to the
/// user-default factory/api_key/base_url per field, and compose the final
/// model name as `name@factory` when a factory is present and the name does
/// not already carry one.
pub fn resolve_per_model_config(
    entry: &ModelEntry,
    backup_factory: &str,
    backup_api_key: &str,
    backup_base_url: &str,
) -> ModelEntry {
    let name = entry.name.trim().to_owned();
    let factory = entry
        .factory
        .clone()
        .filter(|v| !v.is_empty())
        .or_else(|| (!backup_factory.is_empty()).then(|| backup_factory.to_owned()));
    let api_key = entry
        .api_key
        .clone()
        .filter(|v| !v.is_empty())
        .or_else(|| (!backup_api_key.is_empty()).then(|| backup_api_key.to_owned()));
    let base_url = entry
        .base_url
        .clone()
        .filter(|v| !v.is_empty())
        .or_else(|| (!backup_base_url.is_empty()).then(|| backup_base_url.to_owned()));

    let mut name = name;
    if !name.is_empty() && !name.contains('@') {
        if let Some(factory) = &factory {
            name = format!("{name}@{factory}");
        }
    }
    ModelEntry {
        name,
        factory,
        api_key,
        base_url,
    }
}

/// `common/settings.py::init_secret_key` — an explicit `RAGFLOW_SECRET_KEY`
/// wins when it is at least 32 chars; otherwise a configured key is accepted
/// when it is at least 32 chars and differs from today's date (a stale
/// placeholder); otherwise `None` (the runtime then generates one).
pub fn init_secret_key(env_key: Option<&str>, configured_key: Option<&str>) -> Option<String> {
    if let Some(key) = env_key {
        if key.len() >= 32 {
            return Some(key.to_owned());
        }
    }
    let today = {
        let now = time::OffsetDateTime::now_utc();
        format!(
            "{:04}-{:02}-{:02}",
            now.year(),
            now.month() as u8,
            now.day()
        )
    };
    if let Some(key) = configured_key {
        if key.len() >= 32 && key != today {
            return Some(key.to_owned());
        }
    }
    None
}

/// `common/settings.py` `DISABLE_PASSWORD_LOGIN` env parsing: "1"/"true"/"yes"
/// (case-insensitive) enable the switch; anything else leaves it to config.
pub fn parse_disable_password_login_env(value: &str) -> bool {
    matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes")
}

/// All settings snapshotted from the environment, mirroring the Python module
/// globals. Field names follow the Python names so porting stays mechanical.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// `TZ`
    pub timezone: String,
    /// `DB_TYPE`
    pub database_type: String,
    /// `STORAGE_IMPL`
    pub storage_impl: StorageImpl,
    /// `DOC_ENGINE`
    pub doc_engine: DocEngine,
    /// `MAX_CONTENT_LENGTH` → `DOC_MAXIMUM_SIZE`
    pub doc_maximum_size: u64,
    /// `DOC_BULK_SIZE`
    pub doc_bulk_size: u32,
    /// `EMBEDDING_BATCH_SIZE`
    pub embedding_batch_size: u32,
    /// `REGISTER_ENABLED`
    pub register_enabled: i64,
    /// `DISABLE_PASSWORD_LOGIN` (env part; config may still override)
    pub disable_password_login: bool,
    /// `STRONG_TEST_COUNT`
    pub strong_test_count: u32,
    /// `MAX_FILE_NUM_PER_USER`
    pub max_file_num_per_user: u32,
    /// `RAGFLOW_SECRET_KEY` (when ≥ 32 chars)
    pub secret_key: Option<String>,
    /// `RAGFLOW_CRYPTO_ENABLED == "true"`
    pub crypto_enabled: bool,
    /// `RAGFLOW_CRYPTO_ALGORITHM`
    pub crypto_algorithm: String,
    /// `SANDBOX_ENABLED != 0`
    pub sandbox_enabled: bool,
    /// `SANDBOX_HOST`
    pub sandbox_host: String,
    /// `user_default_llm.factory`
    pub llm_factory: String,
    /// `user_default_llm.base_url`
    pub llm_base_url: String,
    /// `user_default_llm.default_models` chat/embedding/rerank/asr/image2text
    pub default_models: BTreeMap<String, ModelEntry>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timezone: DEFAULT_TZ.to_owned(),
            database_type: "mysql".to_owned(),
            storage_impl: StorageImpl::Minio,
            doc_engine: DocEngine::Elasticsearch,
            doc_maximum_size: DEFAULT_DOC_MAXIMUM_SIZE,
            doc_bulk_size: DEFAULT_DOC_BULK_SIZE,
            embedding_batch_size: DEFAULT_EMBEDDING_BATCH_SIZE,
            register_enabled: 1,
            disable_password_login: false,
            strong_test_count: DEFAULT_STRONG_TEST_COUNT,
            max_file_num_per_user: 0,
            secret_key: None,
            crypto_enabled: false,
            crypto_algorithm: DEFAULT_CRYPTO_ALGORITHM.to_owned(),
            sandbox_enabled: false,
            sandbox_host: DEFAULT_SANDBOX_HOST.to_owned(),
            llm_factory: String::new(),
            llm_base_url: String::new(),
            default_models: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// Snapshot the process environment (`os.getenv` semantics).
    pub fn from_env() -> Self {
        Self::from_reader(|key| std::env::var(key).ok())
    }

    /// Build settings from an injected key lookup. Pure and testable.
    pub fn from_reader(get: impl Fn(&str) -> Option<String>) -> Self {
        let mut settings = Settings::default();

        if let Some(value) = get("TZ") {
            settings.timezone = value;
        }
        if let Some(value) = get("DB_TYPE") {
            settings.database_type = value;
        }
        settings.storage_impl = StorageImpl::from_env_value(
            get("STORAGE_IMPL")
                .as_deref()
                .unwrap_or(DEFAULT_STORAGE_IMPL),
        );
        settings.doc_engine =
            DocEngine::from_env_value(get("DOC_ENGINE").as_deref().unwrap_or(DEFAULT_DOC_ENGINE));
        settings.doc_maximum_size = get("MAX_CONTENT_LENGTH")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_DOC_MAXIMUM_SIZE);
        settings.doc_bulk_size = get("DOC_BULK_SIZE")
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(DEFAULT_DOC_BULK_SIZE);
        settings.embedding_batch_size = get("EMBEDDING_BATCH_SIZE")
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(DEFAULT_EMBEDDING_BATCH_SIZE);
        settings.register_enabled = get("REGISTER_ENABLED")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(1);
        if let Some(value) = get("DISABLE_PASSWORD_LOGIN") {
            settings.disable_password_login = parse_disable_password_login_env(&value);
        }
        settings.strong_test_count = get("STRONG_TEST_COUNT")
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(DEFAULT_STRONG_TEST_COUNT);
        settings.max_file_num_per_user = get("MAX_FILE_NUM_PER_USER")
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(0);
        settings.secret_key = init_secret_key(get("RAGFLOW_SECRET_KEY").as_deref(), None);
        if let Some(value) = get("RAGFLOW_CRYPTO_ENABLED") {
            settings.crypto_enabled = value.eq_ignore_ascii_case("true");
        }
        if let Some(value) = get("RAGFLOW_CRYPTO_ALGORITHM") {
            settings.crypto_algorithm = value;
        }
        if let Some(value) = get("SANDBOX_ENABLED") {
            settings.sandbox_enabled = value.trim().parse::<i64>().unwrap_or(0) != 0;
        }
        if let Some(value) = get("SANDBOX_HOST") {
            settings.sandbox_host = value;
        }
        settings
    }

    /// `rag/settings.py::get_svr_queue_name` — priority 0 uses the base queue,
    /// any other priority is suffixed.
    pub fn svr_queue_name(&self, priority: i32) -> String {
        get_svr_queue_name(priority)
    }

    /// `rag/settings.py::get_svr_queue_names` — `[priority 1, priority 0]`.
    pub fn svr_queue_names(&self) -> Vec<String> {
        get_svr_queue_names()
    }
}

/// `rag/settings.py::get_svr_queue_name(priority)`.
pub fn get_svr_queue_name(priority: i32) -> String {
    if priority == 0 {
        SVR_QUEUE_NAME.to_owned()
    } else {
        format!("{SVR_QUEUE_NAME}_{priority}")
    }
}

/// `rag/settings.py::get_svr_queue_names()`.
pub fn get_svr_queue_names() -> Vec<String> {
    vec![get_svr_queue_name(1), get_svr_queue_name(0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader<'a>(entries: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            entries
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn defaults_match_python_rag_settings() {
        let settings = Settings::from_reader(reader(&[]));
        assert_eq!(settings.timezone, "Asia/Shanghai");
        assert_eq!(settings.storage_impl, StorageImpl::Minio);
        assert_eq!(settings.doc_engine, DocEngine::Elasticsearch);
        assert_eq!(settings.doc_maximum_size, 128 * 1024 * 1024);
        assert_eq!(settings.doc_bulk_size, 4);
        assert_eq!(settings.embedding_batch_size, 16);
        assert_eq!(settings.register_enabled, 1);
        assert!(!settings.disable_password_login);
        assert_eq!(settings.strong_test_count, 8);
        assert_eq!(settings.max_file_num_per_user, 0);
        assert_eq!(settings.secret_key, None);
        assert!(!settings.crypto_enabled);
        assert_eq!(settings.crypto_algorithm, "aes-256-cbc");
        assert!(!settings.sandbox_enabled);
        assert_eq!(settings.sandbox_host, "sandbox-executor-manager");
        assert_eq!(get_svr_queue_name(0), "rag_flow_svr_queue");
        assert_eq!(get_svr_queue_name(1), "rag_flow_svr_queue_1");
        assert_eq!(
            get_svr_queue_names(),
            vec!["rag_flow_svr_queue_1", "rag_flow_svr_queue"]
        );
        assert_eq!(FLOAT_ZERO, 1e-8);
        assert_eq!(PARAM_MAXDEPTH, 5);
    }

    #[test]
    fn env_values_override_defaults_with_python_type_conversion() {
        let settings = Settings::from_reader(reader(&[
            ("TZ", "UTC"),
            ("DB_TYPE", "postgres"),
            ("STORAGE_IMPL", "aws_s3"),
            ("DOC_ENGINE", "Infinity"),
            ("MAX_CONTENT_LENGTH", "1048576"),
            ("DOC_BULK_SIZE", "8"),
            ("EMBEDDING_BATCH_SIZE", "32"),
            ("REGISTER_ENABLED", "0"),
            ("DISABLE_PASSWORD_LOGIN", "true"),
            ("STRONG_TEST_COUNT", "16"),
            ("MAX_FILE_NUM_PER_USER", "100"),
            ("RAGFLOW_CRYPTO_ENABLED", "True"),
            ("RAGFLOW_CRYPTO_ALGORITHM", "aes-256-gcm"),
            ("SANDBOX_ENABLED", "1"),
            ("SANDBOX_HOST", "sandbox-2"),
        ]));
        assert_eq!(settings.timezone, "UTC");
        assert_eq!(settings.database_type, "postgres");
        assert_eq!(settings.storage_impl, StorageImpl::AwsS3);
        assert_eq!(settings.doc_engine, DocEngine::Infinity);
        assert!(settings.doc_engine.is_infinity());
        assert_eq!(settings.doc_maximum_size, 1048576);
        assert_eq!(settings.doc_bulk_size, 8);
        assert_eq!(settings.embedding_batch_size, 32);
        assert_eq!(settings.register_enabled, 0);
        assert!(settings.disable_password_login);
        assert_eq!(settings.strong_test_count, 16);
        assert_eq!(settings.max_file_num_per_user, 100);
        assert!(settings.crypto_enabled);
        assert_eq!(settings.crypto_algorithm, "aes-256-gcm");
        assert!(settings.sandbox_enabled);
        assert_eq!(settings.sandbox_host, "sandbox-2");

        // Loose boolean parsing mirrors Python `in ("1","true","yes")`.
        assert!(parse_disable_password_login_env("YES"));
        assert!(parse_disable_password_login_env("1"));
        assert!(!parse_disable_password_login_env("0"));
        assert!(!parse_disable_password_login_env(""));

        // oceanbase/seekdb family flags.
        assert!(DocEngine::Oceanbase.is_oceanbase());
        assert!(DocEngine::Seekdb.is_oceanbase_family());
        assert!(DocEngine::Oceanbase.is_oceanbase_family());
        assert!(!DocEngine::Elasticsearch.is_oceanbase_family());
    }

    #[test]
    fn secret_key_requires_32_chars_and_rejects_stale_date_placeholders() {
        let short = "0123456789abcdef";
        assert_eq!(init_secret_key(Some(short), None), None);
        let good = "0123456789abcdef0123456789abcdef";
        assert_eq!(init_secret_key(Some(good), None), Some(good.to_owned()));
        // A configured key equal to today's date is a stale placeholder in
        // `init_secret_key`; note the date itself is 10 chars so a ≥32-char
        // key can never equal it — the check is defensive, mirroring Python.
        let today = {
            let now = time::OffsetDateTime::now_utc();
            format!(
                "{:04}-{:02}-{:02}",
                now.year(),
                now.month() as u8,
                now.day()
            )
        };
        let mut padded = today;
        while padded.len() < 32 {
            padded.push('x');
        }
        // padded != today and len >= 32 → accepted (Python: key != str(date.today())).
        assert_eq!(
            init_secret_key(None, Some(&padded)),
            Some(padded.clone())
        );
        let configured = "abcdef0123456789abcdef0123456789";
        assert_eq!(
            init_secret_key(None, Some(configured)),
            Some(configured.to_owned())
        );
        // Explicit env key always wins over the configured one.
        assert_eq!(
            init_secret_key(Some(good), Some(configured)),
            Some(good.to_owned())
        );
    }

    #[test]
    fn model_entry_parsing_and_resolution_match_python() {
        // String entry → bare name, no factory.
        let entry = parse_model_entry(&Value::String("deepseek-v3".into()));
        assert_eq!(entry.name, "deepseek-v3");
        assert_eq!(entry.factory, None);

        // Dict entry with explicit fields.
        let dict = serde_json::json!({
            "name": "qwen-max",
            "factory": "volcengine",
            "api_key": "k",
            "base_url": "https://ark.cn-beijing.volces.com"
        });
        let entry = parse_model_entry(&dict);
        assert_eq!(entry.name, "qwen-max");
        assert_eq!(entry.factory.as_deref(), Some("volcengine"));
        assert_eq!(entry.api_key.as_deref(), Some("k"));

        // Per-model config resolution: name@factory composition, fallbacks.
        // Entry-level factory/api_key/base_url win over the backups — the
        // same `or` chaining as `_resolve_per_model_config`.
        let resolved =
            resolve_per_model_config(&entry, "default-factory", "default-key", "default-url");
        assert_eq!(resolved.name, "qwen-max@volcengine");
        assert_eq!(resolved.factory.as_deref(), Some("volcengine"));
        assert_eq!(resolved.api_key.as_deref(), Some("k"));
        assert_eq!(resolved.base_url.as_deref(), Some("https://ark.cn-beijing.volces.com"));

        // A bare-name entry picks up the backup factory and composes name@factory.
        let bare = parse_model_entry(&Value::String("bge-m3".into()));
        let resolved = resolve_per_model_config(&bare, "tei", "", "");
        assert_eq!(resolved.name, "bge-m3@tei");
        assert_eq!(resolved.factory.as_deref(), Some("tei"));

        // Names already containing '@' are not re-composed.
        let already = parse_model_entry(&Value::String("gpt-4o@openai".into()));
        let resolved = resolve_per_model_config(&already, "other", "", "");
        assert_eq!(resolved.name, "gpt-4o@openai");
    }
}
