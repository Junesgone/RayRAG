//! API modules — RESTful endpoint handlers for RayRAG.
//!
//! Each module replaces a RAGFlow API component.

pub mod admin;
pub mod chatapp_mgr;
pub mod chunks;
pub mod common;
pub mod compilation_templates;
pub mod connector;
pub mod data_source_mgr;
pub mod db;
pub mod document;
pub mod document_metadata;
pub mod evaluation;
pub mod features;
pub mod file_mgr;
pub mod joint_services;
pub mod openai_proxy;
pub mod runtime_config;
pub mod search;
pub mod searchapp_mgr;
pub mod skill_index;
pub mod system;
pub mod system_settings;
pub mod tenant_models;
pub mod utils;
