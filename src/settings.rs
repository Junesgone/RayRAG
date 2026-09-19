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
    if !name.is_empty() && !name.contains('@')
        && let Some(factory) = &factory {
            name = format!("{name}@{factory}");
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
    if let Some(key) = env_key
        && key.len() >= 32 {
            return Some(key.to_owned());
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
    if let Some(key) = configured_key
        && key.len() >= 32 && key != today {
            return Some(key.to_owned());
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

// ═══════════════════════════════════════════════════════════════════════════
// conf/service_conf.yaml coverage — ServiceConf.
//
// RAGFlow's deployment YAML describes the HTTP entry points (ragflow /
// admin), the storage backends (mysql / minio / es / os / infinity /
// oceanbase / redis), the task executor queue and the `user_default_llm`
// block. RayRAG keeps the env-driven settings in [`Settings`] (which
// already mirrors `rag/settings.py` + `common/settings.py`); this section
// adds the *file shape* so a conf/service_conf.yaml-equivalent document can
// be parsed and applied on top of the environment snapshot.
//
// Note on logging: service_conf.yaml carries no logging stanza — RAGFlow
// logging is environment-driven (LOG_LEVEL / LOG_FILE etc.), which RayRAG
// handles in `src/logging.rs`. Nothing to port here.
// ═══════════════════════════════════════════════════════════════════════════

/// Default RAGFlow HTTP port (`ragflow.http_port` in service_conf.yaml).
pub const DEFAULT_HTTP_PORT: u16 = 9380;
/// Default RAGFlow admin port (`admin.http_port` in service_conf.yaml).
pub const DEFAULT_ADMIN_HTTP_PORT: u16 = 9381;
/// Default task executor message queue type (`task_executor.message_queue_type`).
pub const DEFAULT_MESSAGE_QUEUE_TYPE: &str = "redis";
/// Default MySQL database name (`mysql.name` in service_conf.yaml).
pub const DEFAULT_MYSQL_DATABASE: &str = "rag_flow";
/// Default MySQL port.
pub const DEFAULT_MYSQL_PORT: u16 = 3306;
/// Default Redis logical database (`redis.db`).
pub const DEFAULT_REDIS_DB: i64 = 1;

/// `mysql:` block of service_conf.yaml.
#[derive(Debug, Clone, PartialEq)]
pub struct MysqlConf {
    pub name: String,
    pub user: String,
    pub password: String,
    pub host: String,
    pub port: u16,
    pub max_connections: u32,
    pub stale_timeout: u32,
}

/// `minio:` block of service_conf.yaml.
#[derive(Debug, Clone, PartialEq)]
pub struct MinioConf {
    pub user: String,
    pub password: String,
    pub host: String,
    pub bucket: String,
    pub prefix_path: String,
}

/// `es:` block of service_conf.yaml.
#[derive(Debug, Clone, PartialEq)]
pub struct EsConf {
    pub hosts: String,
    pub username: String,
    pub password: String,
}

/// `os:` block of service_conf.yaml (OpenSearch).
#[derive(Debug, Clone, PartialEq)]
pub struct OsConf {
    pub hosts: String,
    pub username: String,
    pub password: String,
}

/// `infinity:` block of service_conf.yaml.
#[derive(Debug, Clone, PartialEq)]
pub struct InfinityConf {
    pub uri: String,
    pub postgres_port: u16,
    pub db_name: String,
}

/// `file_syncer:` block of service_conf.yaml. Legacy dead-config in RAGFlow
/// v0.26.4 (no active Python consumer; the Go server only carries the config
/// struct). RayRAG parses the keys for config parity; the manual
/// `sync_data_source` endpoint matches upstream behavior.
#[derive(Debug, Clone, PartialEq)]
pub struct FileSyncerConf {
    pub max_concurrent_syncs: u32,
    pub sync_interval: u32,
}

/// `redis:` block of service_conf.yaml.
#[derive(Debug, Clone, PartialEq)]
pub struct RedisConf {
    pub db: i64,
    pub username: String,
    pub password: String,
    pub host: String,
}

/// `user_default_llm:` block of service_conf.yaml — the default factory,
/// api_key, base_url and per-capability default models.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserDefaultLlm {
    pub factory: String,
    pub api_key: String,
    pub base_url: String,
    /// capability key (`embedding_model`, `chat_model`, `rerank_model`, ...)
    /// → model entry.
    pub default_models: BTreeMap<String, ModelEntry>,
}

/// The parsed shape of RAGFlow `conf/service_conf.yaml`.
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceConf {
    pub ragflow_host: String,
    pub http_port: u16,
    pub admin_host: String,
    pub admin_port: u16,
    pub mysql: MysqlConf,
    pub minio: MinioConf,
    pub es: EsConf,
    pub os: OsConf,
    pub infinity: InfinityConf,
    pub redis: RedisConf,
    pub message_queue_type: String,
    pub file_syncer: FileSyncerConf,
    pub user_default_llm: UserDefaultLlm,
}

impl Default for ServiceConf {
    /// The shipped defaults of conf/service_conf.yaml (the docker-compose
    /// deployment shape). Secrets here are the well-known dev defaults of
    /// the reference deployment, not RayRAG runtime values.
    fn default() -> Self {
        Self {
            ragflow_host: "0.0.0.0".to_owned(),
            http_port: DEFAULT_HTTP_PORT,
            admin_host: "0.0.0.0".to_owned(),
            admin_port: DEFAULT_ADMIN_HTTP_PORT,
            mysql: MysqlConf {
                name: DEFAULT_MYSQL_DATABASE.to_owned(),
                user: "root".to_owned(),
                password: "infini_rag_flow".to_owned(),
                host: "localhost".to_owned(),
                port: DEFAULT_MYSQL_PORT,
                max_connections: 900,
                stale_timeout: 300,
            },
            minio: MinioConf {
                user: "rag_flow".to_owned(),
                password: "infini_rag_flow".to_owned(),
                host: "localhost:9000".to_owned(),
                bucket: String::new(),
                prefix_path: String::new(),
            },
            es: EsConf {
                hosts: "http://localhost:1200".to_owned(),
                username: "elastic".to_owned(),
                password: "infini_rag_flow".to_owned(),
            },
            os: OsConf {
                hosts: "http://localhost:1201".to_owned(),
                username: "admin".to_owned(),
                password: "infini_rag_flow_OS_01".to_owned(),
            },
            infinity: InfinityConf {
                uri: "localhost:23817".to_owned(),
                postgres_port: 5432,
                db_name: "default_db".to_owned(),
            },
            redis: RedisConf {
                db: DEFAULT_REDIS_DB,
                username: String::new(),
                password: "infini_rag_flow".to_owned(),
                host: "localhost:6379".to_owned(),
            },
            message_queue_type: DEFAULT_MESSAGE_QUEUE_TYPE.to_owned(),
            file_syncer: FileSyncerConf {
                max_concurrent_syncs: 4,
                sync_interval: 3,
            },
            user_default_llm: UserDefaultLlm::default(),
        }
    }
}

/// Look up a top-level section of a service_conf document as a JSON object.
fn service_section<'a>(root: &'a Value, key: &str) -> Option<&'a serde_json::Map<String, Value>> {
    root.as_object()
        .and_then(|map| map.get(key))
        .and_then(Value::as_object)
}

impl ServiceConf {
    /// Parse a service_conf document given as JSON (YAML is a superset of
    /// JSON, so a `serde_yaml`-converted or hand-authored JSON document with
    /// the same keys works). Missing keys fall back to [`ServiceConf::default`].
    pub fn from_value(value: &Value) -> Self {
        let mut conf = ServiceConf::default();
        let str_field = |map: Option<&serde_json::Map<String, Value>>, key: &str| {
            map.and_then(|map| map.get(key))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let u16_field = |map: Option<&serde_json::Map<String, Value>>, key: &str| {
            map.and_then(|map| map.get(key))
                .and_then(Value::as_u64)
                .map(|v| v as u16)
        };
        let u32_field = |map: Option<&serde_json::Map<String, Value>>, key: &str| {
            map.and_then(|map| map.get(key))
                .and_then(Value::as_u64)
                .map(|v| v as u32)
        };
        let i64_field = |map: Option<&serde_json::Map<String, Value>>, key: &str| {
            map.and_then(|map| map.get(key)).and_then(Value::as_i64)
        };

        let ragflow = service_section(value, "ragflow");
        conf.ragflow_host = str_field(ragflow, "host").unwrap_or(conf.ragflow_host);
        conf.http_port = u16_field(ragflow, "http_port").unwrap_or(conf.http_port);
        let admin = service_section(value, "admin");
        conf.admin_host = str_field(admin, "host").unwrap_or(conf.admin_host);
        conf.admin_port = u16_field(admin, "http_port").unwrap_or(conf.admin_port);

        if let Some(mysql) = service_section(value, "mysql") {
            conf.mysql.name = str_field(Some(mysql), "name").unwrap_or(conf.mysql.name);
            conf.mysql.user = str_field(Some(mysql), "user").unwrap_or(conf.mysql.user);
            conf.mysql.password = str_field(Some(mysql), "password").unwrap_or(conf.mysql.password);
            conf.mysql.host = str_field(Some(mysql), "host").unwrap_or(conf.mysql.host);
            conf.mysql.port = u16_field(Some(mysql), "port").unwrap_or(conf.mysql.port);
            conf.mysql.max_connections =
                u32_field(Some(mysql), "max_connections").unwrap_or(conf.mysql.max_connections);
            conf.mysql.stale_timeout =
                u32_field(Some(mysql), "stale_timeout").unwrap_or(conf.mysql.stale_timeout);
        }
        if let Some(minio) = service_section(value, "minio") {
            conf.minio.user = str_field(Some(minio), "user").unwrap_or(conf.minio.user);
            conf.minio.password = str_field(Some(minio), "password").unwrap_or(conf.minio.password);
            conf.minio.host = str_field(Some(minio), "host").unwrap_or(conf.minio.host);
            conf.minio.bucket = str_field(Some(minio), "bucket").unwrap_or(conf.minio.bucket);
            conf.minio.prefix_path =
                str_field(Some(minio), "prefix_path").unwrap_or(conf.minio.prefix_path);
        }
        if let Some(es) = service_section(value, "es") {
            conf.es.hosts = str_field(Some(es), "hosts").unwrap_or(conf.es.hosts);
            conf.es.username = str_field(Some(es), "username").unwrap_or(conf.es.username);
            conf.es.password = str_field(Some(es), "password").unwrap_or(conf.es.password);
        }
        if let Some(os) = service_section(value, "os") {
            conf.os.hosts = str_field(Some(os), "hosts").unwrap_or(conf.os.hosts);
            conf.os.username = str_field(Some(os), "username").unwrap_or(conf.os.username);
            conf.os.password = str_field(Some(os), "password").unwrap_or(conf.os.password);
        }
        if let Some(infinity) = service_section(value, "infinity") {
            conf.infinity.uri = str_field(Some(infinity), "uri").unwrap_or(conf.infinity.uri);
            conf.infinity.postgres_port =
                u16_field(Some(infinity), "postgres_port").unwrap_or(conf.infinity.postgres_port);
            conf.infinity.db_name =
                str_field(Some(infinity), "db_name").unwrap_or(conf.infinity.db_name);
        }
        if let Some(redis) = service_section(value, "redis") {
            conf.redis.db = i64_field(Some(redis), "db").unwrap_or(conf.redis.db);
            conf.redis.username = str_field(Some(redis), "username").unwrap_or(conf.redis.username);
            conf.redis.password = str_field(Some(redis), "password").unwrap_or(conf.redis.password);
            conf.redis.host = str_field(Some(redis), "host").unwrap_or(conf.redis.host);
        }
        if let Some(task_executor) = service_section(value, "task_executor") {
            conf.message_queue_type = str_field(Some(task_executor), "message_queue_type")
                .unwrap_or(conf.message_queue_type);
        }
        if let Some(file_syncer) = service_section(value, "file_syncer") {
            conf.file_syncer.max_concurrent_syncs = u32_field(Some(file_syncer), "max_concurrent_syncs")
                .unwrap_or(conf.file_syncer.max_concurrent_syncs);
            conf.file_syncer.sync_interval =
                u32_field(Some(file_syncer), "sync_interval").unwrap_or(conf.file_syncer.sync_interval);
        }
        if let Some(llm) = service_section(value, "user_default_llm") {
            conf.user_default_llm.factory =
                str_field(Some(llm), "factory").unwrap_or(conf.user_default_llm.factory);
            conf.user_default_llm.api_key =
                str_field(Some(llm), "api_key").unwrap_or(conf.user_default_llm.api_key);
            conf.user_default_llm.base_url =
                str_field(Some(llm), "base_url").unwrap_or(conf.user_default_llm.base_url);
            if let Some(default_models) = llm.get("default_models").and_then(Value::as_object) {
                for (capability, entry) in default_models {
                    conf.user_default_llm
                        .default_models
                        .insert(capability.clone(), parse_model_entry(entry));
                }
            }
        }
        conf
    }
    /// Apply the `user_default_llm` block onto a [`Settings`] snapshot —
    /// fills the `llm_factory` / `llm_base_url` / `default_models` fields
    /// that the env reader intentionally leaves empty (RAGFlow reads them
    /// from service_conf.yaml, not from environment variables).
    pub fn apply_to_settings(&self, settings: &mut Settings) {
        settings.llm_factory = self.user_default_llm.factory.clone();
        settings.llm_base_url = self.user_default_llm.base_url.clone();
        for (capability, entry) in &self.user_default_llm.default_models {
            let resolved = resolve_per_model_config(
                entry,
                &self.user_default_llm.factory,
                &self.user_default_llm.api_key,
                &self.user_default_llm.base_url,
            );
            settings.default_models.insert(capability.clone(), resolved);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// conf/mapping*.json coverage — ES / Infinity field-schema semantics.
//
// RAGFlow's document store is pluggable; each engine ships a field mapping:
//   - conf/mapping.json + conf/os_mapping.json  — Elasticsearch / OpenSearch
//     chunk-index `dynamic_templates` (field-name suffix → ES type).
//   - conf/doc_meta_es_mapping.json             — ES doc-meta index
//     (keyword `id` / `kb_id` + dynamic `meta_fields` object).
//   - conf/infinity_mapping.json                — Infinity chunk fields
//     (varchar / integer / float, analyzers, secondary indexes).
//   - conf/doc_meta_infinity_mapping.json       — Infinity doc-meta fields.
//   - conf/message_infinity_mapping.json        — Infinity memory-message
//     fields (memory / agent / session scoping + validity windows).
//   - conf/skill_es_mapping.json + conf/skill_infinity_mapping.json —
//     skill-store fields (name/tags/description/content + q_<dims>_vec).
//
// RayRAG replaces these engines rather than deploying their schemas verbatim.
// Exact doc-meta, message-Infinity and Skill ES/Infinity schemas are fixed below;
// their JSON/PostgreSQL/zvec ownership is documented at each replacement
// boundary. Other engine mapping contracts retain their independently audited
// coverage status.
// ═══════════════════════════════════════════════════════════════════════════

/// Why the fixed ES/Infinity document-metadata mappings are replaced.
pub const ENGINE_MAPPING_NOTE: &str = "RayRAG replaces ES/OpenSearch/Infinity mappings: document metadata uses an \
     atomic JSON snapshot with an optional opaque PostgreSQL 18 mirror, and \
     zvec-rust stores chunk and Skill vectors in isolated collections. No ES/Infinity metadata index is \
     created; typed Rust schemas preserve the fixed wire names, types and defaults.";

/// Why the fixed password-transport PEM pair is replaced, not copied.
pub const PASSWORD_TRANSPORT_KEYS_POLICY: &str = "RAGFlow conf/private.pem + public.pem are an \
     RSAES-PKCS1-v1_5 password-transport pair, not JWT signing keys. RayRAG never ships the \
     globally known private key. A deployment-specific RSA private key may be mounted through \
     RAYRAG_PASSWORD_PRIVATE_KEY_FILE; RayRAG derives its public key and accepts the compatible \
     base64(RSA(base64(UTF-8))) wire form. Plaintext compatibility remains available and requires \
     TLS at the deployment boundary; stored passwords use salted Argon2id hashes.";

/// Field kinds derived from the ES/OpenSearch dynamic-template suffix rules
/// (conf/mapping.json / conf/os_mapping.json).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Int,
    Ulong,
    Long,
    Short,
    Float,
    /// `*_tks` — whitespace-analyzed text with scripted idf similarity.
    Tokens,
    /// `*_ltks` — whitespace-analyzed long text.
    LongTokens,
    /// `*_kwd`, `*_id(s)`, `*_uid(s)` — keyword (boolean similarity).
    Keyword,
    /// `*_dt` / `*_time` / `*_at` — date with RAGFlow's format list.
    DateTime,
    /// `*_nst` — nested object.
    Nested,
    /// `*_obj` — dynamic object.
    Object,
    /// `*_with_weight` / `*_list` — stored text, not indexed.
    UnindexedText,
    /// `*_fea` — rank_feature.
    RankFeature,
    /// `*_feas` — rank_features.
    RankFeatures,
    /// `*_<dims>_vec` — dense vector (dims ∈ 512..10240).
    DenseVector(u64),
    /// `*_bin` — binary blob.
    Binary,
    /// Explicit fixed `lat_lon` geo_point property.
    GeoPoint,
    /// Unmatched field (dynamic string default in ES).
    Text,
}

impl FieldKind {
    pub fn as_str(self) -> String {
        match self {
            FieldKind::Int => "integer".to_owned(),
            FieldKind::Ulong => "unsigned_long".to_owned(),
            FieldKind::Long => "long".to_owned(),
            FieldKind::Short => "short".to_owned(),
            FieldKind::Float => "float".to_owned(),
            FieldKind::Tokens => "text(whitespace,scripted_sim)".to_owned(),
            FieldKind::LongTokens => "text(whitespace)".to_owned(),
            FieldKind::Keyword => "keyword".to_owned(),
            FieldKind::DateTime => "date".to_owned(),
            FieldKind::Nested => "nested".to_owned(),
            FieldKind::Object => "object(dynamic)".to_owned(),
            FieldKind::UnindexedText => "text(index=false)".to_owned(),
            FieldKind::RankFeature => "rank_feature".to_owned(),
            FieldKind::RankFeatures => "rank_features".to_owned(),
            FieldKind::DenseVector(dims) => format!("dense_vector({dims})"),
            FieldKind::Binary => "binary".to_owned(),
            FieldKind::GeoPoint => "geo_point".to_owned(),
            FieldKind::Text => "text".to_owned(),
        }
    }
}

/// Search-engine mapping whose dynamic-template order is being evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMappingFlavor {
    Elasticsearch,
    OpenSearch,
}

/// Common non-vector suffix rules shared by the fixed ES and OpenSearch files.
/// Regex templates and vectors are kept separate because the two files differ.
pub const SEARCH_FIELD_SUFFIX_RULES: &[(&str, FieldKind)] = &[
    ("_int", FieldKind::Int),
    ("_ulong", FieldKind::Ulong),
    ("_long", FieldKind::Long),
    ("_short", FieldKind::Short),
    ("_flt", FieldKind::Float),
    ("_tks", FieldKind::Tokens),
    ("_ltks", FieldKind::LongTokens),
    ("_kwd", FieldKind::Keyword),
    ("_nst", FieldKind::Nested),
    ("_obj", FieldKind::Object),
    ("_with_weight", FieldKind::UnindexedText),
    ("_list", FieldKind::UnindexedText),
    ("_fea", FieldKind::RankFeature),
    ("_feas", FieldKind::RankFeatures),
    ("_bin", FieldKind::Binary),
];

/// Exact vector dimensions from fixed `conf/mapping.json`.
pub const ES_CHUNK_VECTOR_DIMENSIONS: &[u64] = &[512, 768, 1024, 1536];

/// Exact vector dimensions from fixed `conf/os_mapping.json`.
pub const OPENSEARCH_CHUNK_VECTOR_DIMENSIONS: &[u64] =
    &[512, 768, 1024, 1536, 2048, 4096, 6144, 8192, 10240];

/// Exact keyword regex from fixed `conf/mapping.json`.
pub const ES_CHUNK_KEYWORD_PATTERN: &str = r"^(.*_(kwd|id|ids|uid|uids)|uid|id)$";

/// Exact keyword regex from fixed `conf/os_mapping.json`. Unlike ES, a bare
/// `id` is not captured and therefore remains dynamically mapped text.
pub const OPENSEARCH_CHUNK_KEYWORD_PATTERN: &str = r"^(.*_(kwd|id|ids|uid|uids)|uid)$";

pub const SEARCH_CHUNK_DATE_PATTERN: &str = r"^.*(_dt|_time|_at)$";
pub const SEARCH_CHUNK_DATE_FORMAT: &str = "yyyy-MM-dd HH:mm:ss||yyyy-MM-dd||yyyy-MM-dd_HH:mm:ss";
pub const SEARCH_CHUNK_SCRIPTED_SIMILARITY: &str = "double idf = Math.log(1+(field.docCount-term.docFreq+0.5)/(term.docFreq + 0.5))/Math.log(1+((field.docCount-0.5)/1.5)); return query.boost * idf * Math.min(doc.freq, 1);";
pub const SEARCH_CHUNK_NUMBER_OF_SHARDS: u8 = 2;
pub const SEARCH_CHUNK_NUMBER_OF_REPLICAS: u8 = 0;
pub const SEARCH_CHUNK_REFRESH_INTERVAL: &str = "1000ms";
pub const SEARCH_CHUNK_DATE_DETECTION: &str = "true";

pub const ES_CHUNK_MAPPING_UPSTREAM_BLOB: &str = "495f7c7763cfbfd6ad1847df63005d8b86be339a";
pub const ES_CHUNK_MAPPING_UPSTREAM_SHA256: &str =
    "c6991ef11034289e79d9b7b8dabb37f650b041b7e215e17357d4f167660f7b82";
pub const ES_CHUNK_MAPPING_UPSTREAM_BYTES: usize = 4_453;
pub const ES_CHUNK_MAPPING_UPSTREAM_LINES: usize = 212;
pub const OPENSEARCH_CHUNK_MAPPING_UPSTREAM_BLOB: &str = "47b7c24b0f53a6e9cea08b7b10d2bf8027cad6bb";
pub const OPENSEARCH_CHUNK_MAPPING_UPSTREAM_SHA256: &str =
    "e06d7f2d9e2d39d70dc791a88118d328743b5bbd52adb1fd9deb4e58e730f97d";
pub const OPENSEARCH_CHUNK_MAPPING_UPSTREAM_BYTES: usize = 5_766;
pub const OPENSEARCH_CHUNK_MAPPING_UPSTREAM_LINES: usize = 268;

/// Resolve a fixed ES/OpenSearch field kind, mirroring dynamic-template order.
pub fn search_field_kind(flavor: SearchMappingFlavor, field: &str) -> FieldKind {
    if field == "lat_lon" {
        return FieldKind::GeoPoint;
    }
    for (suffix, kind) in SEARCH_FIELD_SUFFIX_RULES {
        if field.ends_with(suffix) {
            return *kind;
        }
    }
    let keyword_pattern = match flavor {
        SearchMappingFlavor::Elasticsearch => ES_CHUNK_KEYWORD_PATTERN,
        SearchMappingFlavor::OpenSearch => OPENSEARCH_CHUNK_KEYWORD_PATTERN,
    };
    if regex_matches(keyword_pattern, field) {
        return FieldKind::Keyword;
    }
    if regex_matches(SEARCH_CHUNK_DATE_PATTERN, field) {
        return FieldKind::DateTime;
    }
    let dimensions = match flavor {
        SearchMappingFlavor::Elasticsearch => ES_CHUNK_VECTOR_DIMENSIONS,
        SearchMappingFlavor::OpenSearch => OPENSEARCH_CHUNK_VECTOR_DIMENSIONS,
    };
    if let Some(dims) = vector_field_dimension(field)
        && dimensions.contains(&dims)
    {
        return FieldKind::DenseVector(dims);
    }
    FieldKind::Text
}

pub fn es_field_kind(field: &str) -> FieldKind {
    search_field_kind(SearchMappingFlavor::Elasticsearch, field)
}

pub fn opensearch_field_kind(field: &str) -> FieldKind {
    search_field_kind(SearchMappingFlavor::OpenSearch, field)
}

/// PostgreSQL/zvec replacement column kind. The replacement deliberately
/// accepts any positive `q_<dimension>_vec`, while the fixed native mappings
/// remain restricted to their respective dimension lists above.
pub fn zvec_column_kind(field: &str) -> String {
    let kind = vector_field_dimension(field)
        .map(FieldKind::DenseVector)
        .unwrap_or_else(|| es_field_kind(field));
    match kind {
        FieldKind::Int | FieldKind::Short => "INTEGER".to_owned(),
        FieldKind::Ulong | FieldKind::Long => "BIGINT".to_owned(),
        FieldKind::Float => "REAL".to_owned(),
        FieldKind::DenseVector(dims) => format!("VECTOR({dims})"),
        FieldKind::Binary => "BYTEA".to_owned(),
        FieldKind::GeoPoint => "JSONB".to_owned(),
        FieldKind::Nested | FieldKind::Object => "JSONB".to_owned(),
        _ => "TEXT".to_owned(),
    }
}

fn vector_field_dimension(field: &str) -> Option<u64> {
    field
        .strip_suffix("_vec")?
        .rsplit('_')
        .next()?
        .parse::<u64>()
        .ok()
        .filter(|dimension| *dimension > 0)
}

/// Tiny regex subset for the two documented ES templates (`^...$` anchors,
/// `.*`, character classes, alternation). Enough for the conf patterns.
fn regex_matches(pattern: &str, field: &str) -> bool {
    let body = pattern
        .strip_prefix('^')
        .and_then(|p| p.strip_suffix('$'))
        .unwrap_or(pattern);
    if body == ".*" {
        return true;
    }
    // ^(.*_(kwd|id|ids|uid|uids)|uid|id)$ — the ES `kwd` template: any field
    // ending in one of the keyword suffixes, or exactly `uid` / `id`.
    if matches!(
        body,
        "(.*_(kwd|id|ids|uid|uids)|uid|id)" | "(.*_(kwd|id|ids|uid|uids)|uid)"
    ) {
        return field == "uid"
            || (body.ends_with("|id)") && field == "id")
            || ["_kwd", "_id", "_ids", "_uid", "_uids"]
                .iter()
                .any(|suffix| field.ends_with(suffix));
    }
    // ^.*(_dt|_time|_at)$ — the ES `dt` template: any field ending in one of
    // the date suffixes.
    if let Some(rest) = body.strip_prefix(".*(")
        && let Some(alts) = rest.strip_suffix(')') {
            return alts.split('|').any(|suffix| field.ends_with(suffix));
        }
    body == field
}

/// Exact fixed-version settings from `conf/doc_meta_es_mapping.json`.
pub const DOC_META_ES_NUMBER_OF_SHARDS: u8 = 2;
pub const DOC_META_ES_NUMBER_OF_REPLICAS: u8 = 0;
pub const DOC_META_ES_REFRESH_INTERVAL: &str = "1000ms";
pub const DOC_META_ES_SOURCE_ENABLED: bool = true;
pub const DOC_META_ES_DYNAMIC: &str = "runtime";

/// One field shared by the ES and Infinity document-metadata schemas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocMetaFieldSchema {
    pub name: &'static str,
    pub es_type: &'static str,
    pub es_store: bool,
    pub es_dynamic: bool,
    pub infinity_type: &'static str,
    pub infinity_default: &'static str,
}

/// Exact field/type/default contract shared by
/// `conf/doc_meta_es_mapping.json` and `conf/doc_meta_infinity_mapping.json`.
pub const DOC_META_FIELDS: &[DocMetaFieldSchema] = &[
    DocMetaFieldSchema {
        name: "id",
        es_type: "keyword",
        es_store: true,
        es_dynamic: false,
        infinity_type: "varchar",
        infinity_default: "",
    },
    DocMetaFieldSchema {
        name: "kb_id",
        es_type: "keyword",
        es_store: true,
        es_dynamic: false,
        infinity_type: "varchar",
        infinity_default: "",
    },
    DocMetaFieldSchema {
        name: "meta_fields",
        es_type: "object",
        es_store: false,
        es_dynamic: true,
        infinity_type: "json",
        infinity_default: "{}",
    },
];

/// Fixed Git identity of `conf/infinity_mapping.json`.
pub const INFINITY_CHUNK_UPSTREAM_BLOB: &str = "dfc07d2e44814c392929301ae71e3972bca14748";
pub const INFINITY_CHUNK_UPSTREAM_BYTES: usize = 5_413;
pub const INFINITY_CHUNK_UPSTREAM_LINES: usize = 76;

/// Exact JSON default kind used by one base Infinity chunk column.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InfinityChunkDefault {
    Text(&'static str),
    Integer(i64),
    Float(f64),
}

/// The mapping distinguishes a single analyzer string from an analyzer array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfinityChunkAnalyzer {
    None,
    Single(&'static str),
    Multiple(&'static [&'static str]),
}

/// One ordered base column from `conf/infinity_mapping.json`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InfinityChunkFieldSchema {
    pub name: &'static str,
    pub infinity_type: &'static str,
    pub default: InfinityChunkDefault,
    pub analyzer: InfinityChunkAnalyzer,
    /// Comma-separated logical aliases used by Infinity's SQL rewriter.
    pub comment: Option<&'static str>,
    /// Present only for the two `secondary` indexes in the fixed mapping.
    pub secondary_cardinality: Option<&'static str>,
}

const INFINITY_RAG_ANALYZERS: &[&str] = &["rag-coarse", "rag-fine"];
const INFINITY_WHITESPACE_HASH: InfinityChunkAnalyzer =
    InfinityChunkAnalyzer::Single("whitespace-#");

const fn infinity_chunk_field(
    name: &'static str,
    infinity_type: &'static str,
    default: InfinityChunkDefault,
    analyzer: InfinityChunkAnalyzer,
    comment: Option<&'static str>,
    secondary_cardinality: Option<&'static str>,
) -> InfinityChunkFieldSchema {
    InfinityChunkFieldSchema {
        name,
        infinity_type,
        default,
        analyzer,
        comment,
        secondary_cardinality,
    }
}

const fn infinity_text(name: &'static str) -> InfinityChunkFieldSchema {
    infinity_chunk_field(
        name,
        "varchar",
        InfinityChunkDefault::Text(""),
        InfinityChunkAnalyzer::None,
        None,
        None,
    )
}

const fn infinity_whitespace(name: &'static str) -> InfinityChunkFieldSchema {
    infinity_chunk_field(
        name,
        "varchar",
        InfinityChunkDefault::Text(""),
        INFINITY_WHITESPACE_HASH,
        None,
        None,
    )
}

const fn infinity_integer(name: &'static str, default: i64) -> InfinityChunkFieldSchema {
    infinity_chunk_field(
        name,
        "integer",
        InfinityChunkDefault::Integer(default),
        InfinityChunkAnalyzer::None,
        None,
        None,
    )
}

const fn infinity_float(name: &'static str, default: f64) -> InfinityChunkFieldSchema {
    infinity_chunk_field(
        name,
        "float",
        InfinityChunkDefault::Float(default),
        InfinityChunkAnalyzer::None,
        None,
        None,
    )
}

/// All 73 ordered base columns from the fixed mapping. The dimension-specific
/// vector and optional table-parser `chunk_data` columns are added at runtime.
pub const INFINITY_CHUNK_FIELDS: &[InfinityChunkFieldSchema] = &[
    infinity_text("id"),
    infinity_text("doc_id"),
    infinity_chunk_field(
        "kb_id",
        "varchar",
        InfinityChunkDefault::Text(""),
        InfinityChunkAnalyzer::None,
        None,
        Some("low"),
    ),
    infinity_text("mom_id"),
    infinity_text("mom"),
    infinity_text("create_time"),
    infinity_float("create_timestamp_flt", 0.0),
    infinity_text("img_id"),
    infinity_chunk_field(
        "docnm",
        "varchar",
        InfinityChunkDefault::Text(""),
        InfinityChunkAnalyzer::Multiple(INFINITY_RAG_ANALYZERS),
        Some("docnm_kwd, title_tks, title_sm_tks"),
        None,
    ),
    infinity_whitespace("name_kwd"),
    infinity_whitespace("tag_kwd"),
    infinity_integer("important_kwd_empty_count", 0),
    infinity_chunk_field(
        "important_keywords",
        "varchar",
        InfinityChunkDefault::Text(""),
        InfinityChunkAnalyzer::Multiple(INFINITY_RAG_ANALYZERS),
        Some("important_kwd, important_tks"),
        None,
    ),
    infinity_chunk_field(
        "questions",
        "varchar",
        InfinityChunkDefault::Text(""),
        InfinityChunkAnalyzer::Multiple(INFINITY_RAG_ANALYZERS),
        Some("question_kwd, question_tks"),
        None,
    ),
    infinity_chunk_field(
        "content",
        "varchar",
        InfinityChunkDefault::Text(""),
        InfinityChunkAnalyzer::Multiple(INFINITY_RAG_ANALYZERS),
        Some("content_with_weight, content_ltks, content_sm_ltks"),
        None,
    ),
    infinity_chunk_field(
        "authors",
        "varchar",
        InfinityChunkDefault::Text(""),
        InfinityChunkAnalyzer::Multiple(INFINITY_RAG_ANALYZERS),
        Some("authors_tks, authors_sm_tks"),
        None,
    ),
    infinity_text("page_num_int"),
    infinity_text("top_int"),
    infinity_text("position_int"),
    infinity_integer("weight_int", 0),
    infinity_float("weight_flt", 0.0),
    infinity_integer("chunk_order_int", 0),
    infinity_integer("rank_int", 0),
    infinity_float("rank_flt", 0.0),
    infinity_chunk_field(
        "available_int",
        "integer",
        InfinityChunkDefault::Integer(1),
        InfinityChunkAnalyzer::None,
        None,
        Some("low"),
    ),
    infinity_text("knowledge_graph_kwd"),
    infinity_whitespace("entities_kwd"),
    infinity_integer("pagerank_fea", 0),
    infinity_chunk_field(
        "tag_feas",
        "varchar",
        InfinityChunkDefault::Text(""),
        InfinityChunkAnalyzer::Single("rankfeatures"),
        None,
        None,
    ),
    infinity_whitespace("from_entity_kwd"),
    infinity_whitespace("to_entity_kwd"),
    infinity_whitespace("entity_kwd"),
    infinity_whitespace("entity_type_kwd"),
    infinity_whitespace("source_id"),
    infinity_text("n_hop_with_weight"),
    infinity_text("mom_with_weight"),
    infinity_whitespace("removed_kwd"),
    infinity_whitespace("doc_type_kwd"),
    infinity_whitespace("toc_kwd"),
    infinity_whitespace("raptor_kwd"),
    infinity_integer("raptor_layer_int", 0),
    infinity_text("extra"),
    infinity_whitespace("compile_kwd"),
    infinity_whitespace("source_chunk_ids"),
    infinity_whitespace("source_doc_ids"),
    infinity_whitespace("compilation_template_ids"),
    infinity_whitespace("compilation_template_kind_kwd"),
    infinity_whitespace("chunk_hash_kwd"),
    infinity_whitespace("input_hash_kwd"),
    infinity_whitespace("artifact_slug_kwd"),
    infinity_text("md_with_weight"),
    infinity_text("summary_with_weight"),
    infinity_text("skill_with_weight"),
    infinity_whitespace("skill_kwd"),
    infinity_whitespace("children_kwd"),
    infinity_whitespace("doc_ids_kwd"),
    infinity_whitespace("slug_kwd"),
    infinity_whitespace("title_kwd"),
    infinity_whitespace("page_type_kwd"),
    infinity_whitespace("entity_names_kwd"),
    infinity_whitespace("outlinks_kwd"),
    infinity_whitespace("related_kb_pages_kwd"),
    infinity_whitespace("type_kwd"),
    infinity_whitespace("from_kwd"),
    infinity_whitespace("to_kwd"),
    infinity_whitespace("rechunk_kwd"),
    infinity_text("rechunked_from_template_id"),
    infinity_whitespace("rechunked_from_chunk_ids"),
    infinity_text("superseded_by_chunk_id"),
    infinity_integer("doc_count_int", 0),
    infinity_integer("depth_int", 0),
    infinity_integer("outlinks_int", 0),
    infinity_integer("token_num", 0),
];

pub const INFINITY_CHUNK_VECTOR_FIELD_PREFIX: &str = "q_";
pub const INFINITY_CHUNK_VECTOR_FIELD_SUFFIX: &str = "_vec";
pub const INFINITY_CHUNK_VECTOR_ELEMENT_TYPE: &str = "float";
pub const INFINITY_CHUNK_VECTOR_INDEX_SUFFIX: &str = "_idx";
/// Fixed Python connector index name. Go deliberately uses the dimensioned
/// name returned by [`infinity_chunk_vector_index`].
pub const INFINITY_CHUNK_PYTHON_VECTOR_INDEX: &str = "q_vec_idx";
pub const INFINITY_CHUNK_VECTOR_INDEX_TYPE: &str = "hnsw";
pub const INFINITY_CHUNK_VECTOR_INDEX_M: u8 = 16;
pub const INFINITY_CHUNK_VECTOR_INDEX_EF_CONSTRUCTION: u8 = 50;
pub const INFINITY_CHUNK_VECTOR_INDEX_METRIC: &str = "cosine";
pub const INFINITY_CHUNK_VECTOR_INDEX_ENCODING: &str = "lvq";
pub const INFINITY_CHUNK_DATA_FIELD: &str = "chunk_data";
pub const INFINITY_CHUNK_DATA_TYPE: &str = "json";
pub const INFINITY_CHUNK_DATA_DEFAULT: &str = "{}";

pub fn infinity_chunk_vector_field(dimension: usize) -> String {
    format!("{INFINITY_CHUNK_VECTOR_FIELD_PREFIX}{dimension}{INFINITY_CHUNK_VECTOR_FIELD_SUFFIX}")
}

pub fn infinity_chunk_vector_index(dimension: usize) -> String {
    format!(
        "{}{INFINITY_CHUNK_VECTOR_INDEX_SUFFIX}",
        infinity_chunk_vector_field(dimension)
    )
}

pub fn infinity_chunk_secondary_index(field: &str) -> String {
    format!("sec_{field}")
}

fn infinity_index_fragment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect()
}

pub fn infinity_chunk_fulltext_index(field: &str, analyzer: &str) -> String {
    format!(
        "ft_{}_{}",
        infinity_index_fragment(field),
        infinity_index_fragment(analyzer)
    )
}

/// Resolve a SQL-facing logical alias (for example `content_ltks`) to the
/// physical base column (`content`) declared by the mapping comment.
pub fn infinity_chunk_field_for_alias(alias: &str) -> Option<&'static str> {
    INFINITY_CHUNK_FIELDS.iter().find_map(|field| {
        field.comment.and_then(|comment| {
            comment
                .split(',')
                .map(str::trim)
                .any(|candidate| candidate == alias)
                .then_some(field.name)
        })
    })
}

/// JSON default value used by one fixed Infinity message field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MessageInfinityDefault {
    Text(&'static str),
    Integer(i64),
    Float(f64),
}

/// One exact base column from `conf/message_infinity_mapping.json`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MessageInfinityFieldSchema {
    pub name: &'static str,
    pub infinity_type: &'static str,
    pub default: MessageInfinityDefault,
    pub analyzers: &'static [&'static str],
    pub comment: Option<&'static str>,
}

const NO_ANALYZERS: &[&str] = &[];
const MESSAGE_CONTENT_ANALYZERS: &[&str] = &["rag-coarse", "rag-fine"];

/// Exact 17-column base schema from the fixed-version message mapping.
/// Infinity adds the dimension-specific vector column at table creation.
pub const MESSAGE_INFINITY_FIELDS: &[MessageInfinityFieldSchema] = &[
    MessageInfinityFieldSchema {
        name: "id",
        infinity_type: "varchar",
        default: MessageInfinityDefault::Text(""),
        analyzers: NO_ANALYZERS,
        comment: None,
    },
    MessageInfinityFieldSchema {
        name: "message_id",
        infinity_type: "integer",
        default: MessageInfinityDefault::Integer(0),
        analyzers: NO_ANALYZERS,
        comment: None,
    },
    MessageInfinityFieldSchema {
        name: "message_type_kwd",
        infinity_type: "varchar",
        default: MessageInfinityDefault::Text(""),
        analyzers: NO_ANALYZERS,
        comment: None,
    },
    MessageInfinityFieldSchema {
        name: "source_id",
        infinity_type: "integer",
        default: MessageInfinityDefault::Integer(0),
        analyzers: NO_ANALYZERS,
        comment: None,
    },
    MessageInfinityFieldSchema {
        name: "memory_id",
        infinity_type: "varchar",
        default: MessageInfinityDefault::Text(""),
        analyzers: NO_ANALYZERS,
        comment: None,
    },
    MessageInfinityFieldSchema {
        name: "user_id",
        infinity_type: "varchar",
        default: MessageInfinityDefault::Text(""),
        analyzers: NO_ANALYZERS,
        comment: None,
    },
    MessageInfinityFieldSchema {
        name: "agent_id",
        infinity_type: "varchar",
        default: MessageInfinityDefault::Text(""),
        analyzers: NO_ANALYZERS,
        comment: None,
    },
    MessageInfinityFieldSchema {
        name: "session_id",
        infinity_type: "varchar",
        default: MessageInfinityDefault::Text(""),
        analyzers: NO_ANALYZERS,
        comment: None,
    },
    MessageInfinityFieldSchema {
        name: "valid_at",
        infinity_type: "varchar",
        default: MessageInfinityDefault::Text(""),
        analyzers: NO_ANALYZERS,
        comment: None,
    },
    MessageInfinityFieldSchema {
        name: "valid_at_flt",
        infinity_type: "float",
        default: MessageInfinityDefault::Float(0.0),
        analyzers: NO_ANALYZERS,
        comment: None,
    },
    MessageInfinityFieldSchema {
        name: "invalid_at",
        infinity_type: "varchar",
        default: MessageInfinityDefault::Text(""),
        analyzers: NO_ANALYZERS,
        comment: None,
    },
    MessageInfinityFieldSchema {
        name: "invalid_at_flt",
        infinity_type: "float",
        default: MessageInfinityDefault::Float(0.0),
        analyzers: NO_ANALYZERS,
        comment: None,
    },
    MessageInfinityFieldSchema {
        name: "forget_at",
        infinity_type: "varchar",
        default: MessageInfinityDefault::Text(""),
        analyzers: NO_ANALYZERS,
        comment: None,
    },
    MessageInfinityFieldSchema {
        name: "forget_at_flt",
        infinity_type: "float",
        default: MessageInfinityDefault::Float(0.0),
        analyzers: NO_ANALYZERS,
        comment: None,
    },
    MessageInfinityFieldSchema {
        name: "status_int",
        infinity_type: "integer",
        default: MessageInfinityDefault::Integer(1),
        analyzers: NO_ANALYZERS,
        comment: None,
    },
    MessageInfinityFieldSchema {
        name: "zone_id",
        infinity_type: "integer",
        default: MessageInfinityDefault::Integer(0),
        analyzers: NO_ANALYZERS,
        comment: None,
    },
    MessageInfinityFieldSchema {
        name: "content",
        infinity_type: "varchar",
        default: MessageInfinityDefault::Text(""),
        analyzers: MESSAGE_CONTENT_ANALYZERS,
        comment: Some("content_ltks"),
    },
];

pub const MESSAGE_INFINITY_VECTOR_FIELD_PREFIX: &str = "q_";
pub const MESSAGE_INFINITY_VECTOR_FIELD_SUFFIX: &str = "_vec";
pub const MESSAGE_INFINITY_VECTOR_ELEMENT_TYPE: &str = "float";
pub const MESSAGE_INFINITY_VECTOR_INDEX_NAME: &str = "q_vec_idx";
pub const MESSAGE_INFINITY_VECTOR_INDEX_TYPE: &str = "hnsw";
pub const MESSAGE_INFINITY_VECTOR_INDEX_M: u8 = 16;
pub const MESSAGE_INFINITY_VECTOR_INDEX_EF_CONSTRUCTION: u8 = 50;
pub const MESSAGE_INFINITY_VECTOR_INDEX_METRIC: &str = "cosine";
pub const MESSAGE_INFINITY_VECTOR_INDEX_ENCODING: &str = "lvq";

/// One explicit property kind from the fixed `conf/skill_es_mapping.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillEsFieldType {
    Keyword,
    Text,
    DenseVector(usize),
    Long,
}

impl SkillEsFieldType {
    fn es_name(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Text => "text",
            Self::DenseVector(_) => "dense_vector",
            Self::Long => "long",
        }
    }
}

/// One exact property declaration from the fixed Skill ES mapping. `None`
/// means the key is omitted so Elasticsearch applies its native default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillEsFieldSchema {
    pub name: &'static str,
    pub field_type: SkillEsFieldType,
    pub indexed: Option<bool>,
    pub stored: Option<bool>,
    pub analyzer: Option<&'static str>,
    pub similarity: Option<&'static str>,
}

pub const SKILL_ES_NUMBER_OF_SHARDS: u8 = 1;
pub const SKILL_ES_NUMBER_OF_REPLICAS: u8 = 0;
pub const SKILL_ES_REFRESH_INTERVAL: &str = "1000ms";
pub const SKILL_ES_DYNAMIC: bool = false;
pub const SKILL_ES_TOKEN_ANALYZER: &str = "whitespace";
pub const SKILL_ES_TOKEN_SIMILARITY: &str = "scripted_sim";
pub const SKILL_ES_SIMILARITY_TYPE: &str = "scripted";
pub const SKILL_ES_SIMILARITY_SCRIPT: &str = "double idf = Math.log(1+(field.docCount-term.docFreq+0.5)/(term.docFreq + 0.5))/Math.log(1+((field.docCount-0.5)/1.5)); return query.boost * idf * Math.min(doc.freq, 1);";
pub const SKILL_ES_VECTOR_SIMILARITY: &str = "cosine";
pub const SKILL_ES_VECTOR_DIMS: &[usize] = &[3072, 2560, 1536, 1024, 768, 512, 256];

const fn skill_es_field(
    name: &'static str,
    field_type: SkillEsFieldType,
    indexed: Option<bool>,
    stored: Option<bool>,
    analyzer: Option<&'static str>,
    similarity: Option<&'static str>,
) -> SkillEsFieldSchema {
    SkillEsFieldSchema {
        name,
        field_type,
        indexed,
        stored,
        analyzer,
        similarity,
    }
}

/// Exact ordered 22-property schema from `conf/skill_es_mapping.json`.
pub const SKILL_ES_FIELDS: &[SkillEsFieldSchema] = &[
    skill_es_field(
        "skill_id",
        SkillEsFieldType::Keyword,
        None,
        Some(true),
        None,
        None,
    ),
    skill_es_field(
        "space_id",
        SkillEsFieldType::Keyword,
        None,
        Some(true),
        None,
        None,
    ),
    skill_es_field(
        "folder_id",
        SkillEsFieldType::Keyword,
        None,
        Some(true),
        None,
        None,
    ),
    skill_es_field(
        "name",
        SkillEsFieldType::Text,
        Some(false),
        Some(true),
        None,
        None,
    ),
    skill_es_field(
        "name_tks",
        SkillEsFieldType::Text,
        None,
        Some(true),
        Some(SKILL_ES_TOKEN_ANALYZER),
        Some(SKILL_ES_TOKEN_SIMILARITY),
    ),
    skill_es_field(
        "tags",
        SkillEsFieldType::Text,
        Some(false),
        Some(true),
        None,
        None,
    ),
    skill_es_field(
        "tags_tks",
        SkillEsFieldType::Text,
        None,
        Some(true),
        Some(SKILL_ES_TOKEN_ANALYZER),
        Some(SKILL_ES_TOKEN_SIMILARITY),
    ),
    skill_es_field(
        "description",
        SkillEsFieldType::Text,
        Some(false),
        Some(true),
        None,
        None,
    ),
    skill_es_field(
        "description_tks",
        SkillEsFieldType::Text,
        None,
        Some(true),
        Some(SKILL_ES_TOKEN_ANALYZER),
        Some(SKILL_ES_TOKEN_SIMILARITY),
    ),
    skill_es_field(
        "content",
        SkillEsFieldType::Text,
        Some(false),
        Some(true),
        None,
        None,
    ),
    skill_es_field(
        "content_tks",
        SkillEsFieldType::Text,
        None,
        Some(true),
        Some(SKILL_ES_TOKEN_ANALYZER),
        Some(SKILL_ES_TOKEN_SIMILARITY),
    ),
    skill_es_field(
        "q_3072_vec",
        SkillEsFieldType::DenseVector(3072),
        Some(true),
        None,
        None,
        Some(SKILL_ES_VECTOR_SIMILARITY),
    ),
    skill_es_field(
        "q_2560_vec",
        SkillEsFieldType::DenseVector(2560),
        Some(true),
        None,
        None,
        Some(SKILL_ES_VECTOR_SIMILARITY),
    ),
    skill_es_field(
        "q_1536_vec",
        SkillEsFieldType::DenseVector(1536),
        Some(true),
        None,
        None,
        Some(SKILL_ES_VECTOR_SIMILARITY),
    ),
    skill_es_field(
        "q_1024_vec",
        SkillEsFieldType::DenseVector(1024),
        Some(true),
        None,
        None,
        Some(SKILL_ES_VECTOR_SIMILARITY),
    ),
    skill_es_field(
        "q_768_vec",
        SkillEsFieldType::DenseVector(768),
        Some(true),
        None,
        None,
        Some(SKILL_ES_VECTOR_SIMILARITY),
    ),
    skill_es_field(
        "q_512_vec",
        SkillEsFieldType::DenseVector(512),
        Some(true),
        None,
        None,
        Some(SKILL_ES_VECTOR_SIMILARITY),
    ),
    skill_es_field(
        "q_256_vec",
        SkillEsFieldType::DenseVector(256),
        Some(true),
        None,
        None,
        Some(SKILL_ES_VECTOR_SIMILARITY),
    ),
    skill_es_field(
        "version",
        SkillEsFieldType::Keyword,
        None,
        Some(true),
        None,
        None,
    ),
    skill_es_field(
        "status",
        SkillEsFieldType::Keyword,
        None,
        Some(true),
        None,
        None,
    ),
    skill_es_field(
        "create_time",
        SkillEsFieldType::Long,
        None,
        Some(true),
        None,
        None,
    ),
    skill_es_field(
        "update_time",
        SkillEsFieldType::Long,
        None,
        Some(true),
        None,
        None,
    ),
];

/// Materialize the exact fixed Skill ES mapping. RayRAG does not deploy ES;
/// this value is the audited schema contract used by the Rust replacement.
pub fn skill_es_mapping() -> Value {
    let mut properties = serde_json::Map::new();
    for field in SKILL_ES_FIELDS {
        let mut property = serde_json::Map::new();
        property.insert(
            "type".into(),
            Value::String(field.field_type.es_name().into()),
        );
        if let SkillEsFieldType::DenseVector(dims) = field.field_type {
            property.insert("dims".into(), Value::from(dims));
        }
        if let Some(indexed) = field.indexed {
            property.insert("index".into(), Value::Bool(indexed));
        }
        if let Some(stored) = field.stored {
            property.insert("store".into(), Value::Bool(stored));
        }
        if let Some(analyzer) = field.analyzer {
            property.insert("analyzer".into(), Value::String(analyzer.into()));
        }
        if let Some(similarity) = field.similarity {
            property.insert("similarity".into(), Value::String(similarity.into()));
        }
        properties.insert(field.name.into(), Value::Object(property));
    }
    serde_json::json!({
        "settings": {
            "index": {
                "number_of_shards": SKILL_ES_NUMBER_OF_SHARDS,
                "number_of_replicas": SKILL_ES_NUMBER_OF_REPLICAS,
                "refresh_interval": SKILL_ES_REFRESH_INTERVAL,
            },
            "similarity": {
                SKILL_ES_TOKEN_SIMILARITY: {
                    "type": SKILL_ES_SIMILARITY_TYPE,
                    "script": { "source": SKILL_ES_SIMILARITY_SCRIPT },
                },
            },
        },
        "mappings": {
            "dynamic": SKILL_ES_DYNAMIC,
            "properties": properties,
        },
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SkillInfinityDefault {
    Text(&'static str),
    BigInt(i64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkillInfinityFieldSchema {
    pub name: &'static str,
    pub infinity_type: &'static str,
    pub default: SkillInfinityDefault,
    pub analyzers: &'static [&'static str],
    pub index_type: Option<&'static str>,
}

pub const SKILL_INFINITY_ANALYZERS: &[&str] = &["rag-coarse", "rag-fine"];

/// Exact ordered columns from `conf/skill_infinity_mapping.json`.
pub const SKILL_INFINITY_FIELDS: &[SkillInfinityFieldSchema] = &[
    SkillInfinityFieldSchema {
        name: "skill_id",
        infinity_type: "varchar",
        default: SkillInfinityDefault::Text(""),
        analyzers: NO_ANALYZERS,
        index_type: Some("secondary"),
    },
    SkillInfinityFieldSchema {
        name: "space_id",
        infinity_type: "varchar",
        default: SkillInfinityDefault::Text(""),
        analyzers: NO_ANALYZERS,
        index_type: Some("secondary"),
    },
    SkillInfinityFieldSchema {
        name: "folder_id",
        infinity_type: "varchar",
        default: SkillInfinityDefault::Text(""),
        analyzers: NO_ANALYZERS,
        index_type: None,
    },
    SkillInfinityFieldSchema {
        name: "name",
        infinity_type: "varchar",
        default: SkillInfinityDefault::Text(""),
        analyzers: SKILL_INFINITY_ANALYZERS,
        index_type: None,
    },
    SkillInfinityFieldSchema {
        name: "tags",
        infinity_type: "varchar",
        default: SkillInfinityDefault::Text(""),
        analyzers: SKILL_INFINITY_ANALYZERS,
        index_type: None,
    },
    SkillInfinityFieldSchema {
        name: "description",
        infinity_type: "varchar",
        default: SkillInfinityDefault::Text(""),
        analyzers: SKILL_INFINITY_ANALYZERS,
        index_type: None,
    },
    SkillInfinityFieldSchema {
        name: "content",
        infinity_type: "varchar",
        default: SkillInfinityDefault::Text(""),
        analyzers: SKILL_INFINITY_ANALYZERS,
        index_type: None,
    },
    SkillInfinityFieldSchema {
        name: "version",
        infinity_type: "varchar",
        default: SkillInfinityDefault::Text("1.0.0"),
        analyzers: NO_ANALYZERS,
        index_type: None,
    },
    SkillInfinityFieldSchema {
        name: "status",
        infinity_type: "varchar",
        default: SkillInfinityDefault::Text("1"),
        analyzers: NO_ANALYZERS,
        index_type: None,
    },
    SkillInfinityFieldSchema {
        name: "create_time",
        infinity_type: "bigint",
        default: SkillInfinityDefault::BigInt(0),
        analyzers: NO_ANALYZERS,
        index_type: None,
    },
    SkillInfinityFieldSchema {
        name: "update_time",
        infinity_type: "bigint",
        default: SkillInfinityDefault::BigInt(0),
        analyzers: NO_ANALYZERS,
        index_type: None,
    },
];

pub const SKILL_INFINITY_VECTOR_FIELD_PREFIX: &str = "q_";
pub const SKILL_INFINITY_VECTOR_FIELD_SUFFIX: &str = "_vec";
pub const SKILL_INFINITY_VECTOR_ELEMENT_TYPE: &str = "float";
pub const SKILL_INFINITY_VECTOR_INDEX_SUFFIX: &str = "_idx";
pub const SKILL_INFINITY_VECTOR_INDEX_TYPE: &str = "hnsw";
pub const SKILL_INFINITY_VECTOR_INDEX_M: u8 = 16;
pub const SKILL_INFINITY_VECTOR_INDEX_EF_CONSTRUCTION: u8 = 50;
pub const SKILL_INFINITY_VECTOR_INDEX_METRIC: &str = "cosine";
pub const SKILL_INFINITY_VECTOR_INDEX_ENCODING: &str = "lvq";
pub const SKILL_INFINITY_SECONDARY_INDEX_PREFIX: &str = "sec_";
pub const SKILL_INFINITY_FULLTEXT_INDEX_PREFIX: &str = "ft_";

pub fn skill_infinity_vector_field(dimension: usize) -> String {
    format!("{SKILL_INFINITY_VECTOR_FIELD_PREFIX}{dimension}{SKILL_INFINITY_VECTOR_FIELD_SUFFIX}")
}

pub fn skill_infinity_vector_index_name(dimension: usize) -> String {
    format!(
        "{}{SKILL_INFINITY_VECTOR_INDEX_SUFFIX}",
        skill_infinity_vector_field(dimension)
    )
}

pub fn skill_infinity_secondary_index_name(field: &str) -> String {
    format!("{SKILL_INFINITY_SECONDARY_INDEX_PREFIX}{field}")
}

pub fn skill_infinity_fulltext_index_name(field: &str, analyzer: &str) -> String {
    format!(
        "{SKILL_INFINITY_FULLTEXT_INDEX_PREFIX}{field}_{}",
        analyzer.replace('-', "_")
    )
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
        assert_eq!(init_secret_key(None, Some(&padded)), Some(padded.clone()));
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
        assert_eq!(
            resolved.base_url.as_deref(),
            Some("https://ark.cn-beijing.volces.com")
        );

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

    #[test]
    fn service_conf_defaults_mirror_shipped_yaml() {
        let conf = ServiceConf::default();
        assert_eq!(conf.ragflow_host, "0.0.0.0");
        assert_eq!(conf.http_port, 9380);
        assert_eq!(conf.admin_port, 9381);
        assert_eq!(conf.mysql.name, "rag_flow");
        assert_eq!(conf.mysql.port, 3306);
        assert_eq!(conf.mysql.max_connections, 900);
        assert_eq!(conf.minio.host, "localhost:9000");
        assert_eq!(conf.es.hosts, "http://localhost:1200");
        assert_eq!(conf.os.hosts, "http://localhost:1201");
        assert_eq!(conf.infinity.uri, "localhost:23817");
        assert_eq!(conf.infinity.postgres_port, 5432);
        assert_eq!(conf.redis.db, 1);
        assert_eq!(conf.redis.host, "localhost:6379");
        assert_eq!(conf.message_queue_type, "redis");
    }

    #[test]
    fn service_conf_parses_document_and_applies_user_default_llm() {
        // JSON shape of conf/service_conf.yaml (YAML ⊃ JSON).
        let document = serde_json::json!({
            "ragflow": {"host": "127.0.0.1", "http_port": 9390},
            "admin": {"host": "127.0.0.1", "http_port": 9391},
            "mysql": {"name": "rag_flow", "user": "root", "password": "p",
                      "host": "db.internal", "port": 3307, "max_connections": 100,
                      "stale_timeout": 30},
            "minio": {"user": "rag_flow", "password": "p", "host": "minio:9000",
                      "bucket": "rag", "prefix_path": "kb"},
            "es": {"hosts": "http://es:1200", "username": "elastic", "password": "p"},
            "os": {"hosts": "http://os:1201", "username": "admin", "password": "p"},
            "infinity": {"uri": "infinity:23817", "postgres_port": 5433, "db_name": "kb"},
            "redis": {"db": 2, "username": "", "password": "p", "host": "redis:6379"},
            "task_executor": {"message_queue_type": "redis"},
            "file_syncer": {"max_concurrent_syncs": 9, "sync_interval": 7},
            "user_default_llm": {
                "factory": "ZHIPU-AI",
                "api_key": "key",
                "base_url": "https://open.bigmodel.cn/api/paas/v4",
                "default_models": {
                    "embedding_model": {"name": "embedding-3", "factory": "ZHIPU-AI",
                                        "api_key": "key",
                                        "base_url": "https://open.bigmodel.cn/api/paas/v4"}
                }
            }
        });
        let conf = ServiceConf::from_value(&document);
        assert_eq!(conf.ragflow_host, "127.0.0.1");
        assert_eq!(conf.http_port, 9390);
        assert_eq!(conf.admin_port, 9391);
        assert_eq!(conf.mysql.host, "db.internal");
        assert_eq!(conf.mysql.port, 3307);
        assert_eq!(conf.mysql.max_connections, 100);
        assert_eq!(conf.minio.bucket, "rag");
        assert_eq!(conf.es.hosts, "http://es:1200");
        assert_eq!(conf.redis.db, 2);
        assert_eq!(conf.user_default_llm.factory, "ZHIPU-AI");
        // file_syncer keys parse (legacy dead-config parity).
        assert_eq!(conf.file_syncer.max_concurrent_syncs, 9);
        assert_eq!(conf.file_syncer.sync_interval, 7);
        // Missing keys keep the shipped defaults.
        let partial = ServiceConf::from_value(&serde_json::json!({"ragflow": {"http_port": 9400}}));
        assert_eq!(partial.http_port, 9400);
        assert_eq!(partial.admin_port, 9381);
        assert_eq!(partial.mysql.name, "rag_flow");
        assert_eq!(partial.file_syncer.max_concurrent_syncs, 4);

        // Apply onto a settings snapshot: fills the LLM defaults that the env
        // reader intentionally leaves empty, composing name@factory.
        let mut settings = Settings::from_reader(|_| None);
        assert!(settings.llm_factory.is_empty());
        conf.apply_to_settings(&mut settings);
        assert_eq!(settings.llm_factory, "ZHIPU-AI");
        assert_eq!(
            settings.llm_base_url,
            "https://open.bigmodel.cn/api/paas/v4"
        );
        let embedding = settings.default_models.get("embedding_model").unwrap();
        assert_eq!(embedding.name, "embedding-3@ZHIPU-AI");
        assert_eq!(embedding.factory.as_deref(), Some("ZHIPU-AI"));
        assert_eq!(embedding.api_key.as_deref(), Some("key"));
    }

    #[test]
    fn es_field_kind_resolves_suffix_and_regex_templates() {
        // Suffix rules (conf/mapping.json / conf/os_mapping.json).
        assert_eq!(es_field_kind("weight_int"), FieldKind::Int);
        assert_eq!(es_field_kind("rank_flt"), FieldKind::Float);
        assert_eq!(es_field_kind("content_ltks"), FieldKind::LongTokens);
        assert_eq!(es_field_kind("content_tks"), FieldKind::Tokens);
        assert_eq!(
            es_field_kind("content_with_weight"),
            FieldKind::UnindexedText
        );
        assert_eq!(es_field_kind("tag_list"), FieldKind::UnindexedText);
        assert_eq!(es_field_kind("pagerank_fea"), FieldKind::RankFeature);
        assert_eq!(es_field_kind("tag_feas"), FieldKind::RankFeatures);
        assert_eq!(es_field_kind("q_256_vec"), FieldKind::Text);
        assert_eq!(es_field_kind("q_768_vec"), FieldKind::DenseVector(768));
        assert_eq!(es_field_kind("q_3072_vec"), FieldKind::Text);
        assert_eq!(es_field_kind("q_10240_vec"), FieldKind::Text);
        assert_eq!(
            opensearch_field_kind("q_10240_vec"),
            FieldKind::DenseVector(10240)
        );
        assert_eq!(es_field_kind("blob_bin"), FieldKind::Binary);
        // Regex templates: *_(kwd|id|ids|uid|uids)|uid|id → keyword.
        assert_eq!(es_field_kind("docnm_kwd"), FieldKind::Keyword);
        assert_eq!(es_field_kind("id"), FieldKind::Keyword);
        assert_eq!(opensearch_field_kind("id"), FieldKind::Text);
        assert_eq!(es_field_kind("kb_id"), FieldKind::Keyword);
        assert_eq!(es_field_kind("user_id"), FieldKind::Keyword);
        // Regex templates: *(_dt|_time|_at) → date.
        assert_eq!(es_field_kind("create_time"), FieldKind::DateTime);
        assert_eq!(es_field_kind("update_at"), FieldKind::DateTime);
        assert_eq!(es_field_kind("created_dt"), FieldKind::DateTime);
        // Unmatched → dynamic text.
        assert_eq!(es_field_kind("title"), FieldKind::Text);
        // Postgres/zvec column mapping.
        assert_eq!(zvec_column_kind("weight_int"), "INTEGER");
        assert_eq!(zvec_column_kind("rank_flt"), "REAL");
        assert_eq!(zvec_column_kind("q_768_vec"), "VECTOR(768)");
        assert_eq!(zvec_column_kind("content_tks"), "TEXT");
        assert_eq!(zvec_column_kind("blob_bin"), "BYTEA");
        // All 73 ordered base fields from conf/infinity_mapping.json.
        assert_eq!(
            INFINITY_CHUNK_FIELDS
                .iter()
                .map(|field| field.name)
                .collect::<Vec<_>>(),
            vec![
                "id",
                "doc_id",
                "kb_id",
                "mom_id",
                "mom",
                "create_time",
                "create_timestamp_flt",
                "img_id",
                "docnm",
                "name_kwd",
                "tag_kwd",
                "important_kwd_empty_count",
                "important_keywords",
                "questions",
                "content",
                "authors",
                "page_num_int",
                "top_int",
                "position_int",
                "weight_int",
                "weight_flt",
                "chunk_order_int",
                "rank_int",
                "rank_flt",
                "available_int",
                "knowledge_graph_kwd",
                "entities_kwd",
                "pagerank_fea",
                "tag_feas",
                "from_entity_kwd",
                "to_entity_kwd",
                "entity_kwd",
                "entity_type_kwd",
                "source_id",
                "n_hop_with_weight",
                "mom_with_weight",
                "removed_kwd",
                "doc_type_kwd",
                "toc_kwd",
                "raptor_kwd",
                "raptor_layer_int",
                "extra",
                "compile_kwd",
                "source_chunk_ids",
                "source_doc_ids",
                "compilation_template_ids",
                "compilation_template_kind_kwd",
                "chunk_hash_kwd",
                "input_hash_kwd",
                "artifact_slug_kwd",
                "md_with_weight",
                "summary_with_weight",
                "skill_with_weight",
                "skill_kwd",
                "children_kwd",
                "doc_ids_kwd",
                "slug_kwd",
                "title_kwd",
                "page_type_kwd",
                "entity_names_kwd",
                "outlinks_kwd",
                "related_kb_pages_kwd",
                "type_kwd",
                "from_kwd",
                "to_kwd",
                "rechunk_kwd",
                "rechunked_from_template_id",
                "rechunked_from_chunk_ids",
                "superseded_by_chunk_id",
                "doc_count_int",
                "depth_int",
                "outlinks_int",
                "token_num",
            ]
        );
        assert_eq!(INFINITY_CHUNK_UPSTREAM_BYTES, 5_413);
        assert_eq!(INFINITY_CHUNK_UPSTREAM_LINES, 76);
        assert_eq!(INFINITY_CHUNK_UPSTREAM_BLOB.len(), 40);
        assert_eq!(INFINITY_CHUNK_FIELDS.len(), 73);
        assert_eq!(
            INFINITY_CHUNK_FIELDS
                .iter()
                .filter(|field| field.infinity_type == "varchar")
                .count(),
            59
        );
        assert_eq!(
            INFINITY_CHUNK_FIELDS
                .iter()
                .filter(|field| field.infinity_type == "integer")
                .count(),
            11
        );
        assert_eq!(
            INFINITY_CHUNK_FIELDS
                .iter()
                .filter(|field| field.infinity_type == "float")
                .count(),
            3
        );
        let fulltext_indexes = INFINITY_CHUNK_FIELDS
            .iter()
            .map(|field| match field.analyzer {
                InfinityChunkAnalyzer::None => 0,
                InfinityChunkAnalyzer::Single(_) => 1,
                InfinityChunkAnalyzer::Multiple(analyzers) => analyzers.len(),
            })
            .sum::<usize>();
        assert_eq!(fulltext_indexes, 45);
        assert_eq!(
            INFINITY_CHUNK_FIELDS
                .iter()
                .filter(|field| field.secondary_cardinality.is_some())
                .count(),
            2
        );
        assert_eq!(
            INFINITY_CHUNK_FIELDS
                .iter()
                .filter(|field| field.comment.is_some())
                .count(),
            5
        );
        assert_eq!(INFINITY_CHUNK_FIELDS[2].secondary_cardinality, Some("low"));
        assert_eq!(
            INFINITY_CHUNK_FIELDS[24].default,
            InfinityChunkDefault::Integer(1)
        );
        assert_eq!(INFINITY_CHUNK_FIELDS[24].secondary_cardinality, Some("low"));
        assert_eq!(
            INFINITY_CHUNK_FIELDS[8].analyzer,
            InfinityChunkAnalyzer::Multiple(INFINITY_RAG_ANALYZERS)
        );
        assert_eq!(
            INFINITY_CHUNK_FIELDS[9].analyzer,
            InfinityChunkAnalyzer::Single("whitespace-#")
        );
        assert_eq!(
            INFINITY_CHUNK_FIELDS[28].analyzer,
            InfinityChunkAnalyzer::Single("rankfeatures")
        );
        assert_eq!(
            infinity_chunk_field_for_alias("content_ltks"),
            Some("content")
        );
        assert_eq!(
            infinity_chunk_field_for_alias("title_sm_tks"),
            Some("docnm")
        );
        assert_eq!(infinity_chunk_field_for_alias("missing"), None);
        assert_eq!(infinity_chunk_vector_field(1024), "q_1024_vec");
        assert_eq!(infinity_chunk_vector_index(1024), "q_1024_vec_idx");
        assert_eq!(INFINITY_CHUNK_PYTHON_VECTOR_INDEX, "q_vec_idx");
        assert_eq!(infinity_chunk_secondary_index("kb_id"), "sec_kb_id");
        assert_eq!(
            infinity_chunk_fulltext_index("content", "whitespace-#"),
            "ft_content_whitespace__"
        );
        assert_eq!(INFINITY_CHUNK_VECTOR_INDEX_TYPE, "hnsw");
        assert_eq!(INFINITY_CHUNK_VECTOR_INDEX_M, 16);
        assert_eq!(INFINITY_CHUNK_VECTOR_INDEX_EF_CONSTRUCTION, 50);
        assert_eq!(INFINITY_CHUNK_VECTOR_INDEX_METRIC, "cosine");
        assert_eq!(INFINITY_CHUNK_VECTOR_INDEX_ENCODING, "lvq");
        assert_eq!(INFINITY_CHUNK_DATA_FIELD, "chunk_data");
        assert_eq!(INFINITY_CHUNK_DATA_TYPE, "json");
        assert_eq!(INFINITY_CHUNK_DATA_DEFAULT, "{}");
        assert_eq!(MESSAGE_INFINITY_FIELDS.len(), 17);
        assert_eq!(MESSAGE_INFINITY_FIELDS[0].name, "id");
        assert_eq!(MESSAGE_INFINITY_FIELDS[0].infinity_type, "varchar");
        assert_eq!(
            MESSAGE_INFINITY_FIELDS[0].default,
            MessageInfinityDefault::Text("")
        );
        assert_eq!(MESSAGE_INFINITY_FIELDS[9].name, "valid_at_flt");
        assert_eq!(
            MESSAGE_INFINITY_FIELDS[9].default,
            MessageInfinityDefault::Float(0.0)
        );
        assert_eq!(MESSAGE_INFINITY_FIELDS[14].name, "status_int");
        assert_eq!(
            MESSAGE_INFINITY_FIELDS[14].default,
            MessageInfinityDefault::Integer(1)
        );
        assert_eq!(
            MESSAGE_INFINITY_FIELDS[16].analyzers,
            ["rag-coarse", "rag-fine"]
        );
        assert_eq!(MESSAGE_INFINITY_FIELDS[16].comment, Some("content_ltks"));
        assert_eq!(MESSAGE_INFINITY_VECTOR_FIELD_PREFIX, "q_");
        assert_eq!(MESSAGE_INFINITY_VECTOR_FIELD_SUFFIX, "_vec");
        assert_eq!(MESSAGE_INFINITY_VECTOR_ELEMENT_TYPE, "float");
        assert_eq!(MESSAGE_INFINITY_VECTOR_INDEX_NAME, "q_vec_idx");
        assert_eq!(MESSAGE_INFINITY_VECTOR_INDEX_TYPE, "hnsw");
        assert_eq!(MESSAGE_INFINITY_VECTOR_INDEX_M, 16);
        assert_eq!(MESSAGE_INFINITY_VECTOR_INDEX_EF_CONSTRUCTION, 50);
        assert_eq!(MESSAGE_INFINITY_VECTOR_INDEX_METRIC, "cosine");
        assert_eq!(MESSAGE_INFINITY_VECTOR_INDEX_ENCODING, "lvq");
        let skill_es = skill_es_mapping();
        assert_eq!(skill_es["settings"]["index"]["number_of_shards"], 1);
        assert_eq!(skill_es["settings"]["index"]["number_of_replicas"], 0);
        assert_eq!(skill_es["settings"]["index"]["refresh_interval"], "1000ms");
        assert_eq!(
            skill_es["settings"]["similarity"]["scripted_sim"]["type"],
            "scripted"
        );
        assert_eq!(
            skill_es["settings"]["similarity"]["scripted_sim"]["script"]["source"],
            SKILL_ES_SIMILARITY_SCRIPT
        );
        assert_eq!(skill_es["mappings"]["dynamic"], false);
        let skill_es_properties = skill_es["mappings"]["properties"].as_object().unwrap();
        assert_eq!(skill_es_properties.len(), 22);
        assert_eq!(SKILL_ES_FIELDS.len(), 22);
        assert_eq!(
            skill_es_properties
                .keys()
                .map(String::as_str)
                .collect::<std::collections::HashSet<_>>(),
            SKILL_ES_FIELDS
                .iter()
                .map(|field| field.name)
                .collect::<std::collections::HashSet<_>>()
        );
        for field in ["name", "tags", "description", "content"] {
            assert_eq!(skill_es_properties[field]["type"], "text");
            assert_eq!(skill_es_properties[field]["index"], false);
            assert_eq!(skill_es_properties[field]["store"], true);
            let token_field = format!("{field}_tks");
            assert_eq!(skill_es_properties[&token_field]["analyzer"], "whitespace");
            assert_eq!(
                skill_es_properties[&token_field]["similarity"],
                "scripted_sim"
            );
            assert_eq!(skill_es_properties[&token_field]["store"], true);
        }
        assert_eq!(
            SKILL_ES_VECTOR_DIMS,
            [3072, 2560, 1536, 1024, 768, 512, 256]
        );
        for dims in SKILL_ES_VECTOR_DIMS {
            let vector_field = format!("q_{dims}_vec");
            assert_eq!(skill_es_properties[&vector_field]["type"], "dense_vector");
            assert_eq!(skill_es_properties[&vector_field]["dims"], *dims);
            assert_eq!(skill_es_properties[&vector_field]["index"], true);
            assert_eq!(skill_es_properties[&vector_field]["similarity"], "cosine");
            assert!(skill_es_properties[&vector_field].get("store").is_none());
        }
        assert_eq!(SKILL_INFINITY_FIELDS.len(), 11);
        assert_eq!(SKILL_INFINITY_FIELDS[0].name, "skill_id");
        assert_eq!(SKILL_INFINITY_FIELDS[0].infinity_type, "varchar");
        assert_eq!(
            SKILL_INFINITY_FIELDS[0].default,
            SkillInfinityDefault::Text("")
        );
        assert_eq!(SKILL_INFINITY_FIELDS[0].index_type, Some("secondary"));
        assert_eq!(SKILL_INFINITY_FIELDS[1].name, "space_id");
        assert_eq!(SKILL_INFINITY_FIELDS[1].index_type, Some("secondary"));
        for field in &SKILL_INFINITY_FIELDS[3..=6] {
            assert_eq!(field.analyzers, ["rag-coarse", "rag-fine"]);
        }
        assert_eq!(SKILL_INFINITY_FIELDS[7].name, "version");
        assert_eq!(
            SKILL_INFINITY_FIELDS[7].default,
            SkillInfinityDefault::Text("1.0.0")
        );
        assert_eq!(SKILL_INFINITY_FIELDS[8].name, "status");
        assert_eq!(
            SKILL_INFINITY_FIELDS[8].default,
            SkillInfinityDefault::Text("1")
        );
        assert_eq!(SKILL_INFINITY_FIELDS[9].infinity_type, "bigint");
        assert_eq!(
            SKILL_INFINITY_FIELDS[9].default,
            SkillInfinityDefault::BigInt(0)
        );
        assert_eq!(SKILL_INFINITY_VECTOR_FIELD_PREFIX, "q_");
        assert_eq!(SKILL_INFINITY_VECTOR_FIELD_SUFFIX, "_vec");
        assert_eq!(SKILL_INFINITY_VECTOR_ELEMENT_TYPE, "float");
        assert_eq!(SKILL_INFINITY_VECTOR_INDEX_SUFFIX, "_idx");
        assert_eq!(SKILL_INFINITY_VECTOR_INDEX_TYPE, "hnsw");
        assert_eq!(SKILL_INFINITY_VECTOR_INDEX_M, 16);
        assert_eq!(SKILL_INFINITY_VECTOR_INDEX_EF_CONSTRUCTION, 50);
        assert_eq!(SKILL_INFINITY_VECTOR_INDEX_METRIC, "cosine");
        assert_eq!(SKILL_INFINITY_VECTOR_INDEX_ENCODING, "lvq");
        assert_eq!(SKILL_INFINITY_SECONDARY_INDEX_PREFIX, "sec_");
        assert_eq!(SKILL_INFINITY_FULLTEXT_INDEX_PREFIX, "ft_");
        assert_eq!(skill_infinity_vector_field(1024), "q_1024_vec");
        assert_eq!(skill_infinity_vector_index_name(1024), "q_1024_vec_idx");
        assert_eq!(
            skill_infinity_secondary_index_name("skill_id"),
            "sec_skill_id"
        );
        assert_eq!(
            skill_infinity_secondary_index_name("space_id"),
            "sec_space_id"
        );
        for field in ["name", "tags", "description", "content"] {
            for analyzer in SKILL_INFINITY_ANALYZERS {
                assert_eq!(
                    skill_infinity_fulltext_index_name(field, analyzer),
                    format!("ft_{field}_{}", analyzer.replace('-', "_"))
                );
            }
        }
        assert_eq!(DOC_META_FIELDS.len(), 3);
        assert_eq!(DOC_META_ES_NUMBER_OF_SHARDS, 2);
        assert_eq!(DOC_META_ES_NUMBER_OF_REPLICAS, 0);
        assert_eq!(DOC_META_ES_REFRESH_INTERVAL, "1000ms");
        assert_eq!(
            serde_json::to_value(DOC_META_ES_SOURCE_ENABLED).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(DOC_META_ES_DYNAMIC, "runtime");
        assert_eq!(DOC_META_FIELDS[0].name, "id");
        assert_eq!(DOC_META_FIELDS[0].es_type, "keyword");
        assert!(DOC_META_FIELDS[0].es_store);
        assert_eq!(DOC_META_FIELDS[0].infinity_type, "varchar");
        assert_eq!(DOC_META_FIELDS[0].infinity_default, "");
        assert_eq!(DOC_META_FIELDS[2].name, "meta_fields");
        assert_eq!(DOC_META_FIELDS[2].es_type, "object");
        assert!(DOC_META_FIELDS[2].es_dynamic);
        assert_eq!(DOC_META_FIELDS[2].infinity_type, "json");
        assert_eq!(DOC_META_FIELDS[2].infinity_default, "{}");
        // Replacement markers document the actual JSON/PostgreSQL/zvec split.
        assert!(ENGINE_MAPPING_NOTE.contains("optional opaque PostgreSQL 18 mirror"));
        assert!(ENGINE_MAPPING_NOTE.contains("isolated collections"));
        assert!(PASSWORD_TRANSPORT_KEYS_POLICY.contains("not JWT signing keys"));
        assert!(PASSWORD_TRANSPORT_KEYS_POLICY.contains("RAYRAG_PASSWORD_PRIVATE_KEY_FILE"));
        assert!(PASSWORD_TRANSPORT_KEYS_POLICY.contains("Argon2id"));
    }
}
