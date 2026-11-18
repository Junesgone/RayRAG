//! LLM hypergraph extraction — ported from RAGFlow's
//! `rag/advanced_rag/knowlege_compile/structure.py`.
//!
//! Mirrors the knowledge-compilation pipeline for non-`tree` templates:
//! `_struct_hypergraph_prompts` renders node/edge prompts from a template
//! `parser_config`, `_struct_extract_hypergraph` runs two LLM JSON passes
//! (entities first, then relations constrained to the known entities), and
//! `compile_structure_from_text` fans every chunk through the extractor and
//! flattens the results. `gen_json` is the JSON-output helper (strips
//! markdown fences, parses `{"items": [...]}`).

use crate::Result;
use crate::llm::{ChatMessage, LlmClient};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Render `_struct_render_fields`: (bulleted field descriptions, JSON
/// skeleton for one item).
fn render_fields(fields: &[Value]) -> (String, String) {
    let mut lines = Vec::new();
    let mut skeleton_parts = Vec::new();
    for f in fields {
        let name = f.get("name").and_then(Value::as_str).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let ftype = f.get("type").and_then(Value::as_str).unwrap_or("str");
        let desc = f.get("description").and_then(Value::as_str).unwrap_or("");
        let required = f.get("required");
        let req_label = if required == Some(&Value::Bool(false)) {
            "optional"
        } else {
            "required"
        };
        lines.push(format!("- {name} ({ftype}, {req_label}): {desc}"));
        let placeholder = match ftype {
            "list" => "[<string>, ...]",
            "int" => "<int>",
            "float" => "<float>",
            "bool" => "<true|false>",
            _ => "<string>",
        };
        skeleton_parts.push(format!("\"{name}\": {placeholder}"));
    }
    (
        lines.join("\n"),
        format!("{{ {} }}", skeleton_parts.join(", ")),
    )
}

/// Render `_struct_render_type_fields` (compilation-template shape: allowed
/// item `type` values with descriptions/rules).
fn render_type_fields(fields: &[Value], kind: &str) -> (String, String) {
    let mut lines = Vec::new();
    let mut type_values: Vec<String> = Vec::new();
    for f in fields {
        let typ = f.get("type").and_then(Value::as_str).unwrap_or("").trim();
        if typ.is_empty() {
            continue;
        }
        type_values.push(typ.to_string());
        lines.push(format!("- type: {typ}"));
        let desc = f.get("description").and_then(Value::as_str).unwrap_or("");
        let rule = f.get("rule").and_then(Value::as_str).unwrap_or("");
        if !desc.is_empty() {
            lines.push(format!("  description: {desc}"));
        }
        if !rule.is_empty() {
            lines.push(format!("  rule: {rule}"));
        }
    }
    if type_values.is_empty() {
        type_values.push("other".to_string());
        lines.push("- type: other".to_string());
    }
    let allowed = type_values.join("|");
    let skeleton = if kind == "relation" {
        format!(
            "{{ \"type\": \"<one of: {allowed}>\", \"source\": \"<known entity name>\", \
             \"target\": \"<known entity name>\", \"description\": \"<evidence or relation description>\" }}"
        )
    } else {
        format!(
            "{{ \"type\": \"<one of: {allowed}>\", \"name\": \"<exact extracted item text>\", \
             \"description\": \"<evidence, definition, or detail from the source>\" }}"
        )
    };
    (lines.join("\n"), skeleton)
}

/// Infer the auto-type from the template config (`_struct_infer_type`):
/// `list` when output.entities has `ordered`, `set` when `unique` is true,
/// else `hypergraph` when relations are configured, otherwise `list`.
fn infer_type(config: &Value) -> String {
    let output = config.get("output").and_then(Value::as_object);
    let entities = output
        .and_then(|o| o.get("entities"))
        .and_then(Value::as_object);
    if let Some(e) = entities {
        if e.get("ordered").and_then(Value::as_bool).unwrap_or(false) {
            return "list".into();
        }
        if e.get("unique").and_then(Value::as_bool).unwrap_or(false) {
            return "set".into();
        }
    }
    let has_relations = config
        .get("relation")
        .map(|v| !v.is_null())
        .or_else(|| {
            output
                .and_then(|o| o.get("relations"))
                .map(|v| !v.is_null())
        })
        .unwrap_or(false);
    if has_relations {
        "hypergraph".into()
    } else {
        "list".into()
    }
}

/// Pick entity/relation configs supporting both the legacy `output` shape and
/// the compilation-template `entity`/`relation` shape.
fn entity_config(config: &Value) -> Value {
    if config.get("entity").is_some() {
        config.get("entity").cloned().unwrap_or(Value::Null)
    } else {
        config
            .get("output")
            .and_then(|o| o.get("entities"))
            .cloned()
            .unwrap_or(Value::Null)
    }
}

fn relation_config(config: &Value) -> Value {
    if config.get("relation").is_some() {
        config.get("relation").cloned().unwrap_or(Value::Null)
    } else {
        config
            .get("output")
            .and_then(|o| o.get("relations"))
            .cloned()
            .unwrap_or(Value::Null)
    }
}

/// Render node and edge prompts (`_struct_hypergraph_prompts`). Returns
/// `(node_prompt, edge_prompt_template)`; the edge template contains the
/// `{known_nodes}` placeholder and is empty when no relations are configured.
pub fn hypergraph_prompts(config: &Value) -> (String, String) {
    let autotype = infer_type(config);
    let guideline = config.get("guideline").cloned().unwrap_or(Value::Null);
    let target = guideline
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or("");
    let rules_e = guideline
        .get("rules_for_entities")
        .and_then(Value::as_str)
        .unwrap_or("");
    let rules_r = guideline
        .get("rules_for_relations")
        .and_then(Value::as_str)
        .unwrap_or("");
    let rules_t = guideline
        .get("rules_for_time")
        .and_then(Value::as_str)
        .unwrap_or("");
    let global_rules = config
        .get("global_rules")
        .and_then(Value::as_str)
        .unwrap_or("");

    let entities_cfg = entity_config(config);
    let relations_cfg = relation_config(config);
    let ent_desc = entities_cfg
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let rel_desc = relations_cfg
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let ent_fields = entities_cfg
        .get("fields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let rel_fields = relations_cfg
        .get("fields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let uses_template_shape = config.get("entity").is_some() || config.get("relation").is_some();

    let (ent_fields_text, ent_skel) = if uses_template_shape {
        render_type_fields(&ent_fields, "entity")
    } else {
        render_fields(&ent_fields)
    };
    let (rel_fields_text, rel_skel) = if uses_template_shape {
        render_type_fields(&rel_fields, "relation")
    } else {
        render_fields(&rel_fields)
    };

    let mut node_parts = Vec::new();
    if !target.is_empty() {
        node_parts.push(format!("# Role and Task:\n{target}"));
    }
    if !global_rules.is_empty() {
        node_parts.push(format!("## Global Rules:\n{global_rules}"));
    }
    if !rules_e.is_empty() {
        node_parts.push(format!("## Entity Extraction Rules:\n{rules_e}"));
    }
    if !ent_desc.is_empty() {
        node_parts.push(format!("## Entity Description:\n{ent_desc}"));
    }
    node_parts.push(format!("## Entity Fields:\n{ent_fields_text}"));
    node_parts.push(format!(
        "## Response Format:\nReply with a single JSON object of the form: \
         {{\"items\": [{ent_skel}, ...]}}.\n\
         Auto-type: \"{autotype}\". {}\nReturn JSON only, no commentary.",
        if autotype == "set" {
            "Items must be unique."
        } else {
            ""
        }
    ));
    let node_prompt = node_parts.join("\n\n");

    if relations_cfg.is_null() {
        return (node_prompt, String::new());
    }

    let mut edge_parts = Vec::new();
    if !target.is_empty() {
        edge_parts.push(format!("# Role and Task:\n{target}"));
    }
    if !global_rules.is_empty() {
        edge_parts.push(format!("## Global Rules:\n{global_rules}"));
    }
    if !rules_r.is_empty() {
        edge_parts.push(format!("## Relation Extraction Rules:\n{rules_r}"));
    }
    if !rules_t.is_empty() {
        edge_parts.push(format!("## Time Rules:\n{rules_t}"));
    }
    if !rel_desc.is_empty() {
        edge_parts.push(format!("## Relation Description:\n{rel_desc}"));
    }
    edge_parts.push(format!("## Relation Fields:\n{rel_fields_text}"));
    edge_parts.push("## Known Entities:\n{known_nodes}".to_string());
    edge_parts.push(format!(
        "## Response Format:\nReply with a single JSON object of the form: \
         {{\"items\": [{rel_skel}, ...]}}.\n\
         Only create relations between entities listed in 'Known Entities'. \
         \nReturn JSON only, no commentary."
    ));
    (node_prompt, edge_parts.join("\n\n"))
}

/// `gen_json` helper: run the chat model and parse a single JSON object from
/// the reply (markdown fences stripped, first `{...}` block taken).
pub async fn gen_json(client: &LlmClient, system_prompt: &str, user_prompt: &str) -> Result<Value> {
    gen_json_with_temperature(client, system_prompt, user_prompt, None).await
}

/// `gen_json` with an optional temperature override (RAGFlow's merge path
/// uses 0.0; hypergraph extraction uses 0.1 via the default generation).
pub async fn gen_json_with_temperature(
    client: &LlmClient,
    system_prompt: &str,
    user_prompt: &str,
    temperature: Option<f32>,
) -> Result<Value> {
    let messages = vec![
        ChatMessage::new("system", system_prompt),
        ChatMessage::new("user", user_prompt),
    ];
    let reply = match temperature {
        Some(temperature) => {
            let patch = crate::generation_params::GenerationParamsPatch {
                temperature: Some(temperature),
                ..Default::default()
            };
            client
                .chat_completion_with_generation(&messages, patch)
                .await?
                .content
        }
        None => client.chat(&messages).await?,
    };
    parse_json_reply(&reply).ok_or_else(|| {
        anyhow::anyhow!(
            "LLM JSON reply could not be parsed: {}",
            truncate(&reply, 160)
        )
    })
}

/// Extract a JSON object from an LLM reply, tolerating markdown fences and
/// surrounding prose.
pub fn parse_json_reply(reply: &str) -> Option<Value> {
    let trimmed = reply.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    let body = body.strip_suffix("```").unwrap_or(body).trim();
    // Try the body first, then progressively strip leading prose until a
    // balanced JSON object parses.
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        return Some(value);
    }
    let start = body.find('{')?;
    let end = body.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<Value>(&body[start..=end]).ok()
}

/// Unwrap `{"items": [...]}` (or a bare array / object) into a Vec of items.
pub fn unwrap_items(value: Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items,
        Value::Object(map) => match map.get("items") {
            Some(Value::Array(items)) => items.clone(),
            Some(_) => vec![Value::Object(map)],
            None => vec![Value::Object(map)],
        },
        _ => vec![],
    }
}

/// Two-stage hypergraph extraction over one text chunk
/// (`_struct_extract_hypergraph`): entities first, then relations constrained
/// to the known entity names. Returns `(entities, relations)` as raw JSON
/// item arrays.
pub async fn extract_hypergraph(
    client: &LlmClient,
    text: &str,
    config: &Value,
) -> Result<(Vec<Value>, Vec<Value>)> {
    let (node_prompt, edge_prompt_template) = hypergraph_prompts(config);
    let user_prompt = format!("## Source Text:\n{text}\n\n## Output (JSON only):");
    let node_res = gen_json(client, &node_prompt, &user_prompt).await?;
    let nodes = unwrap_items(node_res);

    // Known-entity constraint for the relation pass.
    let id_field = "name";
    let mut known_keys: Vec<String> = Vec::new();
    for n in &nodes {
        if let Some(v) = n.get(id_field).and_then(Value::as_str) {
            let v = v.trim();
            if !v.is_empty() && !known_keys.iter().any(|k| k == v) {
                known_keys.push(v.to_string());
            }
        }
    }
    let known_str = if known_keys.is_empty() {
        "(none)".to_string()
    } else {
        known_keys
            .iter()
            .map(|k| format!("- {k}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    if edge_prompt_template.is_empty() {
        return Ok((nodes, vec![]));
    }
    let edge_prompt = edge_prompt_template.replace("{known_nodes}", &known_str);
    let edge_res = gen_json(client, &edge_prompt, &user_prompt).await?;
    Ok((nodes, unwrap_items(edge_res)))
}

/// Flatten chunk-level extractions into one entity/relation list
/// (`compile_structure_from_text`, per-chunk independent extraction without
/// cross-chunk merge). Chunks with the same entity name are deduplicated
/// keeping the first description.
pub async fn compile_hypergraph(
    client: &LlmClient,
    chunks: &[(String, String)],
    config: &Value,
) -> Result<(Vec<Value>, Vec<Value>)> {
    let mut entities: Vec<Value> = Vec::new();
    let mut relations: Vec<Value> = Vec::new();
    let mut seen_names: Vec<String> = Vec::new();
    for (chunk_id, text) in chunks {
        let (es, rs) = extract_hypergraph(client, text, config).await?;
        for e in es {
            let name = e
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let name = name.trim().to_string();
            if !name.is_empty() && !seen_names.iter().any(|n| n == &name) {
                seen_names.push(name.clone());
                let mut entity = e;
                if let Some(map) = entity.as_object_mut() {
                    map.insert(
                        "source_chunk_ids".into(),
                        Value::Array(vec![Value::String(chunk_id.clone())]),
                    );
                }
                entities.push(entity);
            }
        }
        relations.extend(rs);
    }
    Ok((entities, relations))
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let cut: String = s.chars().take(n).collect();
        format!("{cut}…")
    }
}

/// Outcome of running one non-`tree` (hypergraph/list/set) compilation
/// template: extracted entities and relations after LLM dedup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypergraphTemplateResult {
    /// Template id the extraction ran for.
    pub template_id: String,
    /// Deduplicated entity payloads.
    pub entities: Vec<Value>,
    /// Relation payloads.
    pub relations: Vec<Value>,
    /// Entity count after dedup.
    pub entity_count: usize,
    /// Relation count.
    pub relation_count: usize,
}


