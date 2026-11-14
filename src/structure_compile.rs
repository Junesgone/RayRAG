//! Structure knowledge compilation — ported from RAGFlow v0.26.4
//! `rag/advanced_rag/knowlege_compile/structure.py` + `_common.py`.
//!
//! Extends `crate::hypergraph` (which owns the extraction half) with the
//! merge pipeline: per-doc ES-doc building (`_struct_to_es_doc`), local
//! pairwise-cosine dedup + LLM merge (`_struct_local_dedup`), store-side
//! KNN dedup with merge-or-insert (`_struct_es_dedup_one`), strict-chain
//! validation/correction for `list`/`timeline` kinds
//! (`validate_and_correct_chain`), and the compact document-scoped graph
//! JSON rebuild (`rebuild_structure_graph_json`).
//!
//! The shared chunked-pipeline engines (`build_chunk_batches`,
//! `run_chunked_pipeline`) and the three-phase bulk dedup
//! (`bulk_dedup_items`) from `_common.py` live here too; the wiki pipeline
//! reuses them.

use crate::Result;
use crate::embed::Embedder;
use crate::llm::LlmClient;
use crate::merge;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// RAGFlow `prompts/generator.py` `INPUT_UTILIZATION`.
pub const INPUT_UTILIZATION: f64 = 0.5;
/// Token-budget floor shared by all chunked pipelines.
pub const BUDGET_FLOOR: usize = 1024;
/// `_STRUCT_TYPES`.
pub const STRUCT_TYPES: [&str; 3] = ["list", "set", "hypergraph"];
/// Kinds whose relations must form a strict linear chain.
pub const CHAIN_KINDS: [&str; 2] = ["list", "timeline"];
/// Max source-chunk text length passed to the chain-correction LLM prompt.
pub const CHAIN_CORRECTION_MAX_CHUNK_CHARS: usize = 8196;
/// Max source chunks passed to the chain-correction LLM prompt.
pub const CHAIN_CORRECTION_MAX_CHUNKS: usize = 12;
/// Default merge similarity threshold (`merge_compiled_structures`).
pub const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.99;
/// Default max concurrent batches for `run_chunked_pipeline`.
pub const DEFAULT_MAX_WORKERS: usize = 6;
/// Default batch size for the greedy artifact packing mode.
pub const DEFAULT_BATCH_SIZE_CAP: usize = 8;
/// Default window fraction for the greedy artifact packing mode.
pub const DEFAULT_WINDOW_FRACTION: f64 = 0.5;

// ---------------------------------------------------------------------------
// Config helpers (_struct_get / _struct_normalize_kind / _struct_infer_type)
// ---------------------------------------------------------------------------

/// `_struct_get`: case-insensitive lookup against the first matching key.
pub fn cfg_get<'a>(cfg: &'a Value, keys: &[&str], default: &'a Value) -> &'a Value {
    if !cfg.is_object() {
        return default;
    }
    let map = cfg.as_object().unwrap();
    for k in keys {
        if let Some(v) = map.get(*k) {
            return v;
        }
        let kl = k.to_lowercase();
        for (ck, v) in map {
            if ck.eq_ignore_ascii_case(&kl) {
                return v;
            }
        }
    }
    default
}

/// `_struct_normalize_kind`: lowercase, strip, `-`→`_`, and collapse the
/// pageindex/page_index/knowledge_graph aliases into `timeline`.
pub fn normalize_kind(kind: &Value) -> String {
    let Some(s) = kind.as_str() else {
        return String::new();
    };
    let normalized = s.trim().to_lowercase().replace('-', "_");
    match normalized.as_str() {
        "pageindex" | "page_index" | "knowledge_graph" => "timeline".to_string(),
        other => other.to_string(),
    }
}

/// `_struct_infer_type`: explicit `compile_type`, else `kind`, else
/// hypergraph when `output.entities` + `output.relations` exist, else list.
pub fn infer_type(config: &Value) -> String {
    let explicit = normalize_kind(cfg_get(config, &["compile_type"], &Value::Null));
    if STRUCT_TYPES.contains(&explicit.as_str()) {
        return explicit;
    }
    let kind = normalize_kind(cfg_get(config, &["kind"], &Value::Null));
    if !kind.is_empty() {
        return kind;
    }
    let output = cfg_get(config, &["output"], &Value::Null);
    let entities = cfg_get(output, &["entities"], &Value::Null);
    let relations = cfg_get(output, &["relations"], &Value::Null);
    if entities.is_object() && relations.is_object() {
        return "hypergraph".to_string();
    }
    "list".to_string()
}

/// `_struct_supported_type`.
pub fn supported_type(config: &Value, autotype: &str) -> bool {
    if STRUCT_TYPES.contains(&autotype) {
        return true;
    }
    normalize_kind(cfg_get(config, &["kind"], &Value::Null)) == autotype
}

/// `_struct_localize` (structure.py:51) — render multilingual values.
pub fn localize(value: &Value, language: &str) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(i, item)| format!("{}. {}", i + 1, stringify_localized(item)))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => {
            let v = map.get(language).or_else(|| {
                if language != "en" {
                    map.get("en")
                } else {
                    None
                }
            });
            match v {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(items)) => items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| format!("{}. {}", i + 1, stringify_localized(item)))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            }
        }
        _ => String::new(),
    }
}

/// Python `f"{item}"` semantics: strings interpolate bare, other JSON renders quoted.
fn stringify_localized(item: &Value) -> String {
    match item {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `_struct_render_fields`: (bulleted field descriptions, JSON skeleton).
pub fn render_fields(fields: &[Value], language: &str) -> (String, String) {
    let mut lines = Vec::new();
    let mut skeleton_parts = Vec::new();
    for f in fields {
        let name = f.get("name").and_then(Value::as_str).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let ftype = f.get("type").and_then(Value::as_str).unwrap_or("str");
        let desc = localize(f.get("description").unwrap_or(&Value::Null), language);
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

/// `_struct_render_type_fields`: compilation-template shape (allowed item
/// `type` values with descriptions/rules).
pub fn render_type_fields(fields: &[Value], language: &str, kind: &str) -> (String, String) {
    let mut lines = Vec::new();
    let mut type_values: Vec<String> = Vec::new();
    for f in fields {
        let typ = f.get("type").and_then(Value::as_str).unwrap_or("").trim();
        if typ.is_empty() {
            continue;
        }
        type_values.push(typ.to_string());
        lines.push(format!("- type: {typ}"));
        let desc = localize(f.get("description").unwrap_or(&Value::Null), language);
        let rule = localize(f.get("rule").unwrap_or(&Value::Null), language);
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

/// `_struct_hypergraph_prompts`: node/edge prompts for hypergraph kinds.
/// Mirrors `crate::hypergraph::hypergraph_prompts` but honours the
/// `language` argument for multilingual config values.
pub fn hypergraph_prompts(config: &Value, language: &str) -> (String, String) {
    let autotype = infer_type(config);
    let guideline = cfg_get(config, &["guideline"], &Value::Null);
    let output = cfg_get(config, &["output"], &Value::Null);
    let options = cfg_get(config, &["options"], &Value::Null);
    let uses_template_shape = cfg_get(config, &["entity"], &Value::Null).is_object()
        || cfg_get(config, &["relation"], &Value::Null).is_object();

    let target = localize(cfg_get(guideline, &["target"], &Value::Null), language);
    let rules_e = localize(
        cfg_get(guideline, &["rules_for_entities"], &Value::Null),
        language,
    );
    let rules_r = localize(
        cfg_get(guideline, &["rules_for_relations"], &Value::Null),
        language,
    );
    let rules_t = localize(
        cfg_get(guideline, &["rules_for_time"], &Value::Null),
        language,
    );
    let global_rules = localize(cfg_get(config, &["global_rules"], &Value::Null), language);

    let mut rules_t = rules_t;
    let observation_time = cfg_get(options, &["observation_time"], &Value::Null);
    let observation_time = if observation_time.is_string() {
        observation_time.as_str().unwrap().to_string()
    } else {
        chrono::Local::now().format("%Y-%m-%d").to_string()
    };
    if rules_t.contains("{observation_time}") {
        rules_t = rules_t.replace("{observation_time}", &observation_time);
    }

    let entities_cfg = if uses_template_shape {
        cfg_get(config, &["entity"], &Value::Null)
    } else {
        cfg_get(output, &["entities"], &Value::Null)
    };
    let relations_cfg = if uses_template_shape {
        cfg_get(config, &["relation"], &Value::Null)
    } else {
        cfg_get(output, &["relations"], &Value::Null)
    };
    let ent_desc = localize(
        cfg_get(entities_cfg, &["description"], &Value::Null),
        language,
    );
    let rel_desc = localize(
        cfg_get(relations_cfg, &["description"], &Value::Null),
        language,
    );
    let ent_fields: Vec<Value> = cfg_get(entities_cfg, &["fields"], &Value::Null)
        .as_array()
        .cloned()
        .unwrap_or_default();
    let rel_fields: Vec<Value> = cfg_get(relations_cfg, &["fields"], &Value::Null)
        .as_array()
        .cloned()
        .unwrap_or_default();

    let (ent_fields_text, ent_skel) = if uses_template_shape {
        render_type_fields(&ent_fields, language, "entity")
    } else {
        render_fields(&ent_fields, language)
    };
    let (rel_fields_text, rel_skel) = if uses_template_shape {
        render_type_fields(&rel_fields, language, "relation")
    } else {
        render_fields(&rel_fields, language)
    };

    let mut node_parts: Vec<String> = Vec::new();
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
    let uniqueness = if autotype == "set" {
        "Items must be unique. "
    } else {
        ""
    };
    node_parts.push(format!(
        "## Response Format:\nReply with a single JSON object of the form: {{\"items\": [{ent_skel}, ...]}}.\n\
         Auto-type: \"{autotype}\". {uniqueness}Return JSON only, no commentary."
    ));
    let node_prompt = node_parts.join("\n\n");

    if !relations_cfg.is_object() {
        return (node_prompt, String::new());
    }

    let mut edge_parts: Vec<String> = Vec::new();
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
        "## Response Format:\nReply with a single JSON object of the form: {{\"items\": [{rel_skel}, ...]}}.\n\
         Only create relations between entities listed in 'Known Entities'. Return JSON only, no commentary."
    ));
    let edge_prompt = edge_parts.join("\n\n");
    (node_prompt, edge_prompt)
}

/// `_struct_entity_id_field`: the payload field identifying an entity.
pub fn entity_id_field(config: &Value) -> String {
    if cfg_get(config, &["entity"], &Value::Null).is_object() {
        return "name".to_string();
    }
    let identifiers = cfg_get(config, &["identifiers"], &Value::Null);
    let entity_id = cfg_get(identifiers, &["entity_id"], &Value::Null);
    if let Some(s) = entity_id.as_str() {
        let s = s.trim();
        if !s.is_empty() && !s.contains('{') {
            return s.to_string();
        }
    }
    let output = cfg_get(config, &["output"], &Value::Null);
    let entities_cfg = cfg_get(output, &["entities"], &Value::Null);
    if let Some(fields) = entities_cfg.get("fields").and_then(Value::as_array) {
        for f in fields {
            if f.get("required") != Some(&Value::Bool(false)) {
                return f
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("name")
                    .to_string();
            }
        }
    }
    "name".to_string()
}

/// `_struct_unwrap_items`: extract dict items from an LLM JSON reply.
pub fn unwrap_items(res: &Value) -> Vec<Value> {
    match res {
        Value::Null => Vec::new(),
        Value::Object(map) => match map.get("items") {
            Some(Value::Array(items)) => {
                items.iter().filter(|it| it.is_object()).cloned().collect()
            }
            _ => Vec::new(),
        },
        Value::Array(items) => items.iter().filter(|it| it.is_object()).cloned().collect(),
        _ => Vec::new(),
    }
}

/// `_struct_extract_hypergraph`: two LLM JSON passes (entities, then
/// relations constrained to the known entities).
pub async fn extract_hypergraph(
    client: &LlmClient,
    text: &str,
    config: &Value,
    language: &str,
) -> Result<(Vec<Value>, Vec<Value>)> {
    let (node_prompt, edge_prompt_template) = hypergraph_prompts(config, language);
    let user_prompt = format!("## Source Text:\n{text}\n\n## Output (JSON only):");
    let node_res =
        crate::hypergraph::gen_json_with_temperature(client, &node_prompt, &user_prompt, Some(0.1))
            .await?;
    let nodes = unwrap_items(&node_res);

    let id_field = entity_id_field(config);
    let mut known_keys: Vec<String> = Vec::new();
    for n in &nodes {
        if let Some(v) = n.get(&id_field) {
            let s = v.to_string().trim().to_string();
            if !s.is_empty() && !known_keys.iter().any(|k| k == &s) {
                known_keys.push(s);
            }
        }
    }
    let known_str = if known_keys.is_empty() {
        "(none)".to_string()
    } else {
        format!("- {}", known_keys.join("\n- "))
    };

    if edge_prompt_template.is_empty() {
        return Ok((nodes, Vec::new()));
    }
    let edge_prompt = edge_prompt_template.replace("{known_nodes}", &known_str);
    let edge_res =
        crate::hypergraph::gen_json_with_temperature(client, &edge_prompt, &user_prompt, Some(0.1))
            .await?;
    let edges = unwrap_items(&edge_res);
    Ok((nodes, edges))
}

// ---------------------------------------------------------------------------
// Payload helpers (_struct_payload_description / graph row builders)
// ---------------------------------------------------------------------------

/// `_struct_payload_description`: concat string values of every
/// non-description field (lists flattened).
pub fn payload_description(payload: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(map) = payload.as_object() {
        for v in map.values() {
            match v {
                Value::Array(items) => {
                    for item in items {
                        if item.is_null() {
                            continue;
                        }
                        let s = item.to_string().trim().to_string();
                        if !s.is_empty() {
                            parts.push(s);
                        }
                    }
                }
                other => {
                    let s = other.to_string().trim().to_string();
                    if !s.is_empty() {
                        parts.push(s);
                    }
                }
            }
        }
    }
    parts.join(" ")
}

/// `_struct_load_payload`: parse `content_with_weight` into a dict.
pub fn load_payload(doc: &Value) -> Value {
    let raw = doc
        .get("content_with_weight")
        .and_then(Value::as_str)
        .unwrap_or("{}");
    serde_json::from_str(raw).unwrap_or_else(|_| json!({}))
}

/// Order-preserving union of string lists (`_common.union_ordered`).
pub fn union_ordered<I, S, T>(lists: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: IntoIterator<Item = T>,
    T: AsRef<str>,
{
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for list in lists {
        for v in list {
            let s = v.as_ref();
            if s.is_empty() {
                continue;
            }
            if seen.insert(s.to_string()) {
                out.push(s.to_string());
            }
        }
    }
    out
}

/// `_struct_graph_entity`: convert a payload into the graph JSON entity row.
pub fn graph_entity(payload: &Value, source_chunk_ids: Option<&[String]>) -> Option<Value> {
    let name = ["name", "text", "term", "title"]
        .iter()
        .find_map(|k| payload.get(*k).and_then(Value::as_str))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if name.is_empty() {
        return None;
    }
    let typ = payload
        .get("type")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "other".to_string());
    let aliases: Vec<String> = match payload.get("aliases") {
        Some(Value::String(s)) => vec![s.trim().to_string()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    };
    let description = ["description", "discription", "definition_excerpt"]
        .iter()
        .find_map(|k| payload.get(*k).and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string();
    let chunk_ids: Vec<String> = source_chunk_ids.map(|ids| ids.to_vec()).unwrap_or_default();
    Some(json!({
        "aliases": aliases,
        "mention_count": 1,
        "name": name,
        "source_chunk_ids": chunk_ids,
        "type": typ,
        "discription": description,
    }))
}

/// `_struct_graph_relation`: convert a payload into the graph JSON relation row.
pub fn graph_relation(payload: &Value) -> Option<Value> {
    let src = ["source", "src", "from"]
        .iter()
        .find_map(|k| payload.get(*k).and_then(Value::as_str))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let tgt = ["target", "tgt", "to"]
        .iter()
        .find_map(|k| payload.get(*k).and_then(Value::as_str))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if src.is_empty() || tgt.is_empty() {
        return None;
    }
    let typ = payload
        .get("type")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "related".to_string());
    Some(json!({ "from": src, "to": tgt, "type": typ }))
}

/// `_struct_merge_graph_entities`: merge entities by (name, type).
pub fn merge_graph_entities(entities: &[Value]) -> Vec<Value> {
    let mut merged: HashMap<(String, String), Value> = HashMap::new();
    let mut order: Vec<(String, String)> = Vec::new();
    for entity in entities {
        let name = entity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let typ = entity
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("other")
            .to_string();
        let key = (name, typ);
        if let Some(target) = merged.get_mut(&key) {
            let mc = target
                .get("mention_count")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                + entity
                    .get("mention_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(1);
            target["mention_count"] = json!(mc);
            if let Some(aliases) = target.get_mut("aliases").and_then(Value::as_array_mut)
                && let Some(incoming) = entity.get("aliases").and_then(Value::as_array)
            {
                for a in incoming {
                    if let Some(s) = a.as_str()
                        && !aliases.iter().any(|x| x.as_str() == Some(s))
                    {
                        aliases.push(json!(s));
                    }
                }
            }
            let t_desc = target
                .get("discription")
                .and_then(Value::as_str)
                .unwrap_or("");
            let i_desc = entity
                .get("discription")
                .and_then(Value::as_str)
                .unwrap_or("");
            if t_desc.is_empty() && !i_desc.is_empty() {
                target["discription"] = json!(i_desc);
            }
            let merged_ids = union_ordered([
                target
                    .get("source_chunk_ids")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                entity
                    .get("source_chunk_ids")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            ]);
            target["source_chunk_ids"] = json!(merged_ids);
        } else {
            merged.insert(key.clone(), entity.clone());
            order.push(key);
        }
    }
    order
        .into_iter()
        .filter_map(|k| merged.get(&k).cloned())
        .collect()
}

// ---------------------------------------------------------------------------
// ES-doc model (_struct_to_es_doc)
// ---------------------------------------------------------------------------

/// `_struct_to_es_doc`: one store row for an extracted entity or relation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompiledRow {
    pub content_with_weight: String,
    pub compile_kwd: String,
    pub knowledge_graph_kwd: String,
    pub doc_id: String,
    pub source_chunk_ids: Vec<String>,
    pub content_ltks: Vec<String>,
    pub content_sm_ltks: Vec<String>,
    pub q_vec: Vec<f32>,
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compilation_template_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compilation_template_kind_kwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_entity_kwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_entity_kwd: Option<String>,
    /// Present only on graph rows (`knowledge_graph_kwd == "graph"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kb_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_int: Option<i64>,
}

/// `_struct_to_es_doc`: build one store row.
pub fn to_es_doc(
    payload: &Value,
    compile_kwd: &str,
    doc_id: &str,
    chunk_ids: &[String],
    vec: Vec<f32>,
    kind: &str,
    src_field: Option<&str>,
    target_field: Option<&str>,
    compilation_template_id: Option<&str>,
    compilation_template_kind: Option<&str>,
) -> CompiledRow {
    let content_with_weight = payload.to_string();
    let description = payload_description(payload);
    let (content_ltks, content_sm_ltks) = tokenize_for_search(&description);
    let template_id_str = compilation_template_id.unwrap_or("").trim().to_string();
    let mut seed_parts = vec![content_with_weight.clone(), doc_id.to_string()];
    if !template_id_str.is_empty() {
        seed_parts.push(template_id_str.clone());
    }
    let row_id = stable_row_id(&seed_parts);

    let mut from_entity: Option<String> = None;
    let mut to_entity: Option<String> = None;
    if kind == "relation" {
        if let Some(sf) = src_field
            && let Some(v) = payload.get(sf)
        {
            let s = v.to_string().trim().to_string();
            if !s.is_empty() {
                from_entity = Some(s);
            }
        }
        if let Some(tf) = target_field
            && let Some(v) = payload.get(tf)
        {
            let s = v.to_string().trim().to_string();
            if !s.is_empty() {
                to_entity = Some(s);
            }
        }
    }

    CompiledRow {
        content_with_weight,
        compile_kwd: compile_kwd.to_string(),
        knowledge_graph_kwd: kind.to_string(),
        doc_id: doc_id.to_string(),
        source_chunk_ids: chunk_ids.to_vec(),
        content_ltks,
        content_sm_ltks,
        q_vec: vec,
        id: row_id,
        compilation_template_ids: if template_id_str.is_empty() {
            Vec::new()
        } else {
            vec![template_id_str]
        },
        compilation_template_kind_kwd: compilation_template_kind
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        from_entity_kwd: from_entity,
        to_entity_kwd: to_entity,
        kb_id: None,
        available_int: None,
    }
}

/// `stable_row_id`: xxh3-64 hexdigest of `":".join(parts)` — stable per
/// part tuple (RAGFlow uses xxh64; RayRAG uses xxh3-64 with the same
/// idempotent-upsert contract).
pub fn stable_row_id(parts: &[String]) -> String {
    let key = parts.join(":");
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(key.as_bytes()))
}

/// `tokenize_for_search`: (content_ltks, content_sm_ltks) — simplified
/// whitespace/word split mirroring `rag_tokenizer.tokenize` as RayRAG's
/// naive parser does.
pub fn tokenize_for_search(text: &str) -> (Vec<String>, Vec<String>) {
    if text.trim().is_empty() {
        return (Vec::new(), Vec::new());
    }
    let ltks: Vec<String> = text.split_whitespace().map(String::from).collect();
    (ltks.clone(), ltks)
}

/// `find_vec_field`: locate the `q_<dim>_vec` field on a row.
pub fn find_vec_field(row: &Value) -> Option<Vec<f32>> {
    if let Some(map) = row.as_object() {
        for (key, val) in map {
            if key.starts_with("q_") && key.ends_with("_vec") {
                let vec: Vec<f32> = val
                    .as_array()
                    .map(|v| {
                        v.iter()
                            .filter_map(|x| x.as_f64().map(|f| f as f32))
                            .collect()
                    })
                    .unwrap_or_default();
                if !vec.is_empty() {
                    return Some(vec);
                }
            }
        }
    }
    None
}

/// `_struct_doc_template_id`: first non-empty entry of
/// `compilation_template_ids`.
pub fn doc_template_id(row: &Value) -> Option<String> {
    match row.get("compilation_template_ids") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(|s| s.trim().to_string())
            .find(|s| !s.is_empty()),
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

/// `_struct_filter_key`: dedup bucket key (doc_id, compile_kwd,
/// from/to entity, template id).
pub fn filter_key(
    row: &CompiledRow,
) -> (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    (
        row.doc_id.clone(),
        row.compile_kwd.clone(),
        row.from_entity_kwd.clone(),
        row.to_entity_kwd.clone(),
        row.compilation_template_ids.first().cloned(),
    )
}

// ---------------------------------------------------------------------------
// Chunked pipeline engine (_common.build_chunk_batches / run_chunked_pipeline)
// ---------------------------------------------------------------------------

/// One packed batch entry: `{label, chunk_id, text}`.
#[derive(Debug, Clone)]
pub struct PackedEntry {
    pub label: String,
    pub chunk_id: String,
    pub text: String,
}

/// `build_chunk_batches` info dict.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BatchInfo {
    pub total: usize,
    pub kept: usize,
    pub skipped_resume: usize,
    pub skipped_empty: usize,
    pub input_budget: usize,
    pub n_batches: usize,
}

/// Input chunk for the chunked pipelines: id + text.
#[derive(Debug, Clone)]
pub struct ChunkInput {
    pub id: String,
    pub text: String,
}

/// `build_chunk_batches`: filter chunks, pack into batches.
///
/// Two packing modes:
/// - default (`split_chunks`): budget = max_length*UTILIZATION -
///   prompt_overhead_tokens (floored);
/// - greedy: `batch_size_cap` + `window_fraction` token cap.
pub fn build_chunk_batches(
    chunks: &[ChunkInput],
    max_length: usize,
    prompt_overhead_tokens: usize,
    resume_chunk_ids: Option<&HashSet<String>>,
    batch_size_cap: Option<usize>,
    window_fraction: Option<f64>,
    budget_floor: usize,
) -> (Vec<Vec<PackedEntry>>, BatchInfo) {
    let resume_set = resume_chunk_ids.cloned().unwrap_or_default();
    let mut chunk_ids: Vec<String> = Vec::new();
    let mut chunk_texts: Vec<String> = Vec::new();
    let mut skipped_resume = 0usize;
    let mut skipped_empty = 0usize;

    for chunk in chunks {
        if chunk.id.is_empty() {
            skipped_empty += 1;
            continue;
        }
        if resume_set.contains(&chunk.id) {
            skipped_resume += 1;
            continue;
        }
        let text = chunk.text.trim();
        if text.is_empty() {
            skipped_empty += 1;
            continue;
        }
        chunk_ids.push(chunk.id.clone());
        chunk_texts.push(text.to_string());
    }

    let mut batches: Vec<Vec<PackedEntry>> = Vec::new();
    let input_budget: usize;

    if let Some(cap) = batch_size_cap {
        let fraction = window_fraction.unwrap_or(DEFAULT_WINDOW_FRACTION);
        let token_cap = ((max_length as f64 * fraction) as usize).max(budget_floor);
        input_budget = token_cap;
        let mut current: Vec<PackedEntry> = Vec::new();
        let mut current_tks = 0usize;
        for (idx, text) in chunk_texts.iter().enumerate() {
            let tks = crate::chunk::tokenizer::token_count(text);
            let would_overflow_count = current.len() >= cap;
            let would_overflow_tokens = !current.is_empty() && (current_tks + tks > token_cap);
            if would_overflow_count || would_overflow_tokens {
                batches.push(std::mem::take(&mut current));
                current_tks = 0;
            }
            current.push(PackedEntry {
                label: format!("C{}", current.len() + 1),
                chunk_id: chunk_ids[idx].clone(),
                text: text.clone(),
            });
            current_tks += tks;
        }
        if !current.is_empty() {
            batches.push(current);
        }
    } else {
        input_budget = ((max_length as f64 * INPUT_UTILIZATION) as usize)
            .saturating_sub(prompt_overhead_tokens)
            .max(budget_floor);
        // `split_chunks`: pack greedily by token budget, never splitting a
        // single chunk; each batch is a list of {position: text} maps.
        let mut current: Vec<(usize, String)> = Vec::new();
        let mut batch_tokens = 0usize;
        for (idx, text) in chunk_texts.iter().enumerate() {
            let t = crate::chunk::tokenizer::token_count(text);
            if batch_tokens + t > input_budget && !current.is_empty() {
                batches.push(
                    current
                        .drain(..)
                        .enumerate()
                        .map(|(position, (cidx, text))| PackedEntry {
                            label: format!("C{}", position + 1),
                            chunk_id: chunk_ids[cidx].clone(),
                            text,
                        })
                        .collect(),
                );
                batch_tokens = 0;
            }
            current.push((idx, text.clone()));
            batch_tokens += t;
        }
        if !current.is_empty() {
            batches.push(
                current
                    .into_iter()
                    .enumerate()
                    .map(|(position, (cidx, text))| PackedEntry {
                        label: format!("C{}", position + 1),
                        chunk_id: chunk_ids[cidx].clone(),
                        text,
                    })
                    .collect(),
            );
        }
    }

    let info = BatchInfo {
        total: chunks.len(),
        kept: chunk_texts.len(),
        skipped_resume,
        skipped_empty,
        input_budget,
        n_batches: batches.len(),
    };
    (batches, info)
}

/// Progress callback: `(progress 0..=1, message)`.
pub type ProgressCallback = Arc<dyn Fn(f32, &str) + Send + Sync>;

/// `run_chunked_pipeline`: run `process_batch` over batches in parallel
/// under a semaphore; cancel siblings on error; optional aggregate.
pub async fn run_chunked_pipeline<U, T, F, Fut, A>(
    batches: Vec<Vec<PackedEntry>>,
    process_batch: F,
    aggregate: A,
    max_workers: usize,
) -> Result<T>
where
    U: Send + 'static,
    T: Send + 'static,
    F: Fn(Vec<PackedEntry>, usize, usize) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Vec<U>>> + Send,
    A: Fn(Vec<Vec<U>>) -> T + Send + Sync,
{
    if batches.is_empty() {
        return Ok(aggregate(Vec::new()));
    }
    let total = batches.len();
    let semaphore = if max_workers > 0 {
        Some(Arc::new(tokio::sync::Semaphore::new(max_workers)))
    } else {
        None
    };
    let mut tasks = Vec::new();
    let process_batch = std::sync::Arc::new(process_batch);
    for (i, batch) in batches.into_iter().enumerate() {
        let sem = semaphore.clone();
        let process_batch = process_batch.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = match sem {
                Some(s) => Some(s.acquire_owned().await.map_err(|e| anyhow::anyhow!(e))?),
                None => None,
            };
            process_batch(batch, i, total).await
        }));
    }
    let mut results: Vec<Vec<U>> = Vec::new();
    let mut first_error: Option<anyhow::Error> = None;
    for task in tasks {
        match task.await {
            Ok(Ok(item)) => results.push(item),
            Ok(Err(e)) => {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
            Err(e) => {
                if first_error.is_none() {
                    first_error = Some(anyhow::anyhow!("batch task panicked: {e}"));
                }
            }
        }
    }
    if let Some(e) = first_error {
        return Err(e);
    }
    Ok(aggregate(results))
}

// ---------------------------------------------------------------------------
// Dedup: exact + embedding + LLM (`_common.bulk_dedup_items`)
// ---------------------------------------------------------------------------

/// `normalize_key`: lowercase + strip whitespace + strip ASCII punctuation.
pub fn normalize_key(name: &str) -> String {
    name.to_lowercase()
        .trim()
        .chars()
        .filter(|c| !c.is_ascii_punctuation())
        .collect()
}

/// `_exact_dedup_by_key`: group by (normalize(name), type).
pub fn exact_dedup_by_key(
    items: &[Value],
    name_key: &str,
    type_key: Option<&str>,
    aggregate_extra: Option<&dyn Fn(&[Value]) -> Option<Value>>,
) -> Vec<Value> {
    let mut groups: Vec<((String, Option<String>), Vec<Value>)> = Vec::new();
    for it in items {
        if !it.is_object() {
            continue;
        }
        let norm = normalize_key(it.get(name_key).and_then(Value::as_str).unwrap_or(""));
        if norm.is_empty() {
            continue;
        }
        let type_val = type_key
            .and_then(|tk| it.get(tk))
            .and_then(Value::as_str)
            .map(String::from);
        let key = (norm, type_val);
        if let Some((_, group)) = groups.iter_mut().find(|(k, _)| k == &key) {
            group.push(it.clone());
        } else {
            groups.push((key, vec![it.clone()]));
        }
    }

    let mut canonical: Vec<Value> = Vec::new();
    for ((norm, type_val), group) in groups {
        // canonical 名称 = 组内首个出现的名称（对齐 RAGFlow 保留首现语义）
        let best = group
            .first()
            .and_then(|it| it.get(name_key))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default();

        let mut aliases: HashSet<String> = HashSet::new();
        let mut chunk_id_lists: Vec<Vec<String>> = Vec::new();
        let mut mention_count = 0usize;
        for it in &group {
            if let Some(n) = it.get(name_key).and_then(Value::as_str)
                && !n.is_empty()
            {
                aliases.insert(n.to_string());
            }
            if let Some(a) = it.get("aliases").and_then(Value::as_array) {
                for v in a {
                    if let Some(s) = v.as_str()
                        && !s.is_empty()
                    {
                        aliases.insert(s.to_string());
                    }
                }
            }
            chunk_id_lists.push(
                it.get("chunk_ids")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default(),
            );
            mention_count += it.get("mention_count").and_then(Value::as_u64).unwrap_or(1) as usize;
        }
        aliases.remove(&best);

        let mut record = json!({
            name_key: best,
            "aliases": sorted(&aliases),
            "mention_count": mention_count,
            "chunk_ids": union_ordered(chunk_id_lists.iter().map(|v| v.as_slice())),
            "_norm": norm,
        });
        if let Some(tv) = &type_val
            && let Some(tk) = type_key
        {
            record[tk] = json!(tv);
        }
        if let Some(agg) = aggregate_extra
            && let Some(extras) = agg(&group)
            && extras.is_object()
            && let Some(map) = record.as_object_mut()
        {
            for (k, v) in extras.as_object().unwrap() {
                map.insert(k.clone(), v.clone());
            }
        }
        canonical.push(record);
    }
    canonical
}

fn sorted(set: &HashSet<String>) -> Vec<String> {
    let mut v: Vec<String> = set.iter().cloned().collect();
    v.sort();
    v
}

/// Union-find collapse after embedding/LLM merges (`_apply_dedup_merges`).
fn apply_dedup_merges(
    canonical: &[Value],
    merged_into: &HashMap<usize, usize>,
    name_key: &str,
) -> Vec<Value> {
    fn root(i: usize, merged_into: &HashMap<usize, usize>) -> usize {
        let mut cur = i;
        while let Some(parent) = merged_into.get(&cur) {
            cur = *parent;
        }
        cur
    }
    let mut roots: Vec<usize> = (0..canonical.len()).map(|i| root(i, merged_into)).collect();
    roots.sort_unstable();
    roots.dedup();

    let mut out: Vec<Value> = Vec::new();
    for ri in roots {
        let mut base = canonical[ri].clone();
        let mut aliases: HashSet<String> = base
            .get("aliases")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let mut chunk_id_lists: Vec<Vec<String>> = vec![
            base.get("chunk_ids")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
        ];
        let mut mention_count = base
            .get("mention_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        for (i, it) in canonical.iter().enumerate() {
            if i == ri || root(i, merged_into) != ri {
                continue;
            }
            mention_count += it.get("mention_count").and_then(Value::as_u64).unwrap_or(0);
            if let Some(a) = it.get("aliases").and_then(Value::as_array) {
                for v in a {
                    if let Some(s) = v.as_str() {
                        aliases.insert(s.to_string());
                    }
                }
            }
            if let Some(n) = it.get(name_key).and_then(Value::as_str) {
                aliases.insert(n.to_string());
            }
            chunk_id_lists.push(
                it.get("chunk_ids")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default(),
            );
        }
        aliases.remove(base.get(name_key).and_then(Value::as_str).unwrap_or(""));
        base["aliases"] = json!(sorted(&aliases));
        base["mention_count"] = json!(mention_count);
        base["chunk_ids"] = json!(union_ordered(chunk_id_lists.iter().map(|v| v.as_slice())));
        out.push(base);
    }
    out
}

/// `bulk_dedup_items`: three-phase dedup → canonical items.
///
/// Phase 1 (always): exact dedup. Phase 2 (embd provided, n>1):
/// vectorised pairwise cosine; ≥ merge_threshold auto-merge, ambiguous
/// bucket [ambiguous_low, merge_threshold) goes to phase 3. Phase 3
/// (chat provided): batched LLM boolean disambiguation.
pub async fn bulk_dedup_items(
    items: Vec<Value>,
    name_key: &str,
    type_key: Option<&str>,
    chat: Option<&LlmClient>,
    embd: Option<&dyn Embedder>,
    merge_threshold: f32,
    ambiguous_low: f32,
    ambiguous_batch_size: usize,
    strip_norm_key: bool,
) -> Result<Vec<Value>> {
    let canonical = exact_dedup_by_key(&items, name_key, type_key, None);
    if canonical.len() <= 1 || embd.is_none() {
        return Ok(strip_norm(canonical, strip_norm_key));
    }
    let embd = embd.unwrap();
    let names: Vec<&str> = canonical
        .iter()
        .map(|it| it.get(name_key).and_then(Value::as_str).unwrap_or(""))
        .collect();
    let vectors = match embd.embed(&names).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("bulk_dedup: embedding batch failed ({e}); keeping exact-dedup result");
            return Ok(strip_norm(canonical, strip_norm_key));
        }
    };
    if vectors.len() != canonical.len() {
        tracing::warn!("bulk_dedup: embedding count mismatch; keeping exact-dedup result");
        return Ok(strip_norm(canonical, strip_norm_key));
    }

    let n = canonical.len();
    let mut merged_into: HashMap<usize, usize> = HashMap::new();
    let mut ambiguous_pairs: Vec<(usize, usize)> = Vec::new();

    for i in 0..n {
        for j in (i + 1)..n {
            if let Some(tk) = type_key
                && canonical[i].get(tk) != canonical[j].get(tk)
            {
                continue;
            }
            let s = merge::cosine_similarity(&vectors[i], &vectors[j]);
            if s >= merge_threshold {
                merge_root(&mut merged_into, i, j, &canonical);
            } else if s >= ambiguous_low {
                ambiguous_pairs.push((i, j));
            }
        }
    }
    ambiguous_pairs.retain(|(i, j)| root(*i, &merged_into) != root(*j, &merged_into));

    if let Some(chat) = chat
        && !ambiguous_pairs.is_empty()
    {
        let mut k = 0usize;
        while k < ambiguous_pairs.len() {
            let end = (k + ambiguous_batch_size).min(ambiguous_pairs.len());
            let batch: Vec<(usize, usize)> = ambiguous_pairs[k..end]
                .iter()
                .copied()
                .filter(|(i, j)| root(*i, &merged_into) != root(*j, &merged_into))
                .collect();
            if !batch.is_empty() {
                let mut lines: Vec<String> = Vec::new();
                for (idx, (i, j)) in batch.iter().enumerate() {
                    let a_type = type_key
                        .and_then(|tk| canonical[*i].get(tk).and_then(Value::as_str))
                        .map(|t| format!(" ({t})"))
                        .unwrap_or_default();
                    let b_type = type_key
                        .and_then(|tk| canonical[*j].get(tk).and_then(Value::as_str))
                        .map(|t| format!(" ({t})"))
                        .unwrap_or_default();
                    lines.push(format!(
                        "{}. \"{}\"{a_type} vs \"{}\"{b_type}",
                        idx + 1,
                        canonical[*i]
                            .get(name_key)
                            .and_then(Value::as_str)
                            .unwrap_or(""),
                        canonical[*j]
                            .get(name_key)
                            .and_then(Value::as_str)
                            .unwrap_or(""),
                    ));
                }
                let user_prompt = format!(
                    "For each pair below, determine if they refer to the same real-world entity.\n\
                         Return a JSON array of exactly {} booleans (true = same entity, false = different).\n\
                         Return ONLY the JSON array.\n\n{}",
                    batch.len(),
                    lines.join("\n"),
                );
                let system = DEFAULT_DISAMBIGUATE_SYSTEM;
                match crate::hypergraph::gen_json_with_temperature(
                    chat,
                    system,
                    &user_prompt,
                    Some(0.0),
                )
                .await
                {
                    Ok(res) => {
                        let decisions: Vec<Value> = match &res {
                            Value::Array(items) => items.clone(),
                            Value::Object(map) => map
                                .values()
                                .find(|v| v.is_array())
                                .and_then(Value::as_array)
                                .cloned()
                                .unwrap_or_default(),
                            _ => Vec::new(),
                        };
                        for (idx, (i, j)) in batch.iter().enumerate() {
                            let verdict =
                                decisions.get(idx).and_then(Value::as_bool).unwrap_or(false);
                            if verdict {
                                merge_root(&mut merged_into, *i, *j, &canonical);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("bulk_dedup: disambiguation call failed: {e}");
                    }
                }
            }
            k = end;
        }
    }

    let collapsed = apply_dedup_merges(&canonical, &merged_into, name_key);
    Ok(strip_norm(collapsed, strip_norm_key))
}

fn strip_norm(items: Vec<Value>, strip: bool) -> Vec<Value> {
    if !strip {
        return items;
    }
    items
        .into_iter()
        .map(|mut it| {
            if let Some(map) = it.as_object_mut() {
                map.remove("_norm");
            }
            it
        })
        .collect()
}

fn root(i: usize, merged_into: &HashMap<usize, usize>) -> usize {
    let mut cur = i;
    while let Some(parent) = merged_into.get(&cur) {
        cur = *parent;
    }
    cur
}

fn merge_root(merged_into: &mut HashMap<usize, usize>, i: usize, j: usize, canonical: &[Value]) {
    let ri = root(i, merged_into);
    let rj = root(j, merged_into);
    if ri == rj {
        return;
    }
    let mi = canonical[ri]
        .get("mention_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mj = canonical[rj]
        .get("mention_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if mi >= mj {
        merged_into.insert(rj, ri);
    } else {
        merged_into.insert(ri, rj);
    }
}

/// `DEFAULT_DISAMBIGUATE_SYSTEM`.
pub const DEFAULT_DISAMBIGUATE_SYSTEM: &str =
    "You are a named-entity resolution assistant. Return only JSON.";

// ---------------------------------------------------------------------------
// GraphRAG phase-completion markers (rag/graphrag/phase_markers.py)
// ---------------------------------------------------------------------------

/// Phase marker: entity resolution has completed for a KB.
pub const PHASE_RESOLUTION: &str = "resolution_done";
/// Phase marker: community summarization has completed for a KB.
pub const PHASE_COMMUNITY: &str = "community_done";
/// All known phase markers, in pipeline order.
pub const ALL_PHASES: [&str; 2] = [PHASE_RESOLUTION, PHASE_COMMUNITY];
/// Marker TTL: 7 days — well above any single GraphRAG run; keeps stale
/// markers self-pruning if an invalidation path is missed.
pub const PHASE_MARKER_DEFAULT_TTL_SECONDS: u64 = 7 * 24 * 3600;
/// Redis key prefix: markers live under `graphrag:phase:{kb_id}:{phase}`.
pub const PHASE_MARKER_PREFIX: &str = "graphrag:phase:";

/// Build the marker key for `(kb_id, phase)` — KB-scoped (not task-scoped)
/// so markers survive task cancellation and a new task on resume.
pub fn phase_marker_key(kb_id: &str, phase: &str) -> String {
    format!("{PHASE_MARKER_PREFIX}{kb_id}:{phase}")
}

// ---------------------------------------------------------------------------
// GraphRAG entity resolution (rag/graphrag/entity_resolution.py)
// ---------------------------------------------------------------------------

/// `DEFAULT_RECORD_DELIMITER` — record delimiter in the resolution output.
pub const DEFAULT_RECORD_DELIMITER: &str = "##";
/// `DEFAULT_ENTITY_INDEX_DELIMITER` — entity-index delimiter: `<|>1<|>`.
pub const DEFAULT_ENTITY_INDEX_DELIMITER: &str = "<|>";
/// `DEFAULT_RESOLUTION_RESULT_DELIMITER` — verdict delimiter: `&&yes&&`.
pub const DEFAULT_RESOLUTION_RESULT_DELIMITER: &str = "&&";
/// Candidate pairs sent to the LLM per batch.
pub const RESOLUTION_BATCH_SIZE: usize = 100;
/// Max concurrent LLM resolution tasks.
pub const RESOLUTION_MAX_CONCURRENT_TASKS: usize = 5;
/// NER label skip-list (`ner/graph_extractor.py` `_SKIP_SPACY_LABELS`).
pub const NER_SKIP_SPACY_LABELS: [&str; 2] = ["ORDINAL", "CARDINAL"];

/// Port of `rag/nlp.is_english` (single-string form): >80% of the non-empty
/// characters must be ASCII letters/digits or the punctuation set
/// `` `.,':;/"?<>!()- ``.
pub fn text_is_english(s: &str) -> bool {
    let kept: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
    if kept.is_empty() {
        return false;
    }
    let eng = kept
        .iter()
        .filter(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    '`' | '.'
                        | ','
                        | ':'
                        | '\''
                        | ';'
                        | '/'
                        | '"'
                        | '?'
                        | '<'
                        | '>'
                        | '!'
                        | '('
                        | ')'
                        | '-'
                )
        })
        .count();
    (eng as f64 / kept.len() as f64) > 0.8
}

/// Classic Levenshtein edit distance (stands in for `editdistance.eval`).
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// `_has_digit_in_2gram_diff`: the symmetric difference of the two 2-gram
/// sets contains a digram with a digit (names likely differ in a numeric
/// field, so they should not be resolution candidates).
pub fn has_digit_in_2gram_diff(a: &str, b: &str) -> bool {
    fn to_2gram_set(s: &str) -> HashSet<String> {
        let chars: Vec<char> = s.chars().collect();
        (0..chars.len().saturating_sub(1))
            .map(|i| chars[i..i + 2].iter().collect())
            .collect()
    }
    let set_a = to_2gram_set(a);
    let set_b = to_2gram_set(b);
    set_a
        .symmetric_difference(&set_b)
        .any(|pair| pair.chars().any(|c| c.is_numeric()))
}

/// `is_similarity`: deterministic name-similarity gate used to build the
/// entity-resolution candidate pairs (entity_resolution.py).
///
/// * digit-bearing 2-gram diff → not similar;
/// * both English → Levenshtein ≤ min(len)/2;
/// * otherwise → character-set overlap: >1 common chars when max set < 4,
///   else overlap / max_set ≥ 0.8.
pub fn entity_name_similarity(a: &str, b: &str) -> bool {
    if has_digit_in_2gram_diff(a, b) {
        return false;
    }
    if text_is_english(a) && text_is_english(b) {
        return levenshtein_distance(a, b) <= a.chars().count().min(b.chars().count()) / 2;
    }
    let set_a: HashSet<char> = a.chars().collect();
    let set_b: HashSet<char> = b.chars().collect();
    let max_l = set_a.len().max(set_b.len());
    let overlap = set_a.intersection(&set_b).count();
    if max_l < 4 {
        return overlap > 1;
    }
    (overlap as f64 / max_l as f64) >= 0.8
}

/// `__call__` candidate generation: group node names by `entity_type`
/// (default `-`), then take unordered pairs within a group where at least
/// one endpoint belongs to `subgraph_nodes` and `entity_name_similarity`
/// holds.  Returns `(a, b)` name pairs, ordered by (entity_type, node order).
pub fn entity_resolution_candidates(
    nodes: &[(String, Option<&str>)],
    subgraph_nodes: &HashSet<String>,
) -> Vec<(String, String)> {
    let mut types: Vec<&str> = nodes
        .iter()
        .map(|(_, t)| t.unwrap_or("-"))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    types.sort_unstable();
    let mut clusters: Vec<(String, Vec<String>)> = types
        .into_iter()
        .map(|t| (t.to_string(), Vec::new()))
        .collect();
    for (name, etype) in nodes {
        let key = etype.unwrap_or("-");
        if let Some((_, group)) = clusters.iter_mut().find(|(k, _)| k == key) {
            group.push(name.clone());
        }
    }
    let mut pairs = Vec::new();
    for (_, group) in clusters {
        for i in 0..group.len() {
            for j in (i + 1)..group.len() {
                let (a, b) = (&group[i], &group[j]);
                if (subgraph_nodes.contains(a) || subgraph_nodes.contains(b))
                    && entity_name_similarity(a, b)
                {
                    pairs.push((a.clone(), b.clone()));
                }
            }
        }
    }
    pairs
}

/// `_process_results`: parse the LLM resolution output into the 1-based
/// candidate-pair indices whose verdict is `yes`.  Records are split on
/// `record_delimiter`; each record is scanned for `<entity_index_delimiter>
/// <digits> <entity_index_delimiter>` and `<resolution_result_delimiter>
/// <letters> <resolution_result_delimiter>`; indices above `records_length`
/// are dropped (mirrors `re.search` semantics).
pub fn parse_resolution_results(
    results: &str,
    records_length: usize,
    record_delimiter: &str,
    entity_index_delimiter: &str,
    resolution_result_delimiter: &str,
) -> Vec<usize> {
    let re_int = regex::Regex::new(&format!(
        r"{}(\d+){}",
        regex::escape(entity_index_delimiter),
        regex::escape(entity_index_delimiter)
    ))
    .expect("valid entity-index regex");
    let re_bool = regex::Regex::new(&format!(
        r"{}([a-zA-Z]+){}",
        regex::escape(resolution_result_delimiter),
        regex::escape(resolution_result_delimiter)
    ))
    .expect("valid resolution-result regex");

    let mut ans = Vec::new();
    for record in results.split(record_delimiter) {
        let record = record.trim();
        let res_int: usize = re_int
            .captures(record)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0);
        if res_int == 0 || res_int > records_length {
            continue;
        }
        let res_bool = re_bool
            .captures(record)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_ascii_lowercase())
            .unwrap_or_default();
        if !res_bool.is_empty() && res_bool == "yes" {
            ans.push(res_int);
        }
    }
    ans
}

// ---------------------------------------------------------------------------
// Store abstraction (ES I/O wrappers in RayRAG terms)
// ---------------------------------------------------------------------------

/// Result of persisting one row (`_struct_es_dedup_one`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistOutcome {
    Inserted,
    Updated,
    Skipped,
}

/// Knowledge-compilation store: the RayRAG analogue of RAGFlow's ES
/// doc-store wrappers (`_common.es_search` / `es_insert` / `es_delete` /
/// `es_upsert_one`).
#[async_trait::async_trait]
pub trait CompileStore: Send + Sync {
    /// Search rows matching the filter, restricted to one kb.
    /// Returns rows with their stable ids.
    async fn search(
        &self,
        kb_id: &str,
        condition: &CompileFilter,
        limit: usize,
    ) -> Result<Vec<Value>>;
    /// KNN search: filter + top-k by cosine similarity over `q_vec`.
    async fn search_knn(
        &self,
        kb_id: &str,
        condition: &CompileFilter,
        query: &[f32],
        top_n: usize,
    ) -> Result<Vec<Value>>;
    /// Insert rows.
    async fn insert(&self, kb_id: &str, rows: &[CompiledRow]) -> Result<()>;
    /// Update one row by id (partial field overlay).
    async fn update(&self, kb_id: &str, id: &str, fields: &Value) -> Result<()>;
    /// Delete rows matching a filter.
    async fn delete(&self, kb_id: &str, condition: &CompileFilter) -> Result<()>;
    /// Get one row by id.
    async fn get(&self, kb_id: &str, id: &str) -> Result<Option<Value>>;
    /// Delete every row of a document (used when a doc is replaced/removed).
    async fn delete_document(&self, kb_id: &str, doc_id: &str) -> Result<()>;
    /// Downcast support for store-specific operations (graph JSON rebuild).
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Filter condition mirroring the RAGFlow dict conditions.
#[derive(Debug, Clone, Default)]
pub struct CompileFilter {
    pub compile_kwd: Option<String>,
    pub doc_id: Option<String>,
    pub knowledge_graph_kwd: Option<String>,
    pub from_entity_kwd: Option<String>,
    pub to_entity_kwd: Option<String>,
    pub compilation_template_id: Option<String>,
    /// All docs (no doc_id filter).
    pub any_doc: bool,
}

impl CompiledRow {
    /// Condition derived from a row (`_struct_es_dedup_one`).
    pub fn filter_of(&self) -> CompileFilter {
        CompileFilter {
            compile_kwd: Some(self.compile_kwd.clone()),
            doc_id: Some(self.doc_id.clone()),
            knowledge_graph_kwd: (!self.knowledge_graph_kwd.is_empty())
                .then(|| self.knowledge_graph_kwd.clone()),
            from_entity_kwd: self.from_entity_kwd.clone(),
            to_entity_kwd: self.to_entity_kwd.clone(),
            compilation_template_id: self.compilation_template_ids.first().cloned(),
            any_doc: false,
        }
    }
}

impl CompileFilter {
    fn matches(&self, row: &CompiledRow) -> bool {
        if !self.any_doc
            && let Some(d) = &self.doc_id
            && &row.doc_id != d
        {
            return false;
        }
        if let Some(c) = &self.compile_kwd
            && &row.compile_kwd != c
        {
            return false;
        }
        if let Some(k) = &self.knowledge_graph_kwd
            && &row.knowledge_graph_kwd != k
        {
            return false;
        }
        if let Some(f) = &self.from_entity_kwd
            && row.from_entity_kwd.as_ref() != Some(f)
        {
            return false;
        }
        if let Some(t) = &self.to_entity_kwd
            && row.to_entity_kwd.as_ref() != Some(t)
        {
            return false;
        }
        if let Some(tpl) = &self.compilation_template_id
            && !row.compilation_template_ids.iter().any(|t| t == tpl)
        {
            return false;
        }
        true
    }
}

/// In-memory `CompileStore` (tests + single-process default).
#[derive(Clone, Default)]
pub struct MemoryCompileStore {
    rows: Arc<Mutex<HashMap<String, CompiledRow>>>,
}

impl MemoryCompileStore {
    pub fn new() -> Self {
        Self::default()
    }
    fn key(kb_id: &str, id: &str) -> String {
        format!("{kb_id}:{id}")
    }
}

#[async_trait::async_trait]
impl CompileStore for MemoryCompileStore {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn search(
        &self,
        kb_id: &str,
        condition: &CompileFilter,
        limit: usize,
    ) -> Result<Vec<Value>> {
        let rows = self.rows.lock().unwrap();
        let mut out: Vec<Value> = rows
            .iter()
            .filter(|(k, row)| k.starts_with(&format!("{kb_id}:")) && condition.matches(row))
            .take(limit)
            .map(|(_, row)| serde_json::to_value(row).unwrap_or(Value::Null))
            .collect();
        out.sort_by_key(|v| {
            v.get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        });
        Ok(out)
    }

    async fn search_knn(
        &self,
        kb_id: &str,
        condition: &CompileFilter,
        query: &[f32],
        top_n: usize,
    ) -> Result<Vec<Value>> {
        let rows = self.rows.lock().unwrap();
        let mut scored: Vec<(f32, Value)> = rows
            .iter()
            .filter(|(k, row)| k.starts_with(&format!("{kb_id}:")) && condition.matches(row))
            .map(|(_, row)| {
                let sim = merge::cosine_similarity(query, &row.q_vec);
                (sim, serde_json::to_value(row).unwrap_or(Value::Null))
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored.into_iter().take(top_n).map(|(_, v)| v).collect())
    }

    async fn insert(&self, kb_id: &str, rows: &[CompiledRow]) -> Result<()> {
        let mut store = self.rows.lock().unwrap();
        for row in rows {
            store.insert(Self::key(kb_id, &row.id), row.clone());
        }
        Ok(())
    }

    async fn update(&self, kb_id: &str, id: &str, fields: &Value) -> Result<()> {
        let mut store = self.rows.lock().unwrap();
        let key = Self::key(kb_id, id);
        let Some(existing) = store.get_mut(&key) else {
            return Ok(());
        };
        if let Some(map) = fields.as_object() {
            let current = serde_json::to_value(existing.clone()).unwrap_or(Value::Null);
            let mut merged = current;
            if let Some(m) = merged.as_object_mut() {
                for (k, v) in map {
                    m.insert(k.clone(), v.clone());
                }
            }
            if let Ok(row) = serde_json::from_value(merged) {
                *existing = row;
            }
        }
        Ok(())
    }

    async fn delete(&self, kb_id: &str, condition: &CompileFilter) -> Result<()> {
        let mut store = self.rows.lock().unwrap();
        store.retain(|k, row| !(k.starts_with(&format!("{kb_id}:")) && condition.matches(row)));
        Ok(())
    }

    async fn get(&self, kb_id: &str, id: &str) -> Result<Option<Value>> {
        let store = self.rows.lock().unwrap();
        Ok(store
            .get(&Self::key(kb_id, id))
            .map(|row| serde_json::to_value(row).unwrap_or(Value::Null)))
    }

    async fn delete_document(&self, kb_id: &str, doc_id: &str) -> Result<()> {
        let mut store = self.rows.lock().unwrap();
        store.retain(|k, row| !(k.starts_with(&format!("{kb_id}:")) && row.doc_id == doc_id));
        Ok(())
    }
}

/// JSON-file backed `CompileStore` (production default): rows live under
/// `<data_root>/structure_compile/<kb_id>.json`, atomically rewritten on
/// each mutation.
pub struct JsonFileCompileStore {
    dir: PathBuf,
    cache: Arc<Mutex<HashMap<String, Vec<CompiledRow>>>>,
}

impl JsonFileCompileStore {
    pub fn new(data_root: &Path) -> Result<Self> {
        let dir = data_root.join("structure_compile");
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn path(&self, kb_id: &str) -> PathBuf {
        self.dir.join(format!("{kb_id}.json"))
    }

    fn load(&self, kb_id: &str) -> Result<Vec<CompiledRow>> {
        if let Some(rows) = self.cache.lock().unwrap().get(kb_id) {
            return Ok(rows.clone());
        }
        let path = self.path(kb_id);
        let rows: Vec<CompiledRow> = if path.exists() {
            let raw = std::fs::read(&path)?;
            serde_json::from_slice(&raw).unwrap_or_default()
        } else {
            Vec::new()
        };
        self.cache
            .lock()
            .unwrap()
            .insert(kb_id.to_string(), rows.clone());
        Ok(rows)
    }

    fn save(&self, kb_id: &str, rows: &[CompiledRow]) -> Result<()> {
        let bytes = serde_json::to_vec(rows)?;
        crate::persistence::atomic_write(&self.path(kb_id), &bytes)?;
        self.cache
            .lock()
            .unwrap()
            .insert(kb_id.to_string(), rows.to_vec());
        Ok(())
    }

    /// Persist the document-scoped structure graph row (id-stable upsert).
    pub async fn upsert_graph_json(
        &self,
        kb_id: &str,
        graph: &Value,
        compile_kwd: &str,
        doc_id: &str,
        compilation_template_id: Option<&str>,
    ) -> Result<()> {
        let row_id = graph_row_id(doc_id, compile_kwd, compilation_template_id);
        let mut row = CompiledRow {
            content_with_weight: graph.to_string(),
            compile_kwd: compile_kwd.to_string(),
            knowledge_graph_kwd: "graph".to_string(),
            doc_id: doc_id.to_string(),
            source_chunk_ids: Vec::new(),
            content_ltks: Vec::new(),
            content_sm_ltks: Vec::new(),
            q_vec: Vec::new(),
            id: row_id,
            compilation_template_ids: compilation_template_id
                .map(|s| vec![s.to_string()])
                .unwrap_or_default(),
            compilation_template_kind_kwd: None,
            from_entity_kwd: None,
            to_entity_kwd: None,
            kb_id: Some(kb_id.to_string()),
            available_int: Some(0),
        };
        let rows = self.load(kb_id)?;
        let mut rows = rows;
        if let Some(existing) = rows.iter_mut().find(|r| r.id == row.id) {
            std::mem::swap(existing, &mut row);
            row.kb_id = Some(kb_id.to_string());
            row.available_int = Some(0);
        } else {
            rows.push(row);
        }
        self.save(kb_id, &rows)
    }

    /// Rebuild + persist the document-scoped graph (`rebuild_structure_graph_json`).
    pub async fn rebuild_structure_graph_json(
        &self,
        kb_id: &str,
        doc_id: &str,
        compile_kwd: &str,
        compilation_template_id: Option<&str>,
    ) -> Result<Value> {
        let filter = CompileFilter {
            compile_kwd: Some(compile_kwd.to_string()),
            doc_id: Some(doc_id.to_string()),
            knowledge_graph_kwd: None,
            from_entity_kwd: None,
            to_entity_kwd: None,
            compilation_template_id: compilation_template_id.map(String::from),
            any_doc: false,
        };
        let rows = self.search(kb_id, &filter, 10000).await?;
        let mut entities: Vec<Value> = Vec::new();
        let mut relations: Vec<Value> = Vec::new();
        for row in rows {
            let payload = load_payload(&row);
            if row.get("knowledge_graph_kwd").and_then(Value::as_str) == Some("relation") {
                if let Some(relation) = graph_relation(&payload) {
                    relations.push(relation);
                }
            } else if let Some(entity) = graph_entity(&payload, None) {
                let mut entity = entity;
                if let Some(ids) = row.get("source_chunk_ids").and_then(Value::as_array) {
                    entity["source_chunk_ids"] = json!(
                        ids.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect::<Vec<_>>()
                    );
                }
                entities.push(entity);
            }
        }
        let graph = json!({
            "entities": merge_graph_entities(&entities),
            "relations": relations,
        });
        self.upsert_graph_json(kb_id, &graph, compile_kwd, doc_id, compilation_template_id)
            .await?;
        Ok(graph)
    }
}

/// `_struct_graph_row_id`.
pub fn graph_row_id(
    doc_id: &str,
    compile_kwd: &str,
    compilation_template_id: Option<&str>,
) -> String {
    let tpl = compilation_template_id.unwrap_or("");
    format!(
        "{:016x}",
        xxhash_rust::xxh3::xxh3_64(
            format!("{doc_id}:structure_graph:{compile_kwd}:{tpl}").as_bytes()
        )
    )
}

#[async_trait::async_trait]
impl CompileStore for JsonFileCompileStore {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn search(
        &self,
        kb_id: &str,
        condition: &CompileFilter,
        limit: usize,
    ) -> Result<Vec<Value>> {
        let rows = self.load(kb_id)?;
        let mut out: Vec<Value> = rows
            .iter()
            .filter(|row| condition.matches(row))
            .take(limit)
            .map(|row| serde_json::to_value(row).unwrap_or(Value::Null))
            .collect();
        out.sort_by_key(|v| {
            v.get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        });
        Ok(out)
    }

    async fn search_knn(
        &self,
        kb_id: &str,
        condition: &CompileFilter,
        query: &[f32],
        top_n: usize,
    ) -> Result<Vec<Value>> {
        let rows = self.load(kb_id)?;
        let mut scored: Vec<(f32, Value)> = rows
            .iter()
            .filter(|row| condition.matches(row))
            .map(|row| {
                let sim = merge::cosine_similarity(query, &row.q_vec);
                (sim, serde_json::to_value(row).unwrap_or(Value::Null))
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored.into_iter().take(top_n).map(|(_, v)| v).collect())
    }

    async fn insert(&self, kb_id: &str, rows: &[CompiledRow]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut current = self.load(kb_id)?;
        for row in rows {
            if let Some(existing) = current.iter_mut().find(|r| r.id == row.id) {
                *existing = row.clone();
            } else {
                current.push(row.clone());
            }
        }
        self.save(kb_id, &current)
    }

    async fn update(&self, kb_id: &str, id: &str, fields: &Value) -> Result<()> {
        let mut current = self.load(kb_id)?;
        let Some(existing) = current.iter_mut().find(|r| r.id == id) else {
            return Ok(());
        };
        if let Some(map) = fields.as_object() {
            let merged = serde_json::to_value(existing.clone()).unwrap_or(Value::Null);
            let mut merged = merged;
            if let Some(m) = merged.as_object_mut() {
                for (k, v) in map {
                    m.insert(k.clone(), v.clone());
                }
            }
            if let Ok(row) = serde_json::from_value(merged) {
                *existing = row;
            }
        }
        self.save(kb_id, &current)
    }

    async fn delete(&self, kb_id: &str, condition: &CompileFilter) -> Result<()> {
        let current = self.load(kb_id)?;
        let kept: Vec<CompiledRow> = current
            .into_iter()
            .filter(|row| !condition.matches(row))
            .collect();
        self.save(kb_id, &kept)
    }

    async fn get(&self, kb_id: &str, id: &str) -> Result<Option<Value>> {
        let rows = self.load(kb_id)?;
        Ok(rows
            .iter()
            .find(|r| r.id == id)
            .map(|row| serde_json::to_value(row).unwrap_or(Value::Null)))
    }

    async fn delete_document(&self, kb_id: &str, doc_id: &str) -> Result<()> {
        let current = self.load(kb_id)?;
        let kept: Vec<CompiledRow> = current
            .into_iter()
            .filter(|row| row.doc_id != doc_id)
            .collect();
        self.save(kb_id, &kept)
    }
}

// ---------------------------------------------------------------------------
// Merge pipeline (structure.py merge_compiled_structures)
// ---------------------------------------------------------------------------

/// `_struct_relation_member_fields`: (source_field, target_field) for
/// relation payloads.
pub fn relation_member_fields(config: &Value) -> (Option<String>, Option<String>) {
    let identifiers = cfg_get(config, &["identifiers"], &Value::Null);
    let members = cfg_get(identifiers, &["relation_members"], &Value::Null);
    if members.is_object() {
        let map = members.as_object().unwrap();
        let src = map
            .get("source")
            .or_else(|| map.get("src"))
            .and_then(Value::as_str)
            .map(String::from);
        let tgt = map
            .get("target")
            .or_else(|| map.get("tgt"))
            .and_then(Value::as_str)
            .map(String::from);
        if src.is_some() || tgt.is_some() {
            return (src, tgt);
        }
    }
    if cfg_get(config, &["relation"], &Value::Null).is_object() {
        return (Some("source".to_string()), Some("target".to_string()));
    }
    let output = cfg_get(config, &["output"], &Value::Null);
    let relations_cfg = cfg_get(output, &["relations"], &Value::Null);
    if let Some(fields) = relations_cfg.get("fields").and_then(Value::as_array) {
        let mut names: HashSet<String> = HashSet::new();
        for f in fields {
            if let Some(n) = f.get("name").and_then(Value::as_str) {
                names.insert(n.to_string());
            }
        }
        if names.contains("source") && names.contains("target") {
            return (Some("source".to_string()), Some("target".to_string()));
        }
    }
    (None, None)
}

/// `_struct_rebuild_es_doc`: rebuild a row from a merged payload, then
/// overlay identity fields from `base_doc`.
pub fn rebuild_es_doc(
    payload: &Value,
    base_doc: &Value,
    vec: Vec<f32>,
    chunk_ids: &[String],
    preserve_id: bool,
    compilation_template_id: Option<&str>,
    compilation_template_kind: Option<&str>,
) -> CompiledRow {
    let kind = base_doc
        .get("knowledge_graph_kwd")
        .and_then(Value::as_str)
        .unwrap_or("entity");
    let mut src_field: Option<String> = None;
    let mut target_field: Option<String> = None;
    if kind == "relation" {
        let payload_obj = load_payload(base_doc);
        if payload_obj.get("source").is_some() && payload_obj.get("target").is_some() {
            src_field = Some("source".to_string());
            target_field = Some("target".to_string());
        }
    }
    let mut new_doc = to_es_doc(
        payload,
        base_doc
            .get("compile_kwd")
            .and_then(Value::as_str)
            .unwrap_or(""),
        base_doc.get("doc_id").and_then(Value::as_str).unwrap_or(""),
        chunk_ids,
        vec,
        kind,
        src_field.as_deref(),
        target_field.as_deref(),
        compilation_template_id,
        compilation_template_kind,
    );
    if preserve_id && let Some(id) = base_doc.get("id").and_then(Value::as_str) {
        new_doc.id = id.to_string();
    }
    for kwd in ["from_entity_kwd", "to_entity_kwd"] {
        if let Some(v) = base_doc.get(kwd).and_then(Value::as_str)
            && !v.is_empty()
        {
            if kwd == "from_entity_kwd" {
                new_doc.from_entity_kwd = Some(v.to_string());
            } else {
                new_doc.to_entity_kwd = Some(v.to_string());
            }
        }
    }
    new_doc
}

/// `_struct_local_dedup`: single-pass dedup inside `docs`.
/// Returns (deduped, dropped_count).
pub async fn local_dedup(
    docs: &[CompiledRow],
    chat: &LlmClient,
    embd: &dyn Embedder,
    similarity_threshold: f32,
) -> Result<(Vec<CompiledRow>, usize)> {
    let mut groups: Vec<(CompileFilter, Vec<CompiledRow>)> = Vec::new();
    for doc in docs {
        let key = doc.filter_of();
        if let Some((_, group)) = groups.iter_mut().find(|(k, _)| {
            k.doc_id == key.doc_id
                && k.compile_kwd == key.compile_kwd
                && k.from_entity_kwd == key.from_entity_kwd
                && k.to_entity_kwd == key.to_entity_kwd
                && k.compilation_template_id == key.compilation_template_id
        }) {
            group.push(doc.clone());
        } else {
            groups.push((key, vec![doc.clone()]));
        }
    }

    let mut dropped = 0usize;
    let mut deduped: Vec<CompiledRow> = Vec::new();

    for (_, group) in groups {
        let mut kept: Vec<CompiledRow> = Vec::new();
        for incoming in group {
            if incoming.q_vec.is_empty() || kept.is_empty() {
                kept.push(incoming);
                continue;
            }
            let mut kept_with_vecs: Vec<(usize, Vec<f32>)> = Vec::new();
            for (idx, kd) in kept.iter().enumerate() {
                if !kd.q_vec.is_empty() {
                    kept_with_vecs.push((idx, kd.q_vec.clone()));
                }
            }
            if kept_with_vecs.is_empty() {
                kept.push(incoming);
                continue;
            }
            let mut best_idx = 0usize;
            let mut best_sim = f32::MIN;
            for (pos, (_, kv)) in kept_with_vecs.iter().enumerate() {
                let s = merge::cosine_similarity(&incoming.q_vec, kv);
                if s > best_sim {
                    best_sim = s;
                    best_idx = pos;
                }
            }
            if best_sim < similarity_threshold {
                kept.push(incoming);
                continue;
            }
            let existing_idx = kept_with_vecs[best_idx].0;
            let existing = kept[existing_idx].clone();
            let existing_payload = load_payload(&serde_json::to_value(&existing)?);
            let incoming_payload = load_payload(&serde_json::to_value(&incoming)?);
            let merged_payload =
                match merge::merge_pair(&existing_payload, &incoming_payload, chat).await {
                    Ok(Some(m)) => m,
                    _ => {
                        kept.push(incoming);
                        continue;
                    }
                };
            let merged_chunk_ids = union_ordered([
                existing.source_chunk_ids.clone(),
                incoming.source_chunk_ids.clone(),
            ]);
            let desc = payload_description(&merged_payload);
            let new_vec = match embd.embed(&[desc.as_str()]).await {
                Ok(mut v) if !v.is_empty() => v.remove(0),
                _ => {
                    dropped += 1;
                    continue;
                }
            };
            let rebuilt = rebuild_es_doc(
                &merged_payload,
                &serde_json::to_value(&existing)?,
                new_vec,
                &merged_chunk_ids,
                true,
                existing
                    .compilation_template_ids
                    .first()
                    .map(String::as_str),
                existing.compilation_template_kind_kwd.as_deref(),
            );
            kept[existing_idx] = rebuilt;
            dropped += 1;
        }
        deduped.extend(kept);
    }
    Ok((deduped, dropped))
}

/// `_struct_es_dedup_one`: persist a single doc with merge-or-insert.
pub async fn es_dedup_one(
    store: &dyn CompileStore,
    doc: &CompiledRow,
    chat: &LlmClient,
    embd: &dyn Embedder,
    kb_id: &str,
    _similarity_threshold: f32,
) -> Result<PersistOutcome> {
    let condition = doc.filter_of();
    if doc.q_vec.is_empty() {
        store.insert(kb_id, std::slice::from_ref(doc)).await?;
        return Ok(PersistOutcome::Inserted);
    }
    let hits = store.search_knn(kb_id, &condition, &doc.q_vec, 1).await?;
    let Some(old_doc) = hits.into_iter().next() else {
        store.insert(kb_id, std::slice::from_ref(doc)).await?;
        return Ok(PersistOutcome::Inserted);
    };
    let mut old_doc = old_doc;
    if old_doc.get("id").and_then(Value::as_str).is_none()
        && let Some(map) = old_doc.as_object_mut()
    {
        map.insert("id".into(), json!(doc.id));
    }
    let old_payload = load_payload(&old_doc);
    let incoming_payload = load_payload(&serde_json::to_value(doc)?);
    let merged_payload = match merge::merge_pair(&old_payload, &incoming_payload, chat).await {
        Ok(Some(m)) => m,
        _ => {
            store.insert(kb_id, std::slice::from_ref(doc)).await?;
            return Ok(PersistOutcome::Inserted);
        }
    };
    let merged_chunk_ids = union_ordered([
        old_doc
            .get("source_chunk_ids")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
        doc.source_chunk_ids.clone(),
    ]);
    let desc = payload_description(&merged_payload);
    let new_vec = match embd.embed(&[desc.as_str()]).await {
        Ok(mut v) if !v.is_empty() => v.remove(0),
        _ => return Ok(PersistOutcome::Skipped),
    };
    let rebuilt = rebuild_es_doc(
        &merged_payload,
        &old_doc,
        new_vec,
        &merged_chunk_ids,
        true,
        doc.compilation_template_ids.first().map(String::as_str),
        doc.compilation_template_kind_kwd.as_deref(),
    );
    let old_id = old_doc
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(&doc.id)
        .to_string();
    let fields = serde_json::to_value(&rebuilt)?;
    store.update(kb_id, &old_id, &fields).await?;
    Ok(PersistOutcome::Updated)
}

/// `merge_compiled_structures`: local dedup then store-side KNN dedup with
/// LLM merge, plus per-(doc, compile_kwd, template) graph JSON rebuild.
pub async fn merge_compiled_structures(
    store: &dyn CompileStore,
    docs: &[CompiledRow],
    chat: &LlmClient,
    embd: &dyn Embedder,
    kb_id: &str,
    similarity_threshold: f32,
) -> Result<serde_json::Value> {
    if docs.is_empty() {
        return Ok(json!({
            "inserted": 0, "updated": 0, "duplicates_dropped": 0, "graphs": 0,
        }));
    }
    let (deduped, dropped) = local_dedup(docs, chat, embd, similarity_threshold).await?;

    let mut graph_keys: Vec<(String, String, String)> = Vec::new();
    for d in &deduped {
        if d.knowledge_graph_kwd == "entity" || d.knowledge_graph_kwd == "relation" {
            let tpl = d
                .compilation_template_ids
                .first()
                .cloned()
                .unwrap_or_default();
            let key = (d.doc_id.clone(), d.compile_kwd.clone(), tpl);
            if !graph_keys.iter().any(|k| k == &key) {
                graph_keys.push(key);
            }
        }
    }

    let mut inserted = 0usize;
    let mut updated = 0usize;
    for d in &deduped {
        match es_dedup_one(store, d, chat, embd, kb_id, similarity_threshold).await {
            Ok(PersistOutcome::Inserted) => inserted += 1,
            Ok(PersistOutcome::Updated) => updated += 1,
            Ok(PersistOutcome::Skipped) => {}
            Err(e) => {
                tracing::warn!("merge_compiled_structures: per-doc dedup failed: {e}");
            }
        }
    }

    let mut graphs = 0usize;
    if let Some(json_store) = store.as_any().downcast_ref::<JsonFileCompileStore>() {
        for (doc_id, compile_kwd, template_id) in &graph_keys {
            let tpl = (!template_id.is_empty()).then_some(template_id.as_str());
            match json_store
                .rebuild_structure_graph_json(kb_id, doc_id, compile_kwd, tpl)
                .await
            {
                Ok(_) => graphs += 1,
                Err(e) => {
                    tracing::warn!(
                        "merge_compiled_structures: graph rebuild failed for doc={doc_id} compile_kwd={compile_kwd}: {e}"
                    );
                }
            }
        }
    }

    Ok(json!({
        "inserted": inserted,
        "updated": updated,
        "duplicates_dropped": dropped,
        "graphs": graphs,
    }))
}

// ---------------------------------------------------------------------------
// Strict-chain validation (_struct chain helpers)
// ---------------------------------------------------------------------------

/// `CHAIN_CORRECTION_PROMPT`.
pub const CHAIN_CORRECTION_PROMPT: &str = "You are correcting an extracted {kind}-kind structure.\n\n\
Constraint: the relations must form a strict linear chain — every entity has\n\
at most one predecessor and at most one successor, and there must be no\n\
cycle. The relations below were flagged by an automated detector as\n\
violating this constraint. Each one carries the issue that was detected.\n\n\
Bad relations (review and keep only those supported by the source text):\n\
{bad_relations_json}\n\n\
Source chunks the relations were extracted from:\n\
{source_chunks_text}\n\n\
Your task: from the bad relations above, pick the subset that should be\n\
kept. Drop the rest. Do not invent new relations. Use only ``from`` and\n\
``to`` slugs that appear verbatim in the bad-relations list. The result\n\
must satisfy the strict-chain constraint.\n\n\
Return ONLY a JSON object with this exact shape (no markdown fences, no\n\
commentary):\n\
{{\n  \"keep\": [\n    {{\"from\": \"<slug>\", \"to\": \"<slug>\"}},\n    ...\n  ]\n}}";

/// `_chain_extract_edge`: (from_slug, to_slug) for a relation row.
pub fn chain_extract_edge(doc: &Value) -> Option<(String, String)> {
    if doc.get("knowledge_graph_kwd").and_then(Value::as_str) != Some("relation") {
        return None;
    }
    let src = doc.get("from_entity_kwd").and_then(Value::as_str);
    let tgt = doc.get("to_entity_kwd").and_then(Value::as_str);
    if let (Some(s), Some(t)) = (src, tgt) {
        let (s, t) = (s.trim(), t.trim());
        if !s.is_empty() && !t.is_empty() {
            return Some((s.to_string(), t.to_string()));
        }
    }
    let payload = load_payload(doc);
    for (src_key, tgt_key) in [("source", "target"), ("from", "to"), ("src", "tgt")] {
        let s = payload.get(src_key).and_then(Value::as_str);
        let t = payload.get(tgt_key).and_then(Value::as_str);
        if let (Some(s), Some(t)) = (s, t) {
            let (s, t) = (s.trim(), t.trim());
            if !s.is_empty() && !t.is_empty() {
                return Some((s.to_string(), t.to_string()));
            }
        }
    }
    None
}

/// `_chain_detect_violations`: self-loop / fan-out / fan-in / cycle (SCC).
/// Returns {edge: [issue strings]}.
pub fn chain_detect_violations(
    edges: &[(String, String)],
) -> HashMap<(String, String), Vec<String>> {
    let mut issues: HashMap<(String, String), Vec<String>> = HashMap::new();
    let mut add = |edge: (String, String), reason: String| {
        issues.entry(edge).or_default().push(reason);
    };

    let mut out_groups: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut in_groups: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for e in edges {
        if e.0 == e.1 {
            add(e.clone(), "self-loop".to_string());
        }
        out_groups.entry(e.0.clone()).or_default().push(e.clone());
        in_groups.entry(e.1.clone()).or_default().push(e.clone());
    }
    for (node, group) in &out_groups {
        if group.len() > 1 {
            let mut siblings: Vec<String> = group.iter().map(|g| g.1.clone()).collect();
            siblings.sort();
            siblings.dedup();
            let reason = format!(
                "fan-out from '{node}' (also points to {})",
                siblings.join(", ")
            );
            for e in group {
                add(e.clone(), reason.clone());
            }
        }
    }
    for (node, group) in &in_groups {
        if group.len() > 1 {
            let mut siblings: Vec<String> = group.iter().map(|g| g.0.clone()).collect();
            siblings.sort();
            siblings.dedup();
            let reason = format!(
                "fan-in to '{node}' (also reached from {})",
                siblings.join(", ")
            );
            for e in group {
                add(e.clone(), reason.clone());
            }
        }
    }

    // Iterative Tarjan SCC — any SCC of size ≥ 2 is a cycle.
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    let mut nodes: HashSet<String> = HashSet::new();
    for (src, tgt) in edges {
        nodes.insert(src.clone());
        nodes.insert(tgt.clone());
        adj.entry(src.clone()).or_default().push(tgt.clone());
    }
    let sccs = tarjan_scc_iterative(&adj);
    for comp in &sccs {
        if comp.len() < 2 {
            continue;
        }
        let mut sorted_comp: Vec<&String> = comp.iter().collect();
        sorted_comp.sort();
        let label = format!(
            "cycle within [{}]",
            sorted_comp
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        for (src, tgt) in edges {
            if comp.contains(src) && comp.contains(tgt) {
                add((src.clone(), tgt.clone()), label.clone());
            }
        }
    }
    issues
}

/// Iterative Tarjan strongly-connected-components.
pub fn tarjan_scc_iterative(adj: &HashMap<String, Vec<String>>) -> Vec<HashSet<String>> {
    let mut index_map: HashMap<String, usize> = HashMap::new();
    let mut lowlink: HashMap<String, usize> = HashMap::new();
    let mut on_stack: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = Vec::new();
    let mut next_index = 0usize;
    let mut sccs: Vec<HashSet<String>> = Vec::new();

    // Explicit frame stack: (node, neighbor_iter_index, phase)
    let mut nodes: Vec<String> = adj.keys().cloned().collect();
    nodes.sort();
    for start in nodes {
        if index_map.contains_key(&start) {
            continue;
        }
        // frames: (node, Option<next child>)
        let mut frames: Vec<(String, usize)> = vec![(start.clone(), 0)];
        index_map.insert(start.clone(), next_index);
        lowlink.insert(start.clone(), next_index);
        next_index += 1;
        stack.push(start.clone());
        on_stack.insert(start.clone());

        while let Some((node, child_pos)) = frames.last_mut() {
            let node = node.clone();
            let children = adj.get(&node).cloned().unwrap_or_default();
            let pos = *child_pos;
            if pos < children.len() {
                *child_pos = pos + 1;
                let w = children[pos].clone();
                if !index_map.contains_key(&w) {
                    index_map.insert(w.clone(), next_index);
                    lowlink.insert(w.clone(), next_index);
                    next_index += 1;
                    stack.push(w.clone());
                    on_stack.insert(w.clone());
                    frames.push((w.clone(), 0));
                } else if on_stack.contains(&w) {
                    let l = lowlink[&node].min(index_map[&w]);
                    lowlink.insert(node.clone(), l);
                }
            } else {
                frames.pop();
                if let Some((parent, _)) = frames.last() {
                    let l = lowlink[parent].min(lowlink[&node]);
                    lowlink.insert(parent.clone(), l);
                }
                if lowlink[&node] == index_map[&node] {
                    let mut comp: HashSet<String> = HashSet::new();
                    loop {
                        let w = stack.pop().unwrap();
                        on_stack.remove(&w);
                        comp.insert(w.clone());
                        if w == node {
                            break;
                        }
                    }
                    sccs.push(comp);
                }
            }
        }
    }
    sccs
}

/// `_chain_gather_chunk_text`: collect (chunk_id, text) pairs for the LLM
/// prompt — deduplicated, capped.
pub fn chain_gather_chunk_text(
    bad_docs: &[Value],
    chunks_by_id: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<(String, String)> = Vec::new();
    for doc in bad_docs {
        if let Some(ids) = doc.get("source_chunk_ids").and_then(Value::as_array) {
            for v in ids {
                let Some(cid) = v.as_str() else { continue };
                if !seen.insert(cid.to_string()) {
                    continue;
                }
                let Some(text) = chunks_by_id.get(cid) else {
                    continue;
                };
                if text.trim().is_empty() {
                    continue;
                }
                let truncated: String = text
                    .chars()
                    .take(CHAIN_CORRECTION_MAX_CHUNK_CHARS)
                    .collect();
                out.push((cid.to_string(), truncated));
                if out.len() >= CHAIN_CORRECTION_MAX_CHUNKS {
                    return out;
                }
            }
        }
    }
    out
}

/// `validate_and_correct_chain`: ensure chain shape; LLM picks the subset
/// of offending relations to keep. Best-effort — on any failure returns
/// `docs` verbatim.
pub async fn validate_and_correct_chain(
    docs: &[Value],
    chunks_by_id: &HashMap<String, String>,
    chat: &LlmClient,
    kind: &str,
) -> Vec<Value> {
    if docs.is_empty() || !CHAIN_KINDS.contains(&kind) {
        return docs.to_vec();
    }
    let mut edge_to_docs: HashMap<(String, String), Vec<Value>> = HashMap::new();
    let mut all_edges: Vec<(String, String)> = Vec::new();
    for d in docs {
        if let Some(e) = chain_extract_edge(d) {
            edge_to_docs.entry(e.clone()).or_default().push(d.clone());
            all_edges.push(e);
        }
    }
    let violations = chain_detect_violations(&all_edges);
    if violations.is_empty() {
        return docs.to_vec();
    }
    let bad_edges: Vec<(String, String)> = violations.keys().cloned().collect();
    let mut bad_docs: Vec<Value> = Vec::new();
    for e in &bad_edges {
        if let Some(ds) = edge_to_docs.get(e) {
            bad_docs.extend(ds.iter().cloned());
        }
    }
    let bad_relations_repr: Vec<Value> = violations
        .iter()
        .map(|(e, reasons)| {
            json!({
                "from": e.0,
                "to": e.1,
                "issue": reasons.join("; "),
            })
        })
        .collect();
    let chunk_pairs = chain_gather_chunk_text(&bad_docs, chunks_by_id);
    let source_chunks_text = if chunk_pairs.is_empty() {
        "(no source chunks available)".to_string()
    } else {
        chunk_pairs
            .iter()
            .map(|(cid, text)| format!("[{cid}]\n{text}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let prompt = CHAIN_CORRECTION_PROMPT
        .replace("{kind}", kind)
        .replace(
            "{bad_relations_json}",
            &serde_json::to_string(&bad_relations_repr).unwrap_or_default(),
        )
        .replace("{source_chunks_text}", &source_chunks_text);

    let res = crate::hypergraph::gen_json_with_temperature(
        chat,
        "You correct extracted graph relations to satisfy a strict-chain constraint.",
        &prompt,
        Some(0.0),
    )
    .await;
    let Ok(res) = res else {
        return docs.to_vec();
    };
    let Some(keep_raw) = res.get("keep").and_then(Value::as_array) else {
        return docs.to_vec();
    };
    let bad_edge_set: HashSet<(String, String)> = bad_edges.iter().cloned().collect();
    let mut keep_set: HashSet<(String, String)> = HashSet::new();
    for item in keep_raw {
        let Some(s) = item.get("from").and_then(Value::as_str) else {
            continue;
        };
        let Some(t) = item.get("to").and_then(Value::as_str) else {
            continue;
        };
        let edge = (s.trim().to_string(), t.trim().to_string());
        if bad_edge_set.contains(&edge) {
            keep_set.insert(edge);
        }
    }
    if keep_set == bad_edge_set {
        return docs.to_vec();
    }
    let mut dropped_doc_ids: HashSet<String> = HashSet::new();
    for edge in bad_edge_set.difference(&keep_set) {
        if let Some(ds) = edge_to_docs.get(edge) {
            for d in ds {
                if let Some(id) = d.get("id").and_then(Value::as_str) {
                    dropped_doc_ids.insert(id.to_string());
                }
            }
        }
    }
    if dropped_doc_ids.is_empty() {
        return docs.to_vec();
    }
    docs.iter()
        .filter(|d| {
            d.get("id")
                .and_then(Value::as_str)
                .map(|id| !dropped_doc_ids.contains(id))
                .unwrap_or(true)
        })
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// GraphRAG entity/relation alignment
// (rag/graphrag/general/graph_extractor.py + general/extractor.py +
//  rag/graphrag/utils.py + rag/graphrag/ner/graph_extractor.py)
//
// Pure-function ports of the record-parsing half of the LLM graph
// extractor: tolerant JSON repair (the `json_repair` behaviour RAGFlow
// applies to LLM JSON output), the tuple-delimited `("entity"<|>...)`
// record format with per-record fault tolerance, and the
// `_merge_nodes` / `_merge_edges` merge semantics.  Also carries the
// graphrag constants that were missing from this module.
// ---------------------------------------------------------------------------

/// `DEFAULT_ENTITY_TYPES` — extractor.py default when none configured.
pub const DEFAULT_ENTITY_TYPES: [&str; 5] = ["organization", "person", "geo", "event", "category"];

/// `ENTITY_EXTRACTION_MAX_GLEANINGS` — extractor.py:47.
pub const ENTITY_EXTRACTION_MAX_GLEANINGS: usize = 2;

/// `MAX_CONCURRENT_PROCESS_AND_EXTRACT_CHUNK` — extractor.py:48.
pub const MAX_CONCURRENT_PROCESS_AND_EXTRACT_CHUNK: usize = 10;

/// `GRAPHRAG_MAX_ERRORS` — extractor.py `__call__` per-chunk error budget.
pub const GRAPHRAG_MAX_ERRORS: usize = 3;

/// `_handle_entity_relation_summary` truncation budget (extractor.py:353).
pub const ENTITY_SUMMARY_MAX_TOKENS: usize = 512;

/// `_handle_entity_relation_summary` — descriptions lists longer than this
/// are sent to the LLM for summarisation (extractor.py:356).
pub const ENTITY_SUMMARY_DESCRIPTION_LIST_LIMIT: usize = 12;

/// `GRAPH_FIELD_SEP` — utils.py:36, separator for joined descriptions.
pub const GRAPH_FIELD_SEP: &str = "<SEP>";

/// `DEFAULT_RECORD_DELIMITER` — defined above in the entity-resolution
/// section (line ~1465); shared by the tuple-record parsing here.
/// `DEFAULT_TUPLE_DELIMITER` — graph_prompt.py:21.
pub const DEFAULT_TUPLE_DELIMITER: &str = "<|>";
/// `DEFAULT_COMPLETION_DELIMITER` — graph_prompt.py:23.
pub const DEFAULT_COMPLETION_DELIMITER: &str = "<|COMPLETE|>";

/// `SPACY_TO_APP_ENTITY_TYPE` — ner/graph_extractor.py:86.
pub const SPACY_TO_APP_ENTITY_TYPE: [(&str, &str); 15] = [
    ("PERSON", "person"),
    ("ORG", "organization"),
    ("GPE", "geo"),
    ("LOC", "geo"),
    ("FAC", "geo"),
    ("EVENT", "event"),
    ("PRODUCT", "category"),
    ("WORK_OF_ART", "category"),
    ("LAW", "category"),
    ("LANGUAGE", "category"),
    ("NORP", "category"),
    ("MONEY", "category"),
    ("QUANTITY", "category"),
    ("TIME", "event"),
    ("DATE", "event"),
];

/// `SPACY_TO_APP_ENTITY_TYPE.get(label, "category")` — labels not listed
/// fall through to `"category"`.
pub fn spacy_to_app_entity_type(label: &str) -> &'static str {
    SPACY_TO_APP_ENTITY_TYPE
        .iter()
        .find(|(k, _)| *k == label)
        .map(|(_, v)| *v)
        .unwrap_or("category")
}

/// `_has_uppercase` — ner/graph_extractor.py:113.
pub fn has_uppercase(text: &str) -> bool {
    text.chars().any(|c| c.is_uppercase())
}

/// `_replace_word` — ner/graph_extractor.py:117 (MGranRAG): normalise
/// spaces around hyphens and apostrophes.
pub fn replace_word(word: &str) -> String {
    word.replace(" - ", "-")
        .replace(" -", "-")
        .replace("- ", "-")
        .replace(" 's", "'s")
        .replace(" 'S", "'S")
}

/// Minimal HTML entity unescape (`&amp; &lt; &gt; &quot; &#39; &nbsp;`) —
/// mirrors Python `html.unescape` for the entities the LLM paths produce.
fn unescape_html_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        let Some(semicolon) = rest.find(';') else {
            out.push_str(rest);
            return out;
        };
        let entity = &rest[..=semicolon];
        let replacement = match entity {
            "&amp;" => "&",
            "&lt;" => "<",
            "&gt;" => ">",
            "&quot;" => "\"",
            "&#39;" => "'",
            "&nbsp;" => " ",
            _ => entity,
        };
        out.push_str(replacement);
        rest = &rest[semicolon + 1..];
    }
    out.push_str(rest);
    out
}

/// `clean_str` — utils.py:145: HTML-unescape, strip, drop control
/// characters (0x00-0x1f, 0x7f-0x9f) and stray double quotes.
pub fn clean_str(input: &str) -> String {
    unescape_html_entities(input.trim())
        .chars()
        .filter(|c| {
            let cp = *c as u32;
            !(cp <= 0x1f || (0x7f..=0x9f).contains(&cp)) && *c != '"'
        })
        .collect()
}

/// `split_string_by_multi_markers` — utils.py:356: split on any of the
/// markers (regex-escaped, joined with `|`), dropping empty pieces.
pub fn split_string_by_multi_markers(content: &str, markers: &[&str]) -> Vec<String> {
    if markers.is_empty() {
        let t = content.trim();
        return if t.is_empty() {
            Vec::new()
        } else {
            vec![t.to_string()]
        };
    }
    let pattern = markers
        .iter()
        .map(|m| regex::escape(m))
        .collect::<Vec<_>>()
        .join("|");
    let re = regex::Regex::new(&pattern).expect("valid marker regex");
    re.split(content)
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .collect()
}

static FLOAT_LITERAL_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();

/// `is_float_regex` — utils.py:364: `^[-+]?[0-9]*\.?[0-9]+$`.
pub fn is_float_regex(value: &str) -> bool {
    FLOAT_LITERAL_RE
        .get_or_init(|| regex::Regex::new(r"^[-+]?[0-9]*\.?[0-9]+$").expect("float literal regex"))
        .is_match(value)
}

/// `_process_single_content` record split (graph_extractor.py:136): split
/// the raw LLM output on (record_delimiter, completion_delimiter), then
/// keep only the parenthesised payload of each record via
/// `re.search(r"\((.*)\)")`.  Records without a parenthesised body are
/// dropped — this is the primary fault-tolerance gate for non-JSON
/// (tuple-delimited) LLM output.
pub fn parse_tuple_records(
    raw: &str,
    record_delimiter: &str,
    completion_delimiter: &str,
) -> Vec<String> {
    let paren_re = regex::Regex::new(r"\((.*)\)").expect("paren capture regex");
    split_string_by_multi_markers(raw, &[record_delimiter, completion_delimiter])
        .into_iter()
        .filter_map(|record| {
            paren_re
                .captures(&record)
                .map(|c| c.get(1).expect("capture 1").as_str().to_string())
        })
        .collect()
}

/// `handle_single_entity_extraction` — utils.py:307.  Accepts both the
/// canonical quoted tag `"entity"` and the bare `entity` (LLM tolerance).
/// Fields are `clean_str`-ed; the name and type are uppercased; records
/// with fewer than 4 attributes or an empty name are dropped.
pub fn handle_single_entity_extraction(
    record_attributes: &[String],
    chunk_key: &str,
) -> Option<Value> {
    if record_attributes.len() < 4 {
        return None;
    }
    let tag = record_attributes[0].trim();
    if tag != "\"entity\"" && tag != "entity" {
        return None;
    }
    let entity_name = clean_str(&record_attributes[1].to_uppercase());
    if entity_name.trim().is_empty() {
        return None;
    }
    let entity_type = clean_str(&record_attributes[2].to_uppercase());
    let entity_description = clean_str(&record_attributes[3]);
    Some(json!({
        "entity_name": entity_name.to_uppercase(),
        "entity_type": entity_type.to_uppercase(),
        "description": entity_description,
        "source_id": chunk_key,
    }))
}

/// `handle_single_relationship_extraction` — utils.py:328.  Accepts both
/// `"relationship"` and `relationship` tags.  Needs ≥5 attributes; the
/// strength is the last attribute parsed as a float when it matches
/// `is_float_regex`, otherwise the default `1.0`; `src_id`/`tgt_id` are
/// the sorted-uppercased endpoint pair.
pub fn handle_single_relationship_extraction(
    record_attributes: &[String],
    chunk_key: &str,
) -> Option<Value> {
    if record_attributes.len() < 5 {
        return None;
    }
    let tag = record_attributes[0].trim();
    if tag != "\"relationship\"" && tag != "relationship" {
        return None;
    }
    let source = clean_str(&record_attributes[1].to_uppercase());
    let target = clean_str(&record_attributes[2].to_uppercase());
    let edge_description = clean_str(&record_attributes[3]);
    let edge_keywords = clean_str(&record_attributes[4]);
    let last = record_attributes.last().expect("len >= 5");
    let weight = if is_float_regex(last) {
        last.trim().parse::<f64>().unwrap_or(1.0)
    } else {
        1.0
    };
    let (src, tgt) = if source <= target {
        (source, target)
    } else {
        (target, source)
    };
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    Some(json!({
        "src_id": src.to_uppercase(),
        "tgt_id": tgt.to_uppercase(),
        "weight": weight,
        "description": edge_description,
        "keywords": edge_keywords,
        "source_id": chunk_key,
        "metadata": { "created_at": created_at },
    }))
}

/// `_entities_and_relations` — extractor.py:114: dispatch each record to
/// the entity/relation handlers.  Entity types must be in the lowercased
/// allow-list; nodes are deduplicated by `entity_name` (first occurrence
/// wins, mirroring the `ent_records` dict); every valid edge record is
/// kept (merging happens downstream, `_merge_edges`).
pub fn entities_and_relations(
    records: &[String],
    tuple_delimiter: &str,
    entity_types: &[&str],
    chunk_key: &str,
) -> (Vec<Value>, Vec<Value>) {
    let allowed: HashSet<String> = entity_types.iter().map(|t| t.to_lowercase()).collect();
    let mut nodes: Vec<Value> = Vec::new();
    let mut seen_nodes: HashSet<String> = HashSet::new();
    let mut edges: Vec<Value> = Vec::new();
    for record in records {
        let attrs = split_string_by_multi_markers(record, &[tuple_delimiter]);
        if let Some(entity) = handle_single_entity_extraction(&attrs, chunk_key) {
            let etype = entity
                .get("entity_type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            if allowed.contains(&etype) {
                let name = entity
                    .get("entity_name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if seen_nodes.insert(name) {
                    nodes.push(entity);
                }
            }
            continue;
        }
        if let Some(relation) = handle_single_relationship_extraction(&attrs, chunk_key) {
            edges.push(relation);
        }
    }
    (nodes, edges)
}

/// Full mirror of the record-parsing half of graph_extractor.py
/// `_process_single_content` + `_entities_and_relations`: raw LLM output →
/// `(nodes, edges)`.  Uses `DEFAULT_ENTITY_TYPES` when `entity_types` is
/// empty (`entity_types or DEFAULT_ENTITY_TYPES`, extractor.py:62).
pub fn extract_entity_relations_from_output(
    raw_output: &str,
    entity_types: &[&str],
    chunk_key: &str,
) -> (Vec<Value>, Vec<Value>) {
    let types: Vec<&str> = if entity_types.is_empty() {
        DEFAULT_ENTITY_TYPES.to_vec()
    } else {
        entity_types.to_vec()
    };
    let records = parse_tuple_records(
        raw_output,
        DEFAULT_RECORD_DELIMITER,
        DEFAULT_COMPLETION_DELIMITER,
    );
    entities_and_relations(&records, DEFAULT_TUPLE_DELIMITER, &types, chunk_key)
}

/// `_merge_nodes` majority type: most frequent value; ties resolved to the
/// first-seen value (Python `sorted(Counter(...), key=count, reverse=True)`
/// is stable).
pub fn most_common_value(values: &[String]) -> Option<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for v in values {
        if !counts.contains_key(v) {
            order.push(v.clone());
        }
        *counts.entry(v.clone()).or_insert(0) += 1;
    }
    let mut best: Option<(&String, usize)> = None;
    for name in &order {
        let c = counts[name];
        if best.map(|(_, bc)| c > bc).unwrap_or(true) {
            best = Some((name, c));
        }
    }
    best.map(|(n, _)| n.clone())
}

/// `flat_uniq_list` — utils.py:717: collect `item[key]` (a string or a
/// list of strings) across items, deduplicated and sorted for determinism
/// (RAGFlow returns an unordered set).
pub fn flat_uniq_list(items: &[Value], key: &str) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for item in items {
        match item.get(key) {
            Some(Value::String(s)) => {
                if !s.is_empty() && seen.insert(s.clone()) {
                    out.push(s.clone());
                }
            }
            Some(Value::Array(arr)) => {
                for v in arr {
                    if let Some(s) = v.as_str()
                        && !s.is_empty()
                        && seen.insert(s.to_string())
                    {
                        out.push(s.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    out.sort();
    out
}

/// `_merge_nodes` — extractor.py:270: majority entity type, `<SEP>`-joined
/// sorted-unique descriptions, flat-unique source ids, name kept from the
/// group key.
pub fn merge_entity_records(records: &[Value]) -> Option<Value> {
    if records.is_empty() {
        return None;
    }
    let types: Vec<String> = records
        .iter()
        .filter_map(|r| {
            r.get("entity_type")
                .and_then(Value::as_str)
                .map(String::from)
        })
        .collect();
    let entity_type = most_common_value(&types).unwrap_or_else(|| "UNKNOWN".to_string());
    let mut descs: Vec<String> = records
        .iter()
        .filter_map(|r| {
            r.get("description")
                .and_then(Value::as_str)
                .map(String::from)
        })
        .collect();
    descs.sort();
    descs.dedup();
    let description = descs.join(GRAPH_FIELD_SEP);
    let source_id = flat_uniq_list(records, "source_id");
    let entity_name = records[0]
        .get("entity_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Some(json!({
        "entity_type": entity_type,
        "description": description,
        "source_id": source_id,
        "entity_name": entity_name,
    }))
}

/// `_merge_edges` — extractor.py:292: summed weight, `<SEP>`-joined
/// sorted-unique descriptions, flat-unique keywords and source ids.
pub fn merge_relation_records(records: &[Value]) -> Option<Value> {
    if records.is_empty() {
        return None;
    }
    let weight: f64 = records
        .iter()
        .filter_map(|r| r.get("weight").and_then(Value::as_f64))
        .sum();
    let mut descs: Vec<String> = records
        .iter()
        .filter_map(|r| {
            r.get("description")
                .and_then(Value::as_str)
                .map(String::from)
        })
        .collect();
    descs.sort();
    descs.dedup();
    let description = descs.join(GRAPH_FIELD_SEP);
    let keywords = flat_uniq_list(records, "keywords");
    let source_id = flat_uniq_list(records, "source_id");
    let src_id = records[0]
        .get("src_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let tgt_id = records[0]
        .get("tgt_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Some(json!({
        "src_id": src_id,
        "tgt_id": tgt_id,
        "description": description,
        "keywords": keywords,
        "weight": weight,
        "source_id": source_id,
    }))
}

// ---------------------------------------------------------------------------
// Non-strict JSON repair (the `json_repair` behaviour RAGFlow applies to
// LLM JSON output: chat_model.py / prompts.py / graphrag/search.py all
// parse LLM replies with `json_repair.loads`).
// ---------------------------------------------------------------------------

/// Find the first balanced `{...}` / `[...]` region of `input`.  Returns
/// `(region, unbalanced)` where `unbalanced` is true when the region runs
/// to the end of the input without closing (a truncation that
/// `balance_closers` can repair).
fn first_balanced_region(input: &str) -> Option<(String, bool)> {
    let start = input.find(['{', '['])?;
    let mut depth: i64 = 0;
    let mut in_string = false;
    let mut quote: char = '"';
    let mut escaped = false;
    for (i, c) in input[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == quote {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                in_string = true;
                quote = c;
            }
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    let end = start + i + c.len_utf8();
                    return Some((input[start..end].to_string(), false));
                }
            }
            _ => {}
        }
    }
    Some((input[start..].to_string(), depth > 0))
}

/// Convert single-quoted strings to double-quoted ones (outside existing
/// double-quoted strings); `\'` (invalid in JSON) is unescaped.
fn single_quotes_to_double(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut in_double = false;
    let mut in_single = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_double {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_double = false;
            }
            i += 1;
            continue;
        }
        if in_single {
            if c == '\\' && i + 1 < chars.len() {
                let next = chars[i + 1];
                if next == '\'' {
                    out.push('\'');
                } else {
                    out.push('\\');
                    out.push(next);
                }
                i += 2;
                continue;
            }
            if c == '\'' {
                out.push('"');
                in_single = false;
            } else if c == '"' {
                out.push('\\');
                out.push('"');
            } else {
                out.push(c);
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_double = true;
                out.push(c);
            }
            '\'' => {
                in_single = true;
                out.push('"');
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

/// Drop commas that trail a `}` / `]` (outside strings).
fn remove_trailing_commas(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                i = j;
                continue;
            }
            out.push(c);
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Quote unquoted JSON keys (`key:` → `"key":`, outside strings).
fn quote_unquoted_keys(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '{' || c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            let key_start = j;
            if j < chars.len()
                && (chars[j].is_ascii_alphabetic() || chars[j] == '_' || chars[j] == '$')
            {
                j += 1;
                while j < chars.len()
                    && (chars[j].is_ascii_alphanumeric() || chars[j] == '_' || chars[j] == '$')
                {
                    j += 1;
                }
                let key_end = j;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if j < chars.len() && chars[j] == ':' {
                    out.push(c);
                    for k in (i + 1)..key_start {
                        out.push(chars[k]);
                    }
                    out.push('"');
                    for k in key_start..key_end {
                        out.push(chars[k]);
                    }
                    out.push('"');
                    for k in key_end..=j {
                        out.push(chars[k]);
                    }
                    i = j + 1;
                    continue;
                }
            }
            out.push(c);
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Quote bare word values (`name: John Smith` → `"John Smith"`), leaving
/// `true` / `false` / `null` and numeric literals untouched.
fn quote_bare_values(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == ':' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            let val_start = j;
            if j < chars.len() && (chars[j].is_ascii_alphabetic() || chars[j] == '_') {
                let mut k = j;
                while k < chars.len() && !matches!(chars[k], ',' | '}' | ']') {
                    if matches!(chars[k], '{' | '[' | '"') {
                        break;
                    }
                    k += 1;
                }
                let word: String = chars[val_start..k].iter().collect();
                let word = word.trim();
                if !word.is_empty()
                    && word != "true"
                    && word != "false"
                    && word != "null"
                    && !is_float_regex(word)
                {
                    out.push(c);
                    for x in (i + 1)..val_start {
                        out.push(chars[x]);
                    }
                    out.push('"');
                    out.push_str(word);
                    out.push('"');
                    i = k;
                    continue;
                }
            }
            out.push(c);
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Append the missing closing brackets so the region balances (truncation
/// recovery: `{"items": [{"name": "A"` → `{"items": [{"name": "A"}]}`).
fn balance_closers(input: &str) -> String {
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for c in input.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' if stack.last() == Some(&c) => {
                stack.pop();
            }
            _ => {}
        }
    }
    let mut out = input.to_string();
    while let Some(closer) = stack.pop() {
        out.push(closer);
    }
    out
}

/// Apply the syntactic repair passes to a JSON region.
fn repair_json_syntax(region: &str, unbalanced: bool) -> String {
    let step1 = single_quotes_to_double(region);
    let step2 = remove_trailing_commas(&step1);
    let step3 = quote_unquoted_keys(&step2);
    let step4 = quote_bare_values(&step3);
    if unbalanced {
        balance_closers(&step4)
    } else {
        step4
    }
}

/// Best-effort repair + parse of a non-strict JSON reply, mirroring the
/// `json_repair` behaviour RAGFlow applies to LLM JSON output.  Tolerates
/// markdown fences, surrounding prose, single-quoted strings, unquoted
/// keys, bare word values, trailing commas, control characters, and
/// truncated objects.  Returns `None` when no JSON value can be recovered.
pub fn repair_json_text(reply: &str) -> Option<Value> {
    // Strip fences, then drop control characters (0x00-0x1f, 0x7f-0x9f).
    let trimmed = reply.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    let body = body.strip_suffix("```").unwrap_or(body);
    let cleaned: String = body
        .chars()
        .filter(|c| {
            let cp = *c as u32;
            !(cp <= 0x1f || (0x7f..=0x9f).contains(&cp))
        })
        .collect();
    let cleaned = cleaned.trim();

    let (region, unbalanced) = first_balanced_region(cleaned)?;
    let repaired = repair_json_syntax(&region, unbalanced);

    if let Ok(v) = serde_json::from_str::<Value>(&repaired) {
        return Some(v);
    }
    // Trailing-prose / truncation fallback: walk closing brackets from the
    // end and try every prefix ending at one (cheap for typical replies).
    let bytes = repaired.as_bytes();
    for (idx, _) in repaired.char_indices().rev() {
        let c = bytes[idx];
        if (c == b'}' || c == b']')
            && let Ok(v) = serde_json::from_str::<Value>(&repaired[..=idx])
        {
            return Some(v);
        }
    }
    None
}

/// Strict parse first, then the tolerant repair path.
pub fn parse_json_lenient(reply: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(reply.trim()) {
        return Some(v);
    }
    repair_json_text(reply)
}

/// RAGFlow `es_upsert_one` exact-id semantics: when a row with the same
/// stable id already exists it is replaced (update), otherwise it is
/// inserted.  This is the explicit "if not exists" check that keeps
/// idempotent re-runs (same chunk → same stable row id) from duplicating
/// rows, independent of the KNN-similarity merge path in `es_dedup_one`.
pub async fn upsert_row_by_id(
    store: &dyn CompileStore,
    kb_id: &str,
    row: &CompiledRow,
) -> Result<PersistOutcome> {
    if store.get(kb_id, &row.id).await?.is_some() {
        let fields = serde_json::to_value(row)?;
        store.update(kb_id, &row.id, &fields).await?;
        Ok(PersistOutcome::Updated)
    } else {
        store.insert(kb_id, std::slice::from_ref(row)).await?;
        Ok(PersistOutcome::Inserted)
    }
}

// ── `_common.py` helpers (v0.3.10ah) ────────────────────────────────────────

/// `env_int`: read a non-empty integer environment override, else `default`;
/// values below `minimum` are clamped (logged upstream).
pub fn env_int(name: &str, default: i64, minimum: Option<i64>) -> i64 {
    let value = match std::env::var(name) {
        Ok(raw) if !raw.trim().is_empty() => raw.trim().parse::<i64>().unwrap_or(default),
        _ => default,
    };
    match minimum {
        Some(floor) if value < floor => floor,
        _ => value,
    }
}

/// `env_float`: read a non-empty float override, else `default`; non-finite
/// values fall back to `default`; the result is clamped to `[minimum, maximum]`.
pub fn env_float(name: &str, default: f64, minimum: Option<f64>, maximum: Option<f64>) -> f64 {
    let mut value = match std::env::var(name) {
        Ok(raw) if !raw.trim().is_empty() => raw.trim().parse::<f64>().unwrap_or(default),
        _ => default,
    };
    if !value.is_finite() {
        value = default;
    }
    if let Some(floor) = minimum
        && value < floor
    {
        value = floor;
    }
    if let Some(cap) = maximum
        && value > cap
    {
        value = cap;
    }
    value
}

/// `knowledge_compile_gen_conf`: add model-specific reasoning controls for
/// knowledge compilation. The port takes the resolved model name instead of
/// the live bundle (`model_config.llm_name` / `llm_name`).
pub fn knowledge_compile_gen_conf(
    model_name: &str,
    gen_conf: Option<&serde_json::Map<String, Value>>,
) -> serde_json::Map<String, Value> {
    let mut conf = gen_conf.cloned().unwrap_or_default();
    let model_name = model_name.to_lowercase();
    if model_name.contains("deepseek-v4") {
        conf.insert(
            "max_completion_tokens".to_string(),
            serde_json::json!(32768),
        );
        let mut extra_body = conf
            .get("extra_body")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        extra_body.insert(
            "thinking".to_string(),
            serde_json::json!({"type": "disabled"}),
        );
        conf.insert("extra_body".to_string(), Value::Object(extra_body));
    } else if model_name.contains("qwen3") {
        conf.insert(
            "enable_thinking".to_string(),
            Value::Bool(model_name.contains("-preview")),
        );
    } else {
        conf.insert(
            "reasoning_effort".to_string(),
            Value::String("none".to_string()),
        );
    }
    conf
}

/// `encode`: the `LLMBundle.encode` seam (`thread_pool_exec(embd_mdl.encode,
/// texts)` in Python). Returns the embeddings list; empty input returns `[]`.
#[async_trait::async_trait]
pub trait EmbeddingBackend: Send + Sync {
    async fn encode(&self, texts: &[String]) -> std::result::Result<Vec<Vec<f32>>, String>;
}

/// `encode(embd_mdl, texts)`.
pub async fn encode(
    embed: &dyn EmbeddingBackend,
    texts: &[String],
) -> std::result::Result<Vec<Vec<f32>>, String> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    embed.encode(texts).await
}

/// `make_input_budget`: `max_length * utilization - tokens(prompts)`, floored.
pub fn make_input_budget(
    max_length: usize,
    prompts: &[&str],
    floor: usize,
    utilization: f64,
) -> usize {
    let overhead: usize = prompts
        .iter()
        .map(|prompt| crate::chunk::tokenizer::token_count(prompt))
        .sum();
    let budget = (max_length as f64 * utilization) as i64 - overhead as i64;
    budget.max(floor as i64) as usize
}

// `ensure_llm_bundle` (Python duck-typing: unwrap a tuple result passed in
// place of the bundle) has no Rust counterpart — the type system rejects a
// tuple where a bundle is expected, so the defensive unwrap is intentionally
// not ported (documented in the ledger note).

/// `index_name(uid)`: `f"ragflow_{uid}"`.
pub fn doc_store_index_name(uid: &str) -> String {
    format!("ragflow_{uid}")
}

/// `doc_storage_search`: thin wrapper over `DocStore.search` + `get_fields`.
/// Returns `{row_id: row}`; `{}` on failure.
pub async fn doc_storage_search(
    store: &dyn crate::doc_store::DocStore,
    query: crate::doc_store::SearchQuery,
    select_fields: &[String],
) -> std::collections::HashMap<String, crate::doc_store::DocRow> {
    match store.search(&query) {
        Ok(response) => store.get_fields(&response, select_fields),
        Err(_) => std::collections::HashMap::new(),
    }
}

/// `doc_storage_insert`: bulk insert; best-effort (failures are logged, not
/// propagated, so a compile that already committed does not report a hard error).
pub async fn doc_storage_insert(
    store: &dyn crate::doc_store::DocStore,
    rows: &[crate::doc_store::DocRow],
    index_name: &str,
    dataset_id: &str,
) {
    if rows.is_empty() {
        return;
    }
    if let Err(error) = store.insert(rows, index_name, dataset_id) {
        tracing::error!(
            %error,
            %index_name,
            %dataset_id,
            rows = rows.len(),
            "Failed to insert compiled structure rows; they stay missing from retrieval"
        );
    }
}

/// `doc_storage_delete`: bulk delete; best-effort (logged).
pub async fn doc_storage_delete(
    store: &dyn crate::doc_store::DocStore,
    condition: &crate::doc_store::FilterCondition,
    index_name: &str,
    dataset_id: &str,
) {
    if let Err(error) = store.delete(condition, index_name, dataset_id) {
        tracing::error!(
            %error,
            %index_name,
            %dataset_id,
            "Failed to delete compiled structure rows; stale rows stay searchable"
        );
    }
}

/// `doc_storage_upsert_one`: delete-by-filter then insert (stable row ids make
/// an id-based upsert the race fallback).
pub async fn doc_storage_upsert_one(
    store: &dyn crate::doc_store::DocStore,
    filter_condition: &crate::doc_store::FilterCondition,
    row: &crate::doc_store::DocRow,
    tenant_id: &str,
    kb_id: &str,
) {
    let index_name = doc_store_index_name(tenant_id);
    doc_storage_delete(store, filter_condition, &index_name, kb_id).await;
    doc_storage_insert(store, std::slice::from_ref(row), &index_name, kb_id).await;
}

// ── `runner.py` — document-scoped structure compilation core (v0.3.10ah) ──
//
// The compile core drives every non-`tree` template over chunk batches with
// bounded concurrency and ordered commits, then runs the optional synthesis,
// dataset-graph, page-index navigation and timeline-cleanup phases. The host
// services (extraction entry, merge, graph rebuilds, wiki synthesis, dataset
// navigation, timeline cleanup, template lookups) arrive through
// [`StructureCompileBackend`]. Documented divergences: the async
// doc-storage condition-variable dance collapses to sequential awaits (same
// ordering guarantee); the batch iterator is a batch list; the LLM pool is a
// pass-through wrapper (concurrency is the host's concern).

/// Errors the runner can surface (`TaskCanceledException` + backend failures).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerError {
    Cancelled,
    Backend(String),
}

/// `DOC_STRUCTURE_COMPILE_BATCH_CHUNKS`.
pub static DOC_STRUCTURE_COMPILE_BATCH_CHUNKS: std::sync::LazyLock<usize> =
    std::sync::LazyLock::new(|| {
        env_int("DOC_STRUCTURE_COMPILE_BATCH_CHUNKS", 4, Some(1)).max(1) as usize
    });
pub static STRUCTURE_CONTEXT_FRACTION: std::sync::LazyLock<f64> = std::sync::LazyLock::new(|| {
    env_float("STRUCTURE_CONTEXT_FRACTION", 0.5, Some(0.01), Some(1.0))
});
pub static STRUCTURE_DEFAULT_CONTEXT: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    env_int("STRUCTURE_DEFAULT_CONTEXT", 100_000, Some(1)).max(1) as usize
});
pub static KNOWLEDGE_GRAPH_CONTEXT_FRACTION: std::sync::LazyLock<f64> =
    std::sync::LazyLock::new(|| {
        env_float(
            "KNOWLEDGE_GRAPH_CONTEXT_FRACTION",
            0.1,
            Some(0.01),
            Some(1.0),
        )
    });
pub static KNOWLEDGE_GRAPH_MIN_BATCH_TOKENS: std::sync::LazyLock<usize> =
    std::sync::LazyLock::new(|| {
        env_int("KNOWLEDGE_GRAPH_MIN_BATCH_TOKENS", 2048, Some(1)).max(1) as usize
    });
pub static KNOWLEDGE_GRAPH_MAX_BATCH_TOKENS: std::sync::LazyLock<usize> =
    std::sync::LazyLock::new(|| {
        let minimum = *KNOWLEDGE_GRAPH_MIN_BATCH_TOKENS as i64;
        env_int("KNOWLEDGE_GRAPH_MAX_BATCH_TOKENS", 4096, Some(minimum)).max(minimum) as usize
    });
pub static DOC_STRUCTURE_COMPILE_MAX_IN_FLIGHT: std::sync::LazyLock<usize> =
    std::sync::LazyLock::new(|| {
        env_int("DOC_STRUCTURE_COMPILE_MAX_IN_FLIGHT", 15, Some(1)).max(1) as usize
    });
pub static DOC_STRUCTURE_LLM_POOL_SIZE: std::sync::LazyLock<usize> =
    std::sync::LazyLock::new(|| {
        env_int("DOC_STRUCTURE_LLM_POOL_SIZE", 20, Some(1)).max(1) as usize
    });
/// `DOC_STRUCTURE_MERGE_MAX_DOCS`.
pub const DOC_STRUCTURE_MERGE_MAX_DOCS: usize = 512;
/// `STRUCTURE_CHAIN_CORRECTION_TIMEOUT_S`.
pub static STRUCTURE_CHAIN_CORRECTION_TIMEOUT_S: std::sync::LazyLock<f64> =
    std::sync::LazyLock::new(|| {
        env_float(
            "STRUCTURE_CHAIN_CORRECTION_TIMEOUT_S",
            120.0,
            Some(0.1),
            None,
        )
    });

/// `_compilation_template_kind`: `strip().lower().replace("-", "_")`.
pub fn runner_template_kind(kind: Option<&str>) -> String {
    kind.map(|value| value.trim().to_lowercase().replace('-', "_"))
        .unwrap_or_default()
}

/// `_is_page_index_template`.
pub fn is_page_index_template(parser_cfg: &Value) -> bool {
    matches!(
        runner_template_kind(parser_cfg.get("kind").and_then(Value::as_str)).as_str(),
        "page_index" | "pageindex"
    )
}

/// `_page_index_graph_summary`.
pub fn page_index_graph_summary(graph: &Value, limit: usize) -> String {
    let Some(entities) = graph.get("entities").and_then(Value::as_array) else {
        return String::new();
    };
    let mut lines: Vec<String> = Vec::new();
    for entity in entities {
        if !entity.is_object() {
            continue;
        }
        let name = entity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let description = entity
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let text = format!("{name}: {description}")
            .trim_matches(|c: char| c == ':' || c == ' ')
            .to_string();
        if !text.is_empty() {
            lines.push(text);
        }
        if lines.len() >= limit {
            break;
        }
    }
    lines.join("\n")
}

// ── `structure.py` pool (v0.3.10ah A2) ─────────────────────────────────────

/// `LLM_POOL_RATE_LIMIT_RETRIES`.
pub static LLM_POOL_RATE_LIMIT_RETRIES: std::sync::LazyLock<usize> =
    std::sync::LazyLock::new(|| env_int("LLM_POOL_RATE_LIMIT_RETRIES", 3, Some(0)).max(0) as usize);
/// `LLM_POOL_RATE_LIMIT_RETRY_BASE_DELAY`.
pub static LLM_POOL_RATE_LIMIT_RETRY_BASE_DELAY: std::sync::LazyLock<f64> =
    std::sync::LazyLock::new(|| {
        env_float("LLM_POOL_RATE_LIMIT_RETRY_BASE_DELAY", 1.0, Some(0.0), None)
    });
/// `LLM_POOL_RATE_LIMIT_RETRY_MAX_DELAY`.
pub static LLM_POOL_RATE_LIMIT_RETRY_MAX_DELAY: std::sync::LazyLock<f64> =
    std::sync::LazyLock::new(|| {
        env_float("LLM_POOL_RATE_LIMIT_RETRY_MAX_DELAY", 30.0, Some(0.0), None)
    });

/// `_LLMModelPoolState`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPoolState {
    pub concurrency: usize,
    pub active: usize,
    pub successes: usize,
    pub last_decrease_at: f64,
    pub last_increase_at: f64,
}

impl ModelPoolState {
    fn new(concurrency: usize) -> Self {
        Self {
            concurrency,
            active: 0,
            successes: 0,
            last_decrease_at: f64::NEG_INFINITY,
            last_increase_at: f64::NEG_INFINITY,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PoolTicket {
    priority: i64,
    seq: u64,
    model_key: String,
}

#[derive(Default)]
struct PoolInner {
    active: usize,
    ticket: u64,
    waiting: Vec<PoolTicket>,
    model_states: std::collections::HashMap<String, ModelPoolState>,
}

/// `LLMCallPool`: task-scoped adaptive priority scheduler for chat-model calls.
/// `max_concurrency` is the task-wide hard ceiling; each model starts at that
/// ceiling, halves its own admission limit after an explicit rate-limit
/// response, retries through the reduced limit, and recovers one slot at a
/// time after sustained success. The clock is injectable for tests.
pub struct LlmCallPool {
    pub max_concurrency: usize,
    pub max_pending: usize,
    pub min_concurrency: usize,
    pub decrease_factor: f64,
    pub decrease_cooldown: f64,
    pub recovery_successes: usize,
    pub recovery_cooldown: f64,
    pub rate_limit_retries: usize,
    pub rate_limit_retry_base_delay: f64,
    pub rate_limit_retry_max_delay: f64,
    clock: std::sync::Arc<dyn Fn() -> f64 + Send + Sync>,
    inner: tokio::sync::Mutex<PoolInner>,
    notify: tokio::sync::Notify,
}

const RATE_LIMIT_MARKERS: [&str; 8] = [
    "429",
    "rate limit",
    "rate_limit",
    "too many requests",
    "requests per minute",
    "concurrency limit",
    "concurrent request",
    "maximum number of concurrent",
];

impl LlmCallPool {
    /// `LLMCallPool(max_concurrency=10, ...)` with the upstream defaults.
    pub fn new(max_concurrency: usize) -> Self {
        let max = max_concurrency.max(1);
        Self {
            max_concurrency: max,
            max_pending: max,
            min_concurrency: 1,
            decrease_factor: 0.5,
            decrease_cooldown: 5.0,
            recovery_successes: 20,
            recovery_cooldown: 30.0,
            rate_limit_retries: *LLM_POOL_RATE_LIMIT_RETRIES,
            rate_limit_retry_base_delay: *LLM_POOL_RATE_LIMIT_RETRY_BASE_DELAY,
            rate_limit_retry_max_delay: *LLM_POOL_RATE_LIMIT_RETRY_MAX_DELAY,
            clock: std::sync::Arc::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0)
            }),
            inner: tokio::sync::Mutex::new(PoolInner::default()),
            notify: tokio::sync::Notify::new(),
        }
    }

    /// Inject a clock (tests use a manual counter).
    pub fn with_clock(mut self, clock: std::sync::Arc<dyn Fn() -> f64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    /// `_model_key`: join the identifying config parts; fall back to the name.
    pub fn model_key_for(model_id: &str, factory: &str, name: &str, endpoint: &str) -> String {
        if !model_id.is_empty() || !factory.is_empty() || !name.is_empty() || !endpoint.is_empty() {
            return format!("{model_id}:{factory}:{name}:{endpoint}");
        }
        name.to_string()
    }

    /// `active_count`.
    pub async fn active_count(&self) -> usize {
        self.inner.lock().await.active
    }

    /// `pending_count`.
    pub async fn pending_count(&self) -> usize {
        let inner = self.inner.lock().await;
        inner.active + inner.waiting.len()
    }

    /// `concurrency_for`.
    pub async fn concurrency_for(&self, model_key: &str) -> usize {
        let mut inner = self.inner.lock().await;
        let max = self.max_concurrency;
        Self::state_mut(&mut inner, model_key, max).concurrency
    }

    fn state_mut<'g>(
        inner: &'g mut PoolInner,
        model_key: &str,
        max: usize,
    ) -> &'g mut ModelPoolState {
        inner
            .model_states
            .entry(model_key.to_string())
            .or_insert_with(|| ModelPoolState::new(max))
    }

    fn next_admissible(inner: &PoolInner, max_concurrency: usize) -> Option<PoolTicket> {
        if inner.active >= max_concurrency {
            return None;
        }
        let mut best: Option<PoolTicket> = None;
        for ticket in &inner.waiting {
            let state = inner.model_states.get(&ticket.model_key);
            let active = state.map(|s| s.active).unwrap_or(0);
            let limit = state.map(|s| s.concurrency).unwrap_or(max_concurrency);
            if active >= limit {
                continue;
            }
            match &best {
                None => best = Some(ticket.clone()),
                Some(current) => {
                    if (ticket.priority, ticket.seq) < (current.priority, current.seq) {
                        best = Some(ticket.clone());
                    }
                }
            }
        }
        best
    }

    /// `_is_rate_limited`.
    pub fn is_rate_limited(value: &str) -> bool {
        let text = value.to_lowercase();
        RATE_LIMIT_MARKERS
            .iter()
            .any(|marker| text.contains(marker))
    }

    /// `_is_error_result`.
    pub fn is_error_result(result: &str) -> bool {
        result.trim_start().to_lowercase().starts_with("**error**")
    }

    /// `_acquire`.
    pub async fn acquire(&self, model_key: &str, priority: i64) {
        let mut my_ticket: Option<PoolTicket> = None;
        loop {
            {
                let mut inner = self.inner.lock().await;
                if my_ticket.is_none() {
                    let pending = inner.active + inner.waiting.len();
                    if pending < self.max_pending {
                        let seq = inner.ticket;
                        inner.ticket += 1;
                        let ticket = PoolTicket {
                            priority,
                            seq,
                            model_key: model_key.to_string(),
                        };
                        inner.waiting.push(ticket.clone());
                        my_ticket = Some(ticket);
                    }
                }
                if let Some(ticket) = &my_ticket {
                    let admissible = Self::next_admissible(&inner, self.max_concurrency)
                        .map(|next| &next == ticket)
                        .unwrap_or(false);
                    if admissible {
                        inner.waiting.retain(|entry| entry != ticket);
                        inner.active += 1;
                        let max = self.max_concurrency;
                        Self::state_mut(&mut inner, model_key, max).active += 1;
                        return;
                    }
                }
            }
            self.notify.notified().await;
        }
    }

    /// `_record_feedback`.
    fn record_feedback(
        &self,
        inner: &mut PoolInner,
        model_key: &str,
        outcome: &str,
    ) -> Option<(usize, usize, &'static str)> {
        let now = (self.clock)();
        let max = self.max_concurrency;
        let state = Self::state_mut(inner, model_key, max);
        if outcome == "rate_limited" {
            state.successes = 0;
            if now - state.last_decrease_at < self.decrease_cooldown {
                return None;
            }
            let old = state.concurrency;
            state.concurrency = self
                .min_concurrency
                .max((state.concurrency as f64 * self.decrease_factor) as usize);
            state.last_decrease_at = now;
            if state.concurrency != old {
                return Some((old, state.concurrency, "rate limited"));
            }
            return None;
        }
        if outcome != "success" {
            state.successes = 0;
            return None;
        }
        if state.concurrency >= self.max_concurrency {
            state.successes = 0;
            return None;
        }
        state.successes += 1;
        if state.successes < self.recovery_successes {
            return None;
        }
        if now - state.last_decrease_at < self.recovery_cooldown
            || now - state.last_increase_at < self.recovery_cooldown
        {
            return None;
        }
        let old = state.concurrency;
        state.concurrency = self.max_concurrency.min(state.concurrency + 1);
        state.successes = 0;
        state.last_increase_at = now;
        Some((old, state.concurrency, "recovered"))
    }

    /// `_release` (the concurrency-change callback is a host concern).
    pub async fn release(&self, model_key: &str, outcome: &str) {
        let mut inner = self.inner.lock().await;
        inner.active = inner.active.saturating_sub(1);
        let max = self.max_concurrency;
        Self::state_mut(&mut inner, model_key, max).active =
            Self::state_mut(&mut inner, model_key, max)
                .active
                .saturating_sub(1);
        let _change = self.record_feedback(&mut inner, model_key, outcome);
        drop(inner);
        self.notify.notify_waiters();
    }

    /// `_rate_limit_retry_delay`.
    pub fn rate_limit_retry_delay(&self, retry: usize) -> f64 {
        let exponent = retry.saturating_sub(1).min(30) as i32;
        self.rate_limit_retry_max_delay
            .min(self.rate_limit_retry_base_delay * 2f64.powi(exponent))
    }

    /// `call(fn, ...)`: run `f` under the pool with rate-limit retries. A
    /// non-rate-limited `**ERROR**` result is returned as a value (upstream
    /// semantics); only transport failures surface as `Err`.
    pub async fn call<F, Fut>(
        &self,
        f: F,
        model_key: &str,
        priority: i64,
        _label: &str,
        _context: Option<&str>,
    ) -> std::result::Result<String, String>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = std::result::Result<String, String>>,
    {
        let mut retry = 0usize;
        loop {
            self.acquire(model_key, priority).await;
            match f().await {
                Ok(result) => {
                    let error_result = Self::is_error_result(&result);
                    let rate_limited = error_result && Self::is_rate_limited(&result);
                    self.release(
                        model_key,
                        if rate_limited {
                            "rate_limited"
                        } else if error_result {
                            "failed"
                        } else {
                            "success"
                        },
                    )
                    .await;
                    if rate_limited && retry < self.rate_limit_retries {
                        // fall through to the retry delay
                    } else {
                        return Ok(result);
                    }
                }
                Err(exc) => {
                    let rate_limited = Self::is_rate_limited(&exc);
                    self.release(
                        model_key,
                        if rate_limited {
                            "rate_limited"
                        } else {
                            "failed"
                        },
                    )
                    .await;
                    if !rate_limited || retry >= self.rate_limit_retries {
                        return Err(exc);
                    }
                }
            }
            let delay = self.rate_limit_retry_delay(retry + 1);
            if delay > 0.0 {
                tokio::time::sleep(std::time::Duration::from_secs_f64(delay)).await;
            }
            retry += 1;
        }
    }

    /// `wrap(chat_mdl, *, priority, label, context)`: a pooled chat surface.
    pub fn wrap<'a>(
        &'a self,
        chat: &'a dyn crate::harness::HarnessChat,
        model_key: &str,
        model_name: &str,
        priority: i64,
        label: &str,
        context: Option<&str>,
    ) -> PooledChatModel<'a> {
        PooledChatModel {
            pool: self,
            inner: chat,
            model_key: model_key.to_string(),
            model_name: model_name.to_string(),
            priority,
            label: label.to_string(),
            context: context.map(str::to_string),
        }
    }

    /// Test support: push a ticket without waiting (ordering assertions).
    #[doc(hidden)]
    pub async fn debug_enqueue(&self, model_key: &str, priority: i64) {
        let mut inner = self.inner.lock().await;
        let seq = inner.ticket;
        inner.ticket += 1;
        inner.waiting.push(PoolTicket {
            priority,
            seq,
            model_key: model_key.to_string(),
        });
    }

    /// Test support: admit the next admissible ticket (returns its model key).
    #[doc(hidden)]
    pub async fn debug_admit_next(&self) -> Option<String> {
        let mut inner = self.inner.lock().await;
        let ticket = Self::next_admissible(&inner, self.max_concurrency)?;
        inner.waiting.retain(|entry| entry != &ticket);
        inner.active += 1;
        let max = self.max_concurrency;
        Self::state_mut(&mut inner, &ticket.model_key, max).active += 1;
        Some(ticket.model_key)
    }
}

/// `PooledChatModel`: wraps a chat surface behind the pool and applies the
/// knowledge-compile generation controls per call.
pub struct PooledChatModel<'a> {
    pool: &'a LlmCallPool,
    inner: &'a dyn crate::harness::HarnessChat,
    model_key: String,
    model_name: String,
    priority: i64,
    label: String,
    context: Option<String>,
}

#[async_trait::async_trait]
impl crate::harness::HarnessChat for PooledChatModel<'_> {
    async fn chat(
        &self,
        system: &str,
        history: &[Value],
        gen_conf: &Value,
    ) -> std::result::Result<String, String> {
        let conf = Value::Object(knowledge_compile_gen_conf(
            &self.model_name,
            gen_conf.as_object(),
        ));
        self.pool
            .call(
                || async { self.inner.chat(system, history, &conf).await },
                &self.model_key,
                self.priority,
                &self.label,
                self.context.as_deref(),
            )
            .await
    }

    fn max_length(&self) -> usize {
        self.inner.max_length()
    }
}

/// The host services the compile core drives.
#[async_trait::async_trait]
pub trait StructureCompileBackend: Send + Sync {
    /// `CompilationTemplateGroupService.resolve_template_ids(group_id, tenant_id)`.
    fn resolve_template_ids_from_group(&self, group_id: &str, tenant_id: &str) -> Vec<String>;
    /// `CompilationTemplateService.get_saved(template_id, tenant_id)`.
    fn saved_template(&self, template_id: &str, tenant_id: &str) -> Option<Value>;
    /// `compile_structure_from_text(batch, parser_cfg, chat_mdl, embd_mdl, doc_id,
    /// doc_name, language, callback, max_workers=3, compilation_template_id)`.
    async fn compile_structure_from_text(
        &self,
        batch: &[Value],
        parser_cfg: &Value,
        doc_id: &str,
        doc_name: &str,
        language: &str,
        compilation_template_id: &str,
    ) -> std::result::Result<Vec<Value>, String>;
    /// `merge_compiled_structures(...)` — returns `{inserted, updated,
    /// duplicates_dropped, compile_kwds}`.
    async fn merge_compiled_structures(
        &self,
        request: MergeFlushRequest<'_>,
    ) -> std::result::Result<Value, String>;
    /// `rebuild_structure_graph_json(tenant_id, kb_id, doc_id, doc_name, compile_kwd, compilation_template_id=…)`.
    async fn rebuild_structure_graph_json(
        &self,
        tenant_id: &str,
        kb_id: &str,
        doc_id: &str,
        doc_name: &str,
        compile_kwd: &str,
        compilation_template_id: &str,
    ) -> std::result::Result<Value, String>;
    /// `rebuild_dataset_structure_graph_json(tenant_id, kb_id, compile_kwd, compilation_template_id, structure_kind)`.
    async fn rebuild_dataset_structure_graph_json(
        &self,
        tenant_id: &str,
        kb_id: &str,
        compile_kwd: &str,
        compilation_template_id: &str,
        structure_kind: Option<&str>,
    ) -> std::result::Result<(), String>;
    /// `upsert_dataset_nav_doc(tenant_id, kb_id, doc_id, summary, embd_mdl=…, chat_mdl=…)`.
    async fn upsert_dataset_nav_doc(
        &self,
        tenant_id: &str,
        kb_id: &str,
        doc_id: &str,
        summary: &str,
    ) -> std::result::Result<(), String>;
    /// `wiki_plan_from_reduction(chat_mdl, embd_mdl, tenant_id, kb_id, callback)`.
    async fn wiki_plan_from_reduction(
        &self,
        tenant_id: &str,
        kb_id: &str,
    ) -> std::result::Result<Value, String>;
    /// `wiki_refine_from_plan(chat_mdl, embd_mdl, tenant_id, kb_id, callback, example)`.
    async fn wiki_refine_from_plan(
        &self,
        tenant_id: &str,
        kb_id: &str,
        example: &str,
    ) -> std::result::Result<Vec<Value>, String>;
    /// `cleanup_timeline_isolated_entities(tenant_id, kb_id, doc_id, doc_name, compilation_template_id)`.
    async fn cleanup_timeline_isolated_entities(
        &self,
        tenant_id: &str,
        kb_id: &str,
        doc_id: &str,
        doc_name: &str,
        compilation_template_id: &str,
    ) -> std::result::Result<(), String>;
}

/// `merge_compiled_structures` call surface for one flush.
pub struct MergeFlushRequest<'a> {
    pub docs: Vec<Value>,
    pub tenant_id: &'a str,
    pub kb_id: &'a str,
    pub doc_id: &'a str,
    pub doc_name: &'a str,
    pub compilation_template_id: &'a str,
    pub chain_kind: &'a str,
    pub timing_context: &'a str,
    /// `MERGE_SCOPE_DATASET` when the template sets `dataset_merge`.
    pub merge_scope_dataset: bool,
    pub chain_timeout_seconds: f64,
    pub chunks_by_id: &'a std::collections::HashMap<String, String>,
    pub progress: &'a (dyn Fn(&str) + Send + Sync),
}

/// `resolve_template_ids_from_groups`.
pub fn resolve_template_ids_from_groups(
    backend: &dyn StructureCompileBackend,
    group_ids: &[String],
    tenant_id: &str,
) -> Vec<String> {
    let mut template_ids: Vec<String> = Vec::new();
    for group_id in group_ids {
        let trimmed = group_id.trim();
        if trimmed.is_empty() {
            continue;
        }
        for template_id in backend.resolve_template_ids_from_group(trimmed, tenant_id) {
            if !template_ids.iter().any(|existing| existing == &template_id) {
                template_ids.push(template_id);
            }
        }
    }
    template_ids
}

/// `load_active_templates`: keep templates whose kind is real and not `wiki`
/// (missing/invalid configs are dropped).
pub fn load_active_templates(
    backend: &dyn StructureCompileBackend,
    template_ids: &[String],
    tenant_id: &str,
) -> Vec<(String, Value)> {
    let mut active: Vec<(String, Value)> = Vec::new();
    for template_id in template_ids {
        let Some(template) = backend.saved_template(template_id, tenant_id) else {
            continue;
        };
        let parser_cfg = template.get("config").cloned().unwrap_or_else(|| json!({}));
        if !parser_cfg.is_object() {
            continue;
        }
        let kind = runner_template_kind(parser_cfg.get("kind").and_then(Value::as_str));
        if kind.is_empty() || kind == "wiki" {
            continue;
        }
        active.push((template_id.clone(), parser_cfg));
    }
    active
}

/// `split_tree_templates`.
pub fn split_tree_templates(
    active_templates: Vec<(String, Value)>,
) -> (Vec<(String, Value)>, Vec<(String, Value)>) {
    let mut tree: Vec<(String, Value)> = Vec::new();
    let mut non_tree: Vec<(String, Value)> = Vec::new();
    for (template_id, cfg) in active_templates {
        if runner_template_kind(cfg.get("kind").and_then(Value::as_str)) == "tree" {
            tree.push((template_id, cfg));
        } else {
            non_tree.push((template_id, cfg));
        }
    }
    (tree, non_tree)
}

/// `_dynamic_batch_budget`.
pub fn dynamic_batch_budget(max_length: Option<usize>, template_kind: &str) -> usize {
    let max_length = max_length
        .filter(|value| *value > 0)
        .unwrap_or(*STRUCTURE_DEFAULT_CONTEXT);
    if template_kind == "knowledge_graph" {
        let lower = ((max_length as f64 * *KNOWLEDGE_GRAPH_CONTEXT_FRACTION) as i64)
            .max(*KNOWLEDGE_GRAPH_MIN_BATCH_TOKENS as i64);
        lower.min(*KNOWLEDGE_GRAPH_MAX_BATCH_TOKENS as i64) as usize
    } else {
        ((max_length as f64 * *STRUCTURE_CONTEXT_FRACTION) as i64).max(1024) as usize
    }
}

/// `_upsert_dataset_nav_from_page_index`.
pub async fn upsert_dataset_nav_from_page_index(
    backend: &dyn StructureCompileBackend,
    active_templates: &[(String, Value)],
    tenant_id: &str,
    kb_id: &str,
    doc_id: &str,
    doc_name: &str,
    progress: &(dyn Fn(&str) + Send + Sync),
    cancel_check: &(dyn Fn() -> bool + Send + Sync),
) -> std::result::Result<(), RunnerError> {
    let page_index_templates: Vec<&(String, Value)> = active_templates
        .iter()
        .filter(|(_, cfg)| is_page_index_template(cfg))
        .collect();
    if page_index_templates.is_empty() {
        return Ok(());
    }
    let mut summaries: Vec<String> = Vec::new();
    for (template_id, _) in &page_index_templates {
        if cancel_check() {
            return Err(RunnerError::Cancelled);
        }
        let mut graph = backend
            .rebuild_structure_graph_json(
                tenant_id,
                kb_id,
                doc_id,
                doc_name,
                "page_index",
                template_id,
            )
            .await
            .unwrap_or_else(|_| json!({}));
        let mut summary = page_index_graph_summary(&graph, 80);
        if summary.is_empty() {
            graph = backend
                .rebuild_structure_graph_json(
                    tenant_id,
                    kb_id,
                    doc_id,
                    doc_name,
                    "timeline",
                    template_id,
                )
                .await
                .unwrap_or_else(|_| json!({}));
            summary = page_index_graph_summary(&graph, 80);
        }
        if !summary.is_empty() {
            summaries.push(summary);
        }
    }
    if summaries.is_empty() {
        return Ok(());
    }
    if cancel_check() {
        return Err(RunnerError::Cancelled);
    }
    progress(&format!(
        "page_index: updating dataset navigation for doc {doc_id} ..."
    ));
    let joined = summaries.join("\n\n");
    backend
        .upsert_dataset_nav_doc(tenant_id, kb_id, doc_id, &joined)
        .await
        .map_err(RunnerError::Backend)
}

/// Batch stream + callbacks for [`run_structure_compile_over_batches`].
pub struct CompileOverBatchesRequest<'a> {
    pub active_templates: Vec<(String, Value)>,
    pub tenant_id: &'a str,
    pub kb_id: &'a str,
    pub doc_id: &'a str,
    pub doc_name: &'a str,
    pub language: &'a str,
    /// Chunks arrive as batches (upstream: an async iterator of batches).
    pub chunk_batches: Vec<Vec<Value>>,
    pub progress: &'a (dyn Fn(&str) + Send + Sync),
    pub cancel_check: &'a (dyn Fn() -> bool + Send + Sync),
    pub record: Option<&'a (dyn Fn(&str, Value) + Send + Sync)>,
}

/// `run_structure_compile_over_batches`: extract + merge structures for every
/// non-`tree` template over chunk batches, then the synthesis / dataset-graph /
/// dataset-nav / timeline phases. Returns `{template_id: {inserted, updated,
/// duplicates_dropped, rechunked_chunk_count}}`.
pub async fn run_structure_compile_over_batches(
    backend: &dyn StructureCompileBackend,
    request: CompileOverBatchesRequest<'_>,
) -> std::result::Result<Value, RunnerError> {
    use futures_util::StreamExt;
    if request.active_templates.is_empty() {
        return Ok(json!({}));
    }
    let total = request.active_templates.len();
    let progress = request.progress;
    let cancel_check = request.cancel_check;
    let mut accumulators: std::collections::HashMap<String, Vec<Value>> = request
        .active_templates
        .iter()
        .map(|(template_id, _)| (template_id.clone(), Vec::new()))
        .collect();
    let template_kinds: std::collections::HashMap<String, String> = request
        .active_templates
        .iter()
        .map(|(template_id, cfg)| {
            (
                template_id.clone(),
                runner_template_kind(cfg.get("kind").and_then(Value::as_str)),
            )
        })
        .collect();
    let merge_scope_dataset: std::collections::HashMap<String, bool> = request
        .active_templates
        .iter()
        .map(|(template_id, cfg)| {
            (
                template_id.clone(),
                cfg.get("dataset_merge")
                    .map(crate::structure_compile::value_is_truthy)
                    .unwrap_or(false),
            )
        })
        .collect();
    let mut compile_kwds_by_tid: std::collections::HashMap<String, Vec<String>> = request
        .active_templates
        .iter()
        .map(|(template_id, _)| (template_id.clone(), Vec::new()))
        .collect();
    let mut agg_infos: std::collections::HashMap<String, serde_json::Map<String, Value>> = request
        .active_templates
        .iter()
        .map(|(template_id, _)| {
            (
                template_id.clone(),
                serde_json::Map::from_iter([
                    ("inserted".to_string(), json!(0)),
                    ("updated".to_string(), json!(0)),
                    ("duplicates_dropped".to_string(), json!(0)),
                ]),
            )
        })
        .collect();
    let mut chunks_by_id: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut flush_sequence = 0usize;
    progress(&format!(
        "Start document knowledge compilation ({total} template(s)) ..."
    ));

    // Flushes run sequentially in submission order (see the module note).
    async fn flush(
        backend: &dyn StructureCompileBackend,
        template_id: &str,
        accumulator: &mut Vec<Value>,
        request: &CompileOverBatchesRequest<'_>,
        template_kinds: &std::collections::HashMap<String, String>,
        merge_scope_dataset: &std::collections::HashMap<String, bool>,
        compile_kwds_by_tid: &mut std::collections::HashMap<String, Vec<String>>,
        agg_infos: &mut std::collections::HashMap<String, serde_json::Map<String, Value>>,
        chunks_by_id: &std::collections::HashMap<String, String>,
        flush_sequence: &mut usize,
    ) -> std::result::Result<(), RunnerError> {
        if accumulator.is_empty() {
            return Ok(());
        }
        let docs = std::mem::take(accumulator);
        *flush_sequence += 1;
        let timing_context = format!(
            "{}:{}:flush-{}",
            request.doc_id, template_id, flush_sequence
        );
        let kind = template_kinds.get(template_id).cloned().unwrap_or_default();
        let flush_request = MergeFlushRequest {
            docs,
            tenant_id: request.tenant_id,
            kb_id: request.kb_id,
            doc_id: request.doc_id,
            doc_name: request.doc_name,
            compilation_template_id: template_id,
            chain_kind: &kind,
            timing_context: &timing_context,
            merge_scope_dataset: *merge_scope_dataset.get(template_id).unwrap_or(&false),
            chain_timeout_seconds: *STRUCTURE_CHAIN_CORRECTION_TIMEOUT_S,
            chunks_by_id,
            progress: request.progress,
        };
        if let Ok(info) = backend.merge_compiled_structures(flush_request).await
            && info.is_object()
        {
            let agg = agg_infos.entry(template_id.to_string()).or_default();
            for key in ["inserted", "updated", "duplicates_dropped"] {
                let current = agg.get(key).and_then(Value::as_i64).unwrap_or(0);
                let delta = info.get(key).and_then(Value::as_i64).unwrap_or(0);
                agg.insert(key.to_string(), json!(current + delta));
            }
            if let Some(kwds) = info.get("compile_kwds").and_then(Value::as_array) {
                let entry = compile_kwds_by_tid
                    .entry(template_id.to_string())
                    .or_default();
                for kwd in kwds {
                    if let Some(text) = kwd.as_str()
                        && !text.is_empty()
                        && !entry.iter().any(|existing| existing == text)
                    {
                        entry.push(text.to_string());
                    }
                }
            }
        }
        Ok(())
    }

    let template_ids_by_id: std::collections::HashMap<String, usize> = request
        .active_templates
        .iter()
        .enumerate()
        .map(|(index, (template_id, _))| (template_id.clone(), index + 1))
        .collect();
    let mut in_flight: futures_util::stream::FuturesUnordered<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = (
                            usize,
                            usize,
                            usize,
                            String,
                            std::result::Result<Vec<Value>, String>,
                        ),
                    > + Send
                    + '_,
            >,
        >,
    > = futures_util::stream::FuturesUnordered::new();
    let mut completed: std::collections::BTreeMap<usize, (usize, usize, String, Vec<Value>)> =
        std::collections::BTreeMap::new();
    let mut submit_sequence = 0usize;
    let mut commit_sequence = 0usize;
    let mut batch_no = 0usize;
    let mut dynamic_buffers: std::collections::HashMap<String, Vec<Value>> = request
        .active_templates
        .iter()
        .map(|(template_id, _)| (template_id.clone(), Vec::new()))
        .collect();
    let mut dynamic_buffer_tokens: std::collections::HashMap<String, usize> = request
        .active_templates
        .iter()
        .map(|(template_id, _)| (template_id.clone(), 0))
        .collect();

    for incoming_batch in &request.chunk_batches {
        for chunk in incoming_batch {
            if let Some(cid) = chunk.get("id").and_then(Value::as_str)
                && !chunks_by_id.contains_key(cid)
            {
                let text = chunk
                    .get("content_with_weight")
                    .or_else(|| chunk.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                chunks_by_id.insert(cid.to_string(), text);
            }
        }
        for (template_id, parser_cfg) in &request.active_templates {
            if cancel_check() {
                return Err(RunnerError::Cancelled);
            }
            let budget = dynamic_batch_budget(
                None,
                template_kinds
                    .get(template_id)
                    .map(String::as_str)
                    .unwrap_or(""),
            );
            let buffer = dynamic_buffers.entry(template_id.clone()).or_default();
            let mut buffer_tokens = *dynamic_buffer_tokens.get(template_id).unwrap_or(&0);
            for chunk in incoming_batch {
                let text = chunk
                    .get("content_with_weight")
                    .or_else(|| chunk.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let chunk_tokens = crate::chunk::tokenizer::token_count(text);
                if !buffer.is_empty() && buffer_tokens + chunk_tokens > budget {
                    batch_no += 1;
                    let batch = std::mem::take(buffer);
                    let sequence = submit_sequence;
                    submit_sequence += 1;
                    let batch_len = batch.len();
                    let template = template_id.clone();
                    let cfg = parser_cfg.clone();
                    let no = batch_no;
                    in_flight.push(Box::pin(async move {
                        let result = backend
                            .compile_structure_from_text(
                                &batch,
                                &cfg,
                                request.doc_id,
                                request.doc_name,
                                request.language,
                                &template,
                            )
                            .await;
                        (sequence, no, batch_len, template, result)
                    }));
                    buffer_tokens = 0;
                    while in_flight.len() + completed.len() >= *DOC_STRUCTURE_COMPILE_MAX_IN_FLIGHT
                    {
                        if let Some((sequence, no, len, template, result)) = in_flight.next().await
                            && let Ok(docs) = result
                        {
                            completed.insert(sequence, (no, len, template, docs));
                        }
                        while let Some((no, len, template, docs)) =
                            completed.remove(&commit_sequence)
                        {
                            let _ = len;
                            if !docs.is_empty() {
                                accumulators
                                    .entry(template.clone())
                                    .or_default()
                                    .extend(docs);
                            }
                            if accumulators.get(&template).map(|a| a.len()).unwrap_or(0)
                                >= DOC_STRUCTURE_MERGE_MAX_DOCS
                            {
                                progress(&format!(
                                    "  merge flush for batch {no} for template ({}/{total})",
                                    template_ids_by_id.get(&template).copied().unwrap_or(0)
                                ));
                                let accumulator = accumulators.entry(template.clone()).or_default();
                                flush(
                                    backend,
                                    &template,
                                    accumulator,
                                    &request,
                                    &template_kinds,
                                    &merge_scope_dataset,
                                    &mut compile_kwds_by_tid,
                                    &mut agg_infos,
                                    &chunks_by_id,
                                    &mut flush_sequence,
                                )
                                .await?;
                            }
                            commit_sequence += 1;
                        }
                    }
                }
                buffer.push(chunk.clone());
                buffer_tokens += chunk_tokens;
                if buffer_tokens >= budget {
                    batch_no += 1;
                    let batch = std::mem::take(buffer);
                    let sequence = submit_sequence;
                    submit_sequence += 1;
                    let batch_len = batch.len();
                    let template = template_id.clone();
                    let cfg = parser_cfg.clone();
                    let no = batch_no;
                    in_flight.push(Box::pin(async move {
                        let result = backend
                            .compile_structure_from_text(
                                &batch,
                                &cfg,
                                request.doc_id,
                                request.doc_name,
                                request.language,
                                &template,
                            )
                            .await;
                        (sequence, no, batch_len, template, result)
                    }));
                    buffer_tokens = 0;
                }
            }
            *dynamic_buffer_tokens
                .entry(template_id.clone())
                .or_default() = buffer_tokens;
        }
    }

    for (template_id, buffer) in dynamic_buffers.clone() {
        if cancel_check() {
            return Err(RunnerError::Cancelled);
        }
        if !buffer.is_empty() {
            batch_no += 1;
            let sequence = submit_sequence;
            submit_sequence += 1;
            let batch_len = buffer.len();
            let cfg = request
                .active_templates
                .iter()
                .find(|(template, _)| template == &template_id)
                .map(|(_, cfg)| cfg.clone())
                .unwrap_or_else(|| json!({}));
            let template = template_id.clone();
            let no = batch_no;
            in_flight.push(Box::pin(async move {
                let result = backend
                    .compile_structure_from_text(
                        &buffer,
                        &cfg,
                        request.doc_id,
                        request.doc_name,
                        request.language,
                        &template,
                    )
                    .await;
                (sequence, no, batch_len, template, result)
            }));
        }
        dynamic_buffers.entry(template_id).or_default().clear();
    }

    while !in_flight.is_empty() {
        if cancel_check() {
            return Err(RunnerError::Cancelled);
        }
        if let Some((sequence, no, len, template, result)) = in_flight.next().await
            && let Ok(docs) = result
        {
            completed.insert(sequence, (no, len, template, docs));
        }
        while let Some((no, len, template, docs)) = completed.remove(&commit_sequence) {
            let _ = len;
            if !docs.is_empty() {
                accumulators
                    .entry(template.clone())
                    .or_default()
                    .extend(docs);
            }
            if accumulators.get(&template).map(|a| a.len()).unwrap_or(0)
                >= DOC_STRUCTURE_MERGE_MAX_DOCS
            {
                progress(&format!(
                    "  merge flush for batch {no} for template ({}/{total})",
                    template_ids_by_id.get(&template).copied().unwrap_or(0)
                ));
                let accumulator = accumulators.entry(template.clone()).or_default();
                flush(
                    backend,
                    &template,
                    accumulator,
                    &request,
                    &template_kinds,
                    &merge_scope_dataset,
                    &mut compile_kwds_by_tid,
                    &mut agg_infos,
                    &chunks_by_id,
                    &mut flush_sequence,
                )
                .await?;
            }
            commit_sequence += 1;
        }
    }
    while let Some((no, len, template, docs)) = completed.remove(&commit_sequence) {
        let _ = len;
        if !docs.is_empty() {
            accumulators
                .entry(template.clone())
                .or_default()
                .extend(docs);
        }
        if accumulators.get(&template).map(|a| a.len()).unwrap_or(0) >= DOC_STRUCTURE_MERGE_MAX_DOCS
        {
            progress(&format!(
                "  merge flush for batch {no} for template ({}/{total})",
                template_ids_by_id.get(&template).copied().unwrap_or(0)
            ));
            let accumulator = accumulators.entry(template.clone()).or_default();
            flush(
                backend,
                &template,
                accumulator,
                &request,
                &template_kinds,
                &merge_scope_dataset,
                &mut compile_kwds_by_tid,
                &mut agg_infos,
                &chunks_by_id,
                &mut flush_sequence,
            )
            .await?;
        }
        commit_sequence += 1;
    }

    for (template_id, _) in &request.active_templates {
        if cancel_check() {
            return Err(RunnerError::Cancelled);
        }
        let accumulator = accumulators.entry(template_id.clone()).or_default();
        flush(
            backend,
            template_id,
            accumulator,
            &request,
            &template_kinds,
            &merge_scope_dataset,
            &mut compile_kwds_by_tid,
            &mut agg_infos,
            &chunks_by_id,
            &mut flush_sequence,
        )
        .await?;
    }

    // Dataset structure graph rebuild (dataset-scope templates).
    for (template_id, _) in &request.active_templates {
        if !*merge_scope_dataset.get(template_id).unwrap_or(&false) {
            continue;
        }
        let structure_kind = backend
            .saved_template(template_id, request.tenant_id)
            .and_then(|template| {
                template
                    .get("kind")
                    .and_then(Value::as_str)
                    .map(|text| text.trim().to_string())
                    .filter(|text| !text.is_empty())
            });
        let mut kwds = compile_kwds_by_tid
            .get(template_id)
            .cloned()
            .unwrap_or_default();
        kwds.sort();
        for compile_kwd in kwds {
            if cancel_check() {
                return Err(RunnerError::Cancelled);
            }
            progress(&format!(
                "Rebuilding dataset structure graph (compile_kwd={compile_kwd}) ..."
            ));
            let _ = backend
                .rebuild_dataset_structure_graph_json(
                    request.tenant_id,
                    request.kb_id,
                    &compile_kwd,
                    template_id,
                    structure_kind.as_deref(),
                )
                .await;
        }
    }

    upsert_dataset_nav_from_page_index(
        backend,
        &request.active_templates,
        request.tenant_id,
        request.kb_id,
        request.doc_id,
        request.doc_name,
        progress,
        cancel_check,
    )
    .await?;

    for (template_id, _) in &request.active_templates {
        if template_kinds.get(template_id).map(String::as_str) != Some("timeline") {
            continue;
        }
        let _ = backend
            .cleanup_timeline_isolated_entities(
                request.tenant_id,
                request.kb_id,
                request.doc_id,
                request.doc_name,
                template_id,
            )
            .await;
    }

    // Per-template progress + recording, then the optional synthesis phase.
    for (index, (template_id, parser_cfg)) in request.active_templates.iter().enumerate() {
        if cancel_check() {
            return Err(RunnerError::Cancelled);
        }
        let agg = agg_infos.get(template_id).cloned().unwrap_or_default();
        if let Some(record) = request.record {
            let mut recorded = agg.clone();
            recorded.insert("rechunked_chunk_count".to_string(), json!(0));
            record(
                &format!("document_structure_compile:{template_id}"),
                Value::Object(recorded),
            );
        }
        progress(&format!(
            "Document knowledge compilation done ({}/{}): inserted={}, updated={}, duplicates_dropped={}",
            index + 1,
            total,
            agg.get("inserted").and_then(Value::as_i64).unwrap_or(0),
            agg.get("updated").and_then(Value::as_i64).unwrap_or(0),
            agg.get("duplicates_dropped")
                .and_then(Value::as_i64)
                .unwrap_or(0),
        ));
        let synthesis_cfg = parser_cfg
            .get("synthesis")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !synthesis_cfg
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        let example = synthesis_cfg
            .get("example")
            .and_then(Value::as_str)
            .unwrap_or("");
        let compile_kwd = synthesis_cfg
            .get("compile_kwd")
            .and_then(Value::as_str)
            .unwrap_or("wiki_page");
        if cancel_check() {
            return Err(RunnerError::Cancelled);
        }
        if example.is_empty() {
            continue;
        }
        progress(&format!(
            "Synthesis PLAN for template {template_id} (kind={compile_kwd}) ..."
        ));
        let plan = backend
            .wiki_plan_from_reduction(request.tenant_id, request.kb_id)
            .await
            .unwrap_or_else(|_| json!({}));
        if cancel_check() {
            return Err(RunnerError::Cancelled);
        }
        let planned = plan
            .get("pages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if planned.is_empty() {
            progress(&format!(
                "Synthesis: no pages planned for template {template_id}."
            ));
            continue;
        }
        progress(&format!(
            "Synthesis REFINE for template {template_id} ({} page(s)) ...",
            planned.len()
        ));
        let mut pages = backend
            .wiki_refine_from_plan(request.tenant_id, request.kb_id, example)
            .await
            .unwrap_or_default();
        for page in pages.iter_mut() {
            if let Some(map) = page.as_object_mut() {
                map.insert("compile_kwd".to_string(), json!(compile_kwd));
            }
        }
        progress(&format!(
            "Synthesis done: {} {compile_kwd} page(s) written.",
            pages.len()
        ));
    }

    Ok(Value::Object(
        agg_infos
            .into_iter()
            .map(|(key, value)| (key, Value::Object(value)))
            .collect(),
    ))
}

/// Local truthiness helper (`if dataset_merge:`).
pub(crate) fn value_is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map(|n| n != 0.0).unwrap_or(true),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

// ── `structure.py` gaps A1 (v0.3.10ah) ─────────────────────────────────────

/// `_RechunkedDocs`: compiled rows plus the formal chunks created by rechunking.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RechunkedDocs {
    pub docs: Vec<Value>,
    pub rechunked_chunks: Vec<Value>,
}

impl RechunkedDocs {
    /// `_RechunkedDocs(docs=None, rechunked_chunks=None)`.
    pub fn new(docs: Vec<Value>, rechunked_chunks: Vec<Value>) -> Self {
        Self {
            docs,
            rechunked_chunks,
        }
    }
}

/// `_struct_merge_lock_key`.
pub fn struct_merge_lock_key(kb_id: &str, compilation_template_id: Option<&str>) -> String {
    format!(
        "struct_merge:{kb_id}:{}",
        compilation_template_id.unwrap_or("")
    )
}

/// `_STRUCT_INVALID_SENTINELS`.
pub const STRUCT_INVALID_SENTINELS: [&str; 1] = ["-1"];

/// `_struct_is_invalid_sentinel`.
pub fn struct_is_invalid_sentinel(value: &Value) -> bool {
    value
        .as_str()
        .map(|text| STRUCT_INVALID_SENTINELS.contains(&text.trim()))
        .unwrap_or(false)
}

/// `_struct_expand_source_chunk_ids`: expand compact positional ids such as
/// `t1-t3` (descending ranges allowed); unknown ids are dropped.
pub fn struct_expand_source_chunk_ids(
    raw_ids: &Value,
    source_texts: &std::collections::HashMap<String, String>,
) -> Vec<String> {
    let values: Vec<Value> = match raw_ids {
        Value::String(_) => vec![raw_ids.clone()],
        Value::Array(items) => items.clone(),
        _ => return Vec::new(),
    };
    let range_re = regex::Regex::new(r"(?i)^t(\d+)\s*-\s*t(\d+)$").expect("range regex");
    let mut expanded: Vec<String> = Vec::new();
    for raw in values {
        let value = match &raw {
            Value::String(text) => text.trim().to_string(),
            other => other.to_string().trim().to_string(),
        };
        let chunk_ids: Vec<String> = if let Some(captures) = range_re.captures(&value) {
            let start: i64 = captures[1].parse().unwrap_or(0);
            let end: i64 = captures[2].parse().unwrap_or(0);
            let step = if start <= end { 1 } else { -1 };
            let mut ids = Vec::new();
            let mut index = start;
            loop {
                ids.push(format!("t{index}"));
                if index == end {
                    break;
                }
                index += step;
            }
            ids
        } else {
            vec![value]
        };
        for chunk_id in chunk_ids {
            if source_texts.contains_key(&chunk_id)
                && !expanded.iter().any(|existing| existing == &chunk_id)
            {
                expanded.push(chunk_id);
            }
        }
    }
    expanded
}

/// `_struct_payload_chunk_ids`: model-selected ids that belong to this batch
/// (falls back to the whole batch when none survive).
pub fn struct_payload_chunk_ids(payload: &Value, batch_ids: &[String]) -> Vec<String> {
    let raw_ids: Vec<Value> = match payload.get("source_chunk_ids") {
        Some(Value::String(_)) => vec![payload.get("source_chunk_ids").cloned().unwrap()],
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    let mut selected: Vec<String> = Vec::new();
    for raw in raw_ids {
        let chunk_id = match &raw {
            Value::String(text) => text.trim().to_string(),
            other => other.to_string().trim().to_string(),
        };
        if batch_ids.iter().any(|batch| batch == &chunk_id)
            && !selected.iter().any(|existing| existing == &chunk_id)
        {
            selected.push(chunk_id);
        }
    }
    if selected.is_empty() {
        batch_ids.to_vec()
    } else {
        selected
    }
}

/// `_struct_merge_graph_relations`: dedupe relation payloads on
/// `(from, to, type|related)` (case-folded), unioning `doc_ids_kwd`.
pub fn struct_merge_graph_relations(relations: &[Value]) -> Vec<Value> {
    let mut merged: std::collections::HashMap<(String, String, String), Value> =
        std::collections::HashMap::new();
    let mut order: Vec<(String, String, String)> = Vec::new();
    for relation in relations {
        let key = (
            relation
                .get("from")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_lowercase(),
            relation
                .get("to")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_lowercase(),
            relation
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("related")
                .trim()
                .to_lowercase(),
        );
        if key.0.is_empty() || key.1.is_empty() {
            continue;
        }
        match merged.get_mut(&key) {
            None => {
                merged.insert(key.clone(), relation.clone());
                order.push(key);
            }
            Some(target) => {
                let mut doc_ids: Vec<String> = Vec::new();
                for source in [target.get("doc_ids_kwd"), relation.get("doc_ids_kwd")] {
                    if let Some(items) = source.and_then(Value::as_array) {
                        for item in items {
                            if let Some(text) = item.as_str()
                                && !doc_ids.iter().any(|existing| existing == text)
                            {
                                doc_ids.push(text.to_string());
                            }
                        }
                    }
                }
                if !doc_ids.is_empty()
                    && let Some(map) = target.as_object_mut()
                {
                    map.insert(
                        "doc_ids_kwd".to_string(),
                        Value::Array(doc_ids.into_iter().map(Value::String).collect()),
                    );
                }
            }
        }
    }
    order
        .into_iter()
        .filter_map(|key| merged.remove(&key))
        .collect()
}

// ── `structure.py` gaps B1 (v0.3.10ah) ─────────────────────────────────────

/// `_struct_resolve_entity_alias`: follow alias chains (cycle-guarded).
pub fn struct_resolve_entity_alias(
    name: &str,
    aliases: &std::collections::HashMap<String, String>,
) -> String {
    let mut current = name.trim().to_string();
    let mut seen: Vec<String> = Vec::new();
    while aliases.contains_key(&current) && !seen.iter().any(|entry| entry == &current) {
        seen.push(current.clone());
        current = aliases[&current].clone();
    }
    current
}

/// `_struct_rewrite_relation_payload`: rewrite `(source|src|from)` and
/// `(target|tgt|to)` fields through the alias map. Returns whether anything
/// changed.
pub fn struct_rewrite_relation_payload(
    payload: &mut Value,
    aliases: &std::collections::HashMap<String, String>,
) -> bool {
    let mut changed = false;
    for fields in [["source", "src", "from"], ["target", "tgt", "to"]] {
        for field in fields {
            let Some(current) = payload.get(field) else {
                continue;
            };
            if current.is_null() {
                continue;
            }
            let old = match current {
                Value::String(text) => text.trim().to_string(),
                other => other.to_string().trim().to_string(),
            };
            let new = struct_resolve_entity_alias(&old, aliases);
            if new != old {
                if let Some(map) = payload.as_object_mut() {
                    map.insert(field.to_string(), Value::String(new));
                }
                changed = true;
            }
        }
    }
    changed
}

/// `_struct_union_chunk_ids`.
pub fn struct_union_chunk_ids(left: Option<&Value>, right: Option<&Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for source in [left, right] {
        if let Some(items) = source.and_then(Value::as_array) {
            for item in items {
                if let Some(text) = item.as_str()
                    && !out.iter().any(|existing| existing == text)
                {
                    out.push(text.to_string());
                }
            }
        }
    }
    out
}

fn parse_content_payload(doc: &Value) -> Option<Value> {
    let raw = doc.get("content_with_weight").and_then(Value::as_str)?;
    let parsed: Value = serde_json::from_str(raw).ok()?;
    parsed.is_object().then_some(parsed)
}

fn string_field(doc: &Value, key: &str) -> String {
    doc.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// `_struct_rebuild_doc_storage_doc`: rebuild a store row from a merged payload
/// via [`to_es_doc`], then overlay the identity fields from `base_doc`.
pub fn struct_rebuild_doc_storage_doc(
    payload: &Value,
    base_doc: &Value,
    vec: Vec<f32>,
    chunk_ids: &[String],
    preserve_id: bool,
) -> CompiledRow {
    let kind = {
        let value = string_field(base_doc, "knowledge_graph_kwd");
        if value.is_empty() {
            "entity".to_string()
        } else {
            value
        }
    };
    let mut src_field: Option<&str> = None;
    let mut target_field: Option<&str> = None;
    if kind == "relation"
        && let Some(existing) = parse_content_payload(base_doc)
        && existing.get("source").is_some()
        && existing.get("target").is_some()
    {
        src_field = Some("source");
        target_field = Some("target");
    }
    let template_id = doc_template_id(base_doc);
    let template_kind = base_doc
        .get("compilation_template_kind_kwd")
        .and_then(Value::as_str)
        .map(str::to_string);
    let doc_name = string_field(base_doc, "docnm_kwd");
    let mut new_doc = to_es_doc(
        payload,
        &string_field(base_doc, "compile_kwd"),
        &string_field(base_doc, "doc_id"),
        chunk_ids,
        vec,
        &kind,
        src_field,
        target_field,
        template_id.as_deref(),
        template_kind.as_deref(),
    );
    let _ = doc_name;
    if preserve_id {
        let id = string_field(base_doc, "id");
        if !id.is_empty() {
            new_doc.id = id;
        }
    }
    for key in ["from_entity_kwd", "to_entity_kwd"] {
        let value = string_field(base_doc, key);
        if !value.is_empty() {
            if key == "from_entity_kwd" {
                new_doc.from_entity_kwd = Some(value);
            } else {
                new_doc.to_entity_kwd = Some(value);
            }
        }
    }
    new_doc
}

/// `_struct_reembed_payload`: re-encode a merged payload's description.
pub async fn struct_reembed_payload(
    payload: &Value,
    embed: &dyn EmbeddingBackend,
) -> Option<Vec<f32>> {
    let text = payload_description(payload);
    let vectors = encode(embed, &[text]).await.ok()?;
    vectors.into_iter().next()
}

/// `_struct_doc_storage_dedup_condition`: the merge-candidate filter.
pub fn struct_doc_storage_dedup_condition(doc: &Value, merge_scope_dataset: bool) -> Value {
    let mut condition = serde_json::Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        serde_json::json!([string_field(doc, "compile_kwd")]),
    );
    if !merge_scope_dataset {
        condition.insert(
            "doc_id".to_string(),
            serde_json::json!([string_field(doc, "doc_id")]),
        );
    }
    for key in ["knowledge_graph_kwd", "from_entity_kwd", "to_entity_kwd"] {
        let value = string_field(doc, key);
        if !value.is_empty() {
            condition.insert(key.to_string(), serde_json::json!([value]));
        }
    }
    if let Some(template_id) = doc_template_id(doc) {
        condition.insert(
            "compilation_template_ids".to_string(),
            serde_json::json!([template_id]),
        );
    }
    Value::Object(condition)
}

/// `_struct_rewrite_relation_doc`.
pub async fn struct_rewrite_relation_doc(
    doc: &Value,
    aliases: &std::collections::HashMap<String, String>,
    embed: &dyn EmbeddingBackend,
) -> Value {
    if string_field(doc, "knowledge_graph_kwd") != "relation" || aliases.is_empty() {
        return doc.clone();
    }
    let Some(mut payload) = parse_content_payload(doc) else {
        return doc.clone();
    };
    if !struct_rewrite_relation_payload(&mut payload, aliases) {
        return doc.clone();
    }
    let vectors = match encode(embed, &[payload_description(&payload)]).await {
        Ok(vectors) => vectors,
        Err(_) => return doc.clone(),
    };
    let Some(vector) = vectors.into_iter().next() else {
        return doc.clone();
    };
    let mut base = doc.clone();
    if let Some(map) = base.as_object_mut() {
        map.insert(
            "content_with_weight".to_string(),
            Value::String(payload.to_string()),
        );
        map.insert(
            "from_entity_kwd".to_string(),
            Value::String(struct_resolve_entity_alias(
                &string_field(doc, "from_entity_kwd"),
                aliases,
            )),
        );
        map.insert(
            "to_entity_kwd".to_string(),
            Value::String(struct_resolve_entity_alias(
                &string_field(doc, "to_entity_kwd"),
                aliases,
            )),
        );
    }
    let chunk_ids = struct_union_chunk_ids(doc.get("source_chunk_ids"), None);
    let rebuilt = struct_rebuild_doc_storage_doc(&payload, &base, vector, &chunk_ids, true);
    serde_json::to_value(&rebuilt).unwrap_or(Value::Null)
}

/// `_struct_merge_exact_entity_payload`: merge same-name entity payloads
/// without vector similarity.
pub fn struct_merge_exact_entity_payload(existing: &Value, incoming: &Value) -> Option<Value> {
    let left = parse_content_payload(existing)?;
    let right = parse_content_payload(incoming)?;
    let mut merged = left.clone();
    if let (Some(target), Some(source)) = (merged.as_object_mut(), right.as_object()) {
        for (key, value) in source {
            let empty = match target.get(key) {
                None | Some(Value::Null) => true,
                Some(Value::String(text)) => text.is_empty(),
                Some(Value::Array(items)) => items.is_empty(),
                _ => false,
            };
            if empty {
                target.insert(key.clone(), value.clone());
            }
        }
    }
    let left_type = left
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let right_type = right
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    for preferred in ["title", "fact", "conclusion"] {
        if left_type == preferred || right_type == preferred {
            if let Some(map) = merged.as_object_mut() {
                map.insert("type".to_string(), Value::String(preferred.to_string()));
            }
            break;
        }
    }
    let left_desc = left
        .get("description")
        .map(str_of_value)
        .unwrap_or_default();
    let right_desc = right
        .get("description")
        .map(str_of_value)
        .unwrap_or_default();
    let description = if right_desc.chars().count() > left_desc.chars().count() {
        right_desc
    } else {
        left_desc
    };
    let chunk_ids =
        struct_union_chunk_ids(left.get("source_chunk_ids"), right.get("source_chunk_ids"));
    if let Some(map) = merged.as_object_mut() {
        map.insert("description".to_string(), Value::String(description));
        map.insert(
            "source_chunk_ids".to_string(),
            Value::Array(chunk_ids.into_iter().map(Value::String).collect()),
        );
    }
    Some(merged)
}

fn str_of_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `_struct_merge_exact_named_entities`: collapse same-name entities before
/// similarity-based dedup. Returns `(docs, dropped)`.
pub async fn struct_merge_exact_named_entities(
    docs: &[Value],
    embed: &dyn EmbeddingBackend,
) -> (Vec<Value>, usize) {
    let mut kept: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut unchanged: Vec<Value> = Vec::new();
    let mut dropped = 0usize;
    for doc in docs {
        let name = crate::dataset_structure_merger::struct_entity_name(doc)
            .trim()
            .to_lowercase();
        if name.is_empty() {
            unchanged.push(doc.clone());
            continue;
        }
        let Some(existing) = kept.get(&name).cloned() else {
            kept.insert(name.clone(), doc.clone());
            order.push(name);
            continue;
        };
        let Some(payload) = struct_merge_exact_entity_payload(&existing, doc) else {
            unchanged.push(doc.clone());
            continue;
        };
        let Some(vector) = struct_reembed_payload(&payload, embed).await else {
            unchanged.push(doc.clone());
            continue;
        };
        let chunk_ids = struct_union_chunk_ids(
            existing.get("source_chunk_ids"),
            doc.get("source_chunk_ids"),
        );
        let rebuilt = struct_rebuild_doc_storage_doc(&payload, &existing, vector, &chunk_ids, true);
        kept.insert(name, serde_json::to_value(&rebuilt).unwrap_or(Value::Null));
        dropped += 1;
    }
    let mut out: Vec<Value> = order
        .into_iter()
        .filter_map(|name| kept.remove(&name))
        .collect();
    out.extend(unchanged);
    (out, dropped)
}

// ── `structure.py` gaps C1 (v0.3.10ah) ─────────────────────────────────────

/// `_struct_filter_key` over store rows (`CompiledRow::filter_key` already
/// provides the typed variant).
pub fn struct_filter_key_value(
    doc: &Value,
) -> (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let opt = |value: String| if value.is_empty() { None } else { Some(value) };
    (
        string_field(doc, "doc_id"),
        string_field(doc, "compile_kwd"),
        opt(string_field(doc, "from_entity_kwd")),
        opt(string_field(doc, "to_entity_kwd")),
        doc_template_id(doc),
    )
}

/// `_dataset_struct_graph_row_id`: stable KB-wide graph-row id. The upstream
/// seed (`"{kb}:dataset_structure_graph:{kwd}:{tpl}"`) is hashed with the
/// crate's row-id helper (xxh3-64; upstream xxh64 — documented divergence).
pub fn dataset_struct_graph_row_id(
    kb_id: &str,
    compile_kwd: &str,
    compilation_template_id: Option<&str>,
) -> String {
    let seed = format!(
        "{kb_id}:dataset_structure_graph:{compile_kwd}:{}",
        compilation_template_id.unwrap_or("")
    );
    stable_row_id(&[seed])
}

/// `_struct_entity_candidate_groups`: partition entity candidates into
/// independent cosine-connected groups (per filter key, union-find over
/// pairwise cosine >= threshold).
pub fn struct_entity_candidate_groups(
    docs: &[Value],
    similarity_threshold: f32,
) -> Vec<Vec<Value>> {
    let mut order: Vec<(
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = Vec::new();
    let mut buckets: std::collections::HashMap<
        (
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
        Vec<Value>,
    > = std::collections::HashMap::new();
    for doc in docs {
        let key = struct_filter_key_value(doc);
        if !buckets.contains_key(&key) {
            order.push(key.clone());
        }
        buckets.entry(key).or_default().push(doc.clone());
    }
    let mut result: Vec<Vec<Value>> = Vec::new();
    for key in order {
        let bucket = buckets.remove(&key).unwrap_or_default();
        let vectors: Vec<Option<Vec<f32>>> = bucket.iter().map(find_vec_field).collect();
        let valid: Vec<usize> = vectors
            .iter()
            .enumerate()
            .filter_map(|(index, vector)| vector.as_ref().map(|_| index))
            .collect();
        let mut parent: Vec<usize> = (0..bucket.len()).collect();
        fn find(parent: &mut Vec<usize>, mut index: usize) -> usize {
            while parent[index] != index {
                parent[index] = parent[parent[index]];
                index = parent[index];
            }
            index
        }
        if valid.len() > 1 {
            for left_offset in 0..valid.len() {
                for right_offset in (left_offset + 1)..valid.len() {
                    let left = valid[left_offset];
                    let right = valid[right_offset];
                    let left_vec = vectors[left].clone().unwrap_or_default();
                    let right_vec = vectors[right].clone().unwrap_or_default();
                    if crate::merge::cosine_similarity(&left_vec, &right_vec)
                        >= similarity_threshold
                    {
                        let left_root = find(&mut parent, left);
                        let right_root = find(&mut parent, right);
                        if left_root != right_root {
                            parent[right_root] = left_root;
                        }
                    }
                }
            }
        }
        let mut components: std::collections::HashMap<usize, Vec<Value>> =
            std::collections::HashMap::new();
        let mut component_order: Vec<usize> = Vec::new();
        for (index, doc) in bucket.into_iter().enumerate() {
            let root = if valid.contains(&index) {
                find(&mut parent, index)
            } else {
                index
            };
            if !components.contains_key(&root) {
                component_order.push(root);
            }
            components.entry(root).or_default().push(doc);
        }
        for root in component_order {
            if let Some(group) = components.remove(&root) {
                result.push(group);
            }
        }
    }
    result
}

// ── `structure.py` gaps C2a (v0.3.10ah) — doc-storage dedup judges ────────

/// `ES_GROUP_MERGE_PROMPT` (verbatim).
pub const ES_GROUP_MERGE_PROMPT: &str = r#"Existing item:
{existing}

Incoming items:
{incoming}

Decide which incoming items refer to the same logical entity or relation as
the existing item. Merge all duplicated incoming items with the existing item.
Incoming items that are not duplicates must remain separate. Do not invent
data and do not merge unrelated incoming items with each other.

Return ONLY JSON with this exact shape:
{
  "duplicate_indices": [<incoming index>, ...],
  "merged": <merged JSON object when duplicate_indices is non-empty, otherwise null>
}
"#;

/// `ES_GROUP_BATCH_MERGE_PROMPT` (verbatim).
pub const ES_GROUP_BATCH_MERGE_PROMPT: &str = r#"You are judging multiple independent ES deduplication groups.

For every group, compare every incoming item with that group's existing item.
You must make a separate duplicated decision for every incoming item. Only
incoming items marked duplicated=true may contribute to that group's merged
payload. Incoming items marked duplicated=false must remain separate. Do not
merge items from different groups and do not invent data.

Return ONLY JSON with this exact shape:
{
  "groups": [
    {
      "group_id": "<group id>",
      "decisions": [
        {"incoming_index": 0, "duplicated": true},
        {"incoming_index": 1, "duplicated": false}
      ],
      "merged": <merged JSON object when any item is duplicated, otherwise null>
    }
  ]
}

Groups:
{groups}
"#;

/// `ES_GROUP_DECISION_BATCH_PROMPT` (verbatim).
pub const ES_GROUP_DECISION_BATCH_PROMPT: &str = r#"You are judging multiple independent ES deduplication groups.

For every incoming item, independently decide whether it is a duplicate of
the existing item in the same group. Do not merge anything and do not judge
items from different groups against each other.

Return ONLY JSON with this exact shape:
{
  "groups": [
    {
      "group_id": "<group id>",
      "decisions": [
        {"incoming_index": 0, "duplicated": true},
        {"incoming_index": 1, "duplicated": false}
      ]
    }
  ]
}

Groups:
{groups}
"#;

/// `_ES_DEDUP_KNN_CONCURRENCY`.
pub const ES_DEDUP_KNN_CONCURRENCY: usize = 8;
/// `_ES_DEDUP_LLM_CONCURRENCY`.
pub const ES_DEDUP_LLM_CONCURRENCY: usize = 16;
/// `_ES_DEDUP_LLM_BATCH_SIZE`.
pub const ES_DEDUP_LLM_BATCH_SIZE: usize = 16;
/// `_ES_DEDUP_EMBED_BATCH_SIZE`.
pub const ES_DEDUP_EMBED_BATCH_SIZE: usize = 64;
/// `_ES_DEDUP_INSERT_BATCH_SIZE`.
pub const ES_DEDUP_INSERT_BATCH_SIZE: usize = 256;

/// Chat-based `gen_json`: strip think blocks / fences, then parse (lenient
/// fallback included). `None` when nothing parses.
async fn struct_gen_json(
    chat: &dyn crate::harness::HarnessChat,
    system: &str,
    user: &str,
) -> Option<Value> {
    let history = vec![serde_json::json!({"role": "user", "content": user})];
    let raw = chat
        .chat(system, &history, &serde_json::json!({"temperature": 0.0}))
        .await
        .ok()?;
    let think = regex::Regex::new(r"(?s)^.*</think>").expect("think regex");
    let stripped = think.replace(&raw, "").to_string();
    let fence = regex::Regex::new(r"```(?:json)?\s*|\s*```").expect("fence regex");
    let cleaned = fence.replace_all(&stripped, "").trim().to_string();
    serde_json::from_str::<Value>(&cleaned)
        .ok()
        .or_else(|| parse_json_lenient(&cleaned))
}

fn struct_payload_of(doc: &Value) -> Option<Value> {
    parse_content_payload(doc)
}

fn struct_store_query(
    select_fields: &[String],
    condition: &Value,
    match_exprs: Vec<crate::doc_store::MatchExpr>,
    index: &str,
    kb_id: &str,
    limit: usize,
) -> crate::doc_store::SearchQuery {
    let condition_map = condition.as_object().cloned().unwrap_or_default();
    crate::doc_store::SearchQuery {
        select_fields: select_fields.to_vec(),
        condition: condition_map,
        match_expressions: match_exprs,
        offset: 0,
        limit,
        index_names: vec![index.to_string()],
        dataset_ids: vec![kb_id.to_string()],
        ..Default::default()
    }
}

fn struct_first_row(
    store: &dyn crate::doc_store::DocStore,
    query: crate::doc_store::SearchQuery,
    select_fields: &[String],
) -> Option<Value> {
    let response = store.search(&query).ok()?;
    let fields = store.get_fields(&response, select_fields);
    let (old_id, old_doc) = fields.into_iter().next()?;
    let mut old_doc = old_doc;
    old_doc
        .entry("id".to_string())
        .or_insert(Value::String(old_id));
    Some(Value::Object(old_doc))
}

/// The select-field list shared by the dedup searches (verbatim).
pub fn struct_dedup_select_fields() -> Vec<String> {
    [
        "id",
        "content_with_weight",
        "source_chunk_ids",
        "knowledge_graph_kwd",
        "compile_kwd",
        "doc_id",
        "docnm_kwd",
        "from_entity_kwd",
        "to_entity_kwd",
        "compilation_template_ids",
        "compilation_template_kind_kwd",
    ]
    .iter()
    .map(|field| field.to_string())
    .collect()
}

fn struct_doc_vec_field(row: &Value) -> Option<(String, Vec<f32>)> {
    let map = row.as_object()?;
    for (key, val) in map {
        if key.starts_with("q_")
            && key.ends_with("_vec")
            && let Some(items) = val.as_array()
        {
            let vector: Vec<f32> = items
                .iter()
                .filter_map(|item| item.as_f64().map(|number| number as f32))
                .collect();
            if !vector.is_empty() {
                return Some((key.clone(), vector));
            }
        }
    }
    None
}

/// `_struct_doc_storage_knn_candidate`: one KNN lookup (exact entity-name check
/// first, then a dense cosine query). The upstream `extra_options {"similarity":
/// …}` hint has no local `MatchExpr::Dense` slot (documented divergence).
#[allow(clippy::too_many_arguments)]
pub fn struct_doc_storage_knn_candidate(
    store: &dyn crate::doc_store::DocStore,
    doc: &Value,
    tenant_id: &str,
    kb_id: &str,
    similarity_threshold: f32,
    merge_scope_dataset: bool,
) -> Option<Value> {
    let _ = similarity_threshold;
    let index = doc_store_index_name(tenant_id);
    let select_fields = struct_dedup_select_fields();
    if string_field(doc, "knowledge_graph_kwd") == "entity" {
        let name = {
            let explicit = string_field(doc, "name_kwd");
            let resolved = if explicit.is_empty() {
                crate::dataset_structure_merger::struct_entity_name(doc)
            } else {
                explicit
            };
            resolved.trim().to_lowercase()
        };
        if !name.is_empty() {
            let mut exact_condition = struct_doc_storage_dedup_condition(doc, merge_scope_dataset);
            if let Some(map) = exact_condition.as_object_mut() {
                map.insert("name_kwd".to_string(), serde_json::json!([name]));
            }
            let query = struct_store_query(
                &select_fields,
                &exact_condition,
                Vec::new(),
                &index,
                kb_id,
                1,
            );
            if let Some(found) = struct_first_row(store, query, &select_fields) {
                return Some(found);
            }
        }
    }
    let (vec_field, vector) = struct_doc_vec_field(doc)?;
    let match_expr = crate::doc_store::MatchExpr::dense(&vec_field, vector, "cosine", 1);
    let condition = struct_doc_storage_dedup_condition(doc, merge_scope_dataset);
    let query = struct_store_query(
        &select_fields,
        &condition,
        vec![match_expr],
        &index,
        kb_id,
        1,
    );
    struct_first_row(store, query, &select_fields)
}

/// `_struct_judge_doc_storage_group_batch`: independent duplicate decisions for
/// every incoming item (no merge generation). Returns `group_id -> duplicated
/// incoming indices`.
pub async fn struct_judge_doc_storage_group_batch(
    chat: &dyn crate::harness::HarnessChat,
    group_specs: &[Value],
) -> std::collections::HashMap<String, std::collections::HashSet<i64>> {
    let empty = |specs: &[Value]| {
        specs
            .iter()
            .filter_map(|spec| {
                spec.get("request_group_id")
                    .and_then(Value::as_str)
                    .map(|id| (id.to_string(), std::collections::HashSet::new()))
            })
            .collect::<std::collections::HashMap<_, _>>()
    };
    let mut prompt_groups: Vec<Value> = Vec::new();
    for spec in group_specs {
        let old_doc = spec.get("old_doc").cloned().unwrap_or(Value::Null);
        let incoming_docs = spec
            .get("incoming_docs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let Some(existing_payload) = struct_payload_of(&old_doc) else {
            continue;
        };
        let incoming_payloads: Vec<Value> =
            incoming_docs.iter().filter_map(struct_payload_of).collect();
        if incoming_payloads.len() != incoming_docs.len() {
            continue;
        }
        prompt_groups.push(serde_json::json!({
            "group_id": spec.get("request_group_id").cloned().unwrap_or(Value::Null),
            "existing": existing_payload,
            "incoming": incoming_payloads
                .iter()
                .enumerate()
                .map(|(index, payload)| serde_json::json!({"index": index, "item": payload}))
                .collect::<Vec<_>>(),
        }));
    }
    if prompt_groups.is_empty() {
        return empty(group_specs);
    }
    let groups_json = Value::Array(prompt_groups).to_string();
    let user_prompt = ES_GROUP_DECISION_BATCH_PROMPT.replace("{groups}", &groups_json);
    let prefix = ES_GROUP_DECISION_BATCH_PROMPT
        .split("Groups:")
        .next()
        .unwrap_or("");
    let system_prompt = format!("{}\n\n{prefix}", crate::merge::MERGE_SYSTEM_PROMPT);
    let Some(res) = struct_gen_json(chat, &system_prompt, &user_prompt).await else {
        return empty(group_specs);
    };
    let Some(raw_groups) = res.get("groups").and_then(Value::as_array) else {
        return empty(group_specs);
    };
    let by_id: std::collections::HashMap<String, &Value> = group_specs
        .iter()
        .filter_map(|spec| {
            spec.get("request_group_id")
                .and_then(Value::as_str)
                .map(|id| (id.to_string(), spec))
        })
        .collect();
    let mut result: std::collections::HashMap<String, std::collections::HashSet<i64>> =
        std::collections::HashMap::new();
    for raw in raw_groups {
        let Some(group_id) = raw.get("group_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(spec) = by_id.get(group_id) else {
            continue;
        };
        let incoming_len = spec
            .get("incoming_docs")
            .and_then(Value::as_array)
            .map(|items| items.len())
            .unwrap_or(0);
        let mut hits: std::collections::HashSet<i64> = std::collections::HashSet::new();
        if let Some(decisions) = raw.get("decisions").and_then(Value::as_array) {
            for item in decisions {
                let duplicated = item
                    .get("duplicated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if duplicated
                    && let Some(index) = item.get("incoming_index").and_then(Value::as_i64)
                    && index >= 0
                    && (index as usize) < incoming_len
                {
                    hits.insert(index);
                }
            }
        }
        result.insert(group_id.to_string(), hits);
    }
    for spec in group_specs {
        if let Some(id) = spec.get("request_group_id").and_then(Value::as_str) {
            result.entry(id.to_string()).or_default();
        }
    }
    result
}

/// `_struct_merge_doc_storage_group_batch`: grouped duplicate decisions WITH
/// merged payloads. Returns `old_id -> (separate docs, merged payload)`.
pub async fn struct_merge_doc_storage_group_batch(
    chat: &dyn crate::harness::HarnessChat,
    group_specs: &[Value],
) -> std::collections::HashMap<String, (Vec<Value>, Option<Value>)> {
    let identity = |specs: &[Value]| {
        specs
            .iter()
            .filter_map(|spec| {
                let id = spec.get("old_id").and_then(Value::as_str)?;
                let incoming = spec
                    .get("incoming_docs")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                Some((id.to_string(), (incoming, None)))
            })
            .collect::<std::collections::HashMap<_, _>>()
    };
    let mut prompt_groups: Vec<Value> = Vec::new();
    for spec in group_specs {
        let old_doc = spec.get("old_doc").cloned().unwrap_or(Value::Null);
        let incoming_docs = spec
            .get("incoming_docs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let Some(existing_payload) = struct_payload_of(&old_doc) else {
            continue;
        };
        let incoming_payloads: Vec<Value> =
            incoming_docs.iter().filter_map(struct_payload_of).collect();
        if incoming_payloads.len() != incoming_docs.len() {
            continue;
        }
        prompt_groups.push(serde_json::json!({
            "group_id": spec.get("old_id").cloned().unwrap_or(Value::Null),
            "existing": existing_payload,
            "incoming": incoming_payloads
                .iter()
                .enumerate()
                .map(|(index, payload)| serde_json::json!({"index": index, "item": payload}))
                .collect::<Vec<_>>(),
        }));
    }
    if prompt_groups.is_empty() {
        return identity(group_specs);
    }
    let groups_json = Value::Array(prompt_groups).to_string();
    let user_prompt = ES_GROUP_BATCH_MERGE_PROMPT.replace("{groups}", &groups_json);
    let prefix = ES_GROUP_BATCH_MERGE_PROMPT
        .split("Groups:")
        .next()
        .unwrap_or("");
    let system_prompt = format!("{}\n\n{prefix}", crate::merge::MERGE_SYSTEM_PROMPT);
    let Some(res) = struct_gen_json(chat, &system_prompt, &user_prompt).await else {
        return identity(group_specs);
    };
    let Some(raw_groups) = res.get("groups").and_then(Value::as_array) else {
        return identity(group_specs);
    };
    let by_id: std::collections::HashMap<String, &Value> = group_specs
        .iter()
        .filter_map(|spec| {
            spec.get("old_id")
                .and_then(Value::as_str)
                .map(|id| (id.to_string(), spec))
        })
        .collect();
    let mut result: std::collections::HashMap<String, (Vec<Value>, Option<Value>)> =
        std::collections::HashMap::new();
    for raw in raw_groups {
        let Some(old_id) = raw.get("group_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(spec) = by_id.get(old_id) else {
            continue;
        };
        let incoming_docs = spec
            .get("incoming_docs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let merged = raw.get("merged").cloned().filter(Value::is_object);
        let mut duplicate_indices: std::collections::HashSet<i64> =
            std::collections::HashSet::new();
        if let Some(decisions) = raw.get("decisions").and_then(Value::as_array) {
            for item in decisions {
                let duplicated = item
                    .get("duplicated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if let Some(index) = item.get("incoming_index").and_then(Value::as_i64)
                    && duplicated
                    && index >= 0
                    && (index as usize) < incoming_docs.len()
                {
                    duplicate_indices.insert(index);
                }
            }
        }
        if duplicate_indices.is_empty() || merged.is_none() {
            result.insert(old_id.to_string(), (incoming_docs, None));
            continue;
        }
        let separate: Vec<Value> = incoming_docs
            .iter()
            .enumerate()
            .filter(|(index, _)| !duplicate_indices.contains(&(*index as i64)))
            .map(|(_, doc)| doc.clone())
            .collect();
        result.insert(old_id.to_string(), (separate, merged));
    }
    for spec in group_specs {
        if let Some(id) = spec.get("old_id").and_then(Value::as_str) {
            result.entry(id.to_string()).or_insert_with(|| {
                let incoming = spec
                    .get("incoming_docs")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                (incoming, None)
            });
        }
    }
    result
}

/// `_struct_merge_pair` with the harness chat contract (shared merge prompts
/// from `crate::merge`).
pub async fn struct_merge_pair_chat(
    chat: &dyn crate::harness::HarnessChat,
    existing: &Value,
    incoming: &Value,
) -> Option<Value> {
    let existing_payload = struct_payload_of(existing)?;
    let incoming_payload = struct_payload_of(incoming)?;
    let user_prompt = crate::merge::MERGE_USER_PROMPT
        .replace("{item_existing}", &existing_payload.to_string())
        .replace("{item_incoming}", &incoming_payload.to_string());
    let system_prompt = format!(
        "{}\n\n{}",
        crate::merge::MERGE_SYSTEM_PROMPT,
        crate::merge::MERGE_DECISION_INSTRUCTION
    );
    let res = struct_gen_json(chat, &system_prompt, &user_prompt).await?;
    if !res
        .get("duplicated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let merged = res.get("merged").cloned().unwrap_or(Value::Null);
    if !merged.is_object() {
        return None;
    }
    Some(crate::merge::apply_merge_invariants(existing, merged))
}

/// `_struct_merge_doc_storage_group`: one candidate group (the single-incoming
/// case routes through the merge-pair prompt like upstream).
pub async fn struct_merge_doc_storage_group(
    chat: &dyn crate::harness::HarnessChat,
    old_doc: &Value,
    incoming_docs: &[Value],
) -> (Vec<Value>, Option<Value>) {
    if incoming_docs.len() == 1 {
        let merged = struct_merge_pair_chat(chat, old_doc, &incoming_docs[0]).await;
        return (
            if merged.is_some() {
                Vec::new()
            } else {
                incoming_docs.to_vec()
            },
            merged,
        );
    }
    let Some(existing_payload) = struct_payload_of(old_doc) else {
        return (incoming_docs.to_vec(), None);
    };
    let incoming_payloads: Vec<Value> =
        incoming_docs.iter().filter_map(struct_payload_of).collect();
    if incoming_payloads.len() != incoming_docs.len() {
        return (incoming_docs.to_vec(), None);
    }
    let system_prompt = format!(
        "{}\n\n{ES_GROUP_MERGE_PROMPT}",
        crate::merge::MERGE_SYSTEM_PROMPT
    );
    let user_prompt = ES_GROUP_MERGE_PROMPT
        .replace("{existing}", &existing_payload.to_string())
        .replace(
            "{incoming}",
            &Value::Array(
                incoming_payloads
                    .iter()
                    .enumerate()
                    .map(|(index, payload)| serde_json::json!({"index": index, "item": payload}))
                    .collect(),
            )
            .to_string(),
        );
    let Some(res) = struct_gen_json(chat, &system_prompt, &user_prompt).await else {
        return (incoming_docs.to_vec(), None);
    };
    let Some(indices) = res.get("duplicate_indices").and_then(Value::as_array) else {
        return (incoming_docs.to_vec(), None);
    };
    let merged = res.get("merged").cloned().unwrap_or(Value::Null);
    if !merged.is_object() {
        return (incoming_docs.to_vec(), None);
    }
    let mut duplicate_indices: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for index in indices {
        if let Some(value) = index.as_i64()
            && value >= 0
            && (value as usize) < incoming_docs.len()
        {
            duplicate_indices.insert(value);
        }
    }
    if duplicate_indices.is_empty() {
        return (incoming_docs.to_vec(), None);
    }
    let separate: Vec<Value> = incoming_docs
        .iter()
        .enumerate()
        .filter(|(index, _)| !duplicate_indices.contains(&(*index as i64)))
        .map(|(_, doc)| doc.clone())
        .collect();
    (separate, Some(merged))
}

// ── `structure.py` C2b (v0.3.10ah) — doc-storage dedup orchestrator ────────

#[derive(Default)]
struct DedupGroupState {
    separate: Vec<Value>,
    duplicate_docs: Vec<Value>,
    merged: Option<Value>,
    chunk_ids: Vec<String>,
    entity_aliases: std::collections::HashMap<String, String>,
}

/// `_struct_doc_storage_dedup_batch`: batch ES dedup — KNN candidates, grouped
/// LLM decisions, grouped merges, canonical-alias relation rewrite. Returns
/// `(inserted, updated)`.
///
/// Documented divergences: the nested batch stages run sequentially
/// (KNN/decisions/merges/embeddings/writes are each internally ordered; the
/// shared read-only KNN snapshot semantics are preserved); doc identity
/// comparisons use the row `id` instead of Python `id()`.
#[allow(clippy::too_many_arguments)]
pub async fn struct_doc_storage_dedup_batch(
    store: &dyn crate::doc_store::DocStore,
    chat: &dyn crate::harness::HarnessChat,
    embed: &dyn EmbeddingBackend,
    docs: &[Value],
    tenant_id: &str,
    kb_id: &str,
    similarity_threshold: f32,
    merge_scope_dataset: bool,
    cancel_check: &(dyn Fn() -> bool + Send + Sync),
) -> std::result::Result<(usize, usize), RunnerError> {
    let index = doc_store_index_name(tenant_id);
    let select_fields = struct_dedup_select_fields();
    let raise_if_canceled = || -> std::result::Result<(), RunnerError> {
        if cancel_check() {
            return Err(RunnerError::Cancelled);
        }
        Ok(())
    };
    raise_if_canceled()?;

    // Stage 1: KNN candidates (read-only snapshot per old row).
    let mut knn_results: Vec<(Value, Option<Value>)> = Vec::new();
    for doc in docs {
        raise_if_canceled()?;
        let candidate = struct_doc_storage_knn_candidate(
            store,
            doc,
            tenant_id,
            kb_id,
            similarity_threshold,
            merge_scope_dataset,
        );
        knn_results.push((doc.clone(), candidate));
    }
    raise_if_canceled()?;

    let mut inserts: Vec<Value> = Vec::new();
    let mut old_ids: Vec<String> = Vec::new();
    let mut old_docs: Vec<Value> = Vec::new();
    let mut incoming_lists: Vec<Vec<Value>> = Vec::new();
    for (doc, old_doc) in knn_results {
        let Some(old_doc) = old_doc else {
            inserts.push(doc);
            continue;
        };
        let old_id = string_field(&old_doc, "id");
        match old_ids.iter().position(|id| id == &old_id) {
            Some(position) => incoming_lists[position].push(doc),
            None => {
                old_ids.push(old_id);
                old_docs.push(old_doc);
                incoming_lists.push(vec![doc]);
            }
        }
    }

    let mut states: Vec<DedupGroupState> = (0..old_ids.len())
        .map(|index| DedupGroupState {
            chunk_ids: struct_union_chunk_ids(old_docs[index].get("source_chunk_ids"), None),
            ..DedupGroupState::default()
        })
        .collect();

    // Stage 2: grouped decisions (batched by incoming count).
    let mut decision_specs: Vec<Value> = Vec::new();
    for (index, old_id) in old_ids.iter().enumerate() {
        let incoming = &incoming_lists[index];
        for (part, start) in (0..incoming.len())
            .step_by(ES_DEDUP_LLM_BATCH_SIZE)
            .enumerate()
        {
            let end = (start + ES_DEDUP_LLM_BATCH_SIZE).min(incoming.len());
            decision_specs.push(serde_json::json!({
                "old_id": old_id,
                "request_group_id": format!("{old_id}:part-{part}"),
                "old_doc": old_docs[index],
                "incoming_docs": incoming[start..end].to_vec(),
            }));
        }
    }
    let mut decision_batches: Vec<Vec<Value>> = Vec::new();
    let mut current_batch: Vec<Value> = Vec::new();
    let mut current_size = 0usize;
    for spec in decision_specs {
        let size = spec
            .get("incoming_docs")
            .and_then(Value::as_array)
            .map(|items| items.len())
            .unwrap_or(0);
        if !current_batch.is_empty() && current_size + size > ES_DEDUP_LLM_BATCH_SIZE {
            decision_batches.push(std::mem::take(&mut current_batch));
            current_size = 0;
        }
        current_batch.push(spec);
        current_size += size;
    }
    if !current_batch.is_empty() {
        decision_batches.push(current_batch);
    }
    let mut decisions_by_group: std::collections::HashMap<String, std::collections::HashSet<i64>> =
        std::collections::HashMap::new();
    for batch in &decision_batches {
        raise_if_canceled()?;
        let result = struct_judge_doc_storage_group_batch(chat, batch).await;
        for (key, value) in result {
            decisions_by_group.entry(key).or_insert(value);
        }
    }
    raise_if_canceled()?;
    for batch in &decision_batches {
        for spec in batch {
            let Some(old_id) = spec.get("old_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(position) = old_ids.iter().position(|id| id == old_id) else {
                continue;
            };
            let request_id = spec
                .get("request_group_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let hits = decisions_by_group
                .get(request_id)
                .cloned()
                .unwrap_or_default();
            let incoming = spec
                .get("incoming_docs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for (incoming_index, doc) in incoming.into_iter().enumerate() {
                if hits.contains(&(incoming_index as i64)) {
                    states[position].duplicate_docs.push(doc);
                } else {
                    states[position].separate.push(doc);
                }
            }
        }
    }

    // Stage 3: per-group merges.
    for position in 0..old_ids.len() {
        raise_if_canceled()?;
        if states[position].duplicate_docs.is_empty() {
            continue;
        }
        let old_doc = old_docs[position].clone();
        let mut current_doc = old_doc.clone();
        let mut current_chunk_ids = states[position].chunk_ids.clone();
        let mut merged_payload: Option<Value> = None;
        let duplicate_docs = states[position].duplicate_docs.clone();
        for start in (0..duplicate_docs.len()).step_by(ES_DEDUP_LLM_BATCH_SIZE) {
            raise_if_canceled()?;
            let end = (start + ES_DEDUP_LLM_BATCH_SIZE).min(duplicate_docs.len());
            let candidate_docs = duplicate_docs[start..end].to_vec();
            let (separate, candidate_merged) =
                struct_merge_doc_storage_group(chat, &current_doc, &candidate_docs).await;
            let separate_ids: std::collections::HashSet<String> =
                separate.iter().map(|doc| string_field(doc, "id")).collect();
            states[position].separate.extend(separate);
            let Some(candidate_merged) = candidate_merged else {
                continue;
            };
            let candidate_merged =
                crate::merge::apply_merge_invariants(&current_doc, candidate_merged);
            if string_field(&old_doc, "knowledge_graph_kwd") == "entity" {
                let old_name = crate::dataset_structure_merger::struct_entity_name(&current_doc);
                let canonical_name = {
                    let candidate =
                        crate::dataset_structure_merger::struct_entity_name(&candidate_merged);
                    if candidate.is_empty() {
                        old_name.clone()
                    } else {
                        candidate
                    }
                };
                for candidate in &candidate_docs {
                    let candidate_name =
                        crate::dataset_structure_merger::struct_entity_name(candidate);
                    if !candidate_name.is_empty() && candidate_name != canonical_name {
                        states[position]
                            .entity_aliases
                            .insert(candidate_name, canonical_name.clone());
                    }
                }
                if !old_name.is_empty() && old_name != canonical_name {
                    states[position]
                        .entity_aliases
                        .insert(old_name, canonical_name);
                }
            }
            for candidate in &candidate_docs {
                if separate_ids.contains(&string_field(candidate, "id")) {
                    continue;
                }
                current_chunk_ids = struct_union_chunk_ids(
                    Some(&Value::Array(
                        current_chunk_ids
                            .iter()
                            .cloned()
                            .map(Value::String)
                            .collect(),
                    )),
                    candidate.get("source_chunk_ids"),
                );
            }
            if let Some(map) = current_doc.as_object_mut() {
                map.insert(
                    "content_with_weight".to_string(),
                    Value::String(candidate_merged.to_string()),
                );
                map.insert(
                    "source_chunk_ids".to_string(),
                    Value::Array(
                        current_chunk_ids
                            .iter()
                            .cloned()
                            .map(Value::String)
                            .collect(),
                    ),
                );
            }
            merged_payload = Some(candidate_merged);
        }
        if merged_payload.is_some() {
            states[position].merged = merged_payload;
            states[position].chunk_ids = current_chunk_ids;
        }
    }
    raise_if_canceled()?;

    // Separate docs join the insert list; merged groups are rebuilt with fresh
    // embeddings before the writes.
    let mut merged_jobs: Vec<(String, usize, Value, Vec<String>)> = Vec::new();
    for (position, state) in states.iter_mut().enumerate() {
        inserts.append(&mut state.separate);
        if state.merged.is_none() {
            continue;
        }
        merged_jobs.push((
            old_ids[position].clone(),
            position,
            state.merged.clone().unwrap_or(Value::Null),
            state.chunk_ids.clone(),
        ));
    }
    let mut rebuilt: Vec<(String, Value)> = Vec::new();
    for start in (0..merged_jobs.len()).step_by(ES_DEDUP_EMBED_BATCH_SIZE) {
        let end = (start + ES_DEDUP_EMBED_BATCH_SIZE).min(merged_jobs.len());
        let batch = &merged_jobs[start..end];
        let texts: Vec<String> = batch
            .iter()
            .map(|(_, _, payload, _)| payload_description(payload))
            .collect();
        let vectors = match encode(embed, &texts).await {
            Ok(vectors) => vectors,
            Err(_) => Vec::new(),
        };
        for (offset, (old_id, position, payload, chunk_ids)) in batch.iter().enumerate() {
            let Some(vector) = vectors.get(offset).cloned() else {
                continue;
            };
            let row = struct_rebuild_doc_storage_doc(
                payload,
                &old_docs[*position],
                vector,
                chunk_ids,
                true,
            );
            rebuilt.push((
                old_id.clone(),
                serde_json::to_value(&row).unwrap_or(Value::Null),
            ));
        }
    }

    let mut writes: Vec<Value> = inserts.clone();
    writes.extend(rebuilt.iter().map(|(_, row)| row.clone()));
    let mut inserted = 0usize;
    let mut updated = 0usize;
    let mut successful_entity_aliases: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for start in (0..writes.len()).step_by(ES_DEDUP_INSERT_BATCH_SIZE) {
        raise_if_canceled()?;
        let end = (start + ES_DEDUP_INSERT_BATCH_SIZE).min(writes.len());
        let batch = &writes[start..end];
        if batch.is_empty() {
            continue;
        }
        let rows: Vec<crate::doc_store::DocRow> = batch
            .iter()
            .filter_map(|row| row.as_object().cloned())
            .collect();
        if store.insert(&rows, &index, kb_id).is_ok() {
            let rebuilt_ids: std::collections::HashSet<String> =
                rebuilt.iter().map(|(old_id, _)| old_id.clone()).collect();
            for row in batch {
                let row_id = string_field(row, "id");
                if rebuilt_ids.contains(&row_id) {
                    updated += 1;
                    if let Some(position) = old_ids.iter().position(|id| id == &row_id)
                        && string_field(&old_docs[position], "knowledge_graph_kwd") == "entity"
                    {
                        successful_entity_aliases.extend(states[position].entity_aliases.clone());
                    }
                } else {
                    inserted += 1;
                }
            }
        }
    }

    // Canonical aliases publish only after the entity writes succeeded; every
    // relation that references an alias is rewritten in place (scope: the
    // old document, or the whole KB in dataset scope).
    let mut entity_aliases = successful_entity_aliases;
    let mut existing_relation_updates = 0usize;
    if !entity_aliases.is_empty() {
        let mut scopes: Vec<(Option<String>, String, Option<String>)> = Vec::new();
        for (position, old_doc) in old_docs.iter().enumerate() {
            if string_field(old_doc, "knowledge_graph_kwd") != "entity" {
                continue;
            }
            let doc_id = if merge_scope_dataset {
                None
            } else {
                let value = string_field(old_doc, "doc_id");
                if value.is_empty() { None } else { Some(value) }
            };
            let scope = (
                doc_id,
                string_field(old_doc, "compile_kwd"),
                doc_template_id(old_doc),
            );
            if !scopes.contains(&scope) {
                scopes.push(scope);
            }
            let _ = position;
        }
        for (doc_id, compile_kwd, template_id) in scopes {
            raise_if_canceled()?;
            let mut condition = serde_json::Map::new();
            condition.insert("compile_kwd".to_string(), serde_json::json!([compile_kwd]));
            condition.insert(
                "knowledge_graph_kwd".to_string(),
                serde_json::json!(["relation"]),
            );
            if let Some(doc_id) = &doc_id {
                condition.insert("doc_id".to_string(), serde_json::json!([doc_id]));
            }
            if let Some(template_id) = &template_id {
                condition.insert(
                    "compilation_template_ids".to_string(),
                    serde_json::json!([template_id]),
                );
            }
            let query = struct_store_query(
                &select_fields,
                &Value::Object(condition),
                Vec::new(),
                &index,
                kb_id,
                10000,
            );
            let Ok(response) = store.search(&query) else {
                continue;
            };
            let rows = store.get_fields(&response, &select_fields);
            let mut rewrite_batch: Vec<(Value, Value)> = Vec::new();
            for (row_id, row) in rows {
                let mut row_value = Value::Object(row);
                if let Some(map) = row_value.as_object_mut() {
                    map.insert("id".to_string(), Value::String(row_id.clone()));
                }
                let Some(mut payload) = parse_content_payload(&row_value) else {
                    continue;
                };
                if !struct_rewrite_relation_payload(&mut payload, &entity_aliases) {
                    continue;
                }
                let from_value = string_field(&row_value, "from_entity_kwd");
                let to_value = string_field(&row_value, "to_entity_kwd");
                if let Some(map) = row_value.as_object_mut() {
                    map.insert(
                        "content_with_weight".to_string(),
                        Value::String(payload.to_string()),
                    );
                    map.insert(
                        "from_entity_kwd".to_string(),
                        Value::String(struct_resolve_entity_alias(&from_value, &entity_aliases)),
                    );
                    map.insert(
                        "to_entity_kwd".to_string(),
                        Value::String(struct_resolve_entity_alias(&to_value, &entity_aliases)),
                    );
                }
                rewrite_batch.push((row_value, payload));
            }
            for start in (0..rewrite_batch.len()).step_by(ES_DEDUP_EMBED_BATCH_SIZE) {
                let end = (start + ES_DEDUP_EMBED_BATCH_SIZE).min(rewrite_batch.len());
                let batch = &rewrite_batch[start..end];
                let texts: Vec<String> = batch
                    .iter()
                    .map(|(_, payload)| payload_description(payload))
                    .collect();
                let Ok(vectors) = encode(embed, &texts).await else {
                    continue;
                };
                let mut rewritten: Vec<crate::doc_store::DocRow> = Vec::new();
                for (offset, (base, payload)) in batch.iter().enumerate() {
                    let Some(vector) = vectors.get(offset).cloned() else {
                        continue;
                    };
                    let chunk_ids = struct_union_chunk_ids(base.get("source_chunk_ids"), None);
                    let row =
                        struct_rebuild_doc_storage_doc(payload, base, vector, &chunk_ids, true);
                    if let Some(map) = serde_json::to_value(&row)
                        .ok()
                        .and_then(|v| v.as_object().cloned())
                    {
                        rewritten.push(map);
                    }
                }
                if !rewritten.is_empty() {
                    let _ = store.insert(&rewritten, &index, kb_id);
                    existing_relation_updates += rewritten.len();
                }
            }
        }
        // Incoming relation inserts and merged relation payloads referencing an
        // alias are rewritten too.
        let mut rewritten_inserts: Vec<Value> = Vec::new();
        for doc in &inserts {
            if string_field(doc, "knowledge_graph_kwd") == "relation" {
                rewritten_inserts
                    .push(struct_rewrite_relation_doc(doc, &entity_aliases, embed).await);
            } else {
                rewritten_inserts.push(doc.clone());
            }
        }
        if rewritten_inserts != inserts {
            let rows: Vec<crate::doc_store::DocRow> = rewritten_inserts
                .iter()
                .filter_map(|row| row.as_object().cloned())
                .collect();
            let _ = store.insert(&rows, &index, kb_id);
        }
        for (position, old_doc) in old_docs.iter().enumerate() {
            if string_field(old_doc, "knowledge_graph_kwd") != "relation" {
                continue;
            }
            let Some(mut payload) = states[position].merged.clone() else {
                continue;
            };
            if !struct_rewrite_relation_payload(&mut payload, &entity_aliases) {
                continue;
            }
            if let Some(vector) = struct_reembed_payload(&payload, embed).await {
                let row = struct_rebuild_doc_storage_doc(
                    &payload,
                    old_doc,
                    vector,
                    &states[position].chunk_ids,
                    true,
                );
                if let Some(map) = serde_json::to_value(&row)
                    .ok()
                    .and_then(|v| v.as_object().cloned())
                {
                    let _ = store.insert(&[map], &index, kb_id);
                }
            }
        }
        entity_aliases.clear();
    }

    Ok((inserted, updated + existing_relation_updates))
}

#[cfg(test)]
mod tests {
    use super::*;

    mod graphrag_extractor_alignment {
        use super::*;

        #[test]
        fn repair_json_text_recovers_non_strict_json() {
            // single quotes + trailing commas + unquoted keys + bare value
            let v = repair_json_text(
                "{'items': [{'name': 'Alice', type: person, 'aliases': ['A',],}]}",
            )
            .unwrap();
            let items = v.get("items").and_then(Value::as_array).unwrap();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0]["name"], "Alice");
            assert_eq!(items[0]["type"], "person");
            assert_eq!(items[0]["aliases"], json!(["A"]));
            // markdown fence + surrounding prose
            let v2 = repair_json_text(
                "Here you go:\n```json\n{\"items\": [{\"name\": \"Bob\"}]}\n```\nHope this helps",
            )
            .unwrap();
            assert_eq!(v2["items"][0]["name"], "Bob");
            // truncated object: missing closers are re-balanced
            let v3 = repair_json_text("{\"items\": [{\"name\": \"Carol\"}").unwrap();
            assert_eq!(v3["items"][0]["name"], "Carol");
            // bare multi-word value
            let v4 = repair_json_text("{items: [{name: John Smith, age: 30}]}").unwrap();
            assert_eq!(v4["items"][0]["name"], "John Smith");
            assert_eq!(v4["items"][0]["age"], 30);
            // strict JSON passes through parse_json_lenient untouched
            let v5 = parse_json_lenient("{\"a\": 1}").unwrap();
            assert_eq!(v5["a"], 1);
            // garbage → None
            assert!(repair_json_text("no json here").is_none());
        }

        #[test]
        fn parse_tuple_records_and_entities_and_relations_mirror_graph_extractor() {
            let raw = concat!(
                "(\"entity\"<|>\"Alice\"<|>\"person\"<|>\"Alice is a person.\")##",
                "(\"entity\"<|>\"Acme\"<|>\"organization\"<|>\"A company.\")##",
                "(\"entity\"<|>\"Dodge\"<|>\"vehicle\"<|>\"A car.\")##",
                "(\"relationship\"<|>\"Alice\"<|>\"Acme\"<|>\"works at\"<|>7)##",
                "(\"relationship\"<|>\"Acme\"<|>\"Alice\"<|>\"works at\"<|>notanumber)##",
                "this record has no parentheses at all##",
                "(\"relationship\"<|>\"Alice\"<|>\"Acme\"<|>\"late\"<|>2.5)<|COMPLETE|>"
            );
            let records =
                parse_tuple_records(raw, DEFAULT_RECORD_DELIMITER, DEFAULT_COMPLETION_DELIMITER);
            assert_eq!(records.len(), 6); // paren-less record dropped
            let (nodes, edges) = entities_and_relations(
                &records,
                DEFAULT_TUPLE_DELIMITER,
                &["person", "organization"],
                "chunk-1",
            );
            assert_eq!(nodes.len(), 2);
            assert_eq!(nodes[0]["entity_name"], "ALICE");
            assert_eq!(nodes[0]["entity_type"], "PERSON");
            assert_eq!(nodes[0]["source_id"], "chunk-1");
            // "vehicle" type filtered out; all 3 relation records kept
            assert_eq!(edges.len(), 3);
            let w: Vec<f64> = edges.iter().filter_map(|e| e["weight"].as_f64()).collect();
            assert!(w.contains(&7.0));
            assert!(w.contains(&1.0)); // non-numeric strength → default 1.0
            assert!(w.contains(&2.5));
            // src/tgt are the sorted-uppercased pair
            assert_eq!(edges[0]["src_id"], "ACME");
            assert_eq!(edges[0]["tgt_id"], "ALICE");
            // empty allow-list → DEFAULT_ENTITY_TYPES (still filters vehicle)
            let (n2, _) = extract_entity_relations_from_output(raw, &[], "chunk-1");
            assert_eq!(n2.len(), 2);
        }

        #[test]
        fn merge_entity_and_relation_records_mirror_extractor() {
            let ents = vec![
                json!({"entity_name": "ALICE", "entity_type": "PERSON", "description": "b desc", "source_id": "c1"}),
                json!({"entity_name": "ALICE", "entity_type": "PERSON", "description": "a desc", "source_id": "c2"}),
                json!({"entity_name": "ALICE", "entity_type": "CATEGORY", "description": "a desc", "source_id": "c2"}),
            ];
            let merged = merge_entity_records(&ents).unwrap();
            assert_eq!(merged["entity_type"], "PERSON"); // majority type
            assert_eq!(
                merged["description"],
                format!("a desc{GRAPH_FIELD_SEP}b desc")
            );
            assert_eq!(merged["source_id"], json!(["c1", "c2"]));
            assert_eq!(merged["entity_name"], "ALICE");

            let rels = vec![
                json!({"src_id": "A", "tgt_id": "B", "weight": 2.0, "description": "d2", "keywords": "k1", "source_id": "c1"}),
                json!({"src_id": "A", "tgt_id": "B", "weight": 3.5, "description": "d1", "keywords": ["k1", "k2"], "source_id": "c2"}),
            ];
            let edge = merge_relation_records(&rels).unwrap();
            assert_eq!(edge["weight"], 5.5);
            assert_eq!(edge["description"], format!("d1{GRAPH_FIELD_SEP}d2"));
            assert_eq!(edge["keywords"], json!(["k1", "k2"]));
            assert_eq!(edge["source_id"], json!(["c1", "c2"]));
            assert_eq!(
                most_common_value(&["a".into(), "b".into(), "a".into()]),
                Some("a".into())
            );
            assert_eq!(
                flat_uniq_list(&rels, "keywords"),
                vec!["k1".to_string(), "k2".to_string()]
            );
            assert!(merge_entity_records(&[]).is_none());
            assert!(merge_relation_records(&[]).is_none());
        }

        #[test]
        fn spacy_mapping_and_keyword_normalization_constants() {
            assert_eq!(spacy_to_app_entity_type("PERSON"), "person");
            assert_eq!(spacy_to_app_entity_type("ORG"), "organization");
            assert_eq!(spacy_to_app_entity_type("GPE"), "geo");
            assert_eq!(spacy_to_app_entity_type("LOC"), "geo");
            assert_eq!(spacy_to_app_entity_type("EVENT"), "event");
            assert_eq!(spacy_to_app_entity_type("QUANTITY"), "category");
            assert_eq!(spacy_to_app_entity_type("NOT_A_LABEL"), "category");
            assert!(has_uppercase("Hello"));
            assert!(!has_uppercase("hello"));
            assert_eq!(replace_word("New - York"), "New-York");
            assert_eq!(replace_word("cat 's"), "cat's");
            assert_eq!(replace_word("A - B"), "A-B");
            // missing graphrag constants now present
            assert_eq!(MAX_CONCURRENT_PROCESS_AND_EXTRACT_CHUNK, 10);
            assert_eq!(GRAPHRAG_MAX_ERRORS, 3);
            assert_eq!(ENTITY_SUMMARY_MAX_TOKENS, 512);
            assert_eq!(ENTITY_SUMMARY_DESCRIPTION_LIST_LIMIT, 12);
            assert_eq!(ENTITY_EXTRACTION_MAX_GLEANINGS, 2);
            assert_eq!(GRAPH_FIELD_SEP, "<SEP>");
            assert_eq!(DEFAULT_TUPLE_DELIMITER, "<|>");
            assert_eq!(DEFAULT_RECORD_DELIMITER, "##");
            assert_eq!(DEFAULT_COMPLETION_DELIMITER, "<|COMPLETE|>");
            assert!(DEFAULT_ENTITY_TYPES.contains(&"person"));
            assert!(is_float_regex("7"));
            assert!(is_float_regex("-3.5"));
            assert!(is_float_regex("10.5"));
            assert!(!is_float_regex("10.")); // regex requires a trailing digit
            assert!(!is_float_regex("abc"));
        }

        #[tokio::test]
        async fn upsert_row_by_id_implements_if_not_exists_semantics() {
            let store = MemoryCompileStore::default();
            let row = to_es_doc(
                &json!({"name": "A", "description": "desc"}),
                "hypergraph",
                "doc-1",
                &["c1".to_string()],
                vec![],
                "entity",
                None,
                None,
                None,
                None,
            );
            assert_eq!(
                upsert_row_by_id(&store, "kb-1", &row).await.unwrap(),
                PersistOutcome::Inserted
            );
            // same stable id → update, not a second row
            assert_eq!(
                upsert_row_by_id(&store, "kb-1", &row).await.unwrap(),
                PersistOutcome::Updated
            );
            let rows = store
                .search("kb-1", &CompileFilter::default(), 100)
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
            // different kb → independent insert
            assert_eq!(
                upsert_row_by_id(&store, "kb-2", &row).await.unwrap(),
                PersistOutcome::Inserted
            );
            let rows = store
                .search("kb-2", &CompileFilter::default(), 100)
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
        }
    }

    fn sample_config() -> Value {
        json!({
            "guideline": {
                "target": "Extract the main entities and relations.",
                "rules_for_entities": "Entities are people, organizations, locations.",
                "rules_for_relations": "Relations link two known entities.",
                "rules_for_time": "Time: {observation_time}"
            },
            "entity": {
                "description": "A named entity.",
                "fields": [
                    {"type": "person", "description": "A person"},
                    {"type": "org", "description": "An org"}
                ]
            },
            "relation": {
                "description": "A relation.",
                "fields": [
                    {"type": "works_at", "description": "works at"}
                ]
            }
        })
    }

    #[test]
    fn infer_type_explicit_and_kind_normalization() {
        assert_eq!(infer_type(&json!({"compile_type": "set"})), "set");
        assert_eq!(infer_type(&json!({"kind": "pageIndex"})), "timeline");
        assert_eq!(infer_type(&json!({"kind": "knowledge_graph"})), "timeline");
        assert_eq!(
            infer_type(&json!({"output": {"entities": {}, "relations": {}}})),
            "hypergraph"
        );
        assert_eq!(infer_type(&json!({})), "list");
    }

    #[test]
    fn localize_supports_language_and_lists() {
        let v = json!({"en": "Hello", "zh": "你好"});
        assert_eq!(localize(&v, "zh"), "你好");
        assert_eq!(localize(&v, "fr"), "Hello");
        assert_eq!(localize(&json!(["a", "b"]), "en"), "1. a\n2. b");
        assert_eq!(localize(&Value::Null, "en"), "");
    }

    #[test]
    fn render_fields_builds_skeleton() {
        let fields = vec![
            json!({"name": "name", "type": "str", "description": "The name"}),
            json!({"name": "tags", "type": "list", "description": "Tags", "required": false}),
        ];
        let (lines, skeleton) = render_fields(&fields, "en");
        assert!(lines.contains("- name (str, required): The name"));
        assert!(lines.contains("- tags (list, optional): Tags"));
        assert!(skeleton.contains("\"name\": <string>"));
        assert!(skeleton.contains("\"tags\": [<string>, ...]"));
    }

    #[test]
    fn hypergraph_prompts_render_both_stages() {
        let (node, edge) = hypergraph_prompts(&sample_config(), "en");
        assert!(node.contains("## Entity Fields:"));
        assert!(node.contains("{\"items\":"));
        assert!(edge.contains("## Known Entities:\n{known_nodes}"));
        assert!(edge.contains("Only create relations between entities listed"));
    }

    #[test]
    fn payload_description_flattens_lists() {
        let p = json!({"name": "A", "tags": ["x", "y"], "description": "d"});
        let desc = payload_description(&p);
        assert!(desc.contains("A"));
        assert!(desc.contains("x"));
        assert!(desc.contains("y"));
        assert!(desc.contains("d"));
    }

    #[test]
    fn graph_entity_and_relation_shapes() {
        let e = graph_entity(
            &json!({"name": "A", "type": "person", "aliases": ["a1"]}),
            Some(&["c1".to_string()]),
        )
        .unwrap();
        assert_eq!(e["mention_count"], 1);
        assert_eq!(e["source_chunk_ids"], json!(["c1"]));
        assert_eq!(e["discription"], "");
        let r = graph_relation(&json!({"source": "A", "target": "B", "type": "x"})).unwrap();
        assert_eq!(r["from"], "A");
        assert_eq!(r["to"], "B");
        assert!(graph_relation(&json!({"source": "A"})).is_none());
    }

    #[test]
    fn merge_graph_entities_unions_by_name_type() {
        let ents = vec![
            json!({"name": "A", "type": "p", "mention_count": 1, "aliases": [], "source_chunk_ids": ["c1"], "discription": ""}),
            json!({"name": "A", "type": "p", "mention_count": 2, "aliases": ["a1"], "source_chunk_ids": ["c2"], "discription": "d"}),
        ];
        let merged = merge_graph_entities(&ents);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0]["mention_count"], 3);
        assert_eq!(merged[0]["source_chunk_ids"], json!(["c1", "c2"]));
        assert_eq!(merged[0]["discription"], "d");
    }

    #[test]
    fn to_es_doc_builds_stable_rows() {
        let row = to_es_doc(
            &json!({"name": "A", "description": "desc"}),
            "list",
            "doc-1",
            &["c1".to_string()],
            vec![1.0, 0.0],
            "entity",
            None,
            None,
            Some("tpl-1"),
            Some("list"),
        );
        assert_eq!(row.compile_kwd, "list");
        assert_eq!(row.knowledge_graph_kwd, "entity");
        assert_eq!(row.source_chunk_ids, vec!["c1"]);
        assert_eq!(row.compilation_template_ids, vec!["tpl-1"]);
        assert_eq!(row.compilation_template_kind_kwd.as_deref(), Some("list"));
        assert!(!row.id.is_empty());
        // Stable: same inputs → same id.
        let row2 = to_es_doc(
            &json!({"name": "A", "description": "desc"}),
            "list",
            "doc-1",
            &["c1".to_string()],
            vec![1.0, 0.0],
            "entity",
            None,
            None,
            Some("tpl-1"),
            Some("list"),
        );
        assert_eq!(row.id, row2.id);
    }

    #[test]
    fn filter_key_includes_template() {
        let a = to_es_doc(
            &json!({"name": "A"}),
            "list",
            "d",
            &[],
            vec![],
            "entity",
            None,
            None,
            Some("t1"),
            None,
        );
        let b = to_es_doc(
            &json!({"name": "A"}),
            "list",
            "d",
            &[],
            vec![],
            "entity",
            None,
            None,
            Some("t2"),
            None,
        );
        assert_ne!(filter_key(&a), filter_key(&b));
    }

    #[test]
    fn build_chunk_batches_split_mode() {
        let chunks: Vec<ChunkInput> = (0..6)
            .map(|i| ChunkInput {
                id: format!("c{i}"),
                text: "word ".repeat(50),
            })
            .collect();
        let (batches, info) = build_chunk_batches(&chunks, 1000, 100, None, None, None, 1024);
        assert_eq!(info.total, 6);
        assert_eq!(info.kept, 6);
        assert_eq!(info.skipped_empty, 0);
        assert!(info.input_budget >= 1024);
        // 50 words ≈ 50 tokens each; budget floor 1024 → all fit in ≤2 batches.
        assert!(info.n_batches >= 1 && info.n_batches <= 2);
        assert!(batches.iter().all(|b| !b.is_empty()));
        // Labels are per-batch positional.
        assert_eq!(batches[0][0].label, "C1");
    }

    #[test]
    fn build_chunk_batches_greedy_mode() {
        let chunks: Vec<ChunkInput> = (0..10)
            .map(|i| ChunkInput {
                id: format!("c{i}"),
                text: "x".repeat(100),
            })
            .collect();
        let (batches, info) = build_chunk_batches(&chunks, 100_000, 0, None, Some(8), None, 1024);
        assert_eq!(info.n_batches, 2); // 10 items / cap 8 → 2 batches
        assert_eq!(batches[0].len(), 8);
        assert_eq!(batches[1].len(), 2);
    }

    #[test]
    fn build_chunk_batches_skips_resume_and_empty() {
        let chunks = vec![
            ChunkInput {
                id: "c0".into(),
                text: "text".into(),
            },
            ChunkInput {
                id: "c1".into(),
                text: "   ".into(),
            },
            ChunkInput {
                id: "c2".into(),
                text: "more".into(),
            },
        ];
        let resume: HashSet<String> = ["c2".into()].into_iter().collect();
        let (batches, info) =
            build_chunk_batches(&chunks, 1000, 0, Some(&resume), None, None, 1024);
        assert_eq!(info.kept, 1);
        assert_eq!(info.skipped_resume, 1);
        assert_eq!(info.skipped_empty, 1);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0][0].chunk_id, "c0");
    }

    #[test]
    fn stable_row_id_is_stable_and_distinct() {
        let a = stable_row_id(&["x".into(), "y".into()]);
        let b = stable_row_id(&["x".into(), "y".into()]);
        let c = stable_row_id(&["x".into(), "z".into()]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn tokenize_for_search_handles_empty() {
        assert_eq!(tokenize_for_search(""), (vec![], vec![]));
        assert_eq!(tokenize_for_search("  "), (vec![], vec![]));
        let (ltks, sm) = tokenize_for_search("hello world");
        assert_eq!(ltks, vec!["hello", "world"]);
        assert_eq!(sm, ltks);
    }

    #[test]
    fn chain_detect_violations_covers_all_four() {
        let edges = vec![
            ("a".to_string(), "a".to_string()), // self-loop
            ("a".to_string(), "b".to_string()), // fan-out (a)
            ("a".to_string(), "c".to_string()),
            ("d".to_string(), "b".to_string()), // fan-in (b)
            ("x".to_string(), "y".to_string()), // cycle x→y→x
            ("y".to_string(), "x".to_string()),
        ];
        let issues = chain_detect_violations(&edges);
        // All six edges carry at least one issue: (a,a) self-loop+fan-out,
        // (a,b) fan-out+fan-in, (a,c) fan-out, (d,b) fan-in, (x,y)/(y,x) cycle.
        assert_eq!(issues.len(), 6);
        assert!(issues[&("a".into(), "a".into())].contains(&"self-loop".to_string()));
        assert!(
            issues[&("a".into(), "b".into())]
                .iter()
                .any(|s| s.contains("fan-out"))
        );
        assert!(
            issues[&("d".into(), "b".into())]
                .iter()
                .any(|s| s.contains("fan-in"))
        );
        assert!(
            issues[&("x".into(), "y".into())]
                .iter()
                .any(|s| s.contains("cycle"))
        );
        assert!(
            issues[&("y".into(), "x".into())]
                .iter()
                .any(|s| s.contains("cycle"))
        );
    }

    #[test]
    fn chain_clean_edges_have_no_issues() {
        let edges = vec![
            ("a".to_string(), "b".to_string()),
            ("b".to_string(), "c".to_string()),
            ("c".to_string(), "d".to_string()),
        ];
        let issues = chain_detect_violations(&edges);
        assert!(issues.is_empty());
    }

    #[test]
    fn tarjan_scc_finds_cycles() {
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        adj.insert("x".into(), vec!["y".into()]);
        adj.insert("y".into(), vec!["x".into(), "z".into()]);
        adj.insert("z".into(), vec![]);
        let sccs = tarjan_scc_iterative(&adj);
        assert!(sccs.iter().any(|c| c.contains("x") && c.contains("y")));
    }

    #[test]
    fn chain_gather_caps_and_dedups() {
        let bad_docs = vec![json!({
            "id": "r1",
            "source_chunk_ids": ["c1", "c2", "c1"],
        })];
        let mut map = HashMap::new();
        map.insert("c1".into(), "text1".into());
        map.insert("c2".into(), "text2".into());
        let pairs = chain_gather_chunk_text(&bad_docs, &map);
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn exact_dedup_groups_by_normalized_key() {
        let items = vec![
            json!({"name": "Apple", "mention_count": 2, "aliases": [], "chunk_ids": ["c1"]}),
            json!({"name": "apple", "mention_count": 3, "aliases": ["APPLE Inc"], "chunk_ids": ["c2"]}),
        ];
        let out = exact_dedup_by_key(&items, "name", None, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["name"], "Apple");
        assert_eq!(out[0]["mention_count"], 5);
        assert!(
            out[0]["aliases"]
                .as_array()
                .unwrap()
                .contains(&json!("APPLE Inc"))
        );
        assert_eq!(out[0]["chunk_ids"], json!(["c1", "c2"]));
    }

    #[test]
    fn memory_store_roundtrip_and_knn() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = MemoryCompileStore::new();
            let row = to_es_doc(
                &json!({"name": "A"}),
                "list",
                "doc-1",
                &["c1".into()],
                vec![1.0, 0.0, 0.0],
                "entity",
                None,
                None,
                None,
                None,
            );
            store.insert("kb1", &[row.clone()]).await.unwrap();
            let found = store.get("kb1", &row.id).await.unwrap();
            assert!(found.is_some());
            let filter = row.filter_of();
            let knn = store
                .search_knn("kb1", &filter, &[0.99, 0.01, 0.0], 1)
                .await
                .unwrap();
            assert_eq!(knn.len(), 1);
            // Different kb is isolated.
            let other = store
                .search_knn("kb2", &filter, &[1.0, 0.0, 0.0], 1)
                .await
                .unwrap();
            assert!(other.is_empty());
            // update
            store
                .update("kb1", &row.id, &json!({"from_entity_kwd": "X"}))
                .await
                .unwrap();
            let after = store.get("kb1", &row.id).await.unwrap().unwrap();
            assert_eq!(after["from_entity_kwd"], "X");
        });
    }

    #[test]
    fn json_store_roundtrip_and_delete_document() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let store = JsonFileCompileStore::new(dir.path()).unwrap();
            let row = to_es_doc(
                &json!({"name": "A"}),
                "list",
                "doc-1",
                &["c1".into()],
                vec![1.0, 0.0],
                "entity",
                None,
                None,
                None,
                None,
            );
            store.insert("kb1", &[row.clone()]).await.unwrap();
            let found = store.get("kb1", &row.id).await.unwrap();
            assert!(found.is_some());
            store.delete_document("kb1", "doc-1").await.unwrap();
            let gone = store.get("kb1", &row.id).await.unwrap();
            assert!(gone.is_none());
            // Graph JSON upsert is id-stable.
            let graph = json!({"entities": [], "relations": []});
            store
                .upsert_graph_json("kb1", &graph, "list", "doc-1", Some("tpl"))
                .await
                .unwrap();
            let g = store
                .get("kb1", &graph_row_id("doc-1", "list", Some("tpl")))
                .await
                .unwrap();
            assert!(g.is_some());
            assert_eq!(g.unwrap()["knowledge_graph_kwd"], "graph");
        });
    }

    #[test]
    fn rebuild_es_doc_preserves_identity() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let base = to_es_doc(
                &json!({"source": "A", "target": "B", "description": "old"}),
                "list",
                "doc-1",
                &["c1".into()],
                vec![1.0, 0.0],
                "relation",
                Some("source"),
                Some("target"),
                None,
                None,
            );
            let base_value = serde_json::to_value(&base).unwrap();
            let rebuilt = rebuild_es_doc(
                &json!({"source": "A", "target": "B", "description": "new"}),
                &base_value,
                vec![0.9, 0.1],
                &["c1".into(), "c2".into()],
                true,
                None,
                None,
            );
            assert_eq!(rebuilt.id, base.id);
            assert_eq!(rebuilt.from_entity_kwd, base.from_entity_kwd);
            assert_eq!(rebuilt.to_entity_kwd, base.to_entity_kwd);
            assert_eq!(rebuilt.source_chunk_ids, vec!["c1", "c2"]);
        });
    }

    #[test]
    fn phase_marker_constants_and_key_format() {
        assert_eq!(PHASE_RESOLUTION, "resolution_done");
        assert_eq!(PHASE_COMMUNITY, "community_done");
        assert_eq!(ALL_PHASES, [PHASE_RESOLUTION, PHASE_COMMUNITY]);
        assert_eq!(PHASE_MARKER_DEFAULT_TTL_SECONDS, 7 * 24 * 3600);
        assert_eq!(
            phase_marker_key("kb-1", PHASE_RESOLUTION),
            "graphrag:phase:kb-1:resolution_done"
        );
        assert_eq!(
            phase_marker_key("kb-1", PHASE_COMMUNITY),
            "graphrag:phase:kb-1:community_done"
        );
        // KB-scoped: distinct KBs never collide.
        assert_ne!(
            phase_marker_key("kb-1", PHASE_RESOLUTION),
            phase_marker_key("kb-2", PHASE_RESOLUTION)
        );
        assert_eq!(NER_SKIP_SPACY_LABELS, ["ORDINAL", "CARDINAL"]);
    }

    #[test]
    fn entity_name_similarity_matches_ragflow_gate() {
        // English edit-distance gate: distance <= min(len)//2.
        assert!(entity_name_similarity("Acme", "Acme"));
        assert!(entity_name_similarity("Apple", "apple")); // distance 1 <= 2
        assert!(entity_name_similarity("abc", "abd")); // distance 1 <= 1
        assert!(!entity_name_similarity("Acme", "Zenith")); // distance > 2
        assert!(!entity_name_similarity("computer", "phone")); // distance > 2
        // Digit-bearing 2-gram symmetric difference short-circuits to false.
        assert!(!entity_name_similarity("version2", "version3"));
        assert!(!entity_name_similarity("iPhone 15", "iPhone 16"));
        // Non-English character-set overlap path.
        assert!(entity_name_similarity("北京", "北京市")); // max set 3 < 4, overlap 2 > 1
        assert!(!entity_name_similarity("北京", "上海")); // overlap 0
        assert!(entity_name_similarity("阿里巴巴", "阿里巴巴"));
        assert!(!entity_name_similarity("阿里巴巴", "阿里巴巴集团")); // 3/5 = 0.6 < 0.8
        // is_english / levenshtein primitives.
        assert!(text_is_english("hello world"));
        assert!(!text_is_english("你好"));
        assert!(!text_is_english("hello你好")); // 5/7 < 0.8
        assert_eq!(levenshtein_distance("kitten", "sitting"), 3);
        assert_eq!(levenshtein_distance("", "abc"), 3);
    }

    #[test]
    fn entity_resolution_candidates_and_result_parsing() {
        let nodes = vec![
            ("Apple".to_string(), Some("org")),
            ("apple".to_string(), Some("org")),
            ("Microsoft".to_string(), Some("org")),
            ("北京".to_string(), Some("geo")),
            ("北京市".to_string(), Some("geo")),
            ("上海".to_string(), Some("geo")),
        ];
        let subgraph: HashSet<String> = ["Apple".into(), "北京".into()].into_iter().collect();
        let pairs = entity_resolution_candidates(&nodes, &subgraph);
        // Same-type unordered pairs, at least one endpoint in the subgraph,
        // passing the similarity gate.  Entity types iterate in sorted order
        // ("geo" < "org"), matching RAGFlow's sorted entity_types dict.
        assert_eq!(
            pairs,
            vec![
                ("北京".to_string(), "北京市".to_string()),
                ("Apple".to_string(), "apple".to_string()),
            ]
        );

        // _process_results: only in-range "yes" records survive.
        let output = "(For question <|>1<|>, &&no&&, different.)##\
                      (For question <|>2<|>, &&yes&&, same.)##\
                      (For question <|>3<|>, &&Yes&&, same.)##\
                      (For question <|>99<|>, &&yes&&, out of range.)";
        let indices = parse_resolution_results(
            output,
            3,
            DEFAULT_RECORD_DELIMITER,
            DEFAULT_ENTITY_INDEX_DELIMITER,
            DEFAULT_RESOLUTION_RESULT_DELIMITER,
        );
        assert_eq!(indices, vec![2, 3]);
        // Malformed / empty output yields no merges.
        assert!(parse_resolution_results("no delimiters here", 5, "##", "<|>", "&&").is_empty());
    }
}
#[cfg(test)]
mod common_helpers_tests {
    use super::*;

    fn set_env(key: &str, value: &str) {
        // SAFETY: tests use unique variable names and do not rely on the
        // process environment otherwise.
        unsafe { std::env::set_var(key, value) }
    }

    fn clear_env(key: &str) {
        unsafe { std::env::remove_var(key) }
    }
    use crate::doc_store::{DocRow, DocStore, FilterCondition, SearchQuery, SearchResponse};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    #[test]
    fn env_int_and_float_read_and_clamp() {
        set_env("RAYRAG_TEST_COMMON_INT", "12");
        assert_eq!(env_int("RAYRAG_TEST_COMMON_INT", 5, None), 12);
        set_env("RAYRAG_TEST_COMMON_INT", "2");
        assert_eq!(env_int("RAYRAG_TEST_COMMON_INT", 5, Some(4)), 4, "floored");
        set_env("RAYRAG_TEST_COMMON_INT", "not-a-number");
        assert_eq!(
            env_int("RAYRAG_TEST_COMMON_INT", 5, None),
            5,
            "invalid -> default"
        );
        set_env("RAYRAG_TEST_COMMON_INT", "  ");
        assert_eq!(
            env_int("RAYRAG_TEST_COMMON_INT", 5, None),
            5,
            "blank -> default"
        );
        clear_env("RAYRAG_TEST_COMMON_INT");
        assert_eq!(env_int("RAYRAG_TEST_COMMON_INT", 7, None), 7);

        set_env("RAYRAG_TEST_COMMON_FLOAT", "0.25");
        assert_eq!(env_float("RAYRAG_TEST_COMMON_FLOAT", 0.5, None, None), 0.25);
        set_env("RAYRAG_TEST_COMMON_FLOAT", "0.01");
        assert_eq!(
            env_float("RAYRAG_TEST_COMMON_FLOAT", 0.5, Some(0.1), None),
            0.1
        );
        set_env("RAYRAG_TEST_COMMON_FLOAT", "2.0");
        assert_eq!(
            env_float("RAYRAG_TEST_COMMON_FLOAT", 0.5, None, Some(1.0)),
            1.0
        );
        set_env("RAYRAG_TEST_COMMON_FLOAT", "nan");
        assert_eq!(
            env_float("RAYRAG_TEST_COMMON_FLOAT", 0.5, None, None),
            0.5,
            "non-finite -> default"
        );
        clear_env("RAYRAG_TEST_COMMON_FLOAT");
    }

    #[test]
    fn gen_conf_model_branches() {
        let deepseek = knowledge_compile_gen_conf("deepseek-v4-flash", None);
        assert_eq!(deepseek["max_completion_tokens"], json!(32768));
        assert_eq!(
            deepseek["extra_body"]["thinking"]["type"],
            json!("disabled")
        );

        let qwen = knowledge_compile_gen_conf("Qwen3-32B", None);
        assert_eq!(qwen["enable_thinking"], json!(false));
        let preview = knowledge_compile_gen_conf("qwen3.8-max-preview", None);
        assert_eq!(preview["enable_thinking"], json!(true));

        let other = knowledge_compile_gen_conf("gpt-4o", None);
        assert_eq!(other["reasoning_effort"], json!("none"));

        let mut base = serde_json::Map::new();
        base.insert("temperature".to_string(), json!(0.1));
        let kept = knowledge_compile_gen_conf("gpt-4o", Some(&base));
        assert_eq!(kept["temperature"], json!(0.1));
    }

    struct MockEmbed;

    #[async_trait]
    impl EmbeddingBackend for MockEmbed {
        async fn encode(&self, texts: &[String]) -> std::result::Result<Vec<Vec<f32>>, String> {
            Ok(texts.iter().map(|_| vec![0.1, 0.2]).collect())
        }
    }

    struct FailingEmbed;

    #[async_trait]
    impl EmbeddingBackend for FailingEmbed {
        async fn encode(&self, _texts: &[String]) -> std::result::Result<Vec<Vec<f32>>, String> {
            Err("boom".to_string())
        }
    }

    #[tokio::test]
    async fn encode_seam_contract() {
        assert!(encode(&MockEmbed, &[]).await.unwrap().is_empty());
        let vectors = encode(&MockEmbed, &["a".to_string(), "b".to_string()])
            .await
            .unwrap();
        assert_eq!(vectors.len(), 2);
        assert!(encode(&FailingEmbed, &["a".to_string()]).await.is_err());
    }

    #[test]
    fn input_budget_math() {
        let budget = make_input_budget(10_000, &[""], 1024, 0.5);
        assert_eq!(budget, 5000, "empty prompts leave half the context");
        let floored = make_input_budget(100, &[""], 1024, 0.5);
        assert_eq!(floored, 1024);
        let overhead = make_input_budget(10_000, &["hello world"], 0, 0.5);
        assert!(overhead < 5000);
    }

    #[test]
    fn index_name_matches_upstream() {
        assert_eq!(doc_store_index_name("t1"), "ragflow_t1");
    }

    #[derive(Default)]
    struct MockStore {
        inserted: Mutex<usize>,
        deleted: Mutex<usize>,
        rows: Mutex<Vec<DocRow>>,
    }

    impl DocStore for MockStore {
        fn db_type(&self) -> &'static str {
            "memory"
        }
        fn health(&self) -> crate::Result<crate::doc_store::HealthStatus> {
            Ok(crate::doc_store::HealthStatus::green("test"))
        }
        fn create_idx(
            &self,
            _index_name: &str,
            _dataset_id: &str,
            _vector_size: usize,
        ) -> crate::Result<()> {
            Ok(())
        }
        fn delete_idx(&self, _index_name: &str, _dataset_id: &str) -> crate::Result<()> {
            Ok(())
        }
        fn index_exist(&self, _index_name: &str, _dataset_id: &str) -> crate::Result<bool> {
            Ok(true)
        }
        fn insert(
            &self,
            rows: &[DocRow],
            _index_name: &str,
            _dataset_id: &str,
        ) -> crate::Result<Vec<String>> {
            *self.inserted.lock().unwrap() += rows.len();
            self.rows.lock().unwrap().extend(rows.iter().cloned());
            Ok(rows
                .iter()
                .filter_map(|row| row.get("id").and_then(|v| v.as_str()).map(str::to_string))
                .collect())
        }
        fn get(
            &self,
            _data_id: &str,
            _index_name: &str,
            _dataset_ids: &[String],
        ) -> crate::Result<Option<DocRow>> {
            Ok(None)
        }
        fn update(
            &self,
            _condition: &FilterCondition,
            _new_value: &DocRow,
            _index_name: &str,
            _dataset_id: &str,
        ) -> crate::Result<bool> {
            Ok(false)
        }
        fn delete(
            &self,
            _condition: &FilterCondition,
            _index_name: &str,
            _dataset_id: &str,
        ) -> crate::Result<usize> {
            *self.deleted.lock().unwrap() += 1;
            Ok(1)
        }
        fn search(&self, _query: &SearchQuery) -> crate::Result<SearchResponse> {
            let rows = self.rows.lock().unwrap().clone();
            Ok(SearchResponse {
                total: rows.len(),
                docs: rows,
                ..SearchResponse::default()
            })
        }
        fn sql(&self, _sql: &str, _fetch_size: usize) -> crate::Result<Vec<Value>> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn doc_storage_wrappers_roundtrip() {
        let store = MockStore::default();
        let row: DocRow = [
            ("id".to_string(), json!("r1")),
            ("name".to_string(), json!("Alpha")),
        ]
        .into_iter()
        .collect();
        doc_storage_insert(&store, std::slice::from_ref(&row), "ragflow_t1", "kb1").await;
        let fields = vec!["id".to_string(), "name".to_string()];
        let found = doc_storage_search(
            &store,
            SearchQuery {
                select_fields: fields.clone(),
                dataset_ids: vec!["kb1".to_string()],
                ..SearchQuery::default()
            },
            &fields,
        )
        .await;
        assert_eq!(
            found.get("r1").and_then(|r| r.get("name")),
            Some(&json!("Alpha"))
        );

        let condition: FilterCondition = [("id".to_string(), json!("r1"))].into_iter().collect();
        doc_storage_upsert_one(&store, &condition, &row, "t1", "kb1").await;
        assert_eq!(*store.deleted.lock().unwrap(), 1);
        assert_eq!(
            *store.inserted.lock().unwrap(),
            2,
            "one direct + one upsert insert"
        );
    }
}
#[cfg(test)]
mod runner_tests {
    use super::*;
    use serde_json::json;

    #[derive(Default)]
    struct MockBackend {
        templates: std::collections::HashMap<String, Value>,
        compile_calls: std::sync::Mutex<Vec<String>>,
        merges: std::sync::Mutex<Vec<String>>,
        dataset_rebuilds: std::sync::Mutex<Vec<String>>,
        nav_upserts: std::sync::Mutex<usize>,
        timeline_cleanups: std::sync::Mutex<usize>,
        plans: std::sync::Mutex<usize>,
        refines: std::sync::Mutex<usize>,
        docs_per_batch: usize,
    }

    #[async_trait::async_trait]
    impl StructureCompileBackend for MockBackend {
        fn resolve_template_ids_from_group(&self, group_id: &str, _tenant_id: &str) -> Vec<String> {
            vec![format!("{group_id}-a"), format!("{group_id}-b")]
        }
        fn saved_template(&self, template_id: &str, _tenant_id: &str) -> Option<Value> {
            self.templates.get(template_id).cloned()
        }
        async fn compile_structure_from_text(
            &self,
            batch: &[Value],
            _parser_cfg: &Value,
            _doc_id: &str,
            _doc_name: &str,
            _language: &str,
            compilation_template_id: &str,
        ) -> std::result::Result<Vec<Value>, String> {
            self.compile_calls
                .lock()
                .unwrap()
                .push(compilation_template_id.to_string());
            Ok((0..self.docs_per_batch.max(batch.len()))
                .map(|index| json!({"id": format!("{compilation_template_id}-{index}")}))
                .collect())
        }
        async fn merge_compiled_structures(
            &self,
            request: MergeFlushRequest<'_>,
        ) -> std::result::Result<Value, String> {
            self.merges
                .lock()
                .unwrap()
                .push(request.compilation_template_id.to_string());
            Ok(json!({
                "inserted": request.docs.len(),
                "updated": 0,
                "duplicates_dropped": 0,
                "compile_kwds": ["list"],
            }))
        }
        async fn rebuild_structure_graph_json(
            &self,
            _tenant_id: &str,
            _kb_id: &str,
            _doc_id: &str,
            _doc_name: &str,
            compile_kwd: &str,
            _compilation_template_id: &str,
        ) -> std::result::Result<Value, String> {
            if compile_kwd == "page_index" {
                return Ok(json!({"entities": [{"name": "Alpha", "description": "first"}]}));
            }
            Ok(json!({}))
        }
        async fn rebuild_dataset_structure_graph_json(
            &self,
            _tenant_id: &str,
            _kb_id: &str,
            compile_kwd: &str,
            _compilation_template_id: &str,
            _structure_kind: Option<&str>,
        ) -> std::result::Result<(), String> {
            self.dataset_rebuilds
                .lock()
                .unwrap()
                .push(compile_kwd.to_string());
            Ok(())
        }
        async fn upsert_dataset_nav_doc(
            &self,
            _tenant_id: &str,
            _kb_id: &str,
            _doc_id: &str,
            _summary: &str,
        ) -> std::result::Result<(), String> {
            *self.nav_upserts.lock().unwrap() += 1;
            Ok(())
        }
        async fn wiki_plan_from_reduction(
            &self,
            _tenant_id: &str,
            _kb_id: &str,
        ) -> std::result::Result<Value, String> {
            *self.plans.lock().unwrap() += 1;
            Ok(json!({"pages": [{"title": "P"}]}))
        }
        async fn wiki_refine_from_plan(
            &self,
            _tenant_id: &str,
            _kb_id: &str,
            _example: &str,
        ) -> std::result::Result<Vec<Value>, String> {
            *self.refines.lock().unwrap() += 1;
            Ok(vec![json!({"title": "P"})])
        }
        async fn cleanup_timeline_isolated_entities(
            &self,
            _tenant_id: &str,
            _kb_id: &str,
            _doc_id: &str,
            _doc_name: &str,
            _compilation_template_id: &str,
        ) -> std::result::Result<(), String> {
            *self.timeline_cleanups.lock().unwrap() += 1;
            Ok(())
        }
    }

    #[test]
    fn template_helpers() {
        assert_eq!(runner_template_kind(Some(" Page-Index ")), "page_index");
        assert_eq!(runner_template_kind(None), "");
        assert!(is_page_index_template(&json!({"kind": "pageindex"})));
        assert!(is_page_index_template(&json!({"kind": "Page-Index"})));
        assert!(!is_page_index_template(&json!({"kind": "tree"})));
        assert!(!is_page_index_template(&json!({})));
    }

    #[test]
    fn page_index_summary_limits_and_formats() {
        let graph = json!({"entities": [
            {"name": "Alpha", "description": "first"},
            {"name": "Beta", "description": "second"},
            {"name": "", "description": ""},
        ]});
        assert_eq!(
            page_index_graph_summary(&graph, 80),
            "Alpha: first\nBeta: second"
        );
        assert_eq!(page_index_graph_summary(&graph, 1), "Alpha: first");
        assert!(page_index_graph_summary(&json!({}), 80).is_empty());
    }

    #[test]
    fn resolve_and_filter_templates() {
        let mut backend = MockBackend::default();
        backend
            .templates
            .insert("g1-a".to_string(), json!({"config": {"kind": "list"}}));
        backend
            .templates
            .insert("g1-b".to_string(), json!({"config": {"kind": "wiki"}}));
        backend
            .templates
            .insert("g2-a".to_string(), json!({"config": {"kind": "tree"}}));
        let ids = resolve_template_ids_from_groups(
            &backend,
            &["g1".to_string(), "g1".to_string(), " ".to_string()],
            "t1",
        );
        assert_eq!(
            ids,
            vec!["g1-a".to_string(), "g1-b".to_string()],
            "deduped, blanks skipped"
        );
        let active = load_active_templates(&backend, &ids, "t1");
        assert_eq!(active.len(), 1, "wiki and missing templates are dropped");
        assert_eq!(active[0].0, "g1-a");

        let mut with_tree = active.clone();
        with_tree.push(("g2-a".to_string(), json!({"kind": "tree"})));
        let (tree, non_tree) = split_tree_templates(with_tree);
        assert_eq!(tree.len(), 1);
        assert_eq!(non_tree.len(), 1);
    }

    #[test]
    fn batch_budget_knowledge_graph_clamped() {
        let kg = dynamic_batch_budget(Some(100_000), "knowledge_graph");
        assert_eq!(
            kg, *KNOWLEDGE_GRAPH_MAX_BATCH_TOKENS,
            "0.1 * 100k = 10k -> capped at 4096"
        );
        let kg_small = dynamic_batch_budget(Some(10_000), "knowledge_graph");
        assert_eq!(
            kg_small, *KNOWLEDGE_GRAPH_MIN_BATCH_TOKENS,
            "1000 < 2048 floor"
        );
        let list = dynamic_batch_budget(Some(10_000), "list");
        assert_eq!(list, 5000);
        assert_eq!(
            dynamic_batch_budget(None, "list"),
            (100_000.0 * 0.5) as usize
        );
    }

    #[tokio::test]
    async fn runner_end_to_end_with_synthesis_and_phases() {
        let mut backend = MockBackend {
            docs_per_batch: 2,
            ..MockBackend::default()
        };
        backend
            .templates
            .insert("tpl-list".to_string(), json!({"config": {"kind": "list"}}));
        backend.templates.insert(
            "tpl-kg".to_string(),
            json!({"config": {"kind": "knowledge_graph", "dataset_merge": true}}),
        );
        backend.templates.insert(
            "tpl-timeline".to_string(),
            json!({"config": {"kind": "timeline"}}),
        );
        backend.templates.insert(
            "tpl-pi".to_string(),
            json!({"config": {"kind": "page_index", "synthesis": {"enabled": true, "example": "{\"title\": \"x\"}", "compile_kwd": "wiki_page"}}}),
        );
        let progress = |_msg: &str| {};
        let cancel = || false;
        let recorded = std::sync::Mutex::new(Vec::new());
        let record = |key: &str, value: Value| {
            recorded.lock().unwrap().push((key.to_string(), value));
        };
        let chunks: Vec<Value> = (0..3)
            .map(|index| json!({"id": format!("c{index}"), "content_with_weight": "alpha beta gamma"}))
            .collect();
        let request = CompileOverBatchesRequest {
            active_templates: vec![
                ("tpl-list".to_string(), json!({"kind": "list"})),
                (
                    "tpl-kg".to_string(),
                    json!({"kind": "knowledge_graph", "dataset_merge": true}),
                ),
                ("tpl-timeline".to_string(), json!({"kind": "timeline"})),
                (
                    "tpl-pi".to_string(),
                    json!({"kind": "page_index", "synthesis": {"enabled": true, "example": "x"}}),
                ),
            ],
            tenant_id: "t1",
            kb_id: "kb1",
            doc_id: "d1",
            doc_name: "Doc",
            language: "en",
            chunk_batches: vec![chunks],
            progress: &progress,
            cancel_check: &cancel,
            record: Some(&record),
        };
        let agg = run_structure_compile_over_batches(&backend, request)
            .await
            .unwrap();
        for template_id in ["tpl-list", "tpl-kg", "tpl-timeline", "tpl-pi"] {
            assert!(agg.get(template_id).is_some(), "{template_id} reported");
            assert_eq!(
                agg[template_id]["inserted"].as_i64().unwrap() > 0,
                true,
                "{template_id} merged docs"
            );
        }
        assert!(
            backend.merges.lock().unwrap().len() >= 4,
            "one flush per template"
        );
        assert!(
            backend
                .dataset_rebuilds
                .lock()
                .unwrap()
                .contains(&"list".to_string()),
            "dataset-scope rebuild runs for harvested kwds"
        );
        assert_eq!(
            *backend.nav_upserts.lock().unwrap(),
            1,
            "page_index nav upsert"
        );
        assert_eq!(*backend.timeline_cleanups.lock().unwrap(), 1);
        assert_eq!(*backend.plans.lock().unwrap(), 1);
        assert_eq!(*backend.refines.lock().unwrap(), 1);
        assert_eq!(recorded.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn runner_cancellation_surfaces() {
        let backend = MockBackend::default();
        let progress = |_msg: &str| {};
        let cancel = || true;
        let request = CompileOverBatchesRequest {
            active_templates: vec![("tpl".to_string(), json!({"kind": "list"}))],
            tenant_id: "t1",
            kb_id: "kb1",
            doc_id: "d1",
            doc_name: "Doc",
            language: "en",
            chunk_batches: vec![vec![json!({"id": "c1", "content_with_weight": "x"})]],
            progress: &progress,
            cancel_check: &cancel,
            record: None,
        };
        assert_eq!(
            run_structure_compile_over_batches(&backend, request).await,
            Err(RunnerError::Cancelled)
        );
    }
}
#[cfg(test)]
mod struct_gap_tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn lock_key_and_sentinel() {
        assert_eq!(
            struct_merge_lock_key("kb1", Some("t1")),
            "struct_merge:kb1:t1"
        );
        assert_eq!(struct_merge_lock_key("kb1", None), "struct_merge:kb1:");
        assert!(struct_is_invalid_sentinel(&json!(" -1 ")));
        assert!(!struct_is_invalid_sentinel(&json!("-2")));
        assert!(!struct_is_invalid_sentinel(&json!(1)));
    }

    #[test]
    fn expand_source_chunk_ids_ranges() {
        let mut texts = HashMap::new();
        for id in ["t1", "t2", "t3", "t4"] {
            texts.insert(id.to_string(), "text".to_string());
        }
        assert_eq!(
            struct_expand_source_chunk_ids(&json!("t1-t3"), &texts),
            vec!["t1", "t2", "t3"]
        );
        assert_eq!(
            struct_expand_source_chunk_ids(&json!(["T4-T2"]), &texts),
            vec!["t4", "t3", "t2"],
            "descending ranges and case-insensitive prefixes"
        );
        assert_eq!(
            struct_expand_source_chunk_ids(&json!(["t9", "t1", "t1"]), &texts),
            vec!["t1"],
            "unknown ids dropped, duplicates removed"
        );
        assert!(struct_expand_source_chunk_ids(&json!(5), &texts).is_empty());
    }

    #[test]
    fn payload_chunk_ids_filters_and_falls_back() {
        let batch = vec!["c1".to_string(), "c2".to_string()];
        let payload = json!({"source_chunk_ids": ["c2", "c9", "c2"]});
        assert_eq!(struct_payload_chunk_ids(&payload, &batch), vec!["c2"]);
        assert_eq!(
            struct_payload_chunk_ids(&json!({}), &batch),
            batch.clone(),
            "no selection -> whole batch"
        );
        assert_eq!(
            struct_payload_chunk_ids(&json!({"source_chunk_ids": ["c9"]}), &batch),
            batch,
            "nothing survives -> whole batch"
        );
        assert_eq!(
            struct_payload_chunk_ids(&json!({"source_chunk_ids": "c1"}), &["c1".to_string()]),
            vec!["c1"]
        );
    }

    #[test]
    fn merge_graph_relations_unions_doc_ids() {
        let relations = vec![
            json!({"from": "Alpha", "to": "Beta", "type": "owns", "doc_ids_kwd": ["d1"]}),
            json!({"from": " alpha ", "to": "BETA", "type": "OWNS", "doc_ids_kwd": ["d1", "d2"]}),
            json!({"from": "", "to": "Beta", "type": "x"}),
            json!({"from": "Gamma", "to": "Delta", "type": "likes"}),
        ];
        let merged = struct_merge_graph_relations(&relations);
        assert_eq!(
            merged.len(),
            2,
            "folded pair + untouched pair; blank from dropped"
        );
        assert_eq!(merged[0]["from"], json!("Alpha"));
        assert_eq!(merged[0]["doc_ids_kwd"], json!(["d1", "d2"]));
        assert_eq!(merged[0]["type"], json!("owns"), "first payload wins");
        assert_eq!(merged[1]["type"], json!("likes"));
    }

    #[test]
    fn rechunked_docs_defaults() {
        let docs = RechunkedDocs::new(vec![json!({"id": "r1"})], Vec::new());
        assert_eq!(docs.docs.len(), 1);
        assert!(docs.rechunked_chunks.is_empty());
        let empty = RechunkedDocs::default();
        assert!(empty.docs.is_empty());
    }
}
#[cfg(test)]
mod struct_b1_tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::HashMap;

    struct MockEmbed;

    #[async_trait]
    impl EmbeddingBackend for MockEmbed {
        async fn encode(&self, texts: &[String]) -> std::result::Result<Vec<Vec<f32>>, String> {
            Ok(texts
                .iter()
                .map(|text| vec![text.chars().count() as f32])
                .collect())
        }
    }

    fn relation_doc() -> Value {
        json!({
            "id": "r1",
            "knowledge_graph_kwd": "relation",
            "compile_kwd": "list",
            "doc_id": "d1",
            "docnm_kwd": "Doc",
            "from_entity_kwd": "OldFrom",
            "to_entity_kwd": "To",
            "source_chunk_ids": ["c1", "c2"],
            "content_with_weight": json!({"source": "OldFrom", "target": "To", "type": "owns"}).to_string(),
        })
    }

    #[test]
    fn alias_chains_are_cycle_guarded() {
        let mut aliases = HashMap::new();
        aliases.insert("A".to_string(), "B".to_string());
        aliases.insert("B".to_string(), "C".to_string());
        assert_eq!(struct_resolve_entity_alias(" A ", &aliases), "C");
        assert_eq!(struct_resolve_entity_alias("X", &aliases), "X");
        aliases.insert("C".to_string(), "A".to_string());
        assert_eq!(
            struct_resolve_entity_alias("A", &aliases),
            "A",
            "loop stops before re-entering A"
        );
    }

    #[test]
    fn rewrite_relation_payload_field_groups() {
        let mut aliases = HashMap::new();
        aliases.insert("OldFrom".to_string(), "NewFrom".to_string());
        let mut payload = json!({"source": "OldFrom", "target": "To", "from": null});
        assert!(struct_rewrite_relation_payload(&mut payload, &aliases));
        assert_eq!(payload["source"], json!("NewFrom"));
        assert_eq!(payload["target"], json!("To"));
        assert!(
            !struct_rewrite_relation_payload(&mut payload, &aliases),
            "second pass is a no-op"
        );
    }

    #[test]
    fn dedup_condition_scopes() {
        let doc = json!({
            "compile_kwd": "list",
            "doc_id": "d1",
            "knowledge_graph_kwd": "entity",
            "compilation_template_ids": ["t1"],
        });
        let doc_scope = struct_doc_storage_dedup_condition(&doc, false);
        assert_eq!(doc_scope["compile_kwd"], json!(["list"]));
        assert_eq!(doc_scope["doc_id"], json!(["d1"]));
        assert_eq!(doc_scope["compilation_template_ids"], json!(["t1"]));
        let dataset_scope = struct_doc_storage_dedup_condition(&doc, true);
        assert!(
            dataset_scope.get("doc_id").is_none(),
            "dataset scope ignores doc_id"
        );
    }

    #[test]
    fn exact_entity_merge_fills_and_unions() {
        let existing = json!({
            "compile_kwd": "list",
            "content_with_weight": json!({
                "name": "Alpha",
                "type": "fact",
                "description": "short",
                "source_chunk_ids": ["c1"],
            }).to_string(),
        });
        let incoming = json!({
            "compile_kwd": "list",
            "content_with_weight": json!({
                "name": "Alpha",
                "type": "title",
                "description": "a much longer description",
                "aliases": ["A"],
                "source_chunk_ids": ["c1", "c2"],
            }).to_string(),
        });
        let merged = struct_merge_exact_entity_payload(&existing, &incoming).unwrap();
        assert_eq!(merged["type"], json!("title"), "preferred type wins");
        assert_eq!(merged["description"], json!("a much longer description"));
        assert_eq!(merged["aliases"], json!(["A"]), "missing keys filled");
        assert_eq!(
            merged["source_chunk_ids"],
            json!(["c1", "c2"]),
            "union preserves order"
        );
    }

    #[test]
    fn rebuild_overlays_identity_fields() {
        let base = relation_doc();
        let payload = json!({"source": "NewFrom", "target": "To", "type": "owns"});
        let row = struct_rebuild_doc_storage_doc(
            &payload,
            &base,
            vec![1.0, 2.0],
            &["c1".to_string()],
            true,
        );
        assert_eq!(row.id, "r1", "preserved id");
        assert_eq!(row.from_entity_kwd.as_deref(), Some("OldFrom"));
        assert_eq!(row.to_entity_kwd.as_deref(), Some("To"));
        assert_eq!(row.knowledge_graph_kwd, "relation");
    }

    #[tokio::test]
    async fn rewrite_relation_doc_end_to_end() {
        let mut aliases = HashMap::new();
        aliases.insert("OldFrom".to_string(), "NewFrom".to_string());
        let rewritten = struct_rewrite_relation_doc(&relation_doc(), &aliases, &MockEmbed).await;
        let payload: Value =
            serde_json::from_str(rewritten["content_with_weight"].as_str().unwrap()).unwrap();
        assert_eq!(payload["source"], json!("NewFrom"));
        assert_eq!(rewritten["from_entity_kwd"], json!("NewFrom"));
        assert_eq!(rewritten["id"], json!("r1"));

        let untouched =
            struct_rewrite_relation_doc(&relation_doc(), &HashMap::new(), &MockEmbed).await;
        assert_eq!(
            untouched,
            relation_doc(),
            "empty alias map returns the doc as-is"
        );
    }

    #[tokio::test]
    async fn merge_exact_named_entities_collapses_duplicates() {
        let docs = vec![
            json!({
                "id": "e1",
                "knowledge_graph_kwd": "entity",
                "compile_kwd": "list",
                "doc_id": "d1",
                "source_chunk_ids": ["c1"],
                "content_with_weight": json!({"name": "Alpha", "description": "first", "type": "fact"}).to_string(),
            }),
            json!({
                "id": "e2",
                "knowledge_graph_kwd": "entity",
                "compile_kwd": "list",
                "doc_id": "d1",
                "source_chunk_ids": ["c2"],
                "content_with_weight": json!({"name": " alpha ", "description": "a longer second description", "type": "title"}).to_string(),
            }),
            json!({
                "id": "e3",
                "knowledge_graph_kwd": "entity",
                "compile_kwd": "list",
                "content_with_weight": json!({"description": "no name here"}).to_string(),
            }),
        ];
        let (out, dropped) = struct_merge_exact_named_entities(&docs, &MockEmbed).await;
        assert_eq!(dropped, 1);
        assert_eq!(out.len(), 2, "one collapsed entity + the unnamed doc");
        let merged: Value =
            serde_json::from_str(out[0]["content_with_weight"].as_str().unwrap()).unwrap();
        assert_eq!(merged["type"], json!("title"));
        assert_eq!(
            merged["source_chunk_ids"],
            json!([]),
            "payload-level ids stay empty (the envelope carries them)"
        );
        assert_eq!(
            out[0]["source_chunk_ids"],
            json!(["c1", "c2"]),
            "the rebuilt row unions the envelope chunk ids"
        );
        assert_eq!(out[0]["id"], json!("e1"), "base id preserved");
    }
}
#[cfg(test)]
mod pool_tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    fn manual_clock() -> (
        std::sync::Arc<dyn Fn() -> f64 + Send + Sync>,
        std::sync::Arc<Mutex<f64>>,
    ) {
        let value = std::sync::Arc::new(Mutex::new(0.0f64));
        let read = value.clone();
        let clock: std::sync::Arc<dyn Fn() -> f64 + Send + Sync> =
            std::sync::Arc::new(move || *read.lock().unwrap());
        (clock, value)
    }

    #[test]
    fn model_key_join_semantics() {
        assert_eq!(
            LlmCallPool::model_key_for("1", "openai", "gpt", "http://x"),
            "1:openai:gpt:http://x"
        );
        assert_eq!(LlmCallPool::model_key_for("", "", "qwen3", ""), "::qwen3:");
        assert_eq!(LlmCallPool::model_key_for("", "", "", ""), "");
    }

    #[test]
    fn rate_limit_and_error_detection() {
        assert!(LlmCallPool::is_rate_limited("HTTP 429 Too Many Requests"));
        assert!(LlmCallPool::is_rate_limited("Rate_Limit exceeded"));
        assert!(!LlmCallPool::is_rate_limited("ok"));
        assert!(LlmCallPool::is_error_result("  **ERROR** boom"));
        assert!(!LlmCallPool::is_error_result("fine"));
    }

    #[test]
    fn retry_delay_is_exponential_capped() {
        let pool = LlmCallPool::new(2);
        assert_eq!(pool.rate_limit_retry_delay(1), 1.0);
        assert_eq!(pool.rate_limit_retry_delay(2), 2.0);
        assert_eq!(pool.rate_limit_retry_delay(5), 16.0);
        assert_eq!(pool.rate_limit_retry_delay(10), 30.0, "capped at max delay");
    }

    #[tokio::test]
    async fn admission_respects_concurrency_and_priority() {
        let pool = LlmCallPool::new(1);
        pool.debug_enqueue("a", 30).await;
        pool.debug_enqueue("b", 10).await;
        assert_eq!(
            pool.debug_admit_next().await.as_deref(),
            Some("b"),
            "lower priority wins"
        );
        assert_eq!(pool.active_count().await, 1);
        assert_eq!(pool.debug_admit_next().await, None, "ceiling reached");
        pool.release("b", "success").await;
        assert_eq!(pool.debug_admit_next().await.as_deref(), Some("a"));
    }

    #[tokio::test]
    async fn acquire_blocks_until_release() {
        let pool = std::sync::Arc::new(LlmCallPool::new(1));
        pool.acquire("m", 20).await;
        let clone = pool.clone();
        let waiting = tokio::spawn(async move { clone.acquire("m", 20).await });
        let blocked = tokio::time::timeout(std::time::Duration::from_millis(80), async {
            loop {
                if pool.active_count().await == 2 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await;
        assert!(
            blocked.is_err(),
            "second acquire must block while the slot is taken"
        );
        pool.release("m", "success").await;
        tokio::time::timeout(std::time::Duration::from_millis(200), waiting)
            .await
            .expect("second acquire proceeds after release")
            .expect("task ok");
        assert_eq!(
            pool.active_count().await,
            1,
            "the first acquire was released; the second now holds the slot"
        );
    }

    #[tokio::test]
    async fn adaptive_decrease_and_cooldown() {
        let (clock, now) = manual_clock();
        let pool = LlmCallPool::new(10).with_clock(clock);
        pool.acquire("m", 20).await;
        pool.release("m", "rate_limited").await;
        assert_eq!(pool.concurrency_for("m").await, 5, "halved");
        pool.acquire("m", 20).await;
        pool.release("m", "rate_limited").await;
        assert_eq!(
            pool.concurrency_for("m").await,
            5,
            "cooldown suppresses the second decrease"
        );
        *now.lock().unwrap() = 6.0;
        pool.acquire("m", 20).await;
        pool.release("m", "rate_limited").await;
        assert_eq!(pool.concurrency_for("m").await, 2, "5 * 0.5 floored");
    }

    #[tokio::test]
    async fn adaptive_recovery_after_successes() {
        let (clock, now) = manual_clock();
        let mut pool = LlmCallPool::new(10).with_clock(clock);
        pool.recovery_successes = 2;
        pool.recovery_cooldown = 0.0;
        pool.decrease_cooldown = 0.0;
        pool.acquire("m", 20).await;
        pool.release("m", "rate_limited").await;
        assert_eq!(pool.concurrency_for("m").await, 5);
        *now.lock().unwrap() = 100.0;
        for _ in 0..2 {
            pool.acquire("m", 20).await;
            pool.release("m", "success").await;
        }
        assert_eq!(pool.concurrency_for("m").await, 6, "one slot recovered");
    }

    #[tokio::test]
    async fn call_retries_rate_limits_and_returns_error_results() {
        let mut pool = LlmCallPool::new(4);
        pool.rate_limit_retry_base_delay = 0.0;
        pool.rate_limit_retry_max_delay = 0.0;
        let attempts = std::sync::Arc::new(Mutex::new(0usize));
        let counter = attempts.clone();
        let result = pool
            .call(
                || {
                    let counter = counter.clone();
                    async move {
                        let mut count = counter.lock().unwrap();
                        *count += 1;
                        if *count < 3 {
                            Err("429 rate limit".to_string())
                        } else {
                            Ok("done".to_string())
                        }
                    }
                },
                "m",
                20,
                "test",
                None,
            )
            .await;
        assert_eq!(result, Ok("done".to_string()));
        assert_eq!(
            *attempts.lock().unwrap(),
            3,
            "two rate-limit retries then success"
        );
        assert_eq!(pool.active_count().await, 0, "slots released");

        let error_text = pool
            .call(
                || async { Ok("**ERROR** provider exploded".to_string()) },
                "m",
                20,
                "test",
                None,
            )
            .await;
        assert_eq!(error_text, Ok("**ERROR** provider exploded".to_string()));
    }

    struct MockChat {
        seen_conf: Mutex<Vec<Value>>,
    }

    #[async_trait::async_trait]
    impl crate::harness::HarnessChat for MockChat {
        async fn chat(
            &self,
            _system: &str,
            _history: &[Value],
            gen_conf: &Value,
        ) -> std::result::Result<String, String> {
            self.seen_conf.lock().unwrap().push(gen_conf.clone());
            Ok("answer".to_string())
        }
        fn max_length(&self) -> usize {
            1234
        }
    }

    #[tokio::test]
    async fn pooled_chat_model_applies_gen_conf() {
        let pool = LlmCallPool::new(2);
        let chat = MockChat {
            seen_conf: Mutex::new(Vec::new()),
        };
        let pooled = pool.wrap(&chat, "key", "gpt-4o", 20, "label", None);
        assert_eq!(crate::harness::HarnessChat::max_length(&pooled), 1234);
        let answer = crate::harness::HarnessChat::chat(
            &pooled,
            "sys",
            &[json!({"role": "user", "content": "hi"})],
            &json!({"temperature": 0.1}),
        )
        .await;
        assert_eq!(answer, Ok("answer".to_string()));
        let conf = chat.seen_conf.lock().unwrap()[0].clone();
        assert_eq!(conf["temperature"], json!(0.1), "caller conf preserved");
        assert_eq!(
            conf["reasoning_effort"],
            json!("none"),
            "model-specific control applied"
        );
    }
}

#[cfg(test)]
mod struct_c1_tests {
    use super::*;
    use serde_json::json;

    fn doc(id: &str, doc_id: &str, vec: [f32; 3]) -> Value {
        json!({
            "id": id,
            "doc_id": doc_id,
            "compile_kwd": "list",
            "knowledge_graph_kwd": "entity",
            "q_1024_vec": vec,
            "content_with_weight": json!({"name": id}).to_string(),
        })
    }

    #[test]
    fn filter_key_value_matches_tuple() {
        let row = json!({
            "doc_id": "d1",
            "compile_kwd": "list",
            "from_entity_kwd": "A",
            "to_entity_kwd": "",
            "compilation_template_ids": ["t1"],
        });
        let key = struct_filter_key_value(&row);
        assert_eq!(key.0, "d1");
        assert_eq!(key.1, "list");
        assert_eq!(key.2.as_deref(), Some("A"));
        assert_eq!(key.3, None);
        assert_eq!(key.4.as_deref(), Some("t1"));
    }

    #[test]
    fn dataset_graph_row_id_is_stable_and_scoped() {
        let first = dataset_struct_graph_row_id("kb1", "list", Some("t1"));
        let again = dataset_struct_graph_row_id("kb1", "list", Some("t1"));
        assert_eq!(first, again);
        assert_ne!(first, dataset_struct_graph_row_id("kb1", "list", None));
        assert_ne!(
            first,
            dataset_struct_graph_row_id("kb2", "list", Some("t1"))
        );
        assert_ne!(first, dataset_struct_graph_row_id("kb1", "set", Some("t1")));
    }

    #[test]
    fn entity_candidate_groups_partition_by_similarity() {
        let docs = vec![
            doc("a1", "d1", [1.0, 0.0, 0.0]),
            doc("a2", "d1", [0.99, 0.01, 0.0]),
            doc("b1", "d1", [0.0, 1.0, 0.0]),
            doc("a3-doc2", "d2", [1.0, 0.0, 0.0]),
        ];
        let groups = struct_entity_candidate_groups(&docs, 0.9);
        // d1: a1+a2 connected, b1 alone; d2: a3-doc2 alone (different filter key).
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].len(), 2);
        assert_eq!(groups[0][0]["id"], json!("a1"));
        assert_eq!(groups[1].len(), 1);
        assert_eq!(groups[2].len(), 1);
        assert_eq!(groups[2][0]["id"], json!("a3-doc2"));
    }

    #[test]
    fn entity_candidate_groups_keep_vectorless_docs_separate() {
        let mut no_vec = doc("c1", "d1", [0.0, 0.0, 0.0]);
        if let Some(map) = no_vec.as_object_mut() {
            map.remove("q_1024_vec");
        }
        let docs = vec![doc("a1", "d1", [1.0, 0.0, 0.0]), no_vec];
        let groups = struct_entity_candidate_groups(&docs, 0.9);
        assert_eq!(
            groups.len(),
            2,
            "a doc without a vector never joins a component"
        );
    }
}
#[cfg(test)]
mod struct_c2a_tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    struct MockChat {
        reply: String,
        seen_user: Mutex<String>,
    }

    #[async_trait]
    impl crate::harness::HarnessChat for MockChat {
        async fn chat(
            &self,
            _system: &str,
            history: &[Value],
            _gen_conf: &Value,
        ) -> std::result::Result<String, String> {
            let user = history
                .first()
                .and_then(|message| message.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            *self.seen_user.lock().unwrap() = user;
            Ok(self.reply.clone())
        }
        fn max_length(&self) -> usize {
            4096
        }
    }

    fn entity_doc(id: &str, name: &str) -> Value {
        json!({
            "id": id,
            "doc_id": "d1",
            "compile_kwd": "list",
            "knowledge_graph_kwd": "entity",
            "content_with_weight": json!({"name": name, "type": "fact", "description": "d"}).to_string(),
        })
    }

    #[tokio::test]
    async fn judge_batch_returns_duplicate_sets() {
        let chat = MockChat {
            reply: json!({
                "groups": [{
                    "group_id": "g1",
                    "decisions": [
                        {"incoming_index": 0, "duplicated": true},
                        {"incoming_index": 1, "duplicated": false},
                        {"incoming_index": 99, "duplicated": true},
                    ],
                }],
            })
            .to_string(),
            seen_user: Mutex::new(String::new()),
        };
        let specs = vec![json!({
            "request_group_id": "g1",
            "old_doc": entity_doc("e1", "Alpha"),
            "incoming_docs": [entity_doc("e2", "Alpha"), entity_doc("e3", "Beta")],
        })];
        let result = struct_judge_doc_storage_group_batch(&chat, &specs).await;
        let hits = result.get("g1").unwrap();
        assert!(hits.contains(&0));
        assert!(!hits.contains(&1));
        assert_eq!(hits.len(), 1, "out-of-range index ignored");
        assert!(chat.seen_user.lock().unwrap().contains("g1"),);
    }

    #[tokio::test]
    async fn merge_group_batch_splits_separate_and_merged() {
        let chat = MockChat {
            reply: json!({
                "groups": [{
                    "group_id": "e1",
                    "decisions": [
                        {"incoming_index": 0, "duplicated": true},
                        {"incoming_index": 1, "duplicated": false},
                    ],
                    "merged": {"name": "Alpha", "type": "title"},
                }],
            })
            .to_string(),
            seen_user: Mutex::new(String::new()),
        };
        let specs = vec![json!({
            "old_id": "e1",
            "old_doc": entity_doc("e1", "Alpha"),
            "incoming_docs": [entity_doc("e2", "Alpha"), entity_doc("e3", "Beta")],
        })];
        let result = struct_merge_doc_storage_group_batch(&chat, &specs).await;
        let (separate, merged) = result.get("e1").unwrap();
        assert_eq!(separate.len(), 1);
        assert_eq!(separate[0]["id"], json!("e3"));
        assert_eq!(merged.as_ref().unwrap()["type"], json!("title"));
    }

    #[tokio::test]
    async fn merge_group_single_uses_pair_prompt() {
        let chat = MockChat {
            reply: json!({"duplicated": true, "merged": {"name": "Alpha", "type": "title"}})
                .to_string(),
            seen_user: Mutex::new(String::new()),
        };
        let old = entity_doc("e1", "Alpha");
        let incoming = vec![entity_doc("e2", "Alpha")];
        let (separate, merged) = struct_merge_doc_storage_group(&chat, &old, &incoming).await;
        assert!(separate.is_empty());
        assert!(merged.is_some());
        assert!(
            chat.seen_user.lock().unwrap().contains("Item B"),
            "the merge-pair prompt is used for single candidates"
        );

        let chat_no = MockChat {
            reply: json!({"duplicated": false}).to_string(),
            seen_user: Mutex::new(String::new()),
        };
        let (separate, merged) = struct_merge_doc_storage_group(&chat_no, &old, &incoming).await;
        assert_eq!(separate.len(), 1);
        assert!(merged.is_none());
    }
}
#[cfg(test)]
mod struct_c2b_tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    struct MockChat {
        replies: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl crate::harness::HarnessChat for MockChat {
        async fn chat(
            &self,
            _system: &str,
            _history: &[Value],
            _gen_conf: &Value,
        ) -> std::result::Result<String, String> {
            let mut queue = self.replies.lock().unwrap();
            if queue.is_empty() {
                Ok(String::new())
            } else {
                Ok(queue.remove(0))
            }
        }
        fn max_length(&self) -> usize {
            4096
        }
    }

    struct MockEmbed;

    #[async_trait]
    impl EmbeddingBackend for MockEmbed {
        async fn encode(&self, texts: &[String]) -> std::result::Result<Vec<Vec<f32>>, String> {
            Ok(texts.iter().map(|_| vec![0.1, 0.2, 0.3]).collect())
        }
    }

    #[derive(Default)]
    struct MockStore {
        search_rows: Vec<Value>,
        inserts: Mutex<Vec<usize>>,
    }

    impl crate::doc_store::DocStore for MockStore {
        fn db_type(&self) -> &'static str {
            "memory"
        }
        fn health(&self) -> crate::Result<crate::doc_store::HealthStatus> {
            Ok(crate::doc_store::HealthStatus::green("test"))
        }
        fn create_idx(
            &self,
            _index_name: &str,
            _dataset_id: &str,
            _vector_size: usize,
        ) -> crate::Result<()> {
            Ok(())
        }
        fn delete_idx(&self, _index_name: &str, _dataset_id: &str) -> crate::Result<()> {
            Ok(())
        }
        fn index_exist(&self, _index_name: &str, _dataset_id: &str) -> crate::Result<bool> {
            Ok(true)
        }
        fn insert(
            &self,
            rows: &[crate::doc_store::DocRow],
            _index_name: &str,
            _dataset_id: &str,
        ) -> crate::Result<Vec<String>> {
            self.inserts.lock().unwrap().push(rows.len());
            Ok(Vec::new())
        }
        fn get(
            &self,
            _data_id: &str,
            _index_name: &str,
            _dataset_ids: &[String],
        ) -> crate::Result<Option<crate::doc_store::DocRow>> {
            Ok(None)
        }
        fn update(
            &self,
            _condition: &crate::doc_store::FilterCondition,
            _new_value: &crate::doc_store::DocRow,
            _index_name: &str,
            _dataset_id: &str,
        ) -> crate::Result<bool> {
            Ok(false)
        }
        fn delete(
            &self,
            _condition: &crate::doc_store::FilterCondition,
            _index_name: &str,
            _dataset_id: &str,
        ) -> crate::Result<usize> {
            Ok(0)
        }
        fn search(
            &self,
            query: &crate::doc_store::SearchQuery,
        ) -> crate::Result<crate::doc_store::SearchResponse> {
            // Only the KNN/relation searches consume rows; return the seeded set
            // for the KNN search (match expressions or name filter), nothing for
            // the relation-rewrite search (`knowledge_graph_kwd = relation`).
            let wants_relation = query
                .condition
                .get("knowledge_graph_kwd")
                .and_then(Value::as_array)
                .map(|items| items.iter().any(|item| item.as_str() == Some("relation")))
                .unwrap_or(false);
            let docs: Vec<crate::doc_store::DocRow> = if wants_relation {
                Vec::new()
            } else {
                self.search_rows
                    .iter()
                    .filter_map(|row| row.as_object().cloned())
                    .collect()
            };
            Ok(crate::doc_store::SearchResponse {
                total: docs.len(),
                docs,
                ..Default::default()
            })
        }
        fn sql(&self, _sql: &str, _fetch_size: usize) -> crate::Result<Vec<Value>> {
            Ok(Vec::new())
        }
    }

    fn entity_doc(id: &str, name: &str) -> Value {
        json!({
            "id": id,
            "doc_id": "d1",
            "compile_kwd": "list",
            "knowledge_graph_kwd": "entity",
            "source_chunk_ids": ["c1"],
            "content_with_weight": json!({"name": name, "type": "fact", "description": "d"}).to_string(),
        })
    }

    #[tokio::test]
    async fn dedup_batch_merges_duplicate_entities() {
        let store = MockStore {
            search_rows: vec![entity_doc("old1", "Alpha")],
            ..MockStore::default()
        };
        let chat = MockChat {
            replies: Mutex::new(vec![
                // judge batch (ES_GROUP_DECISION_BATCH_PROMPT)
                json!({"groups": [{"group_id": "old1:part-0", "decisions": [{"incoming_index": 0, "duplicated": true}]}]}).to_string(),
                // merge group (single candidate -> merge-pair prompt)
                json!({"duplicated": true, "merged": {"name": "Alpha", "type": "title"}}).to_string(),
            ]),
        };
        let docs = vec![entity_doc("new1", "Alpha")];
        let cancel = || false;
        let (inserted, updated) = struct_doc_storage_dedup_batch(
            &store, &chat, &MockEmbed, &docs, "t1", "kb1", 0.9, false, &cancel,
        )
        .await
        .expect("dedup runs");
        assert_eq!(inserted, 0, "the duplicate is not inserted separately");
        assert_eq!(updated, 1, "the existing row is rewritten");
        assert!(
            !store.inserts.lock().unwrap().is_empty(),
            "a write happened"
        );
    }

    #[tokio::test]
    async fn dedup_batch_cancellation_surfaces() {
        let store = MockStore::default();
        let chat = MockChat {
            replies: Mutex::new(Vec::new()),
        };
        let cancel = || true;
        let result = struct_doc_storage_dedup_batch(
            &store,
            &chat,
            &MockEmbed,
            &[entity_doc("new1", "Alpha")],
            "t1",
            "kb1",
            0.9,
            false,
            &cancel,
        )
        .await;
        assert_eq!(result, Err(RunnerError::Cancelled));
    }
}
