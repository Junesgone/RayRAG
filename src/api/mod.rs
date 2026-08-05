//! API modules — RESTful endpoint handlers for RayRAG.
//!
//! Each module replaces a RAGFlow API component.

pub mod chunks;
pub mod compilation_templates;
pub mod connector;
pub mod data_source_mgr;
pub mod document;
pub mod document_metadata;
pub mod evaluation;
pub mod features;
pub mod file_mgr;
pub mod openai_proxy;
pub mod search;
pub mod system;
pub mod tenant_models;
