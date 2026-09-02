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
                && let Some(incoming) = entity.get("aliases").and_then(Value::as_array) {
                    for a in incoming {
                        if let Some(s) = a.as_str()
                            && !aliases.iter().any(|x| x.as_str() == Some(s)) {
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
            && let Some(v) = payload.get(sf) {
                let s = v.to_string().trim().to_string();
                if !s.is_empty() {
                    from_entity = Some(s);
                }
            }
        if let Some(tf) = target_field
            && let Some(v) = payload.get(tf) {
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
                && !n.is_empty() {
                    aliases.insert(n.to_string());
                }
            if let Some(a) = it.get("aliases").and_then(Value::as_array) {
                for v in a {
                    if let Some(s) = v.as_str()
                        && !s.is_empty() {
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
            && let Some(tk) = type_key {
                record[tk] = json!(tv);
            }
        if let Some(agg) = aggregate_extra
            && let Some(extras) = agg(&group)
                && extras.is_object()
                    && let Some(map) = record.as_object_mut() {
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
                && canonical[i].get(tk) != canonical[j].get(tk) {
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
        && !ambiguous_pairs.is_empty() {
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
                && &row.doc_id != d {
                    return false;
                }
        if let Some(c) = &self.compile_kwd
            && &row.compile_kwd != c {
                return false;
            }
        if let Some(k) = &self.knowledge_graph_kwd
            && &row.knowledge_graph_kwd != k {
                return false;
            }
        if let Some(f) = &self.from_entity_kwd
            && row.from_entity_kwd.as_ref() != Some(f) {
                return false;
            }
        if let Some(t) = &self.to_entity_kwd
            && row.to_entity_kwd.as_ref() != Some(t) {
                return false;
            }
        if let Some(tpl) = &self.compilation_template_id
            && !row.compilation_template_ids.iter().any(|t| t == tpl) {
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
    if preserve_id
        && let Some(id) = base_doc.get("id").and_then(Value::as_str) {
            new_doc.id = id.to_string();
        }
    for kwd in ["from_entity_kwd", "to_entity_kwd"] {
        if let Some(v) = base_doc.get(kwd).and_then(Value::as_str)
            && !v.is_empty() {
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
        && let Some(map) = old_doc.as_object_mut() {
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
                        && !s.is_empty() && seen.insert(s.to_string()) {
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
            '}' | ']'
                if stack.last() == Some(&c) => {
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
            && let Ok(v) = serde_json::from_str::<Value>(&repaired[..=idx]) {
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
