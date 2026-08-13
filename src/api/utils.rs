//! Service-layer utility helpers mirroring RAGFlow's `api/utils/*` modules.
//!
//! Coverage map (Python reference -> Rust here):
//! - `file_utils.py`          -> [`FileType`], [`filename_type`], [`normalize_filename_for_type`],
//!                               [`sanitize_path`], size-limit constants, [`hash_md5_hex`]
//! - `common.py::hash128`     -> [`hash128`] (xxh3-128, 32 hex chars)
//! - `api_utils.py::get_parser_config` -> [`get_parser_config`] (per-chunk-method parser defaults
//!                               with recursive deep merge, incl. parent_child flattening)
//! - `db/services/knowledgebase_service.py::create_with_name` -> [`init_dataset_defaults`]
//!                               (KB init defaults: parser_id "naive", merged parser_config, llm_id)
//! - `json_encode.py` + datetime handling -> [`datetime`] submodule (Unix-ms timestamps, RFC3339,
//!                               `%Y-%m-%d %H:%M:%S` RAGFlow JSON formatting)
//!
//! Everything here is additive: no existing public API is modified.

use serde::{Deserialize, Serialize};

/// RAGFlow `api/constants.py::FILE_NAME_LEN_LIMIT`.
pub const FILE_NAME_LEN_LIMIT: usize = 255;
/// RAGFlow `api/constants.py::IMG_BASE64_PREFIX`.
pub const IMG_BASE64_PREFIX: &str = "data:image/png;base64,";
/// RAGFlow `file_utils.py::MAX_BLOB_SIZE_THUMBNAIL` (50 MiB).
pub const MAX_BLOB_SIZE_THUMBNAIL: usize = 50 * 1024 * 1024;
/// RAGFlow `file_utils.py::MAX_BLOB_SIZE_PDF` (100 MiB).
pub const MAX_BLOB_SIZE_PDF: usize = 100 * 1024 * 1024;

/// RAGFlow `api/db/__init__.py::FileType`.
///
/// `filename_type` derives only document/media classifications from a filename;
/// `Virtual` and `Folder` are retained for database/API records created without
/// a physical upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileType {
    Pdf,
    Doc,
    Visual,
    Aural,
    Virtual,
    Folder,
    Other,
}

impl FileType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Doc => "doc",
            Self::Visual => "visual",
            Self::Aural => "aural",
            Self::Virtual => "virtual",
            Self::Folder => "folder",
            Self::Other => "other",
        }
    }
}

impl std::fmt::Display for FileType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `file_utils.py::_normalize_filename_for_type` — safe basename for type detection.
///
/// Returns `None` for `None`, non-string, empty, or over-long names (len > 255),
/// mirroring the Python `("", False)` failure path.
pub fn normalize_filename_for_type(filename: Option<&str>) -> Option<String> {
    let filename = filename?;
    let base = std::path::Path::new(filename)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if base.is_empty() || base.len() > FILE_NAME_LEN_LIMIT {
        return None;
    }
    Some(base)
}

fn ends_with(name: &str, suffix: &str) -> bool {
    name.ends_with(suffix)
}

/// `file_utils.py::filename_type` — classify a file by extension.
///
/// Extension lists are kept byte-for-byte aligned with RAGFlow:
/// PDF -> `pdf`; DOC -> msg|eml|doc|...|epub; AURAL -> wav|flac|...|opus;
/// VISUAL -> jpg|jpeg|...|mkv; everything else -> OTHER.
pub fn filename_type(filename: Option<&str>) -> FileType {
    let Some(name) = normalize_filename_for_type(filename) else {
        return FileType::Other;
    };

    if ends_with(&name, ".pdf") {
        return FileType::Pdf;
    }

    const DOC_EXTENSIONS: &[&str] = &[
        "msg", "eml", "doc", "docx", "ppt", "pptx", "yml", "xml", "htm", "json", "jsonl", "ldjson",
        "csv", "txt", "ini", "xls", "xlsx", "wps", "rtf", "hlp", "pages", "numbers", "key", "md",
        "mdx", "py", "js", "java", "c", "cpp", "h", "php", "go", "ts", "sh", "cs", "kt", "html",
        "sql", "epub",
    ];
    const AURAL_EXTENSIONS: &[&str] = &[
        "wav", "flac", "ape", "alac", "wavpack", "wv", "mp3", "aac", "ogg", "vorbis", "opus",
    ];
    const VISUAL_EXTENSIONS: &[&str] = &[
        "jpg", "jpeg", "png", "tif", "gif", "pcx", "tga", "exif", "fpx", "svg", "psd", "cdr",
        "pcd", "dxf", "ufo", "eps", "ai", "raw", "wmf", "webp", "avif", "apng", "icon", "ico",
        "mpg", "mpeg", "avi", "rm", "rmvb", "mov", "wmv", "asf", "dat", "asx", "wvx", "mpe", "mpa",
        "mp4", "mkv",
    ];

    for ext in DOC_EXTENSIONS {
        if ends_with(&name, &format!(".{ext}")) {
            return FileType::Doc;
        }
    }
    for ext in AURAL_EXTENSIONS {
        if ends_with(&name, &format!(".{ext}")) {
            return FileType::Aural;
        }
    }
    for ext in VISUAL_EXTENSIONS {
        if ends_with(&name, &format!(".{ext}")) {
            return FileType::Visual;
        }
    }

    FileType::Other
}

/// `file_utils.py::sanitize_path` — normalize a user-supplied path segment.
///
/// Converts backslashes to forward slashes, strips leading/trailing slashes, drops
/// `.` and `..` segments, and removes characters outside `[A-Za-z0-9_\-/]`.
pub fn sanitize_path(raw_path: Option<&str>) -> String {
    let Some(raw_path) = raw_path else {
        return String::new();
    };
    let raw_path = raw_path.trim();
    if raw_path.is_empty() {
        return String::new();
    }
    let normalized = raw_path.replace('\\', "/");
    let normalized = normalized.trim_matches('/');
    let mut parts: Vec<&str> = Vec::new();
    for segment in normalized.split('/') {
        if !segment.is_empty() && segment != "." && segment != ".." {
            parts.push(segment);
        }
    }
    let sanitized = parts.join("/");
    sanitized
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-' || *ch == '/')
        .collect()
}

/// `file_utils` hashing — MD5 hex digest (32 chars). RAGFlow hashes file content
/// with md5 when computing document identity; this is the pure helper.
pub fn hash_md5_hex(data: &[u8]) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// `common.py::hash128` — xxh3-128 as a 32-char hex string, the RAGFlow-compatible
/// content hash used for documents and chunk references.
pub fn hash128(data: &[u8]) -> String {
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(data))
}

// ---------------------------------------------------------------------------
// Parser defaults — `api_utils.py::get_parser_config` semantics
// ---------------------------------------------------------------------------

/// `api_utils.py::get_parser_config` — resolve the effective parser configuration
/// for a chunk method.
///
/// Semantics: merge `base_defaults` + per-method defaults + user overrides
/// (recursive, user wins), then flatten an enabled `parent_child` config into
/// `children_delimiter` for the execution layer.
pub fn get_parser_config(
    chunk_method: Option<&str>,
    parser_config: Option<&serde_json::Value>,
) -> serde_json::Value {
    let chunk_method = chunk_method.unwrap_or("naive");

    let base_defaults = serde_json::json!({
        "table_context_size": 0,
        "image_context_size": 0,
    });

    let method_defaults = match chunk_method {
        "naive" => serde_json::json!({
            "layout_recognize": "DeepDOC",
            "chunk_token_num": 512,
            "delimiter": "\n",
            "auto_keywords": 0,
            "auto_questions": 0,
            "html4excel": false,
            "topn_tags": 3,
            "raptor": {
                "use_raptor": true,
                "prompt": "Please summarize the following paragraphs. Be careful with the numbers, do not make things up. Paragraphs as following:\n      {cluster_content}\nThe above is the content you need to summarize.",
                "max_token": 256,
                "threshold": 0.1,
                "max_cluster": 64,
                "random_seed": 0,
            },
            "graphrag": {
                "use_graphrag": true,
                "entity_types": ["organization", "person", "geo", "event", "category"],
                "method": "light",
            },
            "parent_child": {
                "use_parent_child": false,
                "children_delimiter": "\n",
            },
        }),
        "qa" | "manual" | "paper" | "book" | "laws" | "presentation" => serde_json::json!({
            "raptor": { "use_raptor": false },
            "graphrag": { "use_graphrag": false },
        }),
        "knowledge_graph" => serde_json::json!({
            "chunk_token_num": 8192,
            "delimiter": "\\n",
            "entity_types": ["organization", "person", "location", "event", "time"],
            "raptor": { "use_raptor": false },
            "graphrag": { "use_graphrag": false },
        }),
        // tag / resume / table / one / email / picture and any unknown method:
        // no method defaults — only base defaults apply.
        _ => serde_json::Value::Null,
    };

    let mut merged = match parser_config {
        Some(user) if user.is_object() || !user.is_null() => {
            // defaults exist for the method -> merge defaults + user config
            if method_defaults.is_object() {
                let with_defaults = deep_merge(&base_defaults, &method_defaults);
                deep_merge(&with_defaults, user)
            } else {
                deep_merge(&base_defaults, user)
            }
        }
        _ => {
            if method_defaults.is_object() {
                deep_merge(&base_defaults, &method_defaults)
            } else {
                base_defaults
            }
        }
    };

    // Flatten parent_child -> children_delimiter for the execution layer.
    let parent_child = merged
        .get("parent_child")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();
    if parent_child
        .get("use_parent_child")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        let children_delimiter = parent_child
            .get("children_delimiter")
            .and_then(|value| value.as_str())
            .unwrap_or("\n");
        merged["children_delimiter"] = serde_json::Value::String(children_delimiter.to_string());
    } else if !parent_child.is_empty() {
        merged["children_delimiter"] = serde_json::Value::String(String::new());
    }

    merged
}

/// `api_utils.py::deep_merge` — recursive merge; `custom` values win at every
/// level, and non-object `custom` values fully replace the default entry.
pub fn deep_merge(default: &serde_json::Value, custom: &serde_json::Value) -> serde_json::Value {
    match (default, custom) {
        (serde_json::Value::Object(default_map), serde_json::Value::Object(custom_map)) => {
            let mut merged = default_map.clone();
            for (key, custom_value) in custom_map {
                match merged.get(key) {
                    Some(default_value) => {
                        let combined = deep_merge(default_value, custom_value);
                        merged.insert(key.clone(), combined);
                    }
                    None => {
                        merged.insert(key.clone(), custom_value.clone());
                    }
                }
            }
            serde_json::Value::Object(merged)
        }
        (_, custom_value) => custom_value.clone(),
    }
}

// ---------------------------------------------------------------------------
// Dataset (knowledge base) init defaults — knowledgebase_service.create_with_name
// ---------------------------------------------------------------------------

/// Defaults produced when initializing a dataset (`parser_id` "naive").
pub fn default_parser_id(parser_id: Option<&str>) -> String {
    parser_id.unwrap_or("naive").to_string()
}

/// `knowledgebase_service.py::create_with_name` — build the KB init payload:
/// `parser_id` defaults to "naive"; `parser_config` is always the validated
/// merged default (user config layered on top); `llm_id` is injected from the
/// owning tenant when provided.
pub fn init_dataset_defaults(
    parser_id: Option<&str>,
    parser_config: Option<&serde_json::Value>,
    tenant_llm_id: Option<&str>,
) -> (String, serde_json::Value) {
    let parser_id = default_parser_id(parser_id);
    let mut parser_config = get_parser_config(Some(&parser_id), parser_config);
    if let Some(llm_id) = tenant_llm_id {
        parser_config["llm_id"] = serde_json::Value::String(llm_id.to_string());
    }
    (parser_id, parser_config)
}

// ---------------------------------------------------------------------------
// datetime helpers — RAGFlow timestamp / ISO formatting semantics
// ---------------------------------------------------------------------------

/// Timestamp / ISO formatting helpers, aligned with RAGFlow:
/// internal times are Unix milliseconds; JSON serialization uses
/// `%Y-%m-%d %H:%M:%S` (see `json_encode.py::CustomJSONEncoder`) and
/// RFC3339 for API payloads.
pub mod datetime {
    /// Current Unix time in milliseconds (u64).
    pub fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    /// Current time as an RFC3339 string (UTC).
    pub fn now_rfc3339() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    /// Convert Unix milliseconds to an RFC3339 string (UTC).
    pub fn ms_to_rfc3339(timestamp_ms: u64) -> String {
        let seconds = (timestamp_ms / 1000) as i64;
        let nanos = ((timestamp_ms % 1000) * 1_000_000) as u32;
        match chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, nanos) {
            Some(dt) => dt.to_rfc3339(),
            None => String::new(),
        }
    }

    /// RAGFlow JSON encoding: `datetime.datetime.strftime('%Y-%m-%d %H:%M:%S')`.
    pub fn format_datetime(timestamp_ms: u64) -> String {
        let seconds = (timestamp_ms / 1000) as i64;
        let nanos = ((timestamp_ms % 1000) * 1_000_000) as u32;
        match chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, nanos) {
            Some(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
            None => String::new(),
        }
    }

    /// RAGFlow JSON encoding: `datetime.date.strftime('%Y-%m-%d')`.
    pub fn format_date(timestamp_ms: u64) -> String {
        let seconds = (timestamp_ms / 1000) as i64;
        match chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0) {
            Some(dt) => dt.format("%Y-%m-%d").to_string(),
            None => String::new(),
        }
    }

    /// Start of the UTC day containing `timestamp_ms`.
    pub fn start_of_utc_day(timestamp_ms: u64) -> u64 {
        timestamp_ms / 86_400_000 * 86_400_000
    }

    /// Format a Unix-ms timestamp as a UTC `YYYY-MM-DD` day string.
    pub fn format_utc_day(timestamp_ms: u64) -> String {
        format_date(start_of_utc_day(timestamp_ms))
    }

    /// Howard Hinnant's `civil_from_days` — convert days since 1970-01-01 to a
    /// `(year, month, day)` civil date. Deterministic, no chrono dependency for
    /// callers that already track day numbers.
    pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
        let z = z + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = (z - era * 146_097) as u64; // [0, 146096]
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
        let mp = (5 * doy + 2) / 153; // [0, 11]
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
        (if m <= 2 { y + 1 } else { y }, m, d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_type_classifies_ragflow_extensions() {
        // PDF
        assert_eq!(filename_type(Some("report.pdf")), FileType::Pdf);
        assert_eq!(filename_type(Some("a/b/c/report.PDF")), FileType::Pdf);
        // DOC family
        assert_eq!(filename_type(Some("notes.docx")), FileType::Doc);
        assert_eq!(filename_type(Some("script.py")), FileType::Doc);
        assert_eq!(filename_type(Some("data.jsonl")), FileType::Doc);
        assert_eq!(filename_type(Some("page.html")), FileType::Doc);
        // AURAL
        assert_eq!(filename_type(Some("audio.mp3")), FileType::Aural);
        assert_eq!(filename_type(Some("voice.flac")), FileType::Aural);
        // VISUAL
        assert_eq!(filename_type(Some("photo.JPG")), FileType::Visual);
        assert_eq!(filename_type(Some("clip.mp4")), FileType::Visual);
        assert_eq!(filename_type(Some("vector.svg")), FileType::Visual);
        // OTHER
        assert_eq!(filename_type(Some("archive.zip")), FileType::Other);
        assert_eq!(filename_type(Some("noext")), FileType::Other);
        // Robustness: None / empty / over-long names fall back to OTHER
        assert_eq!(filename_type(None), FileType::Other);
        assert_eq!(filename_type(Some("")), FileType::Other);
        assert_eq!(filename_type(Some("   ")), FileType::Other);
        let long_name = format!("{}.pdf", "a".repeat(260));
        assert_eq!(filename_type(Some(&long_name)), FileType::Other);
    }

    #[test]
    fn sanitize_path_strips_traversal_and_unsafe_chars() {
        // Note: RAGFlow's regex [^A-Za-z0-9_\-/] also strips '.' and spaces.
        assert_eq!(sanitize_path(Some("a/b/c.pdf")), "a/b/cpdf");
        assert_eq!(sanitize_path(Some("../../etc/passwd")), "etc/passwd");
        assert_eq!(sanitize_path(Some("a\\b\\c")), "a/b/c");
        assert_eq!(
            sanitize_path(Some("/leading/trailing/")),
            "leading/trailing"
        );
        assert_eq!(sanitize_path(Some("a b*c?d")), "abcd");
        assert_eq!(sanitize_path(Some(".")), "");
        assert_eq!(sanitize_path(Some("..")), "");
        assert_eq!(sanitize_path(None), "");
        assert_eq!(sanitize_path(Some("")), "");
    }

    #[test]
    fn get_parser_config_merges_defaults_and_flattens_parent_child() {
        // naive defaults
        let config = get_parser_config(Some("naive"), None);
        assert_eq!(config["chunk_token_num"], 512);
        assert_eq!(config["layout_recognize"], "DeepDOC");
        assert_eq!(config["table_context_size"], 0);
        assert_eq!(config["raptor"]["use_raptor"], true);
        assert_eq!(config["graphrag"]["entity_types"][1], "person");
        // children_delimiter empty when parent_child disabled
        assert_eq!(config["children_delimiter"], "");

        // user overrides win; unknown keys are appended
        let user = serde_json::json!({
            "chunk_token_num": 1024,
            "raptor": { "use_raptor": false },
            "custom_flag": true,
        });
        let merged = get_parser_config(Some("naive"), Some(&user));
        assert_eq!(merged["chunk_token_num"], 1024);
        assert_eq!(merged["raptor"]["use_raptor"], false);
        assert_eq!(merged["custom_flag"], true);
        assert_eq!(merged["layout_recognize"], "DeepDOC");

        // parent_child enabled -> children_delimiter comes from the flattened config
        let with_pc = serde_json::json!({
            "parent_child": { "use_parent_child": true, "children_delimiter": "\n" }
        });
        let merged_pc = get_parser_config(Some("naive"), Some(&with_pc));
        assert_eq!(merged_pc["children_delimiter"], "\n");

        // method without defaults (e.g. "tag") -> only base defaults
        let tag_config = get_parser_config(Some("tag"), None);
        assert_eq!(tag_config["table_context_size"], 0);
        assert_eq!(tag_config["image_context_size"], 0);
        assert!(tag_config.get("chunk_token_num").is_none());

        // None chunk method -> naive
        assert_eq!(get_parser_config(None, None)["layout_recognize"], "DeepDOC");
    }

    #[test]
    fn init_dataset_defaults_and_datetime_roundtrip() {
        // parser_id defaults to naive, llm_id injected from tenant
        let (parser_id, config) = init_dataset_defaults(None, None, Some("deepseek-chat"));
        assert_eq!(parser_id, "naive");
        assert_eq!(config["llm_id"], "deepseek-chat");
        assert_eq!(config["chunk_token_num"], 512);

        // explicit parser id is preserved
        let (parser_id, config) = init_dataset_defaults(Some("knowledge_graph"), None, None);
        assert_eq!(parser_id, "knowledge_graph");
        assert_eq!(config["chunk_token_num"], 8192);
        assert!(config.get("llm_id").is_none());

        // datetime: known epoch -> RFC3339 / RAGFlow %Y-%m-%d %H:%M:%S
        let ms = 1_700_000_000_000; // 2023-11-14T22:13:20Z
        assert_eq!(datetime::ms_to_rfc3339(ms), "2023-11-14T22:13:20+00:00");
        assert_eq!(datetime::format_datetime(ms), "2023-11-14 22:13:20");
        assert_eq!(datetime::format_date(ms), "2023-11-14");
        assert_eq!(datetime::format_utc_day(ms), "2023-11-14");
        // UTC day start for that timestamp (2023-11-14T00:00:00Z)
        assert_eq!(datetime::start_of_utc_day(ms), 1_699_920_000_000);
        assert_eq!(
            datetime::start_of_utc_day(ms + 3_600_000),
            datetime::start_of_utc_day(ms)
        );

        // civil_from_days cross-checked with chrono
        let days = (ms / 86_400_000) as i64;
        let (y, m, d) = datetime::civil_from_days(days);
        assert_eq!((y, m, d), (2023, 11, 14));
    }

    #[test]
    fn hashes_match_ragflow_formats() {
        // MD5 of "abc"
        assert_eq!(hash_md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        // hash128 is always 32 lowercase hex chars (xxh3-128)
        let digest = hash128(b"hello world");
        assert_eq!(digest.len(), 32);
        assert!(digest.chars().all(|ch| ch.is_ascii_hexdigit()));
        // deterministic
        assert_eq!(hash128(b"hello world"), hash128(b"hello world"));
        assert_ne!(hash128(b"hello world"), hash128(b"hello world!"));
    }
}
