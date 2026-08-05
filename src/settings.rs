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
            user_default_llm: UserDefaultLlm::default(),
        }
    }
}

/// Look up a top-level section of a service_conf document as a JSON object.
fn service_section<'a>(
    root: &'a Value,
    key: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
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
            conf.mysql.password =
                str_field(Some(mysql), "password").unwrap_or(conf.mysql.password);
            conf.mysql.host = str_field(Some(mysql), "host").unwrap_or(conf.mysql.host);
            conf.mysql.port = u16_field(Some(mysql), "port").unwrap_or(conf.mysql.port);
            conf.mysql.max_connections =
                u32_field(Some(mysql), "max_connections").unwrap_or(conf.mysql.max_connections);
            conf.mysql.stale_timeout =
                u32_field(Some(mysql), "stale_timeout").unwrap_or(conf.mysql.stale_timeout);
        }
        if let Some(minio) = service_section(value, "minio") {
            conf.minio.user = str_field(Some(minio), "user").unwrap_or(conf.minio.user);
            conf.minio.password =
                str_field(Some(minio), "password").unwrap_or(conf.minio.password);
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
            conf.redis.username =
                str_field(Some(redis), "username").unwrap_or(conf.redis.username);
            conf.redis.password =
                str_field(Some(redis), "password").unwrap_or(conf.redis.password);
            conf.redis.host = str_field(Some(redis), "host").unwrap_or(conf.redis.host);
        }
        if let Some(task_executor) = service_section(value, "task_executor") {
            conf.message_queue_type =
                str_field(Some(task_executor), "message_queue_type")
                    .unwrap_or(conf.message_queue_type);
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
                    conf.user_default_llm.default_models.insert(
                        capability.clone(),
                        parse_model_entry(entry),
                    );
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
// RayRAG persists chunks and metadata in PostgreSQL with the zvec extension,
// so the ES/Infinity mappings are **N/A by design** — no ES/Infinity index is
// ever created. What is ported is the *field schema*: the suffix rules below
// reproduce the ES dynamic-template semantics so chunk fields keep the same
// names and types across engines, and [`zvec_column_kind`] gives the
// Postgres/zvec column type for each field.
// ═══════════════════════════════════════════════════════════════════════════

/// Why RayRAG does not ship ES/Infinity index mappings (documented, N/A).
pub const ENGINE_MAPPING_NOTE: &str =
    "RayRAG stores chunks/metadata in PostgreSQL + zvec; ES/OpenSearch/Infinity \
     index mappings from conf/mapping*.json are N/A-by-design (no such index is \
     created). The field-suffix schema below preserves cross-engine field names \
     and types; zvec_column_kind maps each field onto its Postgres column type.";

/// Why the PEM signing keys are not ported (documented, N/A).
pub const SIGNING_KEYS_POLICY: &str =
    "RAGFlow conf/private.pem + public.pem (JWT signing keys) are NOT ported: \
     shipping private keys is a security hazard. RayRAG manages signing keys \
     through environment variables (RAGFLOW_SECRET_KEY for symmetric crypto, \
     RAYRAG_JWT_PRIVATE_KEY / RAYRAG_JWT_PUBLIC_KEY for asymmetric JWT signing), \
     generated per deployment.";

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
            FieldKind::Text => "text".to_owned(),
        }
    }
}

/// Suffix → [`FieldKind`] rules from conf/mapping.json (ES) and
/// conf/os_mapping.json (OpenSearch adds the larger knn_vector dims).
pub const ES_FIELD_SUFFIX_RULES: &[(&str, FieldKind)] = &[
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
    ("_512_vec", FieldKind::DenseVector(512)),
    ("_768_vec", FieldKind::DenseVector(768)),
    ("_1024_vec", FieldKind::DenseVector(1024)),
    ("_1536_vec", FieldKind::DenseVector(1536)),
    // Skill-index explicit properties (conf/skill_es_mapping.json q_*_vec).
    ("_2560_vec", FieldKind::DenseVector(2560)),
    ("_3072_vec", FieldKind::DenseVector(3072)),
    // OpenSearch-only dims (conf/os_mapping.json knn_vector templates).
    ("_2048_vec", FieldKind::DenseVector(2048)),
    ("_4096_vec", FieldKind::DenseVector(4096)),
    ("_6144_vec", FieldKind::DenseVector(6144)),
    ("_8192_vec", FieldKind::DenseVector(8192)),
    ("_10240_vec", FieldKind::DenseVector(10240)),
    ("_bin", FieldKind::Binary),
];

/// Regex rules from the ES `kwd` / `dt` dynamic templates. Kept as exact
/// string patterns for documentation; `es_field_kind` applies them when no
/// suffix rule matches.
pub const ES_FIELD_REGEX_RULES: &[(&str, FieldKind)] = &[
    (r"^(.*_(kwd|id|ids|uid|uids)|uid|id)$", FieldKind::Keyword),
    (r"^.*(_dt|_time|_at)$", FieldKind::DateTime),
];

/// Resolve an ES/OpenSearch field kind for a chunk field name, mirroring the
/// dynamic-template evaluation order (regex templates are tried in order).
pub fn es_field_kind(field: &str) -> FieldKind {
    for (suffix, kind) in ES_FIELD_SUFFIX_RULES {
        if field.ends_with(suffix) {
            return *kind;
        }
    }
    for (pattern, kind) in ES_FIELD_REGEX_RULES {
        if regex_matches(pattern, field) {
            return *kind;
        }
    }
    FieldKind::Text
}

/// Postgres/zvec column type for a chunk field (RayRAG's storage backend).
pub fn zvec_column_kind(field: &str) -> String {
    match es_field_kind(field) {
        FieldKind::Int | FieldKind::Short => "INTEGER".to_owned(),
        FieldKind::Ulong | FieldKind::Long => "BIGINT".to_owned(),
        FieldKind::Float => "REAL".to_owned(),
        FieldKind::DenseVector(dims) => format!("VECTOR({dims})"),
        FieldKind::Binary => "BYTEA".to_owned(),
        FieldKind::Nested | FieldKind::Object => "JSONB".to_owned(),
        _ => "TEXT".to_owned(),
    }
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
    if body == "(.*_(kwd|id|ids|uid|uids)|uid|id)" {
        return field == "uid"
            || field == "id"
            || ["_kwd", "_id", "_ids", "_uid", "_uids"]
                .iter()
                .any(|suffix| field.ends_with(suffix));
    }
    // ^.*(_dt|_time|_at)$ — the ES `dt` template: any field ending in one of
    // the date suffixes.
    if let Some(rest) = body.strip_prefix(".*(") {
        if let Some(alts) = rest.strip_suffix(')') {
            return alts.split('|').any(|suffix| field.ends_with(suffix));
        }
    }
    body == field
}

/// doc_meta index fields — ES (conf/doc_meta_es_mapping.json) and Infinity
/// (conf/doc_meta_infinity_mapping.json): `(field, type)`.
pub const DOC_META_FIELDS: &[(&str, &str)] = &[
    ("id", "keyword | varchar"),
    ("kb_id", "keyword | varchar"),
    ("meta_fields", "object(dynamic) | json"),
];

/// Infinity chunk fields — conf/infinity_mapping.json: `(field, type)`.
pub const INFINITY_CHUNK_FIELDS: &[(&str, &str)] = &[
    ("id", "varchar"),
    ("doc_id", "varchar"),
    ("kb_id", "varchar, secondary idx"),
    ("mom_id", "varchar"),
    ("mom", "varchar"),
    ("create_time", "varchar"),
    ("create_timestamp_flt", "float"),
    ("img_id", "varchar"),
    ("docnm", "varchar, analyzer rag-coarse|rag-fine"),
    ("name_kwd", "varchar, whitespace-#"),
    ("tag_kwd", "varchar, whitespace-#"),
    ("important_kwd_empty_count", "integer"),
    ("important_keywords", "varchar, analyzer"),
    ("questions", "varchar, analyzer"),
    ("content", "varchar, analyzer"),
    ("authors", "varchar, analyzer"),
    ("page_num_int", "varchar"),
    ("top_int", "varchar"),
    ("position_int", "varchar"),
    ("weight_int", "integer"),
    ("weight_flt", "float"),
    ("chunk_order_int", "integer"),
    ("rank_int", "integer"),
    ("rank_flt", "float"),
    ("available_int", "integer, secondary idx"),
    ("knowledge_graph_kwd", "varchar"),
    ("entities_kwd", "varchar, whitespace-#"),
    ("pagerank_fea", "integer"),
    ("tag_feas", "varchar, rankfeatures"),
    ("from_entity_kwd", "varchar, whitespace-#"),
    ("to_entity_kwd", "varchar, whitespace-#"),
    ("entity_kwd", "varchar, whitespace-#"),
    ("entity_type_kwd", "varchar, whitespace-#"),
    ("source_id", "varchar, whitespace-#"),
    ("n_hop_with_weight", "varchar"),
    ("mom_with_weight", "varchar"),
    ("removed_kwd", "varchar, whitespace-#"),
    ("doc_type_kwd", "varchar, whitespace-#"),
    ("toc_kwd", "varchar, whitespace-#"),
    ("raptor_kwd", "varchar, whitespace-#"),
    ("raptor_layer_int", "integer"),
];

/// Infinity memory-message fields — conf/message_infinity_mapping.json.
pub const MESSAGE_INFINITY_FIELDS: &[(&str, &str)] = &[
    ("id", "varchar"),
    ("message_id", "integer"),
    ("message_type_kwd", "varchar"),
    ("source_id", "integer"),
    ("memory_id", "varchar"),
    ("user_id", "varchar"),
    ("agent_id", "varchar"),
    ("session_id", "varchar"),
    ("valid_at", "varchar"),
    ("valid_at_flt", "float"),
    ("invalid_at", "varchar"),
    ("invalid_at_flt", "float"),
    ("forget_at", "varchar"),
    ("forget_at_flt", "float"),
    ("status_int", "integer"),
    ("zone_id", "integer"),
    ("content", "varchar, analyzer"),
];

/// Skill-store fields — ES (conf/skill_es_mapping.json) and Infinity
/// (conf/skill_infinity_mapping.json): `(field, type)`.
pub const SKILL_FIELDS: &[(&str, &str)] = &[
    ("skill_id", "keyword | varchar, secondary idx"),
    ("space_id", "keyword | varchar, secondary idx"),
    ("folder_id", "keyword | varchar"),
    ("name", "text(index=false) | varchar, analyzer"),
    ("name_tks", "text(whitespace,scripted_sim) | —"),
    ("tags", "text(index=false) | varchar, analyzer"),
    ("tags_tks", "text(whitespace,scripted_sim) | —"),
    ("description", "text(index=false) | varchar, analyzer"),
    ("description_tks", "text(whitespace,scripted_sim) | —"),
    ("content", "text(index=false) | varchar, analyzer"),
    ("content_tks", "text(whitespace,scripted_sim) | —"),
    ("q_256_vec", "dense_vector(256) | —"),
    ("q_512_vec", "dense_vector(512) | —"),
    ("q_768_vec", "dense_vector(768) | —"),
    ("q_1024_vec", "dense_vector(1024) | —"),
    ("q_1536_vec", "dense_vector(1536) | —"),
    ("q_2560_vec", "dense_vector(2560) | —"),
    ("q_3072_vec", "dense_vector(3072) | —"),
    ("version", "keyword | varchar"),
    ("status", "keyword | varchar"),
    ("create_time", "long | bigint"),
    ("update_time", "long | bigint"),
];

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
        // Missing keys keep the shipped defaults.
        let partial = ServiceConf::from_value(&serde_json::json!({"ragflow": {"http_port": 9400}}));
        assert_eq!(partial.http_port, 9400);
        assert_eq!(partial.admin_port, 9381);
        assert_eq!(partial.mysql.name, "rag_flow");

        // Apply onto a settings snapshot: fills the LLM defaults that the env
        // reader intentionally leaves empty, composing name@factory.
        let mut settings = Settings::from_reader(|_| None);
        assert!(settings.llm_factory.is_empty());
        conf.apply_to_settings(&mut settings);
        assert_eq!(settings.llm_factory, "ZHIPU-AI");
        assert_eq!(settings.llm_base_url, "https://open.bigmodel.cn/api/paas/v4");
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
        assert_eq!(es_field_kind("content_with_weight"), FieldKind::UnindexedText);
        assert_eq!(es_field_kind("tag_list"), FieldKind::UnindexedText);
        assert_eq!(es_field_kind("pagerank_fea"), FieldKind::RankFeature);
        assert_eq!(es_field_kind("tag_feas"), FieldKind::RankFeatures);
        assert_eq!(es_field_kind("q_768_vec"), FieldKind::DenseVector(768));
        assert_eq!(es_field_kind("q_3072_vec"), FieldKind::DenseVector(3072));
        assert_eq!(es_field_kind("q_10240_vec"), FieldKind::DenseVector(10240));
        assert_eq!(es_field_kind("blob_bin"), FieldKind::Binary);
        // Regex templates: *_(kwd|id|ids|uid|uids)|uid|id → keyword.
        assert_eq!(es_field_kind("docnm_kwd"), FieldKind::Keyword);
        assert_eq!(es_field_kind("id"), FieldKind::Keyword);
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
        // Field tables match the conf files' field counts.
        assert_eq!(INFINITY_CHUNK_FIELDS.len(), 41);
        assert_eq!(MESSAGE_INFINITY_FIELDS.len(), 17);
        assert_eq!(SKILL_FIELDS.len(), 22);
        assert_eq!(DOC_META_FIELDS.len(), 3);
        // N/A-by-design markers are documented, not code.
        assert!(ENGINE_MAPPING_NOTE.contains("N/A-by-design"));
        assert!(SIGNING_KEYS_POLICY.contains("NOT ported"));
        assert!(SIGNING_KEYS_POLICY.contains("RAGFLOW_SECRET_KEY"));
    }
}
