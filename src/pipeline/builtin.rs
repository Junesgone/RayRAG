//! RAGFlow v0.27.2 built-in ingestion pipeline registry.
//!
//! Go backend contract: internal/ingestion/pipeline/builtin_registry.go embeds
//! one DSL template per built-in parser under template/ingestion_pipeline_*.json;
//! GET /api/v1/pipelines serves the catalog ({canvas:[{id,title,description,
//! filename}],total}) and GET /api/v1/pipelines/:id serves {dsl:{...}}. The
//! legacy parser_id "naive" is an alias of the canonical pipeline "general".
//!
//! The templates are copied verbatim from the upstream Apache-2.0 source so the
//! DSL the UI receives (component keys, operator params, output slots) is the
//! same one the live Go backend returns.

use serde_json::Value;
use std::sync::OnceLock;

/// Upstream builtinParserOrder: the catalog order of the built-in pipelines.
const TEMPLATES: [(&str, &str); 12] = [
    (
        "general",
        include_str!("templates/ingestion_pipeline_general.json"),
    ),
    ("qa", include_str!("templates/ingestion_pipeline_qa.json")),
    (
        "manual",
        include_str!("templates/ingestion_pipeline_manual.json"),
    ),
    (
        "table",
        include_str!("templates/ingestion_pipeline_table.json"),
    ),
    (
        "paper",
        include_str!("templates/ingestion_pipeline_paper.json"),
    ),
    (
        "book",
        include_str!("templates/ingestion_pipeline_book.json"),
    ),
    (
        "laws",
        include_str!("templates/ingestion_pipeline_laws.json"),
    ),
    (
        "presentation",
        include_str!("templates/ingestion_pipeline_presentation.json"),
    ),
    (
        "picture",
        include_str!("templates/ingestion_pipeline_picture.json"),
    ),
    ("one", include_str!("templates/ingestion_pipeline_one.json")),
    (
        "audio",
        include_str!("templates/ingestion_pipeline_audio.json"),
    ),
    (
        "email",
        include_str!("templates/ingestion_pipeline_email.json"),
    ),
];

/// One built-in pipeline template.
#[derive(Debug, Clone)]
pub struct BuiltinPipeline {
    /// Canonical parser id stored in parser_config.chunk_method.
    pub id: &'static str,
    /// English title (title.en, falling back to the id).
    pub title: String,
    /// English description (description.en, empty when absent).
    pub description: String,
    /// Embedded template filename.
    pub filename: &'static str,
    /// The dsl object exactly as upstream serves it.
    pub dsl: Value,
}

/// Upstream builtinAliases: legacy parser ids mapped to canonical ids.
fn canonical(reference: &str) -> &str {
    match reference.trim() {
        "naive" => "general",
        other => other,
    }
}

struct Registry {
    items: Vec<BuiltinPipeline>,
}

static REGISTRY: OnceLock<Registry> = OnceLock::new();

fn english_text(field: &Value, fallback: &str) -> String {
    field
        .get("en")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

fn filename_for(id: &str) -> &'static str {
    match id {
        "general" => "ingestion_pipeline_general.json",
        "qa" => "ingestion_pipeline_qa.json",
        "manual" => "ingestion_pipeline_manual.json",
        "table" => "ingestion_pipeline_table.json",
        "paper" => "ingestion_pipeline_paper.json",
        "book" => "ingestion_pipeline_book.json",
        "laws" => "ingestion_pipeline_laws.json",
        "presentation" => "ingestion_pipeline_presentation.json",
        "picture" => "ingestion_pipeline_picture.json",
        "one" => "ingestion_pipeline_one.json",
        "audio" => "ingestion_pipeline_audio.json",
        "email" => "ingestion_pipeline_email.json",
        _ => "",
    }
}

fn load_registry() -> Registry {
    let mut items = Vec::with_capacity(TEMPLATES.len());
    for (id, raw) in TEMPLATES {
        let data: Value = serde_json::from_str(raw).unwrap_or_else(|error| {
            panic!("builtin pipeline template {id} is not valid JSON: {error}")
        });
        let dsl = data
            .get("dsl")
            .cloned()
            .unwrap_or_else(|| panic!("builtin pipeline template {id} has no dsl"));
        items.push(BuiltinPipeline {
            id,
            title: english_text(data.get("title").unwrap_or(&Value::Null), id),
            description: english_text(data.get("description").unwrap_or(&Value::Null), ""),
            filename: filename_for(id),
            dsl,
        });
    }
    Registry { items }
}

fn registry() -> &'static Registry {
    REGISTRY.get_or_init(load_registry)
}

/// The built-in pipeline catalog in upstream order.
pub fn list() -> &'static [BuiltinPipeline] {
    &registry().items
}

/// Look up a built-in pipeline by id or legacy alias (trimmed).
pub fn get(reference: &str) -> Option<&'static BuiltinPipeline> {
    let canonical = canonical(reference);
    registry().items.iter().find(|item| item.id == canonical)
}

/// pipeline_registry.IsValid: whether a value names a built-in pipeline,
/// directly or through a legacy alias.
pub fn is_valid(reference: &str) -> bool {
    !reference.trim().is_empty() && get(reference).is_some()
}

/// English title for a built-in pipeline id or alias.
pub fn title_of(reference: &str) -> Option<&'static str> {
    get(reference).map(|item| item.title.as_str())
}

/// Component names of a pipeline DSL in dsl.path order (the execution order the
/// UI operator tabs follow). Templates that do not persist a path are walked in
/// component-map order instead of returning nothing.
pub fn component_sequence(dsl: &Value) -> Vec<String> {
    let mut names = Vec::new();
    let components = dsl.get("components").and_then(Value::as_object);
    let mut push_component = |key: &str| {
        if let Some(component) = components.and_then(|components| components.get(key)) {
            if let Some(name) = component
                .get("obj")
                .and_then(|object| object.get("component_name"))
                .and_then(Value::as_str)
            {
                names.push(name.to_string());
            }
        }
    };
    let mut walked = false;
    if let Some(path) = dsl.get("path").and_then(Value::as_array) {
        for key in path {
            if let Some(key) = key.as_str() {
                push_component(key);
                walked = true;
            }
        }
    }
    if !walked {
        if let Some(components) = components {
            for key in components.keys() {
                push_component(key);
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_serves_the_upstream_catalog_in_order() {
        let ids: Vec<&str> = list().iter().map(|item| item.id).collect();
        assert_eq!(
            ids,
            vec![
                "general",
                "qa",
                "manual",
                "table",
                "paper",
                "book",
                "laws",
                "presentation",
                "picture",
                "one",
                "audio",
                "email",
            ]
        );
        assert_eq!(list().len(), 12);
        assert_eq!(list()[0].title, "General");
        assert!(list()[0].description.contains("token count"));
        assert_eq!(list()[0].filename, "ingestion_pipeline_general.json");
    }

    #[test]
    fn aliases_and_invalid_refs_follow_upstream() {
        assert!(is_valid("general"));
        assert!(is_valid("naive"));
        assert_eq!(get("naive").map(|item| item.id), Some("general"));
        assert!(get("  qa ").is_some());
        assert!(!is_valid(""));
        assert!(!is_valid("knowledge_graph"));
        assert!(!is_valid("nonexistent"));
    }

    #[test]
    fn every_template_carries_a_usable_dsl() {
        for item in list() {
            let components = item
                .dsl
                .get("components")
                .and_then(Value::as_object)
                .unwrap_or_else(|| panic!("{} has no components", item.id));
            assert!(
                !components.is_empty(),
                "{} has an empty component map",
                item.id
            );
            let file = components.values().any(|component| {
                component
                    .pointer("/obj/component_name")
                    .and_then(Value::as_str)
                    == Some("File")
            });
            assert!(file, "{} has no File component", item.id);
            let sequence = component_sequence(&item.dsl);
            assert!(
                sequence.iter().any(|name| name == "Parser"),
                "{} has no Parser component",
                item.id
            );
        }
    }
}
