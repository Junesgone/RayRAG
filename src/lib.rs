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

pub mod advanced_rag;
pub mod agent;
pub mod agent_checkpoint;
pub mod api;
pub mod app_parsers;
pub mod arxiv;
pub mod auth;
pub mod baidu;
pub mod baidu_scholar;
pub mod bing;
pub mod bocha;
pub mod chunk;
pub mod chunk_feedback;
pub mod document_writer;
pub mod duckduckgo;
pub mod eastmoney;
pub mod embed;
pub mod generation_params;
pub mod github;
pub mod google;
pub mod google_scholar;
pub mod graph_store;
pub mod graphrag_enhanced;
pub mod graphrag;
pub mod graphrag_index;
pub mod integration;
pub mod jin10;
pub mod kb;
pub mod llm;
pub mod llm_enhanced;
pub mod logging;
pub mod metrics;
pub mod model_meta;
pub mod naming;
pub mod nlp;
pub mod nlp_enhanced;
pub mod ocr;
pub mod vision;
pub mod audio;
pub mod storage;
pub mod hypergraph;
pub mod merge;
pub mod dialog;
pub mod parser;
pub mod persistence;
pub mod pipeline;
pub mod prompts;
pub mod providers;
pub mod pubmed;
pub mod qweather;
pub mod raptor;
pub mod rerank;
pub mod runtime;
pub mod search;
pub mod searxng;
pub mod surname;
pub mod server;
pub mod store;
pub mod stores;
pub mod tavily;
pub mod tencent_finance;
pub mod tools_bench_flow;
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
    /// Build a document GraphRAG checkpoint and merge it into the KB graph.
    #[serde(alias = "graphEn")]
    pub graphrag_enabled: bool,
    /// Entity labels retained for configuration compatibility with RAGFlow.
    #[serde(alias = "entityTypes")]
    pub graphrag_entity_types: Vec<String>,
    /// Graph extraction mode retained in artifact metadata.
    #[serde(alias = "graphMethod")]
    pub graphrag_method: String,
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
            graphrag_enabled: false,
            graphrag_entity_types: Vec::new(),
            graphrag_method: "light".to_string(),
        }
    }
}

/// Result type alias for RayRAG operations.
pub type Result<T> = anyhow::Result<T>;

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
