//! RayRAG: RAGFlow parsing pipeline rewritten in Rust, powered by zvec.
//!
//! Port of RAGFlow's document parsing and chunking pipeline from Python to Rust,
//! with zvec as the vector database backend instead of Elasticsearch.
//!
//! # Architecture
//!
//! ```text
//! Document → parser → chunk → embed → store
//!   .pdf     pdf.rs   naive   openai   zvec.rs
//!   .docx    docx.rs  sentence
//!   .txt     txt.rs
//!   .md      markdown.rs
//!   .html    html.rs
//! ```

pub use embed::Embedder;
pub use search::SearchEngine;

/// Build identity reported by the startup banner and the version APIs.
///
/// `VERSION` is the crate version (`Cargo.toml`), `PARITY_SLICE` the RAGFlow
/// parity slice currently landed (kept in step with the per-file parity ledger
/// and the operations log, both of which live in the internal engineering
/// repository), and the remaining fields come from `build.rs` so a running
/// process can be traced back to the revision it was built from.
pub mod build_info {
    /// Crate version from `Cargo.toml`.
    pub const VERSION: &str = env!("CARGO_PKG_VERSION");
    /// RAGFlow parity slice currently landed in this build.
    pub const PARITY_SLICE: &str = "v0.3.8n";
    /// Short git revision the binary was built from.
    pub const GIT_REV: &str = env!("RAYRAG_BUILD_GIT_REV");
    /// `clean` or `dirty` working-tree marker captured at build time.
    pub const GIT_DIRTY: &str = env!("RAYRAG_BUILD_GIT_DIRTY");
    /// UTC timestamp of the build.
    pub const BUILT_AT: &str = env!("RAYRAG_BUILD_TIME");

    /// Revision plus working-tree marker, e.g. `c7258bb+dirty`.
    pub fn revision() -> String {
        if GIT_DIRTY == "dirty" {
            format!("{GIT_REV}+dirty")
        } else {
            GIT_REV.to_string()
        }
    }

    /// One-line identity used by the startup banner.
    pub fn version_line() -> String {
        format!(
            "v{VERSION} (parity {PARITY_SLICE}, rev {}, built {BUILT_AT})",
            revision()
        )
    }
}

pub mod advanced_rag;
pub mod agent;
pub mod agent_checkpoint;
pub mod agent_templates;
pub mod akshare;
pub mod api;
pub mod app_parsers;
pub mod arxiv;
pub mod audio;
pub mod auth;
pub mod baidu;
pub mod baidu_scholar;
pub mod baike;
pub mod benchmark;
pub mod bing;
pub mod bocha;
pub mod book;
pub mod channels;
pub mod chat;
pub mod chunk;
pub mod chunk_feedback;
pub mod code_exec;
pub mod common;
pub mod connectors;
pub mod crawler;
pub mod data_source;
pub mod dialog;
pub mod doc_store;
pub mod document_writer;
pub mod duckduckgo;
pub mod eastmoney;
pub mod email;
pub mod embed;
pub mod exesql;
pub mod extractor;
pub mod generation_params;
pub mod github;
pub mod google;
pub mod google_scholar;
pub mod graph_store;
pub mod graphrag;
pub mod graphrag_enhanced;
pub mod graphrag_index;
pub mod hypergraph;
pub mod integration;
pub mod jin10;
pub mod kb;
pub mod laws;
pub mod llm;
pub mod llm_enhanced;
pub mod logging;
pub mod manual;
pub mod mcp_client;
pub mod mcp_server;
pub mod memory;
pub mod merge;
pub mod metadata_filter;
pub mod metrics;
pub mod model_meta;
pub mod naive;
pub mod naming;
pub mod nlp;
pub mod nlp_enhanced;
pub mod oauth_config;
pub mod ocr;
pub mod one;
pub mod paper;
pub mod parser;
pub mod parser_config;
pub mod password_transport;
pub mod persistence;
pub mod pipeline;
pub mod plugin;
pub mod presentation;
pub mod prompts;
pub mod providers;
pub mod pubmed;
pub mod qa;
pub mod qweather;
pub mod raptor;
pub mod rerank;
pub mod resources;
pub mod resume;
pub mod runtime;
pub mod sandbox;
pub mod ssh_exec;
pub mod search;
pub mod searxng;
pub mod server;
pub mod settings;
pub mod storage;
pub mod store;
pub mod stores;
pub mod structure_compile;
pub mod surname;
pub mod table;
pub mod tag;
pub mod task_executor;
pub mod tavily;
pub mod tencent_finance;
pub mod tools_bench_flow;
pub mod translate;
pub mod tushare;
pub mod user_register;
pub mod vision;
pub mod web;
pub mod wikipedia;
pub mod yahoo_finance;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A parsed document before chunking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// Unique document ID
    pub id: Uuid,
    /// Original filename
    pub name: String,
    /// Parsed text content
    pub content: String,
    /// MIME type (e.g., "application/pdf")
    pub mime_type: String,
    /// File size in bytes
    pub size: usize,
    /// Extracted metadata
    pub metadata: std::collections::HashMap<String, String>,
}

/// A text chunk ready for embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    /// Chunk ID (generated or from source doc)
    pub id: String,
    /// The chunk text content
    pub content: String,
    /// Content type: "text", "table", or "image"
    pub content_type: String,
    /// Source document ID
    pub doc_id: Uuid,
    /// Position within the document (for ordering)
    pub position: usize,
    /// Token count
    pub token_count: usize,
    /// Optional embedding vector (set after embedding)
    pub embedding: Option<Vec<f32>>,
    /// Chunk metadata (page numbers, sections, etc.)
    pub metadata: std::collections::HashMap<String, String>,
}

/// Parsing configuration, mirroring RAGFlow's parser_config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ParserConfig {
    /// Chunk token size (default: 2048)
    pub chunk_token_num: usize,
    /// Overlap between chunks (0.0–1.0, default: 0.05)
    pub overlapped_percent: f32,
    /// Chunk delimiter (default: "\n")
    pub delimiter: String,
    /// Enable table context extraction
    pub table_context_size: usize,
    /// Enable image context extraction
    pub image_context_size: usize,
    /// OCR language (default: "English")
    pub ocr_lang: String,
    /// Layout recognizer, including provider-backed PaddleOCR/OpenDataLoader/MinerU/SoMark.
    pub layout_recognize: String,
    /// OpenDataLoader's optional hybrid parser selector.
    #[serde(rename = "hybrid", alias = "opendataloader_hybrid")]
    pub opendataloader_hybrid: Option<String>,
    /// OpenDataLoader's optional image output mode.
    #[serde(rename = "image_output", alias = "opendataloader_image_output")]
    pub opendataloader_image_output: Option<String>,
    /// OpenDataLoader's optional sanitization flag.
    #[serde(rename = "sanitize", alias = "opendataloader_sanitize")]
    pub opendataloader_sanitize: Option<bool>,
    /// MinerU parsing mode: auto, txt, or ocr.
    pub mineru_parse_method: String,
    /// Whether MinerU should recognize formulas.
    pub mineru_formula_enable: bool,
    /// Whether MinerU should recognize and preserve tables.
    pub mineru_table_enable: bool,
    /// User-facing MinerU OCR language name.
    pub mineru_lang: String,
    /// Optional per-parser SoMark image output format.
    pub somark_image_format: Option<String>,
    /// Optional per-parser SoMark formula output format.
    pub somark_formula_format: Option<String>,
    /// Optional per-parser SoMark table output format.
    pub somark_table_format: Option<String>,
    /// Optional per-parser SoMark cross-section output format.
    pub somark_cs_format: Option<String>,
    /// Optional per-parser SoMark text cross-page switch.
    pub somark_enable_text_cross_page: Option<bool>,
    /// Optional per-parser SoMark table cross-page switch.
    pub somark_enable_table_cross_page: Option<bool>,
    /// Optional per-parser SoMark title-level recognition switch.
    pub somark_enable_title_level_recognition: Option<bool>,
    /// Optional per-parser SoMark inline-image switch.
    pub somark_enable_inline_image: Option<bool>,
    /// Optional per-parser SoMark table-image switch.
    pub somark_enable_table_image: Option<bool>,
    /// Optional per-parser SoMark image-understanding switch.
    pub somark_enable_image_understanding: Option<bool>,
    /// Optional per-parser SoMark header/footer retention switch.
    pub somark_keep_header_footer: Option<bool>,
    /// Enable metadata extraction
    pub enable_metadata: bool,
    /// Enable children chunks (parent-child hierarchy)
    pub enable_children: bool,
    /// Build RAPTOR summary chunks and index them with the source document.
    #[serde(alias = "raptorEn")]
    pub raptor_enabled: bool,
    /// Maximum RAPTOR hierarchy depth.
    #[serde(alias = "raptorDepth")]
    pub raptor_depth: usize,
    /// Cosine threshold used while clustering RAPTOR nodes.
    pub raptor_threshold: f32,
    /// RAPTOR summarization scope: Dataset | Single file.
    pub raptor_scope: String,
    /// Custom RAPTOR summarization prompt (empty = built-in prompt).
    pub raptor_prompt: String,
    /// Max tokens per RAPTOR summary.
    pub raptor_max_token: usize,
    /// RAPTOR clustering method: GMM | AHC.
    pub raptor_cluster_method: String,
    /// Max clusters per RAPTOR level.
    pub raptor_max_cluster: usize,
    /// Random seed for clustering.
    pub raptor_random_seed: usize,
    /// Build a document GraphRAG checkpoint and merge it into the KB graph.
    #[serde(alias = "graphEn")]
    pub graphrag_enabled: bool,
    /// Entity labels retained for configuration compatibility with RAGFlow.
    #[serde(alias = "entityTypes")]
    pub graphrag_entity_types: Vec<String>,
    /// Graph extraction mode retained in artifact metadata.
    #[serde(alias = "graphMethod")]
    pub graphrag_method: String,
    /// Batch chunk token size for GraphRAG extraction.
    pub graphrag_batch_chunk_size: usize,
    /// Resolve entities across the knowledge graph.
    pub graphrag_entity_resolution: bool,
    /// Generate community reports (Leiden clustering).
    pub graphrag_community_reports: bool,
    /// KB page rank (0 = default ranking; positive values boost).
    #[serde(default)]
    pub page_rank: usize,
    /// Chunking method: naive | token | title (RAGFlow chunk_method).
    #[serde(alias = "chunkMethod")]
    pub chunk_method: String,
    /// Token chunker delimiter mode: token_size | delimiter | one.
    #[serde(alias = "delimiterMode")]
    pub delimiter_mode: String,
    /// Token chunker delimiters (backtick-wrapped patterns compiled as regex).
    #[serde(alias = "delimiters")]
    pub delimiters: Vec<String>,
    /// Token chunker children delimiters (split text chunks, attach parent as `mom`).
    #[serde(alias = "childrenDelimiters")]
    pub children_delimiters: Vec<String>,
    /// Title chunker levels (each string is one regex family; simplification of
    /// RAGFlow's nested regex groups).
    #[serde(alias = "titleLevels")]
    pub title_levels: Vec<String>,
    /// Title chunker target hierarchy number (1-based, clamped to levels len).
    pub hierarchy: Option<usize>,
    /// Title chunker: include heading text in each chunk.
    #[serde(alias = "includeHeadingContent")]
    pub include_heading_content: bool,
    /// Title chunker: prepend root chunk text to all later chunks, drop root.
    #[serde(alias = "rootChunkAsHeading")]
    pub root_chunk_as_heading: bool,
    /// Auto-extract N key terms per chunk after chunking (0 disables;
    /// RAGFlow `auto_keywords`). Results are stored in each chunk's
    /// `important_kwd` metadata, consumed by hybrid retrieval.
    #[serde(alias = "autoKeywords")]
    pub auto_keywords: usize,
}

impl Default for ParserConfig {
    fn default() -> Self {
        Self {
            chunk_token_num: 2048,
            overlapped_percent: 0.05,
            delimiter: "\n".to_string(),
            table_context_size: 256,
            image_context_size: 256,
            ocr_lang: "English".to_string(),
            layout_recognize: "DeepDOC".to_string(),
            opendataloader_hybrid: None,
            opendataloader_image_output: None,
            opendataloader_sanitize: None,
            mineru_parse_method: "auto".to_string(),
            mineru_formula_enable: true,
            mineru_table_enable: true,
            mineru_lang: "English".to_string(),
            somark_image_format: None,
            somark_formula_format: None,
            somark_table_format: None,
            somark_cs_format: None,
            somark_enable_text_cross_page: None,
            somark_enable_table_cross_page: None,
            somark_enable_title_level_recognition: None,
            somark_enable_inline_image: None,
            somark_enable_table_image: None,
            somark_enable_image_understanding: None,
            somark_keep_header_footer: None,
            enable_metadata: true,
            enable_children: true,
            raptor_enabled: false,
            raptor_depth: 5,
            raptor_threshold: 0.7,
            raptor_scope: "Dataset".into(),
            raptor_prompt: String::new(),
            raptor_max_token: 512,
            raptor_cluster_method: "GMM".into(),
            raptor_max_cluster: 64,
            raptor_random_seed: 0,
            graphrag_enabled: false,
            graphrag_entity_types: Vec::new(),
            graphrag_method: "light".to_string(),
            graphrag_batch_chunk_size: 8196,
            graphrag_entity_resolution: false,
            graphrag_community_reports: false,
            page_rank: 0,
            chunk_method: "naive".to_string(),
            delimiter_mode: "token_size".to_string(),
            delimiters: Vec::new(),
            children_delimiters: Vec::new(),
            title_levels: Vec::new(),
            hierarchy: None,
            include_heading_content: false,
            root_chunk_as_heading: false,
            auto_keywords: 0,
        }
    }
}

/// Result type alias for RayRAG operations.
pub type Result<T> = anyhow::Result<T>;

#[cfg(test)]
mod raptor_graphrag_config_tests {
    use super::*;

    #[test]
    fn raptor_and_graphrag_fields_roundtrip_through_json() {
        let json = serde_json::json!({
            "raptor_enabled": true,
            "raptor_depth": 5,
            "raptor_threshold": 0.6,
            "raptor_scope": "Single file",
            "raptor_prompt": "Summarize this:",
            "raptor_max_token": 1024,
            "raptor_cluster_method": "AHC",
            "raptor_max_cluster": 32,
            "raptor_random_seed": 42,
            "graphrag_enabled": true,
            "graphrag_method": "general",
            "graphrag_batch_chunk_size": 4096,
            "graphrag_entity_resolution": true,
            "graphrag_community_reports": true,
        });
        let cfg: ParserConfig = serde_json::from_value(json).unwrap();
        assert!(cfg.raptor_enabled);
        assert_eq!(cfg.raptor_scope, "Single file");
        assert_eq!(cfg.raptor_cluster_method, "AHC");
        assert_eq!(cfg.raptor_random_seed, 42);
        assert_eq!(cfg.graphrag_batch_chunk_size, 4096);
        assert!(cfg.graphrag_entity_resolution);
        assert!(cfg.graphrag_community_reports);
    }

    #[test]
    fn defaults_match_ragflow_shipping_values() {
        let cfg = ParserConfig::default();
        assert!(!cfg.raptor_enabled);
        assert_eq!(cfg.raptor_depth, 5);
        assert_eq!(cfg.raptor_threshold, 0.7);
        assert_eq!(cfg.raptor_scope, "Dataset");
        assert_eq!(cfg.raptor_cluster_method, "GMM");
        assert_eq!(cfg.raptor_max_cluster, 64);
        assert_eq!(cfg.graphrag_batch_chunk_size, 8196);
        assert_eq!(cfg.graphrag_method, "light");
    }
}

#[cfg(test)]
mod parser_config_tests {
    use super::*;

    #[test]
    fn partial_parser_config_preserves_explicit_values_and_fills_defaults() {
        let config: ParserConfig =
            serde_json::from_str(r#"{"chunk_token_num":512,"overlapped_percent":0.2}"#).unwrap();
        assert_eq!(config.chunk_token_num, 512);
        assert_eq!(config.overlapped_percent, 0.2);
        assert_eq!(config.delimiter, "\n");
        assert_eq!(config.ocr_lang, "English");
    }

    #[test]
    fn parser_config_accepts_web_graphrag_and_raptor_aliases() {
        let config: ParserConfig = serde_json::from_str(
            r#"{"raptorEn":true,"raptorDepth":3,"graphEn":true,"entityTypes":["Person"],"graphMethod":"general"}"#,
        )
        .unwrap();
        assert!(config.raptor_enabled);
        assert_eq!(config.raptor_depth, 3);
        assert!(config.graphrag_enabled);
        assert_eq!(config.graphrag_entity_types, ["Person"]);
        assert_eq!(config.graphrag_method, "general");
    }

    #[test]
    fn parser_config_accepts_opendataloader_request_options() {
        let config: ParserConfig = serde_json::from_str(
            r#"{"hybrid":"docling-fast","image_output":"embedded","sanitize":false}"#,
        )
        .unwrap();
        assert_eq!(
            config.opendataloader_hybrid.as_deref(),
            Some("docling-fast")
        );
        assert_eq!(
            config.opendataloader_image_output.as_deref(),
            Some("embedded")
        );
        assert_eq!(config.opendataloader_sanitize, Some(false));

        let serialized = serde_json::to_value(config).unwrap();
        assert_eq!(serialized["hybrid"], "docling-fast");
        assert_eq!(serialized["image_output"], "embedded");
        assert_eq!(serialized["sanitize"], false);
    }

    #[test]
    fn parser_config_accepts_mineru_request_options() {
        let config: ParserConfig = serde_json::from_str(
            r#"{
                "mineru_parse_method":"ocr",
                "mineru_formula_enable":false,
                "mineru_table_enable":true,
                "mineru_lang":"Japanese"
            }"#,
        )
        .unwrap();
        assert_eq!(config.mineru_parse_method, "ocr");
        assert!(!config.mineru_formula_enable);
        assert!(config.mineru_table_enable);
        assert_eq!(config.mineru_lang, "Japanese");
    }

    #[test]
    fn parser_config_accepts_somark_request_options_without_hiding_absence() {
        let config: ParserConfig = serde_json::from_str(
            r#"{
                "somark_image_format":"base64",
                "somark_formula_format":"mathml",
                "somark_table_format":"markdown",
                "somark_cs_format":"image",
                "somark_enable_text_cross_page":true,
                "somark_enable_table_cross_page":false,
                "somark_enable_title_level_recognition":true,
                "somark_enable_inline_image":false,
                "somark_enable_table_image":true,
                "somark_enable_image_understanding":false,
                "somark_keep_header_footer":true
            }"#,
        )
        .unwrap();
        assert_eq!(config.somark_image_format.as_deref(), Some("base64"));
        assert_eq!(config.somark_formula_format.as_deref(), Some("mathml"));
        assert_eq!(config.somark_table_format.as_deref(), Some("markdown"));
        assert_eq!(config.somark_cs_format.as_deref(), Some("image"));
        assert_eq!(config.somark_enable_text_cross_page, Some(true));
        assert_eq!(config.somark_enable_table_cross_page, Some(false));
        assert_eq!(config.somark_enable_title_level_recognition, Some(true));
        assert_eq!(config.somark_enable_inline_image, Some(false));
        assert_eq!(config.somark_enable_table_image, Some(true));
        assert_eq!(config.somark_enable_image_understanding, Some(false));
        assert_eq!(config.somark_keep_header_footer, Some(true));

        let defaults: ParserConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(defaults.somark_image_format, None);
        assert_eq!(defaults.somark_keep_header_footer, None);
    }
}
