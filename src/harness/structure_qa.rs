//! Ask the chat model to answer a question from a compiled-structure outline
//! — RAGFlow v0.27.2 `rag/advanced_rag/harness/structure_qa.py`.
//!
//! Shared by the two navigation paths that read compiled rows: document
//! structure navigation (catalog / mindmap outlines) and knowledge-graph
//! exploration. Both render entities + relations into the same compact
//! outline, ask the model whether that outline alone answers the question,
//! and use the returned `relevant_entities` to pull the underlying source
//! chunks even when the outline is NOT sufficient.

use serde_json::Value;

/// Cap how much compiled structure is rendered into the prompt.
pub const MAX_ENTITIES: usize = 300;
pub const MAX_RELATIONS: usize = 300;

/// `_NAV_SYSTEM` (`{noun}` is filled by [`nav_system`]).
pub const NAV_SYSTEM: &str = r#"You are given {noun} of one or more documents — an outline of entities and their relations — and a question.

Decide whether that outline alone already answers the question.

Rules:
1. Answer ONLY from the outline below. Do not invent facts.
2. Set "is_sufficient" to true only when the outline genuinely answers the question; otherwise false with an empty answer.
3. Always fill "relevant_entities" with the exact `name` values of the entities most related to the question (up to 10), even when the outline is not sufficient — they are used to pull the underlying source text.

Output ONLY JSON, no prose, no code fences:
{{"is_sufficient": true/false, "answer": "<answer, or empty>", "relevant_entities": ["<entity name>", ...]}}"#;

/// `_NAV_SYSTEM.format(noun=f"the {noun}")`.
pub fn nav_system(noun: &str) -> String {
    NAV_SYSTEM.replace("{noun}", &format!("the {noun}"))
}

fn collapsed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `_render_structure`: compact outline for the prompt.
pub fn render_structure(entities: &[Value], relations: &[Value]) -> String {
    let mut lines: Vec<String> = Vec::new();
    if !entities.is_empty() {
        lines.push("Entities:".to_string());
        for entity in entities.iter().take(MAX_ENTITIES) {
            let name = entity
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if name.is_empty() {
                continue;
            }
            let entity_type = entity
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let entity_type = if entity_type.is_empty() {
                "other".to_string()
            } else {
                entity_type
            };
            let description = collapsed(
                entity
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            );
            let suffix = if description.is_empty() {
                String::new()
            } else {
                format!(": {description}")
            };
            lines.push(format!("- {name} ({entity_type}){suffix}"));
        }
    }
    if !relations.is_empty() {
        lines.push("\nRelations:".to_string());
        for relation in relations.iter().take(MAX_RELATIONS) {
            let source = relation
                .get("from")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let target = relation
                .get("to")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if source.is_empty() || target.is_empty() {
                continue;
            }
            let relation_type = relation
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let relation_type = if relation_type.is_empty() {
                "related".to_string()
            } else {
                relation_type
            };
            lines.push(format!("- {source} -[{relation_type}]-> {target}"));
        }
    }
    lines.join("\n")
}

/// Python `str.capitalize()`: first char upper, the rest lower.
fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let mut out = first.to_uppercase().collect::<String>();
            out.extend(chars.flat_map(|ch| ch.to_lowercase()));
            out
        }
    }
}

/// The user prompt `_ask_structure` builds for the outline question.
pub fn build_user_prompt(
    topic: &str,
    entities: &[Value],
    relations: &[Value],
    noun: &str,
) -> String {
    format!(
        "Question:\n{topic}\n\n{}:\n{}\n\nOutput JSON:",
        capitalize(noun),
        render_structure(entities, relations)
    )
}

/// The model's outline verdict: `(is_sufficient, answer, relevant_entities)`.
pub fn parse_verdict(raw: &str) -> (bool, String, Vec<String>) {
    let think = regex::Regex::new("(?s)^.*</think>").unwrap();
    let fences = regex::Regex::new(r"```(?:json)?\s*|\s*```").unwrap();
    let cleaned = fences
        .replace_all(&think.replace(raw, ""), "")
        .trim()
        .to_string();
    let verdict: Value = serde_json::from_str(&cleaned).unwrap_or_else(|_| serde_json::json!({}));
    let sufficient = verdict
        .get("is_sufficient")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let answer = verdict
        .get("answer")
        .filter(|value| !value.is_null())
        .map(|value| match value {
            Value::String(text) => text.trim().to_string(),
            other => other.to_string(),
        })
        .filter(|_| sufficient)
        .unwrap_or_default();
    let relevant = verdict
        .get("relevant_entities")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    (sufficient, answer, relevant)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_outline_with_caps_and_skips() {
        let entities = vec![
            json!({"name": "Alpha", "type": "Person", "description": "first   entity"}),
            json!({"name": "  ", "type": "X"}),
            json!({"name": "Beta"}),
        ];
        let relations = vec![
            json!({"from": "Alpha", "to": "Beta", "type": "knows"}),
            json!({"from": "", "to": "Beta"}),
        ];
        let rendered = render_structure(&entities, &relations);
        assert!(rendered.starts_with("Entities:\n- Alpha (Person): first entity\n- Beta (other)"));
        assert!(rendered.contains("\nRelations:\n- Alpha -[knows]-> Beta"));
        assert!(!rendered.contains("  "));

        let prompt = build_user_prompt("Who?", &entities, &relations, "document catalog");
        assert!(prompt.starts_with("Question:\nWho?\n\nDocument catalog:\n"));
        assert!(prompt.ends_with("\n\nOutput JSON:"));
        assert!(nav_system("knowledge graph").contains("the knowledge graph"));
    }

    #[test]
    fn verdict_parsing_follows_sufficiency() {
        let (sufficient, answer, relevant) = parse_verdict(
            "<think>x</think>```json\n{\"is_sufficient\": true, \"answer\": \" 42 \", \"relevant_entities\": [\"Alpha\", 3, \"Beta\"]}\n```",
        );
        assert!(sufficient);
        assert_eq!(answer, "42");
        assert_eq!(relevant, vec!["Alpha", "Beta"]);

        // Insufficient outlines keep only the entity hints.
        let (sufficient, answer, relevant) = parse_verdict(
            "{\"is_sufficient\": false, \"answer\": \"should be dropped\", \"relevant_entities\": [\"Alpha\"]}",
        );
        assert!(!sufficient);
        assert!(answer.is_empty());
        assert_eq!(relevant, vec!["Alpha"]);

        let (sufficient, answer, relevant) = parse_verdict("not json");
        assert!(!sufficient);
        assert!(answer.is_empty());
        assert!(relevant.is_empty());
    }
}
