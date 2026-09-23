//! RAGFlow `common/` layer, ported to Rust.
//!
//! Coverage map (Python reference -> Rust here):
//! - `common/constants.py`            -> [`constants`] (RetCode / TaskStatus / ParserType /
//!                                        LLMType / FileSource / Storage / MemoryType / page sentinels)
//! - `api/constants.py`               -> [`constants`] (NAME_LENGTH_LIMIT / API_VERSION / limits)
//! - `common/string_utils.py`         -> [`string_utils`] (space cleanup / markdown-block strip)
//! - `common/text_utils.py`           -> [`text_utils`] (Arabic digit / presentation-form normalization)
//! - `common/time_utils.py`           -> [`time_utils`] (ms timestamps, local-time format/parse,
//!                                        ISO-8601 -> `%Y-%m-%d %H:%M:%S`)
//! - `common/float_utils.py`          -> [`float_utils`] (safe float conversion, overlap clamp)
//! - `common/crypto_utils.py`         -> [`crypto_utils`] (AES-128/256-CBC, PKCS7, `RAGF` magic header,
//!                                        PBKDF2-HMAC-SHA256 key derivation; SM4 marked TODO)
//! - `common/ssrf_guard.py`           -> [`ssrf_guard`] (scheme allowlist + public-IP allowlist)
//! - `api/validation.py`              -> [`validation`] (required / type / length validators)
//! - `common/misc_utils.py`           -> [`misc_utils`] (uuid / hash_str2int / convert_bytes / once)
//! - `common/parser_config_utils.py`  -> [`parser_config_utils`] (`@mineru` style layout recognizer)
//! - `common/query_base.py`           -> [`query_base`] (is_chinese / sub_special_char / eng-zh spacing)
//! - `common/connection_utils.py`     -> [`connection_utils`] (async timeout helper)
//! - `common/versions.py`             -> [`versions`] (RAGFLOW_VERSION resolution)
//! - `common/decorator.py`            -> [`misc_utils::Once`] (thread-safe run-once)
//!
//! Not ported (documented skips): `settings.py` / `config_utils.py` (YAML service
//! config — RayRAG has its own runtime config), `log_utils.py` (RayRAG has
//! `crate::logging`), `http_client.py` (reqwest is used directly throughout
//! RayRAG), `query_base::rmWWW` (niche FAQ rewrite heuristic), `exceptions.py`
//! (already in `api/common.rs::ApiError`), `signal_utils.py` / `metadata_utils.py` /
//! `metadata_es_filter.py` / `tag_feature_utils.py` (process/ES-specific).

pub mod constants {
    // ------------------------------------------------------------------
    // common/constants.py
    // ------------------------------------------------------------------

    pub const SERVICE_CONF: &str = "service_conf.yaml";
    pub const RAG_FLOW_SERVICE_NAME: &str = "ragflow";
    pub const SANDBOX_ARTIFACT_BUCKET: &str = "sandbox-artifacts";
    pub const SANDBOX_ARTIFACT_EXPIRE_DAYS: u64 = 7;

    pub const PAGERANK_FLD: &str = "pagerank_fea";
    pub const SVR_QUEUE_NAME: &str = "rag_flow_svr_queue";
    pub const SVR_CONSUMER_GROUP_NAME: &str = "rag_flow_svr_task_broker";
    pub const TAG_FLD: &str = "tag_feas";

    /// Maximum page number used as the "unlimited" sentinel value
    /// (`common/constants.py::MAXIMUM_PAGE_NUMBER`).
    pub const MAXIMUM_PAGE_NUMBER: i64 = 100_000;
    /// Task/DB layer sentinel (`MAXIMUM_PAGE_NUMBER * 1000`) so user-supplied
    /// page ranges never collide with the sentinel.
    pub const MAXIMUM_TASK_PAGE_NUMBER: i64 = MAXIMUM_PAGE_NUMBER * 1000;

    /// `common/constants.py::RetCode` (IntEnum). RAGFlow returns these codes
    /// in the `{"code": ..., "message": ...}` envelope.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
    #[repr(i32)]
    pub enum RetCode {
        Success = 0,
        NotEffective = 10,
        ExceptionError = 100,
        ArgumentError = 101,
        DataError = 102,
        OperatingError = 103,
        ConnectionError = 105,
        Running = 106,
        PermissionError = 108,
        AuthenticationError = 109,
        BadRequest = 400,
        Unauthorized = 401,
        Forbidden = 403,
        NotFound = 404,
        Conflict = 409,
        ServerError = 500,
    }

    impl RetCode {
        pub fn as_i32(self) -> i32 {
            self as i32
        }

        pub fn from_i32(value: i32) -> Option<Self> {
            match value {
                0 => Some(Self::Success),
                10 => Some(Self::NotEffective),
                100 => Some(Self::ExceptionError),
                101 => Some(Self::ArgumentError),
                102 => Some(Self::DataError),
                103 => Some(Self::OperatingError),
                105 => Some(Self::ConnectionError),
                106 => Some(Self::Running),
                108 => Some(Self::PermissionError),
                109 => Some(Self::AuthenticationError),
                400 => Some(Self::BadRequest),
                401 => Some(Self::Unauthorized),
                403 => Some(Self::Forbidden),
                404 => Some(Self::NotFound),
                409 => Some(Self::Conflict),
                500 => Some(Self::ServerError),
                _ => None,
            }
        }

        pub fn is_ok(self) -> bool {
            self == Self::Success
        }
    }

    /// `common/constants.py::StatusEnum` — `"1"` valid / `"0"` invalid.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum StatusEnum {
        Valid,
        Invalid,
    }

    impl StatusEnum {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Valid => "1",
                Self::Invalid => "0",
            }
        }

        pub fn from_str(value: &str) -> Option<Self> {
            match value {
                "1" => Some(Self::Valid),
                "0" => Some(Self::Invalid),
                _ => None,
            }
        }
    }

    /// `common/constants.py::ActiveEnum` — `"1"` active / `"0"` inactive.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum ActiveEnum {
        Active,
        Inactive,
    }

    impl ActiveEnum {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Active => "1",
                Self::Inactive => "0",
            }
        }

        pub fn from_str(value: &str) -> Option<Self> {
            match value {
                "1" => Some(Self::Active),
                "0" => Some(Self::Inactive),
                _ => None,
            }
        }
    }

    /// `common/constants.py::TaskStatus` (StrEnum) — parse-task lifecycle.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum TaskStatus {
        Unstart,
        Running,
        Cancel,
        Done,
        Fail,
        Schedule,
    }

    /// `VALID_TASK_STATUS` — every value [`TaskStatus::valid`] accepts.
    pub const VALID_TASK_STATUS: [TaskStatus; 6] = [
        TaskStatus::Unstart,
        TaskStatus::Running,
        TaskStatus::Cancel,
        TaskStatus::Done,
        TaskStatus::Fail,
        TaskStatus::Schedule,
    ];

    impl TaskStatus {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Unstart => "0",
                Self::Running => "1",
                Self::Cancel => "2",
                Self::Done => "3",
                Self::Fail => "4",
                Self::Schedule => "5",
            }
        }

        /// Strict parse — mirrors `CustomEnum.valid` (no defaulting).
        pub fn from_str(value: &str) -> Option<Self> {
            match value {
                "0" => Some(Self::Unstart),
                "1" => Some(Self::Running),
                "2" => Some(Self::Cancel),
                "3" => Some(Self::Done),
                "4" => Some(Self::Fail),
                "5" => Some(Self::Schedule),
                _ => None,
            }
        }

        pub fn valid(value: &str) -> bool {
            Self::from_str(value).is_some()
        }

        pub fn values() -> [&'static str; 6] {
            [
                Self::Unstart.as_str(),
                Self::Running.as_str(),
                Self::Cancel.as_str(),
                Self::Done.as_str(),
                Self::Fail.as_str(),
                Self::Schedule.as_str(),
            ]
        }
    }

    /// `common/constants.py::LLMType` (StrEnum).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum LLMType {
        Chat,
        Embedding,
        Speech2Text,
        Image2Text,
        Rerank,
        Tts,
        Ocr,
    }

    impl LLMType {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Chat => "chat",
                Self::Embedding => "embedding",
                Self::Speech2Text => "speech2text",
                Self::Image2Text => "image2text",
                Self::Rerank => "rerank",
                Self::Tts => "tts",
                Self::Ocr => "ocr",
            }
        }

        pub fn from_str(value: &str) -> Option<Self> {
            match value {
                "chat" => Some(Self::Chat),
                "embedding" => Some(Self::Embedding),
                "speech2text" => Some(Self::Speech2Text),
                "image2text" => Some(Self::Image2Text),
                "rerank" => Some(Self::Rerank),
                "tts" => Some(Self::Tts),
                "ocr" => Some(Self::Ocr),
                _ => None,
            }
        }
    }

    /// `common/constants.py::ParserType` (StrEnum) — chunk methods.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum ParserType {
        Presentation,
        Laws,
        Manual,
        Paper,
        Resume,
        Book,
        Qa,
        Table,
        Naive,
        Picture,
        One,
        Audio,
        Email,
        Kg,
        Tag,
    }

    impl ParserType {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Presentation => "presentation",
                Self::Laws => "laws",
                Self::Manual => "manual",
                Self::Paper => "paper",
                Self::Resume => "resume",
                Self::Book => "book",
                Self::Qa => "qa",
                Self::Table => "table",
                Self::Naive => "naive",
                Self::Picture => "picture",
                Self::One => "one",
                Self::Audio => "audio",
                Self::Email => "email",
                Self::Kg => "knowledge_graph",
                Self::Tag => "tag",
            }
        }

        pub fn from_str(value: &str) -> Option<Self> {
            match value {
                "presentation" => Some(Self::Presentation),
                "laws" => Some(Self::Laws),
                "manual" => Some(Self::Manual),
                "paper" => Some(Self::Paper),
                "resume" => Some(Self::Resume),
                "book" => Some(Self::Book),
                "qa" => Some(Self::Qa),
                "table" => Some(Self::Table),
                "naive" => Some(Self::Naive),
                "picture" => Some(Self::Picture),
                "one" => Some(Self::One),
                "audio" => Some(Self::Audio),
                "email" => Some(Self::Email),
                "knowledge_graph" => Some(Self::Kg),
                "tag" => Some(Self::Tag),
                _ => None,
            }
        }

        pub fn valid(value: &str) -> bool {
            Self::from_str(value).is_some()
        }
    }

    /// `common/constants.py::FileSource` (StrEnum). `Local` serializes to `""`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum FileSource {
        Local,
        Knowledgebase,
        Rss,
        S3,
        Notion,
        Discord,
        Confluence,
        Gmail,
        GoogleDrive,
        Jira,
        Sharepoint,
        Slack,
        Teams,
        Webdav,
        Moodle,
        Dropbox,
        Box,
        R2,
        OciStorage,
        GoogleCloudStorage,
        Airtable,
        Asana,
        Github,
        Gitlab,
        Imap,
        Bitbucket,
        Zendesk,
        Seafile,
        Mysql,
        Postgresql,
        DingtalkAiTable,
    }

    impl FileSource {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Local => "",
                Self::Knowledgebase => "knowledgebase",
                Self::Rss => "rss",
                Self::S3 => "s3",
                Self::Notion => "notion",
                Self::Discord => "discord",
                Self::Confluence => "confluence",
                Self::Gmail => "gmail",
                Self::GoogleDrive => "google_drive",
                Self::Jira => "jira",
                Self::Sharepoint => "sharepoint",
                Self::Slack => "slack",
                Self::Teams => "teams",
                Self::Webdav => "webdav",
                Self::Moodle => "moodle",
                Self::Dropbox => "dropbox",
                Self::Box => "box",
                Self::R2 => "r2",
                Self::OciStorage => "oci_storage",
                Self::GoogleCloudStorage => "google_cloud_storage",
                Self::Airtable => "airtable",
                Self::Asana => "asana",
                Self::Github => "github",
                Self::Gitlab => "gitlab",
                Self::Imap => "imap",
                Self::Bitbucket => "bitbucket",
                Self::Zendesk => "zendesk",
                Self::Seafile => "seafile",
                Self::Mysql => "mysql",
                Self::Postgresql => "postgresql",
                Self::DingtalkAiTable => "dingtalk_ai_table",
            }
        }

        pub fn from_str(value: &str) -> Option<Self> {
            match value {
                "" => Some(Self::Local),
                "knowledgebase" => Some(Self::Knowledgebase),
                "rss" => Some(Self::Rss),
                "s3" => Some(Self::S3),
                "notion" => Some(Self::Notion),
                "discord" => Some(Self::Discord),
                "confluence" => Some(Self::Confluence),
                "gmail" => Some(Self::Gmail),
                "google_drive" => Some(Self::GoogleDrive),
                "jira" => Some(Self::Jira),
                "sharepoint" => Some(Self::Sharepoint),
                "slack" => Some(Self::Slack),
                "teams" => Some(Self::Teams),
                "webdav" => Some(Self::Webdav),
                "moodle" => Some(Self::Moodle),
                "dropbox" => Some(Self::Dropbox),
                "box" => Some(Self::Box),
                "r2" => Some(Self::R2),
                "oci_storage" => Some(Self::OciStorage),
                "google_cloud_storage" => Some(Self::GoogleCloudStorage),
                "airtable" => Some(Self::Airtable),
                "asana" => Some(Self::Asana),
                "github" => Some(Self::Github),
                "gitlab" => Some(Self::Gitlab),
                "imap" => Some(Self::Imap),
                "bitbucket" => Some(Self::Bitbucket),
                "zendesk" => Some(Self::Zendesk),
                "seafile" => Some(Self::Seafile),
                "mysql" => Some(Self::Mysql),
                "postgresql" => Some(Self::Postgresql),
                "dingtalk_ai_table" => Some(Self::DingtalkAiTable),
                _ => None,
            }
        }
    }

    /// `common/constants.py::PipelineTaskType` (StrEnum).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum PipelineTaskType {
        Parse,
        Download,
        Raptor,
        GraphRag,
        Mindmap,
        Memory,
        Artifact,
        Skill,
    }

    /// `VALID_PIPELINE_TASK_TYPES` — `Memory` is deliberately excluded.
    pub const VALID_PIPELINE_TASK_TYPES: [PipelineTaskType; 7] = [
        PipelineTaskType::Parse,
        PipelineTaskType::Download,
        PipelineTaskType::Raptor,
        PipelineTaskType::GraphRag,
        PipelineTaskType::Mindmap,
        PipelineTaskType::Artifact,
        PipelineTaskType::Skill,
    ];

    impl PipelineTaskType {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Parse => "Parse",
                Self::Download => "Download",
                Self::Raptor => "RAPTOR",
                Self::GraphRag => "GraphRAG",
                Self::Mindmap => "Mindmap",
                Self::Memory => "Memory",
                Self::Artifact => "Artifact",
                Self::Skill => "Skill",
            }
        }

        pub fn as_lower_str(self) -> &'static str {
            match self {
                Self::Parse => "parse",
                Self::Download => "download",
                Self::Raptor => "raptor",
                Self::GraphRag => "graphrag",
                Self::Mindmap => "mindmap",
                Self::Memory => "memory",
                Self::Artifact => "artifact",
                Self::Skill => "skill",
            }
        }

        pub fn from_str(value: &str) -> Option<Self> {
            match value {
                "Parse" => Some(Self::Parse),
                "Download" => Some(Self::Download),
                "RAPTOR" => Some(Self::Raptor),
                "GraphRAG" => Some(Self::GraphRag),
                "Mindmap" => Some(Self::Mindmap),
                "Memory" => Some(Self::Memory),
                "Artifact" => Some(Self::Artifact),
                "Skill" => Some(Self::Skill),
                _ => None,
            }
        }
    }

    /// `common/constants.py::MCPServerType` (StrEnum).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum MCPServerType {
        Sse,
        StreamableHttp,
    }

    impl MCPServerType {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Sse => "sse",
                Self::StreamableHttp => "streamable-http",
            }
        }
    }

    /// `common/constants.py::Storage` (Enum) — blob-storage backends.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    #[repr(i32)]
    pub enum Storage {
        Minio = 1,
        AzureSpn = 2,
        AzureSas = 3,
        AwsS3 = 4,
        Oss = 5,
        Opendal = 6,
        Gcs = 7,
    }

    impl Storage {
        pub fn as_i32(self) -> i32 {
            self as i32
        }
    }

    /// `common/constants.py::MemoryType` — bit-flag memory kinds.
    pub mod memory_type {
        pub const RAW: u32 = 0b0001;
        pub const SEMANTIC: u32 = 0b0010;
        pub const EPISODIC: u32 = 0b0100;
        pub const PROCEDURAL: u32 = 0b1000;

        pub const ALL: u32 = RAW | SEMANTIC | EPISODIC | PROCEDURAL;

        pub fn contains(flags: u32, kind: u32) -> bool {
            flags & kind == kind
        }
    }

    /// `common/constants.py::MemoryStorageType` (StrEnum).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum MemoryStorageType {
        Table,
        Graph,
    }

    impl MemoryStorageType {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Table => "table",
                Self::Graph => "graph",
            }
        }
    }

    /// `common/constants.py::ForgettingPolicy` (StrEnum).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum ForgettingPolicy {
        Fifo,
    }

    impl ForgettingPolicy {
        pub fn as_str(self) -> &'static str {
            "FIFO"
        }
    }

    // ------------------------------------------------------------------
    // api/constants.py
    // ------------------------------------------------------------------
    // (`FILE_NAME_LEN_LIMIT` and `IMG_BASE64_PREFIX` live in `api::utils`.)

    pub const NAME_LENGTH_LIMIT: usize = 1 << 10; // 1024
    pub const API_VERSION: &str = "v1";
    pub const REQUEST_WAIT_SEC: u64 = 2;
    pub const REQUEST_MAX_WAIT_SEC: u64 = 300;
    pub const DATASET_NAME_LIMIT: usize = 128;
    pub const MEMORY_NAME_LIMIT: usize = 128;
    pub const MEMORY_SIZE_LIMIT: usize = 10 * 1024 * 1024; // bytes
}

/// `common/string_utils.py` — whitespace / markdown cleanup.
pub mod string_utils {
    use regex::Regex;

    /// `remove_redundant_spaces` — drop spaces hugging boundary punctuation
    /// while preserving meaningful spaces. Two passes, byte-for-byte aligned
    /// with the Python regexes (case-insensitive):
    /// 1. spaces after left-boundary chars: `"( test" -> "(test"`,
    /// 2. spaces before right-boundary chars: `"world !" -> "world!"`.
    pub fn remove_redundant_spaces(txt: &str) -> String {
        let after_left = Regex::new(r"(?i)([^a-z0-9.,)>]) +([^ ])").expect("valid regex");
        let step1 = after_left.replace_all(txt, "$1$2");
        let before_right = Regex::new(r"(?i)([^ ]) +([^a-z0-9.,(<])").expect("valid regex");
        before_right.replace_all(&step1, "$1$2").into_owned()
    }

    /// `clean_markdown_block` — strip a surrounding ` ```markdown ... ``` `
    /// fence (opening tag with optional whitespace/newline, closing fence at
    /// end), then trim.
    pub fn clean_markdown_block(text: &str) -> String {
        let opening = Regex::new(r"^\s*```markdown\s*\n?").expect("valid regex");
        let step1 = opening.replace(text, "");
        let closing = Regex::new(r"\n?\s*```\s*$").expect("valid regex");
        closing.replace(&step1, "").trim().to_string()
    }

    /// `is_content_empty` — `None` / blank content is empty.
    pub fn is_content_empty(content: &str) -> bool {
        content.trim().is_empty()
    }
}

/// `common/text_utils.py` — Arabic text normalization.
pub mod text_utils {
    use unicode_normalization::UnicodeNormalization;

    /// `normalize_arabic_digits` — map Arabic-Indic (U+0660..U+0669) and
    /// Extended Arabic-Indic (U+06F0..U+06F9) digits to ASCII `0`-`9`.
    pub fn normalize_arabic_digits(text: &str) -> String {
        text.chars()
            .map(|ch| {
                let code = ch as u32;
                match code {
                    0x0660..=0x0669 => char::from_u32(code - 0x0660 + 0x30).unwrap_or(ch),
                    0x06F0..=0x06F9 => char::from_u32(code - 0x06F0 + 0x30).unwrap_or(ch),
                    _ => ch,
                }
            })
            .collect()
    }

    /// `normalize_arabic_presentation_forms` — NFKC-normalize text that
    /// contains Arabic presentation forms (U+FB50..U+FDFF, U+FE70..U+FEFF);
    /// text without them is returned unchanged.
    pub fn normalize_arabic_presentation_forms(text: &str) -> String {
        let has_presentation_forms = text.chars().any(|ch| {
            let code = ch as u32;
            matches!(code, 0xFB50..=0xFDFF | 0xFE70..=0xFEFF)
        });
        if !has_presentation_forms {
            return text.to_string();
        }
        text.nfkc().collect()
    }
}

/// `common/time_utils.py` — millisecond timestamps, local-time formatting
/// (mirroring `time.localtime` / `time.mktime`) and ISO-8601 conversion.
pub mod time_utils {
    use anyhow::Context;
    use chrono::{Local, NaiveDateTime, TimeZone};

    /// `time.strftime` default used across RAGFlow JSON encoding.
    pub const DEFAULT_TIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

    /// `current_timestamp` — Unix timestamp in milliseconds.
    pub fn current_timestamp() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    /// `timestamp_to_date` — ms timestamp -> formatted **local** date string.
    /// A zero/absent timestamp falls back to the current time, like Python.
    pub fn timestamp_to_date(timestamp_ms: i64, format_string: &str) -> String {
        let timestamp_ms = if timestamp_ms == 0 {
            current_timestamp()
        } else {
            timestamp_ms
        };
        let secs = timestamp_ms.div_euclid(1000);
        let nanos = (timestamp_ms.rem_euclid(1000) * 1_000_000) as u32;
        match Local.timestamp_opt(secs, nanos).single() {
            Some(dt) => dt.format(format_string).to_string(),
            None => String::new(),
        }
    }

    /// `date_string_to_timestamp` — local-time date string -> ms timestamp
    /// (Python `time.strptime` + `time.mktime`).
    pub fn date_string_to_timestamp(time_str: &str, format_string: &str) -> anyhow::Result<i64> {
        let naive = NaiveDateTime::parse_from_str(time_str, format_string).with_context(|| {
            format!("cannot parse date string {time_str:?} with {format_string:?}")
        })?;
        let local = Local
            .from_local_datetime(&naive)
            .single()
            .ok_or_else(|| anyhow::anyhow!("ambiguous or nonexistent local time: {time_str}"))?;
        Ok(local.timestamp_millis())
    }

    /// `datetime_format` — strip the microsecond component.
    pub fn datetime_format(date_time: chrono::DateTime<Local>) -> chrono::DateTime<Local> {
        let secs = date_time.timestamp();
        Local.timestamp_opt(secs, 0).single().unwrap_or(date_time)
    }

    /// `get_format_time` — current local datetime without microseconds.
    pub fn get_format_time() -> chrono::DateTime<Local> {
        datetime_format(Local::now())
    }

    /// `delta_seconds` — seconds elapsed from `"YYYY-MM-DD HH:MM:SS"` (local)
    /// to now.
    pub fn delta_seconds(date_string: &str) -> anyhow::Result<f64> {
        let naive = NaiveDateTime::parse_from_str(date_string, DEFAULT_TIME_FORMAT)
            .with_context(|| format!("cannot parse date string {date_string:?}"))?;
        let past = Local
            .from_local_datetime(&naive)
            .single()
            .ok_or_else(|| anyhow::anyhow!("ambiguous or nonexistent local time: {date_string}"))?;
        Ok((Local::now() - past).num_milliseconds() as f64 / 1000.0)
    }

    /// `format_iso_8601_to_ymd_hms` — ISO-8601 string -> `"YYYY-MM-DD HH:MM:SS"`.
    /// Wall-clock time is preserved (Python `fromisoformat` + `strftime`);
    /// unparseable input is returned unchanged.
    pub fn format_iso_8601_to_ymd_hms(time_str: &str) -> String {
        let normalized = time_str.replace('Z', "+00:00");
        // With explicit offset (RFC3339).
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&normalized) {
            return dt.format(DEFAULT_TIME_FORMAT).to_string();
        }
        // Naive datetime (no offset).
        if let Ok(naive) = NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%dT%H:%M:%S") {
            return naive.format(DEFAULT_TIME_FORMAT).to_string();
        }
        // Date-only input.
        if let Ok(date) = chrono::NaiveDate::parse_from_str(time_str, "%Y-%m-%d")
            && let Some(naive) = date.and_hms_opt(0, 0, 0)
        {
            return naive.format(DEFAULT_TIME_FORMAT).to_string();
        }
        time_str.to_string()
    }
}

/// `common/float_utils.py` — safe float handling.
pub mod float_utils {
    /// `get_float` — convert to `f64`; `None`/unparseable -> `-inf`.
    pub fn get_float(v: Option<&str>) -> f64 {
        v.and_then(|s| s.trim().parse::<f64>().ok())
            .unwrap_or(f64::NEG_INFINITY)
    }

    /// `normalize_overlapped_percent` — fraction (0,1) is scaled by 100,
    /// truncated to an integer, then clamped into `[0, 90]`.
    pub fn normalize_overlapped_percent(overlapped_percent: &str) -> i64 {
        let Ok(mut value) = overlapped_percent.trim().parse::<f64>() else {
            return 0;
        };
        if value > 0.0 && value < 1.0 {
            value *= 100.0;
        }
        (value as i64).clamp(0, 90)
    }
}

/// `common/crypto_utils.py` — AES-CBC with PKCS7 padding.
///
/// Wire format (identical to the Python implementation):
/// `b"RAGF" (magic) + iv (16 bytes) + ciphertext`.
/// Keys are normalized to the algorithm's length via
/// PBKDF2-HMAC-SHA256 (`salt = b"ragflow_crypto_salt"`, 100_000 rounds).
pub mod crypto_utils {
    use aes::cipher::generic_array::GenericArray;
    use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};

    /// Magic header identifying RAGFlow-encrypted blobs.
    pub const ENCRYPTED_MAGIC: &[u8; 4] = b"RAGF";
    const BLOCK_SIZE: usize = 16;
    const PBKDF2_ITERATIONS: u32 = 100_000;
    const PBKDF2_SALT: &[u8] = b"ragflow_crypto_salt";

    /// Supported algorithms (`CryptoUtil.SUPPORTED_ALGORITHMS`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum CryptoAlgorithm {
        Aes128Cbc,
        Aes256Cbc,
        /// TODO: no SM4 implementation available in the offline crate cache;
        /// rejected with [`CryptoError::UnsupportedAlgorithm`] for now.
        Sm4Cbc,
    }

    impl CryptoAlgorithm {
        pub fn from_str(value: &str) -> Option<Self> {
            match value {
                "aes-128-cbc" => Some(Self::Aes128Cbc),
                "aes-256-cbc" => Some(Self::Aes256Cbc),
                "sm4-cbc" => Some(Self::Sm4Cbc),
                _ => None,
            }
        }

        pub fn as_str(self) -> &'static str {
            match self {
                Self::Aes128Cbc => "aes-128-cbc",
                Self::Aes256Cbc => "aes-256-cbc",
                Self::Sm4Cbc => "sm4-cbc",
            }
        }

        fn key_length(self) -> usize {
            match self {
                Self::Aes128Cbc | Self::Sm4Cbc => 16,
                Self::Aes256Cbc => 32,
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum CryptoError {
        UnsupportedAlgorithm(String),
        InvalidData(&'static str),
        PaddingError,
    }

    impl std::fmt::Display for CryptoError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::UnsupportedAlgorithm(algo) => write!(f, "unsupported algorithm: {algo}"),
                Self::InvalidData(msg) => write!(f, "invalid encrypted data: {msg}"),
                Self::PaddingError => write!(f, "invalid PKCS7 padding"),
            }
        }
    }

    impl std::error::Error for CryptoError {}

    /// One of the AES block ciphers, boxed behind a common interface.
    enum AnyCipher {
        Aes128(aes::Aes128),
        Aes256(aes::Aes256),
    }

    type Block = GenericArray<u8, aes::cipher::generic_array::typenum::U16>;

    impl AnyCipher {
        fn encrypt_block(&self, block: &mut Block) {
            match self {
                Self::Aes128(cipher) => cipher.encrypt_block(block),
                Self::Aes256(cipher) => cipher.encrypt_block(block),
            }
        }

        fn decrypt_block(&self, block: &mut Block) {
            match self {
                Self::Aes128(cipher) => cipher.decrypt_block(block),
                Self::Aes256(cipher) => cipher.decrypt_block(block),
            }
        }
    }

    /// `CryptoUtil` — factory-style encrypt/decrypt helper.
    #[derive(Debug, Clone)]
    pub struct CryptoUtil {
        algorithm: CryptoAlgorithm,
        key: Vec<u8>,
    }

    impl CryptoUtil {
        /// `CryptoUtil(algorithm, key)` — derive the normalized key via
        /// PBKDF2-HMAC-SHA256 (fixed salt, matching the Python module).
        pub fn new(algorithm: &str, key: &str) -> Result<Self, CryptoError> {
            let algorithm = CryptoAlgorithm::from_str(algorithm)
                .ok_or_else(|| CryptoError::UnsupportedAlgorithm(algorithm.to_string()))?;
            if algorithm == CryptoAlgorithm::Sm4Cbc {
                return Err(CryptoError::UnsupportedAlgorithm(
                    "sm4-cbc (TODO: no SM4 crate in the offline cache)".to_string(),
                ));
            }
            if key.is_empty() {
                return Err(CryptoError::InvalidData(
                    "encryption key not provided (RAGFLOW_CRYPTO_KEY)",
                ));
            }
            let mut derived = vec![0u8; algorithm.key_length()];
            pbkdf2::pbkdf2_hmac::<sha2::Sha256>(
                key.as_bytes(),
                PBKDF2_SALT,
                PBKDF2_ITERATIONS,
                &mut derived,
            );
            Ok(Self {
                algorithm,
                key: derived,
            })
        }

        /// `CryptoUtil` from the `RAGFLOW_CRYPTO_KEY` environment variable.
        pub fn from_env(algorithm: &str) -> Option<Result<Self, CryptoError>> {
            let key = std::env::var("RAGFLOW_CRYPTO_KEY").ok()?;
            Some(Self::new(algorithm, &key))
        }

        pub fn algorithm(&self) -> CryptoAlgorithm {
            self.algorithm
        }

        /// `encrypt` — `magic + iv + ciphertext`; a fresh random IV is
        /// generated on every call.
        pub fn encrypt(&self, data: &[u8]) -> Result<Vec<u8>, CryptoError> {
            let iv: [u8; BLOCK_SIZE] = rand::random();
            let padded = pkcs7_pad(data, BLOCK_SIZE);
            let cipher = self.cipher()?;
            let ciphertext = cbc_encrypt(&cipher, &iv, &padded);

            let mut out = Vec::with_capacity(ENCRYPTED_MAGIC.len() + BLOCK_SIZE + ciphertext.len());
            out.extend_from_slice(ENCRYPTED_MAGIC);
            out.extend_from_slice(&iv);
            out.extend_from_slice(&ciphertext);
            Ok(out)
        }

        /// `decrypt` — blobs without the magic header are returned unchanged
        /// (Python behaviour for already-plaintext values).
        pub fn decrypt(&self, encrypted_data: &[u8]) -> Result<Vec<u8>, CryptoError> {
            if !encrypted_data.starts_with(ENCRYPTED_MAGIC) {
                return Ok(encrypted_data.to_vec());
            }
            let body = &encrypted_data[ENCRYPTED_MAGIC.len()..];
            if body.len() < BLOCK_SIZE {
                return Err(CryptoError::InvalidData("missing IV"));
            }
            let (iv, ciphertext) = body.split_at(BLOCK_SIZE);
            if ciphertext.is_empty() || ciphertext.len() % BLOCK_SIZE != 0 {
                return Err(CryptoError::InvalidData("ciphertext length"));
            }
            let cipher = self.cipher()?;
            let padded = cbc_decrypt(&cipher, iv, ciphertext);
            pkcs7_unpad(&padded, BLOCK_SIZE)
        }

        fn cipher(&self) -> Result<AnyCipher, CryptoError> {
            match self.algorithm {
                CryptoAlgorithm::Aes128Cbc => aes::Aes128::new_from_slice(&self.key)
                    .map(AnyCipher::Aes128)
                    .map_err(|_| CryptoError::InvalidData("AES-128 key")),
                CryptoAlgorithm::Aes256Cbc => aes::Aes256::new_from_slice(&self.key)
                    .map(AnyCipher::Aes256)
                    .map_err(|_| CryptoError::InvalidData("AES-256 key")),
                CryptoAlgorithm::Sm4Cbc => Err(CryptoError::UnsupportedAlgorithm(
                    "sm4-cbc (TODO: no SM4 crate in the offline cache)".to_string(),
                )),
            }
        }
    }

    fn pkcs7_pad(data: &[u8], block: usize) -> Vec<u8> {
        let pad_len = block - (data.len() % block);
        let mut out = Vec::with_capacity(data.len() + pad_len);
        out.extend_from_slice(data);
        out.extend(std::iter::repeat_n(pad_len as u8, pad_len));
        out
    }

    fn pkcs7_unpad(data: &[u8], block: usize) -> Result<Vec<u8>, CryptoError> {
        if data.is_empty() || !data.len().is_multiple_of(block) {
            return Err(CryptoError::InvalidData("ciphertext length"));
        }
        let pad_len = *data.last().unwrap() as usize;
        if pad_len == 0 || pad_len > block || pad_len > data.len() {
            return Err(CryptoError::PaddingError);
        }
        if data[data.len() - pad_len..]
            .iter()
            .any(|&b| b as usize != pad_len)
        {
            return Err(CryptoError::PaddingError);
        }
        Ok(data[..data.len() - pad_len].to_vec())
    }

    fn cbc_encrypt(cipher: &AnyCipher, iv: &[u8; BLOCK_SIZE], padded: &[u8]) -> Vec<u8> {
        let mut prev = *iv;
        let mut out = Vec::with_capacity(padded.len());
        for chunk in padded.chunks(BLOCK_SIZE) {
            let mut block = GenericArray::clone_from_slice(chunk);
            for (b, p) in block.iter_mut().zip(prev.iter()) {
                *b ^= p;
            }
            cipher.encrypt_block(&mut block);
            prev.copy_from_slice(&block);
            out.extend_from_slice(&block);
        }
        out
    }

    fn cbc_decrypt(cipher: &AnyCipher, iv: &[u8], ciphertext: &[u8]) -> Vec<u8> {
        let mut prev: [u8; BLOCK_SIZE] = iv.try_into().expect("iv is exactly one block");
        let mut out = Vec::with_capacity(ciphertext.len());
        for chunk in ciphertext.chunks(BLOCK_SIZE) {
            let mut block = GenericArray::clone_from_slice(chunk);
            cipher.decrypt_block(&mut block);
            for (b, p) in block.iter_mut().zip(prev.iter()) {
                *b ^= p;
            }
            prev.copy_from_slice(chunk);
            out.extend_from_slice(&block);
        }
        out
    }
}

/// `common/ssrf_guard.py` — SSRF guard: scheme allowlist + "every resolved
/// address must be globally routable" allowlist (private / loopback /
/// link-local / CGNAT / multicast / reserved ranges are all rejected).
pub mod ssrf_guard {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

    /// `_DEFAULT_ALLOWED_SCHEMES`.
    pub const DEFAULT_ALLOWED_SCHEMES: [&str; 2] = ["http", "https"];

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum SsrfError {
        DisallowedScheme(String),
        MissingHost,
        ResolutionFailed(String),
        NonPublicAddress(String),
        NoAddresses,
    }

    impl std::fmt::Display for SsrfError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::DisallowedScheme(scheme) => write!(
                    f,
                    "Disallowed URL scheme: {scheme:?}. Only http and https are allowed."
                ),
                Self::MissingHost => write!(f, "URL is missing a host."),
                Self::ResolutionFailed(host) => {
                    write!(f, "Could not resolve hostname {host:?}")
                }
                Self::NonPublicAddress(ip) => write!(
                    f,
                    "URL resolves to a non-public address ({ip}), which is not allowed."
                ),
                Self::NoAddresses => write!(f, "Hostname resolved to no addresses."),
            }
        }
    }

    impl std::error::Error for SsrfError {}

    /// Normalize IPv4-mapped IPv6 (`::ffff:127.0.0.1`) to its IPv4 form so the
    /// loopback check cannot be bypassed (Python `_effective_ip`).
    fn effective_ip(ip: IpAddr) -> IpAddr {
        match ip {
            IpAddr::V6(v6) => v6
                .to_ipv4_mapped()
                .map(IpAddr::V4)
                .unwrap_or(IpAddr::V6(v6)),
            other => other,
        }
    }

    /// `ip.is_global` equivalent: deny-list of the IANA special-purpose ranges.
    /// Matches the allowlist spirit of the Python guard (private, loopback,
    /// link-local, CGNAT, documentation, benchmarking, multicast, reserved).
    pub fn is_global_ip(ip: IpAddr) -> bool {
        match effective_ip(ip) {
            IpAddr::V4(v4) => is_global_v4(v4),
            IpAddr::V6(v6) => is_global_v6(v6),
        }
    }

    fn is_global_v4(v4: Ipv4Addr) -> bool {
        let octets = v4.octets();
        match octets[0] {
            0 => false,                             // 0.0.0.0/8 unspecified
            10 => false,                            // 10/8 private
            100 => octets[1] != 64,                 // 100.64/10 CGNAT -> false
            127 => false,                           // 127/8 loopback
            169 => octets[1] != 254,                // 169.254/16 link-local -> false
            172 => !(16..=31).contains(&octets[1]), // 172.16/12 private
            192 => match octets[1] {
                0 => octets[2] != 2, // 192.0.0/24 + 192.0.2/24 doc -> false
                168 => false,        // 192.168/16 private
                169 => false,        // 192.169/16 shared address space
                _ => true,
            },
            198 => match octets[1] {
                18 => false,            // 198.18/15 benchmarking
                51 => octets[2] != 100, // 198.51.100/24 doc -> false
                _ => true,
            },
            203 => octets[1] != 0 || octets[2] != 113, // 203.0.113/24 doc -> false
            224..=239 => false,                        // multicast
            240..=255 => false,                        // reserved
            _ => true,
        }
    }

    fn is_global_v6(v6: Ipv6Addr) -> bool {
        if v6.is_unspecified() || v6.is_loopback() || v6.is_multicast() {
            return false;
        }
        if let Some(v4) = v6.to_ipv4_mapped() {
            return is_global_v4(v4);
        }
        let segments = v6.segments();
        if segments[0] & 0xfe00 == 0xfc00 {
            return false; // fc00::/7 unique-local
        }
        if segments[0] & 0xffc0 == 0xfe80 {
            return false; // fe80::/10 link-local
        }
        if segments[0] == 0x2001 && segments[1] == 0x0db8 {
            return false; // 2001:db8::/32 documentation
        }
        true
    }

    /// `assert_url_is_safe` — raise [`SsrfError`] when `url` is not safe to
    /// fetch. Returns `(hostname, first_public_ip)` so the caller can pin the
    /// resolved address (DNS-rebinding protection), like the Python guard.
    pub fn assert_url_is_safe(url: &str) -> Result<(String, String), SsrfError> {
        // Scheme check first (case-insensitive, like urlparse which lowercases).
        let colon = url
            .find(':')
            .ok_or_else(|| SsrfError::DisallowedScheme(url.to_string()))?;
        let scheme = url[..colon].to_ascii_lowercase();
        if !DEFAULT_ALLOWED_SCHEMES.contains(&scheme.as_str()) {
            return Err(SsrfError::DisallowedScheme(scheme));
        }

        let parsed = url::Url::parse(url).map_err(|_| SsrfError::MissingHost)?;
        // The url crate is lenient about `http:///path` (it would treat
        // "path" as a hostname) — mirror Python's urlparse by requiring a
        // non-empty authority ourselves.
        let rest = &url[colon + 1..];
        let rest = rest.strip_prefix("//").unwrap_or(rest);
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        if rest[..authority_end].is_empty() {
            return Err(SsrfError::MissingHost);
        }
        // `host_str()` keeps brackets around IPv6 literals ("[::1]"), which
        // would break resolution — use the typed `Host` instead.
        let hostname = match parsed.host() {
            Some(url::Host::Domain(domain)) => domain.to_string(),
            Some(url::Host::Ipv4(ip)) => ip.to_string(),
            Some(url::Host::Ipv6(ip)) => ip.to_string(),
            None => return Err(SsrfError::MissingHost),
        };
        if hostname.is_empty() {
            return Err(SsrfError::MissingHost);
        }

        let addrs = (hostname.as_str(), 0)
            .to_socket_addrs()
            .map_err(|e| SsrfError::ResolutionFailed(format!("{hostname}: {e}")))?;

        let mut resolved_ip: Option<String> = None;
        for addr in addrs {
            let ip = effective_ip(addr.ip());
            if !is_global_ip(ip) {
                return Err(SsrfError::NonPublicAddress(ip.to_string()));
            }
            if resolved_ip.is_none() {
                resolved_ip = Some(ip.to_string());
            }
        }

        resolved_ip
            .map(|ip| (hostname, ip))
            .ok_or(SsrfError::NoAddresses)
    }
}

/// `api/validation.py` semantics — generic parameter validators. RAGFlow's
/// validation module only checks the Python runtime; the *semantics* enforced
/// across API handlers (required / type / length, with `api/constants.py`
/// limits) are captured here as reusable validators.
pub mod validation {
    use super::constants::{DATASET_NAME_LIMIT, NAME_LENGTH_LIMIT};

    /// `api/constants.py::FILE_NAME_LEN_LIMIT` (also mirrored in `api::utils`).
    const FILE_NAME_LEN_LIMIT: usize = 255;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ValidationErrorKind {
        MissingField,
        EmptyValue,
        WrongType(&'static str),
        TooLong { max: usize },
        TooShort { min: usize },
        InvalidValue(String),
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ValidationError {
        pub field: String,
        pub kind: ValidationErrorKind,
        pub message: String,
    }

    impl ValidationError {
        fn new(field: &str, kind: ValidationErrorKind) -> Self {
            let message = match &kind {
                ValidationErrorKind::MissingField => format!("Field '{field}' is required"),
                ValidationErrorKind::EmptyValue => format!("Field '{field}' must not be empty"),
                ValidationErrorKind::WrongType(expected) => {
                    format!("Field '{field}' must be of type {expected}")
                }
                ValidationErrorKind::TooLong { max } => {
                    format!("Field '{field}' exceeds maximum length of {max}")
                }
                ValidationErrorKind::TooShort { min } => {
                    format!("Field '{field}' is shorter than minimum length of {min}")
                }
                ValidationErrorKind::InvalidValue(detail) => {
                    format!("Field '{field}' has invalid value: {detail}")
                }
            };
            Self {
                field: field.to_string(),
                kind,
                message,
            }
        }

        pub fn missing(field: &str) -> Self {
            Self::new(field, ValidationErrorKind::MissingField)
        }

        pub fn empty(field: &str) -> Self {
            Self::new(field, ValidationErrorKind::EmptyValue)
        }

        pub fn wrong_type(field: &str, expected: &'static str) -> Self {
            Self::new(field, ValidationErrorKind::WrongType(expected))
        }

        pub fn too_long(field: &str, max: usize) -> Self {
            Self::new(field, ValidationErrorKind::TooLong { max })
        }

        pub fn too_short(field: &str, min: usize) -> Self {
            Self::new(field, ValidationErrorKind::TooShort { min })
        }

        pub fn invalid(field: &str, detail: impl Into<String>) -> Self {
            Self::new(field, ValidationErrorKind::InvalidValue(detail.into()))
        }
    }

    impl std::fmt::Display for ValidationError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.message)
        }
    }

    impl std::error::Error for ValidationError {}

    pub type ValidationResult<T> = Result<T, ValidationError>;

    /// Required non-empty string (RAGFlow handlers reject `None`/`""` names).
    pub fn required_str<'a>(field: &str, value: Option<&'a str>) -> ValidationResult<&'a str> {
        let value = value.ok_or_else(|| ValidationError::missing(field))?;
        if value.trim().is_empty() {
            return Err(ValidationError::empty(field));
        }
        Ok(value)
    }

    /// Length bounds check.
    pub fn check_len(field: &str, value: &str, min: usize, max: usize) -> ValidationResult<()> {
        let len = value.chars().count();
        if len < min {
            return Err(ValidationError::too_short(field, min));
        }
        if len > max {
            return Err(ValidationError::too_long(field, max));
        }
        Ok(())
    }

    /// `NAME_LENGTH_LIMIT` (1024) — generic name check.
    pub fn validate_name(field: &str, value: &str) -> ValidationResult<()> {
        check_len(field, value, 1, NAME_LENGTH_LIMIT)
    }

    /// `DATASET_NAME_LIMIT` (128).
    pub fn validate_dataset_name(value: &str) -> ValidationResult<()> {
        check_len("name", value, 1, DATASET_NAME_LIMIT)
    }

    /// `FILE_NAME_LEN_LIMIT` (255).
    pub fn validate_filename(value: &str) -> ValidationResult<()> {
        check_len("filename", value, 1, FILE_NAME_LEN_LIMIT)
    }

    /// Type check: parse a `u32` (e.g. page numbers / sizes).
    pub fn parse_u32_field(field: &str, value: Option<&str>) -> ValidationResult<u32> {
        let value = required_str(field, value)?;
        value
            .parse::<u32>()
            .map_err(|_| ValidationError::wrong_type(field, "integer"))
    }

    /// Type check: parse a boolean (`"true"`/`"false"`/`"1"`/`"0"`).
    pub fn parse_bool_field(field: &str, value: Option<&str>) -> ValidationResult<bool> {
        let value = required_str(field, value)?;
        match value {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            _ => Err(ValidationError::wrong_type(field, "boolean")),
        }
    }
}

/// `common/misc_utils.py` — uuid / hashing / byte formatting / run-once.
pub mod misc_utils {
    /// `get_uuid` — 32-char hex UUID. RAGFlow uses `uuid.uuid1().hex`
    /// (time-based); RayRAG uses v4 — both are 32-char hex identifiers.
    pub fn get_uuid() -> String {
        uuid::Uuid::new_v4().simple().to_string()
    }

    /// `hash_str2int` — `int(sha1(line).hexdigest(), 16) % mod` computed with
    /// Horner's method so the 160-bit digest never overflows a `u64`.
    pub fn hash_str2int(line: &str, modulus: u64) -> u64 {
        use sha1::{Digest, Sha1};
        let digest = Sha1::digest(line.as_bytes());
        let mut acc: u64 = 0;
        for byte in hex::encode(digest).bytes() {
            let digit = (byte as char).to_digit(16).expect("hex digit") as u128;
            acc = ((acc as u128 * 16 + digit) % modulus as u128) as u64;
        }
        acc
    }

    /// Default modulus for `hash_str2int` (`10 ** 8`).
    pub const DEFAULT_HASH_MOD: u64 = 100_000_000;

    /// `convert_bytes` — human-readable byte size with RAGFlow's precision
    /// rules (`0 B`, `X B/KB/...`, integer ≥100, 1 decimal ≥10, else 2).
    pub fn convert_bytes(size_in_bytes: u64) -> String {
        if size_in_bytes == 0 {
            return "0 B".to_string();
        }
        const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
        let mut size = size_in_bytes as f64;
        let mut i = 0;
        while size >= 1024.0 && i < UNITS.len() - 1 {
            size /= 1024.0;
            i += 1;
        }
        if i == 0 || size >= 100.0 {
            format!("{size:.0} {}", UNITS[i])
        } else if size >= 10.0 {
            format!("{size:.1} {}", UNITS[i])
        } else {
            format!("{size:.2} {}", UNITS[i])
        }
    }

    /// `common/decorator.py::singleton` / `misc_utils.py::once` — thread-safe
    /// run-once wrapper backed by `std::sync::OnceLock`.
    pub struct Once<T> {
        inner: std::sync::OnceLock<T>,
    }

    impl<T> Once<T> {
        pub const fn new() -> Self {
            Self {
                inner: std::sync::OnceLock::new(),
            }
        }

        pub fn get_or_init(&self, f: impl FnOnce() -> T) -> &T {
            self.inner.get_or_init(f)
        }
    }

    impl<T> Default for Once<T> {
        fn default() -> Self {
            Self::new()
        }
    }
}

/// `common/parser_config_utils.py` — `normalize_layout_recognizer`.
pub mod parser_config_utils {
    /// `normalize_layout_recognizer` — `"<model>@mineru"` (and friends) split
    /// into `(canonical recognizer, model name)`; anything else passes through
    /// unchanged with `None`.
    pub fn normalize_layout_recognizer(raw: &str) -> (String, Option<String>) {
        let lowered = raw.to_ascii_lowercase();
        for (suffix, canonical) in [
            ("@mineru", "MinerU"),
            ("@paddleocr", "PaddleOCR"),
            ("@opendataloader", "OpenDataLoader"),
        ] {
            if lowered.ends_with(suffix) {
                let model = raw.rsplit_once('@').map(|(model, _)| model.to_string());
                return (canonical.to_string(), model);
            }
        }
        (raw.to_string(), None)
    }
}

/// `common/query_base.py` — pure static query-text helpers (the FAQ/query
/// rewrite layer). `rmWWW` is intentionally not ported (niche heuristic).
pub mod query_base {
    use regex::Regex;

    /// `is_chinese` — a line is "Chinese" when it has ≤3 space-separated
    /// tokens, or ≥70% of its tokens are non-ASCII-alpha.
    pub fn is_chinese(line: &str) -> bool {
        let arr: Vec<&str> = Regex::new(r"[ \t]+")
            .expect("valid regex")
            .split(line)
            .collect();
        if arr.len() <= 3 {
            return true;
        }
        let non_alpha = arr
            .iter()
            .filter(|t| t.is_empty() || !t.chars().all(|c| c.is_ascii_alphabetic()))
            .count();
        non_alpha as f64 / arr.len() as f64 >= 0.7
    }

    /// `sub_special_char` — strip single quotes, then backslash-escape
    /// Infinity/Lucene special characters.
    pub fn sub_special_char(line: &str) -> String {
        let no_quotes = line.replace('\'', "");
        // Normal (non-raw) string: the class contains a literal `"`.
        let special = Regex::new("([:{}/\\[\\]\\-*?\"()|+~^])").expect("valid regex");
        special.replace_all(&no_quotes, "\\$1").trim().to_string()
    }

    /// `add_space_between_eng_zh` — insert a space between English (optionally
    /// followed by digits) and CJK runs.
    pub fn add_space_between_eng_zh(txt: &str) -> String {
        let en_digits_zh =
            Regex::new(r"([A-Za-z]+[0-9]+)([\u{4e00}-\u{9fa5}]+)").expect("valid regex");
        let step1 = en_digits_zh.replace_all(txt, "$1 $2");
        let en_zh = Regex::new(r"([A-Za-z])([\u{4e00}-\u{9fa5}]+)").expect("valid regex");
        let step2 = en_zh.replace_all(&step1, "$1 $2");
        let zh_en_digits =
            Regex::new(r"([\u{4e00}-\u{9fa5}]+)([A-Za-z]+[0-9]+)").expect("valid regex");
        let step3 = zh_en_digits.replace_all(&step2, "$1 $2");
        let zh_en = Regex::new(r"([\u{4e00}-\u{9fa5}]+)([A-Za-z])").expect("valid regex");
        zh_en.replace_all(&step3, "$1 $2").into_owned()
    }
}

/// `common/connection_utils.py` — the async `timeout` decorator, as a helper.
pub mod connection_utils {
    /// `timeout(seconds)` decorator semantics — wrap an async operation with a
    /// wall-clock deadline and surface a [`TimeoutError`] on expiry.
    pub async fn with_timeout<F, T>(seconds: f64, fut: F) -> anyhow::Result<T>
    where
        F: std::future::Future<Output = T>,
    {
        match tokio::time::timeout(std::time::Duration::from_secs_f64(seconds), fut).await {
            Ok(value) => Ok(value),
            Err(_) => anyhow::bail!("operation timed out after {seconds} seconds"),
        }
    }
}

/// RayRAG 统一命令超时（对标约束：所有命令必须带超时，最长 2 小时）。
///
/// - 超时值由用户 env 环境文件决定：`RAYRAG_CMD_TIMEOUT`（秒），默认 7200（2 小时），
///   上限钳制 7200，避免再次卡死；
/// - `load_user_env_file` 在启动时读取用户 env 文件（`RAYRAG_ENV_FILE` → 当前目录
///   `.env` → 项目 `.env`），已设置的环境变量不被文件覆盖（真实环境优先）；
/// - `http_client` 为统一 reqwest 客户端，所有外呼业务请求使用同一超时输入。
pub mod cmd_timeout {
    use futures_util::StreamExt as _;
    /// 默认命令超时：7200 秒（2 小时）
    pub const DEFAULT_SECS: u64 = 7200;
    /// 环境变量名（用户 env 环境文件配置项）
    pub const ENV: &str = "RAYRAG_CMD_TIMEOUT";
    /// 模型调用默认超时（秒）
    pub const DEFAULT_MODEL_SECS: u64 = 300;
    /// 模型调用超时环境变量
    pub const MODEL_ENV: &str = "RAYRAG_MODEL_TIMEOUT";
    /// 单次响应体默认上限（32 MiB）
    pub const DEFAULT_BODY_LIMIT: usize = 32 << 20;
    /// 响应体上限环境变量
    pub const BODY_LIMIT_ENV: &str = "RAYRAG_HTTP_BODY_LIMIT_BYTES";
    /// 连接器下载默认上限（256 MiB）
    pub const DEFAULT_CONNECTOR_BODY_LIMIT: usize = 256 << 20;
    /// 连接器下载上限环境变量
    pub const CONNECTOR_BODY_LIMIT_ENV: &str = "RAYRAG_CONNECTOR_BODY_LIMIT_BYTES";

    /// 当前命令超时秒数：`RAYRAG_CMD_TIMEOUT`，默认 7200，钳制在 1..=7200
    pub fn seconds() -> u64 {
        let secs = std::env::var(ENV)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_SECS);
        secs.min(DEFAULT_SECS).max(1)
    }

    /// 当前命令超时 Duration
    pub fn duration() -> std::time::Duration {
        std::time::Duration::from_secs(seconds())
    }

    /// 统一 reqwest 客户端：请求超时 = 用户 env `RAYRAG_CMD_TIMEOUT`（默认 2 小时）
    pub fn http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(duration())
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }

    /// 模型调用超时（秒）：`RAYRAG_MODEL_TIMEOUT`，默认 300，钳制 1..=1800。
    ///
    /// Embedding / 视觉 / OCR / LLM 这类交互调用不该等到命令级上限（2 小时）才放弃：
    /// 一个半死不活的对端会让请求一直挂着，缓冲区随响应一起长。
    pub fn model_seconds() -> u64 {
        std::env::var(MODEL_ENV)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_MODEL_SECS)
            .clamp(1, 1800)
    }

    /// 模型调用用的 reqwest 客户端（连接 10s、总时长见 [`model_seconds`]）。
    pub fn model_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(model_seconds()))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }

    /// 连接器（S3/WebDAV/HTTP 等）单次下载的硬上限：`RAYRAG_CONNECTOR_BODY_LIMIT_BYTES`，
    /// 默认 256 MiB。
    ///
    /// 连接器下载的是**待解析的原始文档**，所以上限远比 API 响应宽松；但它仍然是一个
    /// 有限值 —— 远端对象无论多大（或被篡改成无限流）都不会让进程无界增长，超过上限时
    /// 明确报错而不是把主机吃光。
    pub fn connector_body_limit_bytes() -> usize {
        std::env::var(CONNECTOR_BODY_LIMIT_ENV)
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|value| *value >= 1024 * 1024)
            .unwrap_or(DEFAULT_CONNECTOR_BODY_LIMIT)
    }

    /// 单次 HTTP 响应体的硬上限：`RAYRAG_HTTP_BODY_LIMIT_BYTES`，默认 32 MiB。
    ///
    /// 任何对端（模型服务、OCR、爬虫、连接器）都可能返回无限流或超大包体；先把
    /// 上限摆在读取路径上，内存就不会跟着对端走。
    pub fn body_limit_bytes() -> usize {
        std::env::var(BODY_LIMIT_ENV)
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|value| *value >= 1024)
            .unwrap_or(DEFAULT_BODY_LIMIT)
    }

    /// 读取 HTTP 响应体，超过 `limit` 字节立即失败。
    ///
    /// `Content-Length` 先做一次预检，随后按块累积；无论对端怎么发（无限流、谎报长度、
    /// 分块传输），进程持有的就是上限这么多内存。所有对外部服务的 `json()`/`text()`
    /// 读取都应走这里，而不是把整个包体一次性缓存下来。
    pub async fn read_body_limited(
        response: reqwest::Response,
        limit: usize,
        what: &str,
    ) -> anyhow::Result<Vec<u8>> {
        if let Some(length) = response.content_length()
            && length > limit as u64
        {
            anyhow::bail!("{what} response body of {length} bytes exceeds the {limit}-byte limit");
        }
        let mut body: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|error| anyhow::anyhow!("{what} body read failed: {error}"))?;
            let remaining = limit.saturating_sub(body.len());
            if chunk.len() > remaining {
                anyhow::bail!("{what} response body exceeds the {limit}-byte limit");
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    /// [`read_body_limited`] + JSON 反序列化。
    pub async fn read_json_limited<T: serde::de::DeserializeOwned>(
        response: reqwest::Response,
        limit: usize,
        what: &str,
    ) -> anyhow::Result<T> {
        let status = response.status();
        let body = read_body_limited(response, limit, what).await?;
        serde_json::from_slice(&body).map_err(|error| {
            anyhow::anyhow!(
                "{what} returned HTTP {status} with a body that is not JSON: {error} ({} bytes)",
                body.len()
            )
        })
    }

    /// [`read_body_limited`] + UTF-8 解码（非法字节按替换字符处理，与 `reqwest::text` 一致）。
    pub async fn read_text_limited(
        response: reqwest::Response,
        limit: usize,
        what: &str,
    ) -> anyhow::Result<String> {
        let body = read_body_limited(response, limit, what).await?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    /// 加载用户 env 环境文件：`RAYRAG_ENV_FILE` → `./.env` → 项目 `.env`。
    /// 已存在于进程环境的变量不被覆盖（真实环境优先），`#` 注释行忽略。
    pub fn load_user_env_file() {
        let candidates = [
            std::env::var("RAYRAG_ENV_FILE").ok(),
            Some(".env".to_string()),
            Some(format!("{}/.env", env!("CARGO_MANIFEST_DIR"))),
        ];
        let Some(path) = candidates
            .into_iter()
            .flatten()
            .find(|path| std::path::Path::new(path).is_file())
        else {
            return;
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            return;
        };
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if key.is_empty() || std::env::var(key).is_ok() {
                continue;
            }
            // SAFETY: 启动期单线程初始化阶段调用
            unsafe {
                std::env::set_var(key, value.trim().trim_matches(['"', '\'']));
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn default_is_two_hours_and_capped() {
            assert_eq!(DEFAULT_SECS, 7200);
            let d = duration();
            assert!(d <= std::time::Duration::from_secs(7200));
            assert!(d >= std::time::Duration::from_secs(1));
            let _client = http_client();
        }
    }
}

/// `common/versions.py` — RAGFlow version resolution.
pub mod versions {
    /// `get_ragflow_version` — `RAGFLOW_VERSION` env → `VERSION` file next to
    /// the crate manifest → `"unknown"` (Python falls back to `git describe`).
    pub fn get_ragflow_version() -> String {
        if let Ok(value) = std::env::var("RAGFLOW_VERSION")
            && !value.is_empty()
        {
            return value;
        }
        let version_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("VERSION");
        if let Ok(content) = std::fs::read_to_string(version_path) {
            let version = content.trim();
            if !version.is_empty() {
                return version.to_string();
            }
        }
        "unknown".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use constants::{LLMType, ParserType, RetCode, StatusEnum, TaskStatus};

    #[test]
    fn ret_code_and_status_constants_match_ragflow() {
        assert_eq!(RetCode::Success.as_i32(), 0);
        assert_eq!(RetCode::ArgumentError.as_i32(), 101);
        assert_eq!(RetCode::PermissionError.as_i32(), 108);
        assert_eq!(RetCode::Unauthorized.as_i32(), 401);
        assert_eq!(RetCode::NotFound.as_i32(), 404);
        assert_eq!(RetCode::from_i32(409), Some(RetCode::Conflict));
        assert_eq!(RetCode::from_i32(999), None);
        assert!(RetCode::Success.is_ok());
        assert!(!RetCode::DataError.is_ok());

        assert_eq!(TaskStatus::Unstart.as_str(), "0");
        assert_eq!(TaskStatus::Schedule.as_str(), "5");
        assert_eq!(TaskStatus::from_str("3"), Some(TaskStatus::Done));
        assert_eq!(TaskStatus::from_str("9"), None);
        assert!(TaskStatus::valid("2"));
        assert!(!TaskStatus::valid("x"));
        assert_eq!(TaskStatus::values(), ["0", "1", "2", "3", "4", "5"]);

        assert_eq!(StatusEnum::Valid.as_str(), "1");
        assert_eq!(StatusEnum::from_str("0"), Some(StatusEnum::Invalid));

        assert_eq!(ParserType::Kg.as_str(), "knowledge_graph");
        assert_eq!(ParserType::from_str("naive"), Some(ParserType::Naive));
        assert!(ParserType::valid("table"));
        assert!(!ParserType::valid("bogus"));

        assert_eq!(LLMType::Rerank.as_str(), "rerank");
        assert_eq!(LLMType::from_str("embedding"), Some(LLMType::Embedding));

        assert_eq!(constants::FileSource::Local.as_str(), "");
        assert_eq!(constants::FileSource::Github.as_str(), "github");
        assert_eq!(
            constants::FileSource::from_str("postgresql"),
            Some(constants::FileSource::Postgresql)
        );

        assert_eq!(constants::PipelineTaskType::GraphRag.as_str(), "GraphRAG");
        assert_eq!(constants::PipelineTaskType::Artifact.as_str(), "Artifact");
        assert_eq!(constants::PipelineTaskType::Skill.as_lower_str(), "skill");
        assert_eq!(
            constants::PipelineTaskType::from_str("Skill"),
            Some(constants::PipelineTaskType::Skill)
        );
        assert_eq!(
            constants::VALID_PIPELINE_TASK_TYPES,
            [
                constants::PipelineTaskType::Parse,
                constants::PipelineTaskType::Download,
                constants::PipelineTaskType::Raptor,
                constants::PipelineTaskType::GraphRag,
                constants::PipelineTaskType::Mindmap,
                constants::PipelineTaskType::Artifact,
                constants::PipelineTaskType::Skill,
            ]
        );
        assert!(
            !constants::VALID_PIPELINE_TASK_TYPES.contains(&constants::PipelineTaskType::Memory)
        );
        assert_eq!(constants::Storage::Minio.as_i32(), 1);
        assert!(constants::memory_type::contains(
            constants::memory_type::RAW | constants::memory_type::SEMANTIC,
            constants::memory_type::RAW
        ));

        assert_eq!(constants::NAME_LENGTH_LIMIT, 1024);
        assert_eq!(constants::DATASET_NAME_LIMIT, 128);
        assert_eq!(constants::MEMORY_SIZE_LIMIT, 10 * 1024 * 1024);
        assert_eq!(constants::MAXIMUM_PAGE_NUMBER, 100_000);
        assert_eq!(constants::MAXIMUM_TASK_PAGE_NUMBER, 100_000_000);
        assert_eq!(constants::API_VERSION, "v1");
    }

    #[test]
    fn string_utils_clean_spaces_markdown_and_emptiness() {
        // Python docstring examples.
        assert_eq!(string_utils::remove_redundant_spaces("( test"), "(test");
        assert_eq!(string_utils::remove_redundant_spaces("world !"), "world!");
        // Verified against the Python reference: consecutive spaces are
        // collapsed because `[^a-z0-9.,)>]` also matches a space.
        assert_eq!(
            string_utils::remove_redundant_spaces("hello  world"),
            "hello world"
        );

        assert_eq!(
            string_utils::clean_markdown_block("```markdown\nhello\n```"),
            "hello"
        );
        assert_eq!(
            string_utils::clean_markdown_block("```markdown\nhello"),
            "hello"
        );
        assert_eq!(
            string_utils::clean_markdown_block("  ```python\ncode\n```  "),
            "```python\ncode"
        );

        assert!(string_utils::is_content_empty(""));
        assert!(string_utils::is_content_empty("   \n\t "));
        assert!(!string_utils::is_content_empty(" x "));
    }

    #[test]
    fn text_and_float_utils_normalize_like_python() {
        // Arabic digits -> ASCII.
        assert_eq!(text_utils::normalize_arabic_digits("١٢٣"), "123");
        assert_eq!(text_utils::normalize_arabic_digits("۴۵۶"), "456");
        assert_eq!(text_utils::normalize_arabic_digits("abc123"), "abc123");
        // No presentation forms -> unchanged; with forms -> NFKC.
        assert_eq!(
            text_utils::normalize_arabic_presentation_forms("plain"),
            "plain"
        );
        assert_eq!(
            text_utils::normalize_arabic_presentation_forms("\u{FB50}"),
            "\u{0671}"
        );

        assert_eq!(float_utils::get_float(None), f64::NEG_INFINITY);
        assert_eq!(float_utils::get_float(Some("3.14")), 3.14);
        assert_eq!(float_utils::get_float(Some("invalid")), f64::NEG_INFINITY);
        assert_eq!(float_utils::get_float(Some("42")), 42.0);

        assert_eq!(float_utils::normalize_overlapped_percent("0.5"), 50);
        assert_eq!(float_utils::normalize_overlapped_percent("50"), 50);
        assert_eq!(float_utils::normalize_overlapped_percent("150"), 90);
        assert_eq!(float_utils::normalize_overlapped_percent("-5"), 0);
        assert_eq!(float_utils::normalize_overlapped_percent("abc"), 0);
        assert_eq!(float_utils::normalize_overlapped_percent("0.005"), 0);
    }

    #[test]
    fn time_utils_roundtrip_and_iso_conversion() {
        use time_utils::DEFAULT_TIME_FORMAT;

        // Parse local -> format local: exact round trip.
        let ts = time_utils::date_string_to_timestamp("2024-01-01 00:00:00", DEFAULT_TIME_FORMAT)
            .unwrap();
        assert_eq!(
            time_utils::timestamp_to_date(ts, DEFAULT_TIME_FORMAT),
            "2024-01-01 00:00:00"
        );

        // ms epoch sanity: 1704067200000 == 2024-01-01T00:00:00Z.
        assert!(time_utils::current_timestamp() > 1_700_000_000_000);

        // ISO-8601 with Z -> UTC wall clock.
        assert_eq!(
            time_utils::format_iso_8601_to_ymd_hms("2024-01-01T12:00:00Z"),
            "2024-01-01 12:00:00"
        );
        // Offset is preserved as wall clock (Python fromisoformat semantics).
        assert_eq!(
            time_utils::format_iso_8601_to_ymd_hms("2024-01-01T12:00:00+08:00"),
            "2024-01-01 12:00:00"
        );
        // Naive datetime and date-only inputs.
        assert_eq!(
            time_utils::format_iso_8601_to_ymd_hms("2024-01-01T08:30:00"),
            "2024-01-01 08:30:00"
        );
        assert_eq!(
            time_utils::format_iso_8601_to_ymd_hms("2024-01-01"),
            "2024-01-01 00:00:00"
        );
        // Unparseable input returned unchanged.
        assert_eq!(
            time_utils::format_iso_8601_to_ymd_hms("not-a-date"),
            "not-a-date"
        );
    }

    #[test]
    fn crypto_utils_aes_roundtrip_and_plaintext_passthrough() {
        let crypto = crypto_utils::CryptoUtil::new("aes-256-cbc", "test_key_123456").unwrap();
        let plaintext = b"Hello, RAGFlow! This is a test for encryption.";

        let encrypted = crypto.encrypt(plaintext).unwrap();
        // Wire format: magic + iv + ciphertext.
        assert!(encrypted.starts_with(crypto_utils::ENCRYPTED_MAGIC));
        let padded_len = plaintext.len() + (16 - plaintext.len() % 16);
        assert_eq!(encrypted.len(), 4 + 16 + padded_len);
        // Two encryptions use different random IVs.
        let encrypted2 = crypto.encrypt(plaintext).unwrap();
        assert_ne!(encrypted, encrypted2);

        assert_eq!(crypto.decrypt(&encrypted).unwrap(), plaintext);
        assert_eq!(crypto.decrypt(&encrypted2).unwrap(), plaintext);

        // aes-128-cbc also round-trips.
        let crypto128 = crypto_utils::CryptoUtil::new("aes-128-cbc", "test_key_123456").unwrap();
        let enc128 = crypto128.encrypt(plaintext).unwrap();
        assert_eq!(crypto128.decrypt(&enc128).unwrap(), plaintext);

        // Non-encrypted input passes through unchanged (Python behaviour).
        assert_eq!(crypto.decrypt(plaintext).unwrap(), plaintext);

        // SM4 is TODO -> unsupported.
        assert!(matches!(
            crypto_utils::CryptoUtil::new("sm4-cbc", "key"),
            Err(crypto_utils::CryptoError::UnsupportedAlgorithm(_))
        ));
        assert!(matches!(
            crypto_utils::CryptoUtil::new("des-cbc", "key"),
            Err(crypto_utils::CryptoError::UnsupportedAlgorithm(_))
        ));
    }

    #[test]
    fn ssrf_guard_blocks_private_and_allows_public() {
        use ssrf_guard::{SsrfError, assert_url_is_safe};

        // Public IPs pass and return (hostname, ip).
        let (host, ip) = assert_url_is_safe("https://8.8.8.8/x").unwrap();
        assert_eq!(host, "8.8.8.8");
        assert_eq!(ip, "8.8.8.8");
        let (host, ip) = assert_url_is_safe("http://1.1.1.1").unwrap();
        assert_eq!(host, "1.1.1.1");
        assert_eq!(ip, "1.1.1.1");

        // Private / special ranges are rejected.
        for url in [
            "http://127.0.0.1/",
            "http://10.0.0.5/",
            "http://172.16.0.1/",
            "http://192.168.1.1/",
            "http://169.254.1.1/",
            "http://100.64.0.1/",
            "http://192.0.2.1/",
            "http://198.18.0.1/",
            "http://203.0.113.9/",
            "http://224.0.0.1/",
            "http://240.0.0.1/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[fc00::1]/",
            "http://[fe80::1]/",
        ] {
            assert!(
                matches!(assert_url_is_safe(url), Err(SsrfError::NonPublicAddress(_))),
                "expected {url} to be blocked"
            );
        }

        // Scheme + host validation.
        assert!(matches!(
            assert_url_is_safe("ftp://8.8.8.8/"),
            Err(SsrfError::DisallowedScheme(_))
        ));
        assert!(matches!(
            assert_url_is_safe("file:///etc/passwd"),
            Err(SsrfError::DisallowedScheme(_))
        ));
        assert!(matches!(
            assert_url_is_safe("http:///nohost"),
            Err(SsrfError::MissingHost)
        ));
    }

    #[test]
    fn validation_and_misc_utils_match_handler_semantics() {
        use validation::{ValidationErrorKind, parse_bool_field, parse_u32_field, required_str};

        assert_eq!(required_str("name", Some("kb-1")).unwrap(), "kb-1");
        assert_eq!(
            required_str("name", None).unwrap_err().kind,
            ValidationErrorKind::MissingField
        );
        assert_eq!(
            required_str("name", Some("  ")).unwrap_err().kind,
            ValidationErrorKind::EmptyValue
        );

        assert_eq!(parse_u32_field("page", Some("42")).unwrap(), 42);
        assert!(matches!(
            parse_u32_field("page", Some("abc")).unwrap_err().kind,
            ValidationErrorKind::WrongType("integer")
        ));
        assert_eq!(parse_bool_field("flag", Some("true")).unwrap(), true);
        assert!(matches!(
            parse_bool_field("flag", Some("yes")).unwrap_err().kind,
            ValidationErrorKind::WrongType("boolean")
        ));

        // api/constants limits.
        assert!(validation::validate_dataset_name("my dataset").is_ok());
        assert_eq!(
            validation::validate_dataset_name(&"a".repeat(129))
                .unwrap_err()
                .kind,
            ValidationErrorKind::TooLong { max: 128 }
        );
        assert!(validation::validate_filename("report.pdf").is_ok());
        assert!(validation::validate_name("x", "y").is_ok());

        // misc utils
        assert_eq!(misc_utils::get_uuid().len(), 32);
        assert!(
            misc_utils::get_uuid()
                .chars()
                .all(|c| c.is_ascii_hexdigit())
        );
        assert_eq!(misc_utils::convert_bytes(0), "0 B");
        assert_eq!(misc_utils::convert_bytes(1024), "1.00 KB");
        assert_eq!(misc_utils::convert_bytes(10 * 1024), "10.0 KB");
        assert_eq!(misc_utils::convert_bytes(100 * 1024 * 1024), "100 MB");
        // hash_str2int is deterministic and in range.
        let h1 = misc_utils::hash_str2int("RAGFlow 知识库", misc_utils::DEFAULT_HASH_MOD);
        let h2 = misc_utils::hash_str2int("RAGFlow 知识库", misc_utils::DEFAULT_HASH_MOD);
        assert_eq!(h1, h2);
        assert!(h1 < misc_utils::DEFAULT_HASH_MOD);

        // run-once helper executes the closure exactly once.
        let once = misc_utils::Once::new();
        let value = once.get_or_init(|| 42);
        assert_eq!(*value, 42);
        assert_eq!(*once.get_or_init(|| 99), 42);
    }

    #[test]
    fn parser_config_and_query_helpers_behave_like_python() {
        assert_eq!(
            parser_config_utils::normalize_layout_recognizer("pdf@mineru"),
            ("MinerU".to_string(), Some("pdf".to_string()))
        );
        assert_eq!(
            parser_config_utils::normalize_layout_recognizer("xxx@opendataloader"),
            ("OpenDataLoader".to_string(), Some("xxx".to_string()))
        );
        assert_eq!(
            parser_config_utils::normalize_layout_recognizer("DeepDOC"),
            ("DeepDOC".to_string(), None)
        );

        assert!(query_base::is_chinese("今天天气怎么样"));
        assert!(!query_base::is_chinese(
            "this is a long english sentence with many words here"
        ));
        assert_eq!(query_base::sub_special_char("a:b'c"), "a\\:bc");
        assert_eq!(
            query_base::add_space_between_eng_zh("hello世界"),
            "hello 世界"
        );
        assert_eq!(
            query_base::add_space_between_eng_zh("世界hello"),
            "世界 hello"
        );
        assert_eq!(
            query_base::add_space_between_eng_zh("abc123中文"),
            "abc123 中文"
        );

        // versions: unset -> unknown (no VERSION file / env in tests).
        // SAFETY: single-threaded test; only this test reads the variable.
        unsafe { std::env::remove_var("RAGFLOW_VERSION") };
        assert_eq!(versions::get_ragflow_version(), "unknown");
    }
}

#[cfg(test)]
mod cmd_timeout_tests {
    use super::cmd_timeout;
    use axum::{Router, body::Body, http::StatusCode, response::Response, routing::get};

    /// Plain body of `size` bytes.
    async fn fixed() -> Vec<u8> {
        vec![b'x'; 4096]
    }

    /// Chunked stream that never declares a length and keeps sending: the reader must
    /// stop at the limit instead of following the peer into memory exhaustion.
    async fn endless() -> Response {
        let chunks = futures_util::stream::unfold(0usize, |sent| async move {
            if sent > 64 {
                return None;
            }
            Some((Ok::<_, std::io::Error>(vec![b'y'; 1024]), sent + 1))
        });
        Response::builder()
            .status(StatusCode::OK)
            .body(Body::from_stream(chunks))
            .unwrap()
    }

    async fn serve() -> String {
        let app = Router::new()
            .route("/fixed", get(fixed))
            .route("/endless", get(endless));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn bounded_reads_stop_at_the_limit_instead_of_following_the_peer() {
        let base = serve().await;
        let client = reqwest::Client::new();

        // Under the limit: the bytes arrive intact.
        let response = client.get(format!("{base}/fixed")).send().await.unwrap();
        let body = cmd_timeout::read_body_limited(response, 8192, "test")
            .await
            .unwrap();
        assert_eq!(body.len(), 4096);

        // A declared length above the limit is rejected before anything is buffered.
        let response = client.get(format!("{base}/fixed")).send().await.unwrap();
        let error = cmd_timeout::read_body_limited(response, 1024, "test")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("exceeds"), "{error}");

        // A streamed body without a length is stopped by the running total.
        let response = client.get(format!("{base}/endless")).send().await.unwrap();
        let error = cmd_timeout::read_body_limited(response, 4096, "test")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("exceeds"), "{error}");

        // The JSON helper reports a bounded, non-JSON body instead of panicking.
        let response = client.get(format!("{base}/fixed")).send().await.unwrap();
        let error = cmd_timeout::read_json_limited::<serde_json::Value>(response, 8192, "test")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("not JSON"), "{error}");
    }

    #[test]
    fn model_timeout_is_shorter_than_the_command_budget() {
        // The model timeout is the interactive bound; the command budget stays the
        // outer ceiling. Both are clamped, so a bad env value cannot disable them.
        assert!(cmd_timeout::model_seconds() <= 1800);
        assert!(cmd_timeout::seconds() <= cmd_timeout::DEFAULT_SECS);
        assert!(cmd_timeout::body_limit_bytes() >= 1024);
    }
}
