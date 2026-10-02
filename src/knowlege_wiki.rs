//! WIKI knowledge compilation pipeline — RAGFlow v0.27.2
//! `rag/advanced_rag/knowlege_compile/wiki.py` (MAP / REDUCE / PLAN / REFINE).
//!
//! Port progress: part 1 — head constants, chunk hashing, doc-id
//! normalization, chunk-state delta, MAP prompt assembly and extraction
//! post-processing helpers.
//!
//! Established divergences (same policy as the other knowlege_compile ports):
//! - `_chunk_hash` uses xxh3-64 (upstream `xxhash.xxh64`), matching the
//!   repo-wide row-id hash divergence.
//! - `thread_pool_exec` fan-out becomes inline calls.
//! - Rust's `regex` crate has no lookaround; the hex-token scrub is a manual
//!   scanner with the same boundary semantics.
//! - Module-level `_env_int(...)` constants become functions evaluated on
//!   call.
use super::knowlege_dataset_nav::index_name;
use crate::doc_store::{DocRow, DocStore, SearchQuery};
use crate::embed::Embedder;
use crate::harness::{HarnessChat, form_message, message_fit_in};
use crate::llm::LlmClient;
use crate::structure_compile::{
    ChunkInput, PackedEntry, build_chunk_batches, bulk_dedup_items, cfg_get, env_int,
    exact_dedup_by_key, knowledge_compile_gen_conf, localize, parse_json_lenient, stable_row_id,
    tokenize_for_search,
};
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;
use uuid::Uuid;

/// `_WIKI_PIPELINE_REV` — bumping this invalidates every cached wiki row.
pub const WIKI_PIPELINE_REV: &str = "v1";

/// `WIKI_MAP_COMPILE_KWD`.
pub const WIKI_MAP_COMPILE_KWD: &str = "wiki_map_extract";
/// `WIKI_MAP_STATE_COMPILE_KWD`.
pub const WIKI_MAP_STATE_COMPILE_KWD: &str = "wiki_map_state";
/// `WIKI_MAP_STATE_META_COMPILE_KWD`.
pub const WIKI_MAP_STATE_META_COMPILE_KWD: &str = "wiki_map_state_meta";

/// `DEFAULT_WIKI_MAP_WORKERS` (`_env_int("WIKI_MAP_WORKERS", 20, minimum=1)`).
pub fn default_wiki_map_workers() -> usize {
    env_int("WIKI_MAP_WORKERS", 20, Some(1)).max(1) as usize
}

/// `DEFAULT_WIKI_MAP_TIMEOUT` (`_env_int("WIKI_MAP_TIMEOUT", 600, minimum=1)`).
pub fn default_wiki_map_timeout() -> i64 {
    env_int("WIKI_MAP_TIMEOUT", 600, Some(1)).max(1)
}

/// Python truthiness for JSON values (None/False/0/""/[]/{} are falsy).
pub fn json_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

/// `_chunk_hash`: xxh64(content + "|" + rev) hexdigest — xxh3-64 here.
pub fn chunk_hash(content: &str) -> String {
    let body = format!("{}|{}", content, WIKI_PIPELINE_REV);
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(body.as_bytes()))
}

/// `_wiki_doc_ids`: normalize a scalar or list-valued document-id field.
pub fn wiki_doc_ids(value: &Value) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_doc_ids(value, &mut out);
    out
}

fn collect_doc_ids(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Null => {}
        Value::String(s) => {
            let t = s.trim();
            if !t.is_empty() {
                out.insert(t.to_string());
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_doc_ids(item, out);
            }
        }
        other => {
            let t = other.to_string();
            let t = t.trim();
            if !t.is_empty() {
                out.insert(t.to_string());
            }
        }
    }
}

/// `_wiki_compare_chunk_states`: chunk-level delta between two states.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ChunkStateDelta {
    pub new_chunk_ids: BTreeSet<String>,
    pub changed_chunk_ids: BTreeSet<String>,
    pub deleted_chunk_ids: BTreeSet<String>,
    pub unchanged_chunk_ids: BTreeSet<String>,
}

fn state_hash(value: &Value) -> String {
    value
        .get("hash")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// `_wiki_compare_chunk_states` — `previous` / `current` map chunk_id to a
/// state dict carrying a `hash` field.
pub fn compare_chunk_states(
    previous: &Map<String, Value>,
    current: &Map<String, Value>,
) -> ChunkStateDelta {
    let previous_ids: BTreeSet<&String> = previous.keys().collect();
    let current_ids: BTreeSet<&String> = current.keys().collect();
    let mut delta = ChunkStateDelta::default();
    for id in current_ids.difference(&previous_ids) {
        delta.new_chunk_ids.insert((*id).clone());
    }
    for id in previous_ids.difference(&current_ids) {
        delta.deleted_chunk_ids.insert((*id).clone());
    }
    for id in previous_ids.intersection(&current_ids) {
        let same = state_hash(&previous[*id]) == state_hash(&current[*id]);
        if same {
            delta.unchanged_chunk_ids.insert((*id).clone());
        } else {
            delta.changed_chunk_ids.insert((*id).clone());
        }
    }
    delta
}

/// `_EXTRACT_LIST_KEYS`.
pub const EXTRACT_LIST_KEYS: [&str; 4] = ["entities", "concepts", "claims", "relations"];

/// `_wiki_empty_extract`.
pub fn empty_extract() -> Value {
    let mut map = Map::new();
    for key in EXTRACT_LIST_KEYS {
        map.insert(key.to_string(), Value::Array(Vec::new()));
    }
    map.insert("topics".to_string(), Value::Array(Vec::new()));
    Value::Object(map)
}

/// Local `_struct_get` wrapper: clone the addressed subtree (Null when absent).
fn cfg_clone(cfg: &Value, keys: &[&str]) -> Value {
    let null = Value::Null;
    cfg_get(cfg, keys, &null).clone()
}

/// `_wiki_render_schema_body`: render the JSON body for one item in the
/// entity/relation schema; always appends a canonical `source_chunk_id` line.
pub fn render_schema_body(
    fields: &[Value],
    language: &str,
    default_body: &str,
    indent: usize,
) -> String {
    if fields.is_empty() {
        return default_body.to_string();
    }
    let pad = " ".repeat(indent);
    let mut lines: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for field in fields {
        let Some(obj) = field.as_object() else {
            continue;
        };
        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() || seen.contains(&name) || name == "source_chunk_id" {
            continue;
        }
        seen.insert(name.clone());
        let ftype = obj.get("type").and_then(Value::as_str).unwrap_or("str");
        let desc = localize(obj.get("description").unwrap_or(&Value::Null), language);
        let placeholder = match ftype {
            "list" => "[\"string\"]".to_string(),
            "int" => "0".to_string(),
            "float" => "0.0".to_string(),
            "bool" => "false".to_string(),
            _ => {
                if desc.is_empty() {
                    "\"string\"".to_string()
                } else {
                    let safe = desc.replace('\n', " ").replace('{', "(").replace('}', ")");
                    format!("\"string — {}\"", safe.trim())
                }
            }
        };
        lines.push(format!("{pad}\"{name}\": {placeholder}"));
    }
    if lines.is_empty() {
        return default_body.to_string();
    }
    lines.push(format!(
        "{pad}\"source_chunk_id\": \"string — exact value from the chunk_id list above\""
    ));
    lines.join(",\n")
}

/// `_wiki_build_custom_rules`: user entity/relation rules as prompt sections.
pub fn build_custom_rules(parser_config: &Value, language: &str) -> String {
    if !parser_config.is_object() {
        return String::new();
    }
    let guideline = cfg_clone(parser_config, &["guideline"]);
    let rules_e = localize(&cfg_clone(&guideline, &["rules_for_entities"]), language);
    let rules_r = localize(&cfg_clone(&guideline, &["rules_for_relations"]), language);
    let mut sections: Vec<String> = Vec::new();
    if !rules_e.is_empty() {
        sections.push(format!(
            "## Entity extraction rules (from knowledge base config):\n{rules_e}"
        ));
    }
    if !rules_r.is_empty() {
        sections.push(format!(
            "## Relation extraction rules (from knowledge base config):\n{rules_r}"
        ));
    }
    if sections.is_empty() {
        return String::new();
    }
    format!("\n{}\n", sections.join("\n\n"))
}

/// `_wiki_template_fields`.
pub fn template_fields(parser_config: &Value, section: &str) -> Vec<Value> {
    if !parser_config.is_object() {
        return Vec::new();
    }
    let cfg = cfg_clone(parser_config, &[section]);
    match cfg_clone(&cfg, &["fields"]) {
        Value::Array(items) => items,
        _ => Vec::new(),
    }
}

/// `_wiki_type_rules`.
pub fn type_rules(fields: &[Value]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for field in fields {
        let Some(obj) = field.as_object() else {
            continue;
        };
        let typ = obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if typ.is_empty() {
            continue;
        }
        let description = obj
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let rule = obj
            .get("rule")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        lines.push(format!("type: {typ}"));
        if !description.is_empty() {
            lines.push(format!("  - description: {description}"));
        }
        if !rule.is_empty() {
            lines.push(format!("  - rule: {rule}"));
        }
    }
    lines.join("\n")
}

/// `_wiki_pipe_join`.
pub fn pipe_join(fields: &[Value], key: &str) -> String {
    let mut values: Vec<String> = Vec::new();
    for field in fields {
        let Some(obj) = field.as_object() else {
            continue;
        };
        let value = obj.get(key).and_then(Value::as_str).unwrap_or("").trim();
        if !value.is_empty() {
            values.push(value.to_string());
        }
    }
    values.join("|")
}

/// `_wiki_colon_join`.
pub fn colon_join(fields: &[Value], left_key: &str, right_key: &str) -> String {
    let mut values: Vec<String> = Vec::new();
    for field in fields {
        let Some(obj) = field.as_object() else {
            continue;
        };
        let left = obj
            .get(left_key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let right = obj
            .get(right_key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !left.is_empty() || !right.is_empty() {
            values.push(format!("{left}:{right}"));
        }
    }
    values.join("\n")
}

/// `_wiki_named_field_description`.
pub fn named_field_description(fields: &[Value], name: &str) -> String {
    for field in fields {
        let Some(obj) = field.as_object() else {
            continue;
        };
        let field_name = obj
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_lowercase();
        if field_name == name {
            let description = obj
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if !description.is_empty() {
                return description.to_string();
            }
        }
        let legacy = obj.get(name).and_then(Value::as_str).unwrap_or("").trim();
        if !legacy.is_empty() {
            return legacy.to_string();
        }
    }
    String::new()
}

/// `_wiki_template_custom_rules`.
pub fn template_custom_rules(parser_config: &Value) -> String {
    if !parser_config.is_object() {
        return String::new();
    }
    parser_config
        .get("global_rules")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Single-pass `{name}` substitution (Python `str.format` semantics: the
/// replacement values are never re-scanned for further placeholders).
pub fn render_template(template: &str, vars: &[(&str, &str)]) -> String {
    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len() + 256);
    let mut i = 0usize;
    while i < template.len() {
        if bytes[i] == b'{' {
            let mut j = i + 1;
            while j < template.len() && bytes[j] != b'}' {
                j += 1;
            }
            if j < template.len() {
                let name = &template[i + 1..j];
                if let Some((_, value)) = vars.iter().find(|(k, _)| *k == name) {
                    out.push_str(value);
                    i = j + 1;
                    continue;
                }
            }
            out.push('{');
            i += 1;
            continue;
        }
        let ch = template[i..].chars().next().expect("char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// `WIKI_MAP_SYSTEM`.
pub const WIKI_MAP_SYSTEM: &str = "You are a knowledge extraction engine. Extract structured knowledge from the provided document section. Return ONLY valid JSON matching the schema exactly. Never include any text outside the JSON object. If a category has no items, use [].Keep the chunks' original language (Chinese/English etc.) for generated data.";

/// `_DEFAULT_ENTITY_SCHEMA_BODY`.
pub const DEFAULT_ENTITY_SCHEMA_BODY: &str = concat!(
    "      \"name\": \"string — entity canonical name as it appears in text\",\n",
    "      \"type\": \"string — one of: person|org|product|regulation|location|system|equipment|other\",\n",
    "      \"aliases\": [\"string\"],\n",
    "      \"source_chunk_id\": \"string — exact value from the chunk_id list above\""
);

/// `_DEFAULT_RELATION_SCHEMA_BODY`.
pub const DEFAULT_RELATION_SCHEMA_BODY: &str = concat!(
    "      \"from\": \"string — source entity/concept name\",\n",
    "      \"to\": \"string — target entity/concept name\",\n",
    "      \"type\": \"string — e.g. owns|part_of|caused_by|regulates|uses|located_in|other\",\n",
    "      \"source_chunk_id\": \"string — exact value from the chunk_id list above\""
);

/// `WIKI_MAP_USER_TEMPLATE` — stored post-`.format` (single braces).
pub const WIKI_MAP_USER_TEMPLATE: &str = r#"## Document context
Document id: {doc_id}
Batch contains {chunk_count} packed chunk(s). Each chunk is introduced by a
``[CHUNK_ID <id>]`` line. The chunk_id values to choose from are:
{chunk_id_list}

## Packed chunks
{packed_chunks}

---

Extract all knowledge from every chunk and return a single JSON object with this
exact schema:

{
  "entities": [
    {
      "name": "string - entity canonical name as it appears in text",
      "type": "string - {entity_type_rules}",
      "aliases": ["string"],
      "source_chunk_id": "string - exact value from the chunk_id list above"
    }
  ],
  "concepts": [
    {
      "term": "string - {concept_term}",
      "definition_excerpt": "string - {concept_definition_excerpt}",
      "source_chunk_id": "string - exact value from the chunk_id list above"
    }
  ],
  "claims": [
    {
      "statement": "string - {claim_statement}",
      "subject": "string - {claim_subject}",
      "confidence": "explicit",
      "source_chunk_id": "string - exact value from the chunk_id list above"
    }
  ],
  "relations": [
    {
      "from": "string - source entity/concept name",
      "to": "string - target entity/concept name",
      "type": "string - {relation_type_rules}",
      "source_chunk_id": "string - exact value from the chunk_id list above"
    }
  ],
  "topics": ["string"]
}

Rules:
- ``source_chunk_id`` MUST be one of the chunk_id values listed above (they
  look like ``C1``, ``C2``, …); do NOT invent new ids. Pick the chunk where
  the item is primarily stated.
- The ``[CHUNK_ID …]`` header lines AND the ``C1``/``C2``/… chunk tags are
  prompt scaffolding — they are NOT part of the document content. Do NOT
  extract them (or any other identifier-looking strings from the headers)
  as entities, concepts, claims, or relations. Entity ``name`` / concept
  ``term`` values must come from the human-readable chunk body only.
- NEVER use bare hexadecimal hashes (such as ``a3f1b2c4d5e6f7a8``),
  UUIDs, database row ids, or any other opaque identifier-looking token
  as an entity ``name`` or concept ``term``. If you cannot find a
  human-readable name for a candidate entity in the chunk body, drop it.
- Concrete examples of values that are ALWAYS WRONG:
    BAD entity: {"name": "C1", "type": "product", "aliases": ["C1"]}
    BAD entity: {"name": "C3", "type": "location"}
    BAD concept: {"term": "C2"}
    BAD entity: {"name": "d523a888c5b2a167", "type": "location"}
    BAD entity: {"name": "41a5271858ca11f1bbb9047c16ec874f", "type": "product"}
  ``C1`` / ``C2`` / etc. are CHUNK TAGS, not products or locations. The
  hex hashes are DATABASE IDS, not entities. If your candidate ``name``
  matches any of these shapes, do not include the item in the output.
- ``confidence`` is ``"explicit"`` (directly stated) or ``"inferred"`` (implied
  by the text).
- Be exhaustive — include all named entities, defined terms, and factual claims.
- For ``concepts``, extract BOTH (a) named terms with definitions AND (b)
  coherent thematic sub-topics that could become their own wiki page.
- Extract ``claims`` LIBERALLY: every factual sentence about an entity is a
  claim. Definitions, attributes, ownership, locations, dates, actions,
  events, financial figures, regulations cited — all qualify. If you
  extract an entity, you should usually extract one or more claims that
  mention it. An empty ``claims`` array is almost always wrong unless the
  chunks are pure boilerplate.
- ``relations`` only fire when the text states an explicit link between two
  named entities/concepts (``A owns B``, ``A is part of B``, ``A regulates B``).
  Otherwise leave ``relations`` empty.
- Return empty arrays ``[]`` for categories with no findings.
- Return ONLY the JSON object, no markdown fences, no commentary.
{custom_rules}"#;

/// `_wiki_build_user_prompt`.
#[allow(clippy::too_many_arguments)]
pub fn build_user_prompt(
    parser_config: &Value,
    language: &str,
    doc_id: &str,
    chunk_count: usize,
    chunk_id_list: &str,
    packed_chunks: &str,
) -> String {
    let ent_fields = template_fields(parser_config, "entity");
    let rel_fields = template_fields(parser_config, "relation");
    let concept_fields = template_fields(parser_config, "concept");
    let claim_fields = template_fields(parser_config, "claim");
    let mut entity_type_rules = type_rules(&ent_fields);
    let mut relation_type_rules = type_rules(&rel_fields);
    let concept_term = pipe_join(&concept_fields, "term");
    let concept_definition_excerpt = colon_join(&concept_fields, "term", "definition_excerpt");
    let claim_statement = named_field_description(&claim_fields, "statement");
    let claim_subject = named_field_description(&claim_fields, "subject");
    let mut custom_rules = template_custom_rules(parser_config);

    if parser_config.is_object() {
        let output = cfg_clone(parser_config, &["output"]);
        let entities_cfg = cfg_clone(&output, &["entities"]);
        let relations_cfg = cfg_clone(&output, &["relations"]);
        let legacy_ent_fields = cfg_clone(&entities_cfg, &["fields"]);
        let legacy_rel_fields = cfg_clone(&relations_cfg, &["fields"]);
        if entity_type_rules.is_empty() {
            if let Value::Array(items) = &legacy_ent_fields {
                if !items.is_empty() {
                    entity_type_rules =
                        render_schema_body(items, language, DEFAULT_ENTITY_SCHEMA_BODY, 6);
                }
            }
        }
        if relation_type_rules.is_empty() {
            if let Value::Array(items) = &legacy_rel_fields {
                if !items.is_empty() {
                    relation_type_rules =
                        render_schema_body(items, language, DEFAULT_RELATION_SCHEMA_BODY, 6);
                }
            }
        }
    }

    let entity_type_rules = if entity_type_rules.is_empty() {
        "person|org|product|regulation|location|system|equipment|other".to_string()
    } else {
        entity_type_rules
    };
    let relation_type_rules = if relation_type_rules.is_empty() {
        "include|ordered|owns|part_of|caused_by|regulates|uses|located_in|other".to_string()
    } else {
        relation_type_rules
    };
    let concept_term = if concept_term.is_empty() {
        "named term or topic".to_string()
    } else {
        concept_term
    };
    let concept_definition_excerpt = if concept_definition_excerpt.is_empty() {
        "short definition excerpt from the source text".to_string()
    } else {
        concept_definition_excerpt
    };
    let claim_statement = if claim_statement.is_empty() {
        "factual statement".to_string()
    } else {
        claim_statement
    };
    let claim_subject = if claim_subject.is_empty() {
        "entity or concept that the claim is about".to_string()
    } else {
        claim_subject
    };
    let custom_rules = if custom_rules.is_empty() {
        build_custom_rules(parser_config, language)
    } else {
        custom_rules
    };

    let chunk_count_s = chunk_count.to_string();
    render_template(
        WIKI_MAP_USER_TEMPLATE,
        &[
            ("doc_id", doc_id),
            ("chunk_count", chunk_count_s.as_str()),
            ("chunk_id_list", chunk_id_list),
            ("packed_chunks", packed_chunks),
            ("entity_type_rules", entity_type_rules.as_str()),
            ("relation_type_rules", relation_type_rules.as_str()),
            ("concept_term", concept_term.as_str()),
            (
                "concept_definition_excerpt",
                concept_definition_excerpt.as_str(),
            ),
            ("claim_statement", claim_statement.as_str()),
            ("claim_subject", claim_subject.as_str()),
            ("custom_rules", custom_rules.as_str()),
        ],
    )
}

/// `_wiki_pick_chunk_text`.
pub fn pick_chunk_text(chunk: &Value) -> String {
    let mut chosen: Option<&Value> = None;
    for key in ["text", "content_with_weight", "content"] {
        if let Some(v) = chunk.get(key) {
            if json_truthy(v) {
                chosen = Some(v);
                break;
            }
        }
    }
    match chosen {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn is_hex_lower(b: u8) -> bool {
    matches!(b, b'0'..=b'9' | b'a'..=b'f')
}

fn hex_scrub_boundary(b: u8) -> bool {
    !matches!(b, b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z')
}

/// Remove standalone runs of exactly `token_len` lowercase-hex bytes bounded
/// by non-alphanumeric bytes (Python `(?<![0-9a-zA-Z])[0-9a-f]{N}(?!…)`).
pub fn scrub_hex_tokens(text: &str, token_len: usize) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let mut end = i;
        while end < bytes.len() && end - i < token_len && is_hex_lower(bytes[end]) {
            end += 1;
        }
        if end - i == token_len
            && (i == 0 || hex_scrub_boundary(bytes[i - 1]))
            && (end == bytes.len() || hex_scrub_boundary(bytes[end]))
        {
            i = end;
            continue;
        }
        let ch = text[i..].chars().next().expect("char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// `_wiki_scrub_known_ids`: strip literal ids, then leftover hex tokens.
pub fn scrub_known_ids(text: &str, ids_to_remove: &[String]) -> String {
    if text.is_empty() {
        return text.to_string();
    }
    let mut out = text.to_string();
    for h in ids_to_remove {
        if !h.is_empty() && out.contains(h.as_str()) {
            out = out.replace(h.as_str(), "");
        }
    }
    let out = scrub_hex_tokens(&out, 16);
    scrub_hex_tokens(&out, 32)
}

fn identifier_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^\s*([Cc]\d{1,5}|[0-9a-fA-F]{16}|[0-9a-fA-F]{32}|[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})\s*$",
        )
        .expect("identifier regex")
    })
}

/// `_wiki_looks_like_identifier`: chunk tag / hash / UUID shape check.
pub fn looks_like_identifier(s: &str) -> bool {
    identifier_re().is_match(s)
}

/// `_wiki_item_has_identifier_name`.
pub fn item_has_identifier_name(key: &str, item: &Value) -> bool {
    let check = |field: &str| -> bool {
        item.get(field)
            .and_then(Value::as_str)
            .map(looks_like_identifier)
            .unwrap_or(false)
    };
    match key {
        "entities" => check("name"),
        "concepts" => check("term"),
        "claims" => check("subject"),
        "relations" => check("from") || check("to"),
        _ => false,
    }
}

/// `_wiki_format_batch_prompt`: (body_text, label_order).
pub fn format_batch_prompt(packed: &[Value]) -> (String, Vec<String>) {
    let mut parts: Vec<String> = Vec::new();
    let mut labels: Vec<String> = Vec::new();
    for entry in packed {
        let label = entry
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        labels.push(label.clone());
        let text = entry.get("text").and_then(Value::as_str).unwrap_or("");
        parts.push(format!("[CHUNK_ID {label}]\n{text}"));
    }
    (parts.join("\n\n"), labels)
}

/// `_wiki_unwrap_extract`: coerce LLM JSON to the canonical 5-key shape.
pub fn unwrap_extract(res: &Value) -> Value {
    let mut out = empty_extract();
    let Some(obj) = res.as_object() else {
        return out;
    };
    {
        let out_map = out.as_object_mut().expect("object");
        for key in EXTRACT_LIST_KEYS {
            if let Some(Value::Array(items)) = obj.get(key) {
                let filtered: Vec<Value> =
                    items.iter().filter(|it| it.is_object()).cloned().collect();
                out_map.insert(key.to_string(), Value::Array(filtered));
            }
        }
        if let Some(Value::Array(topics)) = obj.get("topics") {
            let filtered: Vec<Value> = topics
                .iter()
                .filter(|t| t.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false))
                .cloned()
                .collect();
            out_map.insert("topics".to_string(), Value::Array(filtered));
        }
    }
    out
}

/// `_wiki_merge_extracts` is deferred to part 2 with the state/resume group;
/// `_wiki_resolve_chunk_ids` lands here because it is pure.
///
/// Split a batch extract by source chunk id; returns
/// (merged, per_chunk) where every label keeps an extract-shaped entry.
pub fn resolve_chunk_ids(
    extract: &Value,
    label_to_id: &HashMap<String, String>,
) -> (Value, Map<String, Value>) {
    let mut per_chunk: Map<String, Value> = Map::new();
    for real_id in label_to_id.values() {
        per_chunk.insert(real_id.clone(), empty_extract());
    }
    let mut merged = empty_extract();
    let topics: Vec<Value> = extract
        .get("topics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    merged
        .as_object_mut()
        .expect("object")
        .insert("topics".to_string(), Value::Array(topics.clone()));
    for chunk_extract in per_chunk.values_mut() {
        chunk_extract
            .as_object_mut()
            .expect("object")
            .insert("topics".to_string(), Value::Array(topics.clone()));
    }

    let mut dropped = 0usize;
    let mut dropped_identifier = 0usize;
    for key in EXTRACT_LIST_KEYS {
        let items: Vec<Value> = extract
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for item in items {
            let label = item.get("source_chunk_id").and_then(Value::as_str);
            let real = label.and_then(|l| label_to_id.get(l).cloned());
            let Some(real) = real else {
                dropped += 1;
                continue;
            };
            if item_has_identifier_name(key, &item) {
                dropped_identifier += 1;
                continue;
            }
            let mut new_item = item.as_object().cloned().unwrap_or_default();
            new_item.remove("source_chunk_id");
            new_item.insert(
                "chunk_ids".to_string(),
                Value::Array(vec![Value::String(real.clone())]),
            );
            let new_value = Value::Object(new_item);
            merged
                .as_object_mut()
                .expect("object")
                .get_mut(key)
                .and_then(Value::as_array_mut)
                .expect("array")
                .push(new_value.clone());
            per_chunk
                .get_mut(&real)
                .and_then(Value::as_object_mut)
                .and_then(|m| m.get_mut(key))
                .and_then(Value::as_array_mut)
                .expect("array")
                .push(new_value);
        }
    }
    if dropped > 0 {
        tracing::debug!(
            dropped,
            "wiki_map: dropped item(s) with unrecognized source_chunk_id"
        );
    }
    if dropped_identifier > 0 {
        tracing::info!(
            dropped = dropped_identifier,
            "wiki_map: dropped item(s) whose name looked like a prompt-scaffolding tag or hash"
        );
    }
    (merged, per_chunk)
}

#[cfg(test)]
mod wiki_part1_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chunk_hash_is_stable_and_rev_sensitive() {
        let h1 = chunk_hash("hello");
        let h2 = chunk_hash("hello");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16);
        assert_ne!(h1, chunk_hash("hello "));
    }

    #[test]
    fn doc_ids_normalize_scalars_and_lists() {
        assert!(wiki_doc_ids(&Value::Null).is_empty());
        assert_eq!(
            wiki_doc_ids(&json!(" d1 ")).into_iter().collect::<Vec<_>>(),
            vec!["d1".to_string()]
        );
        let ids = wiki_doc_ids(&json!(["d1", ["d2", ""], null, "d3"]));
        assert_eq!(ids.len(), 3);
        assert!(ids.contains("d1") && ids.contains("d2") && ids.contains("d3"));
    }

    #[test]
    fn compare_chunk_states_buckets() {
        let mut previous = Map::new();
        previous.insert("a".to_string(), json!({"hash": "1"}));
        previous.insert("b".to_string(), json!({"hash": "2"}));
        previous.insert("c".to_string(), json!({"hash": "3"}));
        let mut current = Map::new();
        current.insert("a".to_string(), json!({"hash": "1"}));
        current.insert("b".to_string(), json!({"hash": "9"}));
        current.insert("d".to_string(), json!({"hash": "4"}));
        let delta = compare_chunk_states(&previous, &current);
        assert_eq!(
            delta
                .unchanged_chunk_ids
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );
        assert_eq!(
            delta
                .changed_chunk_ids
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            vec!["b"]
        );
        assert_eq!(
            delta
                .deleted_chunk_ids
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            vec!["c"]
        );
        assert_eq!(
            delta
                .new_chunk_ids
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            vec!["d"]
        );
    }

    #[test]
    fn render_schema_body_skips_and_appends() {
        let fields = vec![
            json!({"name": "title", "type": "str", "description": "The {title}\nlabel"}),
            json!({"name": "title", "type": "str"}),
            json!({"name": "source_chunk_id", "type": "str"}),
            json!({"name": "count", "type": "int"}),
        ];
        let body = render_schema_body(&fields, "en", "FALLBACK", 6);
        assert!(body.contains("\"title\": \"string — The (title) label\""));
        assert_eq!(body.matches("\"title\"").count(), 1);
        assert!(body.contains("\"count\": 0"));
        assert!(body.ends_with(
            "\"source_chunk_id\": \"string — exact value from the chunk_id list above\""
        ));
        assert_eq!(render_schema_body(&[], "en", "FALLBACK", 6), "FALLBACK");
        assert_eq!(
            render_schema_body(&[json!({"type": "str"})], "en", "FALLBACK", 6),
            "FALLBACK"
        );
    }

    #[test]
    fn custom_rules_and_template_helpers() {
        let cfg = json!({"guideline": {"rules_for_entities": "Use full names"}});
        let rules = build_custom_rules(&cfg, "en");
        assert!(rules.starts_with('\n') && rules.ends_with('\n'));
        assert!(
            rules.contains(
                "## Entity extraction rules (from knowledge base config):\nUse full names"
            )
        );
        assert_eq!(build_custom_rules(&json!({}), "en"), "");
        assert_eq!(
            template_custom_rules(&json!({"global_rules": "  be terse  "})),
            "be terse"
        );
        let fields = vec![json!({"type": "person", "description": "who", "rule": "capitalized"})];
        assert_eq!(
            type_rules(&fields),
            "type: person\n  - description: who\n  - rule: capitalized"
        );
        let joined = vec![
            json!({"term": " A "}),
            json!({"term": ""}),
            json!({"term": "B"}),
        ];
        assert_eq!(pipe_join(&joined, "term"), "A|B");
        let colon = vec![
            json!({"term": "A", "definition_excerpt": "d1"}),
            json!({"term": "B"}),
        ];
        assert_eq!(colon_join(&colon, "term", "definition_excerpt"), "A:d1\nB:");
        let claim = vec![json!({"name": "Statement", "description": "the fact"})];
        assert_eq!(named_field_description(&claim, "statement"), "the fact");
        let legacy = vec![json!({"statement": "legacy text"})];
        assert_eq!(named_field_description(&legacy, "statement"), "legacy text");
    }

    #[test]
    fn render_template_is_single_pass() {
        let out = render_template("A={a} B={b} L={", &[("a", "{b}"), ("b", "x")]);
        assert_eq!(out, "A={b} B=x L={");
        assert_eq!(render_template("{unknown}", &[]), "{unknown}");
    }

    #[test]
    fn build_user_prompt_defaults_and_legacy_leg() {
        let prompt = build_user_prompt(&json!({}), "en", "doc1", 2, "C1, C2", "body");
        assert!(prompt.contains("Document id: doc1"));
        assert!(prompt.contains("Batch contains 2 packed chunk(s)"));
        assert!(prompt.contains("person|org|product|regulation|location|system|equipment|other"));
        assert!(
            prompt
                .contains("include|ordered|owns|part_of|caused_by|regulates|uses|located_in|other")
        );
        assert!(prompt.contains("named term or topic"));
        assert!(prompt.contains("factual statement"));
        let legacy = json!({"output": {"entities": {"fields": [{"name": "e1", "type": "str"}]}}});
        let prompt2 = build_user_prompt(&legacy, "en", "doc1", 1, "C1", "body");
        assert!(prompt2.contains("\"e1\": \"string\""));
        assert!(prompt2.contains(
            "\"source_chunk_id\": \"string — exact value from the chunk_id list above\""
        ));
        let custom = json!({"entity": {"fields": [{"type": "org"}]}, "global_rules": "RULE-X"});
        let prompt3 = build_user_prompt(&custom, "en", "d", 1, "C1", "b");
        assert!(prompt3.contains("RULE-X"));
        assert!(prompt3.contains("type: org"));
    }

    #[test]
    fn pick_chunk_text_truthiness() {
        assert_eq!(pick_chunk_text(&json!({"text": "t"})), "t");
        assert_eq!(
            pick_chunk_text(&json!({"text": "", "content_with_weight": "c"})),
            "c"
        );
        assert_eq!(pick_chunk_text(&json!({"content": "z"})), "z");
        assert_eq!(pick_chunk_text(&json!({"text": 7})), "");
        assert_eq!(pick_chunk_text(&json!({})), "");
    }

    #[test]
    fn scrub_strips_literals_and_bounded_hex() {
        let ids = vec!["zzz".to_string(), "".to_string()];
        let out = scrub_known_ids(
            "pre zzz post |0123456789abcdef| mid a0123456789abcdefz |0123456789abcdef0123456789abcdef| end",
            &ids,
        );
        assert!(!out.contains("zzz"));
        assert!(!out.contains("|0123456789abcdef|"));
        assert!(out.contains("a0123456789abcdefz"));
        assert!(!out.contains("|0123456789abcdef0123456789abcdef|"));
        assert_eq!(
            scrub_hex_tokens("x0123456789abcdefy", 16),
            "x0123456789abcdefy"
        );
        assert_eq!(scrub_hex_tokens("(0123456789abcdef)", 16), "()");
    }

    #[test]
    fn identifier_shapes() {
        assert!(looks_like_identifier("C1"));
        assert!(looks_like_identifier(" c0001 "));
        assert!(looks_like_identifier("d523a888c5b2a167"));
        assert!(looks_like_identifier("41a5271858ca11f1bbb9047c16ec874f"));
        assert!(looks_like_identifier(
            "123e4567-e89b-12d3-a456-426614174000"
        ));
        assert!(!looks_like_identifier("Widget"));
        assert!(!looks_like_identifier("C123456"));
        assert!(!looks_like_identifier("xyz"));
        assert!(item_has_identifier_name("entities", &json!({"name": "C2"})));
        assert!(item_has_identifier_name("concepts", &json!({"term": "c3"})));
        assert!(item_has_identifier_name(
            "claims",
            &json!({"subject": "abcdef0123456789"})
        ));
        assert!(item_has_identifier_name(
            "relations",
            &json!({"from": "ok", "to": "C9"})
        ));
        assert!(!item_has_identifier_name(
            "relations",
            &json!({"from": "A", "to": "B"})
        ));
    }

    #[test]
    fn batch_prompt_and_unwrap() {
        let packed = vec![
            json!({"label": "C1", "text": "one"}),
            json!({"label": "C2", "text": "two"}),
        ];
        let (body, labels) = format_batch_prompt(&packed);
        assert_eq!(labels, vec!["C1".to_string(), "C2".to_string()]);
        assert_eq!(body, "[CHUNK_ID C1]\none\n\n[CHUNK_ID C2]\ntwo");
        let unwrapped = unwrap_extract(&json!({
            "entities": [{"name": "A"}, 5],
            "concepts": "bad",
            "topics": ["t1", "  ", 7]
        }));
        assert_eq!(unwrapped["entities"].as_array().unwrap().len(), 1);
        assert_eq!(unwrapped["concepts"].as_array().unwrap().len(), 0);
        assert_eq!(unwrapped["topics"], json!(["t1"]));
        let empty = unwrap_extract(&Value::Null);
        assert_eq!(empty["claims"], json!([]));
    }

    #[test]
    fn resolve_chunk_ids_splits_drops_and_copies_topics() {
        let mut label_to_id = HashMap::new();
        label_to_id.insert("C1".to_string(), "id1".to_string());
        label_to_id.insert("C2".to_string(), "id2".to_string());
        let extract = json!({
            "entities": [
                {"name": "Alpha", "source_chunk_id": "C1"},
                {"name": "Beta", "source_chunk_id": "C9"},
                {"name": "C2", "source_chunk_id": "C2"}
            ],
            "claims": [{"statement": "s", "source_chunk_id": "C2"}],
            "topics": ["t"]
        });
        let (merged, per_chunk) = resolve_chunk_ids(&extract, &label_to_id);
        let entities = merged["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0]["name"], json!("Alpha"));
        assert_eq!(entities[0]["chunk_ids"], json!(["id1"]));
        assert!(entities[0].get("source_chunk_id").is_none());
        assert_eq!(merged["topics"], json!(["t"]));
        let id2 = &per_chunk["id2"];
        assert_eq!(id2["claims"].as_array().unwrap().len(), 1);
        assert_eq!(id2["topics"], json!(["t"]));
        let id1 = &per_chunk["id1"];
        assert_eq!(id1["claims"].as_array().unwrap().len(), 0);
        assert_eq!(id1["entities"].as_array().unwrap().len(), 1);
    }
}

// ---------------------------------------------------------------------------
// Part 2 — resume/state store group (`_wiki_merge_extracts` ..
// `_wiki_commit_active_map_state`).
//
// Adaptation note: RayRAG's doc store matches conditions by JSONB
// containment / exact equality on scalar fields, not Elasticsearch `terms`.
// Upstream conditions that used to rely on `terms` lists are therefore
// rewritten as scalar conditions plus per-value loops and client-side
// filtering (same row sets); `must_not exists` is enforced client-side.
// ---------------------------------------------------------------------------

/// `_wiki_merge_extracts`: concat the five lists across batch extracts.
pub fn merge_extracts(extracts: &[Value]) -> Value {
    let mut out = empty_extract();
    let mut seen_topics: BTreeSet<String> = BTreeSet::new();
    for extract in extracts {
        let Some(obj) = extract.as_object() else {
            continue;
        };
        for key in EXTRACT_LIST_KEYS {
            if let Some(Value::Array(items)) = obj.get(key) {
                if let Some(target) = out.get_mut(key).and_then(Value::as_array_mut) {
                    target.extend(items.iter().cloned());
                }
            }
        }
        if let Some(Value::Array(topics)) = obj.get("topics") {
            for topic in topics {
                let dedup_key = topic
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| topic.to_string());
                if seen_topics.insert(dedup_key) {
                    if let Some(target) = out.get_mut("topics").and_then(Value::as_array_mut) {
                        target.push(topic.clone());
                    }
                }
            }
        }
    }
    out
}

/// `_wiki_build_resume_doc`: one non-searchable ES-style row per chunk.
pub fn build_resume_doc(
    chunk_id: &str,
    doc_id: &str,
    per_chunk_extract: &Value,
    chunk_hash: &str,
) -> Value {
    let doc_id_str = doc_id.to_string();
    json!({
        "id": stable_row_id(&[
            WIKI_MAP_COMPILE_KWD.to_string(),
            doc_id_str.clone(),
            chunk_id.to_string(),
            chunk_hash.to_string(),
        ]),
        "doc_id": doc_id_str,
        "compile_kwd": WIKI_MAP_COMPILE_KWD,
        "source_chunk_ids": [chunk_id],
        "chunk_hash_kwd": chunk_hash,
        "content_with_weight": per_chunk_extract.to_string(),
        "available_int": 0,
    })
}

/// One page of rows through the local doc store (select + condition).
fn search_page(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    fields: &[String],
    condition: &Map<String, Value>,
    offset: usize,
    limit: usize,
) -> crate::Result<Vec<Value>> {
    let query = SearchQuery {
        select_fields: fields.to_vec(),
        condition: condition.clone(),
        match_expressions: Vec::new(),
        offset,
        limit,
        index_names: vec![index_name(tenant_id)],
        dataset_ids: vec![kb_id.to_string()],
        ..Default::default()
    };
    let response = store.search(&query)?;
    Ok(store
        .get_fields(&response, fields)
        .into_values()
        .map(Value::Object)
        .collect())
}

/// `_wiki_load_map_versions`: historical MAP versions as
/// `chunk_id -> hash -> extract` (partial result on read failure).
pub fn load_map_versions(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    doc_ids: &BTreeSet<String>,
    requested_versions: Option<&Map<String, Value>>,
) -> Map<String, Value> {
    let fields: Vec<String> = ["source_chunk_ids", "chunk_hash_kwd", "content_with_weight"]
        .iter()
        .map(|field| field.to_string())
        .collect();
    let mut versions: Map<String, Value> = Map::new();
    let requested_chunk_ids: BTreeSet<String> = requested_versions
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    let requested_hashes: BTreeSet<String> = requested_versions
        .map(|map| {
            map.values()
                .filter_map(Value::as_str)
                .filter(|hash| !hash.is_empty())
                .map(|hash| hash.to_string())
                .collect()
        })
        .unwrap_or_default();
    for doc_id in doc_ids {
        let mut condition = Map::new();
        condition.insert(
            "compile_kwd".to_string(),
            Value::String(WIKI_MAP_COMPILE_KWD.to_string()),
        );
        condition.insert("doc_id".to_string(), Value::String(doc_id.clone()));
        let mut offset = 0usize;
        loop {
            let rows = match search_page(store, tenant_id, kb_id, &fields, &condition, offset, 1000)
            {
                Ok(rows) => rows,
                Err(err) => {
                    tracing::warn!(error = %err, doc = %doc_id, "wiki_map: failed to load historical versions");
                    return versions;
                }
            };
            for row in &rows {
                let chunk_hash = row
                    .get("chunk_hash_kwd")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if chunk_hash.is_empty() {
                    continue;
                }
                if !requested_hashes.is_empty() && !requested_hashes.contains(&chunk_hash) {
                    continue;
                }
                let extract: Value = row
                    .get("content_with_weight")
                    .and_then(Value::as_str)
                    .and_then(|body| serde_json::from_str(body).ok())
                    .unwrap_or(Value::Null);
                if !extract.is_object() {
                    continue;
                }
                for chunk_id in wiki_doc_ids(row.get("source_chunk_ids").unwrap_or(&Value::Null)) {
                    if !requested_chunk_ids.is_empty() && !requested_chunk_ids.contains(&chunk_id) {
                        continue;
                    }
                    if let Some(requested) = requested_versions {
                        let want = requested
                            .get(&chunk_id)
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if want != chunk_hash {
                            continue;
                        }
                    }
                    let entry = versions
                        .entry(chunk_id)
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Some(map) = entry.as_object_mut() {
                        map.entry(chunk_hash.clone())
                            .or_insert_with(|| extract.clone());
                    }
                }
            }
            if rows.len() < 1000 {
                break;
            }
            offset += 1000;
        }
    }
    versions
}

/// `_wiki_persist_extracts`: write one non-searchable row per chunk.
pub fn persist_extracts(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    per_chunk: &Map<String, Value>,
    doc_id: &str,
    chunk_hashes: Option<&Map<String, Value>>,
) {
    if per_chunk.is_empty() {
        return;
    }
    let mut docs: Vec<Value> = Vec::new();
    for (chunk_id, extract) in per_chunk {
        if chunk_id.is_empty() {
            continue;
        }
        let hash = chunk_hashes
            .and_then(|map| map.get(chunk_id))
            .and_then(Value::as_str)
            .unwrap_or("");
        docs.push(build_resume_doc(chunk_id, doc_id, extract, hash));
    }
    if docs.is_empty() {
        return;
    }
    let rows: Vec<DocRow> = docs
        .iter()
        .filter_map(|doc| doc.as_object().cloned())
        .collect();
    if let Err(err) = store.insert(&rows, &index_name(tenant_id), kb_id) {
        tracing::warn!(error = %err, count = rows.len(), "wiki_map: failed to persist resume docs");
    }
}

/// `_wiki_scan_current_chunk_state`: current MAP input hashes of enabled
/// source chunks (`must_not exists compile_kwd` enforced client-side).
pub fn scan_current_chunk_state(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    doc_ids: &BTreeSet<String>,
) -> crate::Result<Map<String, Value>> {
    let mut state: Map<String, Value> = Map::new();
    if doc_ids.is_empty() {
        return Ok(state);
    }
    let fields: Vec<String> = ["id", "doc_id", "content_with_weight", "compile_kwd"]
        .iter()
        .map(|field| field.to_string())
        .collect();
    for doc_id in doc_ids {
        let mut condition = Map::new();
        condition.insert("doc_id".to_string(), Value::String(doc_id.clone()));
        condition.insert("available_int".to_string(), json!(1));
        let mut offset = 0usize;
        loop {
            let rows = search_page(store, tenant_id, kb_id, &fields, &condition, offset, 1000)?;
            for row in &rows {
                if row
                    .get("compile_kwd")
                    .map(|value| !value.is_null())
                    .unwrap_or(false)
                {
                    continue;
                }
                let chunk_id = row
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if chunk_id.is_empty() {
                    continue;
                }
                let mut entry = Map::new();
                entry.insert(
                    "doc_id".to_string(),
                    Value::String(
                        row.get("doc_id")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| doc_id.clone()),
                    ),
                );
                entry.insert(
                    "hash".to_string(),
                    Value::String(chunk_hash(
                        row.get("content_with_weight")
                            .and_then(Value::as_str)
                            .unwrap_or(""),
                    )),
                );
                state.insert(chunk_id, Value::Object(entry));
            }
            if rows.len() < 1000 {
                break;
            }
            offset += 1000;
        }
    }
    Ok(state)
}

/// `_wiki_load_active_map_generation`: the active generation marker value.
pub fn load_active_map_generation(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
) -> crate::Result<String> {
    let fields: Vec<String> = vec!["type_kwd".to_string()];
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_MAP_STATE_META_COMPILE_KWD.to_string()),
    );
    condition.insert(
        "id".to_string(),
        Value::String(stable_row_id(&[
            WIKI_MAP_STATE_META_COMPILE_KWD.to_string(),
            kb_id.to_string(),
        ])),
    );
    let rows = search_page(store, tenant_id, kb_id, &fields, &condition, 0, 1)?;
    for row in &rows {
        let values = wiki_doc_ids(row.get("type_kwd").unwrap_or(&Value::Null));
        if let Some(first) = values.into_iter().next() {
            return Ok(first);
        }
    }
    Ok(String::new())
}

/// `_wiki_load_active_map_state`: chunk versions of the last committed build.
pub fn load_active_map_state(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
) -> crate::Result<Map<String, Value>> {
    let generation = load_active_map_generation(store, tenant_id, kb_id)?;
    if generation.is_empty() {
        return Ok(Map::new());
    }
    let fields: Vec<String> = ["doc_id", "source_chunk_ids", "chunk_hash_kwd"]
        .iter()
        .map(|field| field.to_string())
        .collect();
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_MAP_STATE_COMPILE_KWD.to_string()),
    );
    condition.insert("type_kwd".to_string(), Value::String(generation));
    let mut state: Map<String, Value> = Map::new();
    let mut offset = 0usize;
    loop {
        let rows = search_page(store, tenant_id, kb_id, &fields, &condition, offset, 1000)?;
        for row in &rows {
            let chunk_hash = row
                .get("chunk_hash_kwd")
                .and_then(Value::as_str)
                .unwrap_or("");
            if chunk_hash.is_empty() {
                continue;
            }
            let doc_id = wiki_doc_ids(row.get("doc_id").unwrap_or(&Value::Null))
                .into_iter()
                .next()
                .unwrap_or_default();
            for chunk_id in wiki_doc_ids(row.get("source_chunk_ids").unwrap_or(&Value::Null)) {
                let mut entry = Map::new();
                entry.insert("doc_id".to_string(), Value::String(doc_id.clone()));
                entry.insert("hash".to_string(), Value::String(chunk_hash.to_string()));
                state.insert(chunk_id, Value::Object(entry));
            }
        }
        if rows.len() < 1000 {
            break;
        }
        offset += 1000;
    }
    Ok(state)
}

/// `_wiki_load_map_extracts_for_state`: MAP versions selected by a snapshot.
pub fn load_map_extracts_for_state(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    state: &Map<String, Value>,
    chunk_ids: Option<&BTreeSet<String>>,
) -> Vec<Value> {
    let mut selected: BTreeSet<String> = state.keys().cloned().collect();
    if let Some(ids) = chunk_ids {
        selected = selected.intersection(ids).cloned().collect();
    }
    if selected.is_empty() {
        return Vec::new();
    }
    let mut by_doc: Map<String, Value> = Map::new();
    for chunk_id in &selected {
        let doc_id = state
            .get(chunk_id)
            .and_then(|item| item.get("doc_id"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if doc_id.is_empty() {
            continue;
        }
        by_doc
            .entry(doc_id)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("array")
            .push(Value::String(chunk_id.clone()));
    }
    let mut requested: Map<String, Value> = Map::new();
    for ids in by_doc.values() {
        for id in ids.as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let chunk_id = id.as_str().unwrap_or("").to_string();
            let hash = state
                .get(&chunk_id)
                .and_then(|item| item.get("hash"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            requested.insert(chunk_id, Value::String(hash));
        }
    }
    let doc_set: BTreeSet<String> = by_doc.keys().cloned().collect();
    let versions = load_map_versions(store, tenant_id, kb_id, &doc_set, Some(&requested));
    let mut extracts: Vec<Value> = Vec::new();
    for (doc_id, ids) in &by_doc {
        for id in ids.as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let chunk_id = id.as_str().unwrap_or("").to_string();
            let chunk_hash = state
                .get(&chunk_id)
                .and_then(|item| item.get("hash"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let Some(extract) = versions
                .get(&chunk_id)
                .and_then(|by_hash| by_hash.get(&chunk_hash))
            else {
                continue;
            };
            if !extract.is_object() {
                continue;
            }
            let mut item = extract.as_object().cloned().unwrap_or_default();
            item.insert("doc_id".to_string(), Value::String(doc_id.clone()));
            let mut map_version = Map::new();
            map_version.insert("chunk_id".to_string(), Value::String(chunk_id.clone()));
            map_version.insert("hash".to_string(), Value::String(chunk_hash.clone()));
            item.insert("_map_version".to_string(), Value::Object(map_version));
            extracts.push(Value::Object(item));
        }
    }
    extracts
}

/// `_wiki_commit_active_map_state`: atomically switch the active snapshot.
pub fn commit_active_map_state(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    state: &Map<String, Value>,
) -> crate::Result<()> {
    let previous_generation = load_active_map_generation(store, tenant_id, kb_id)?;
    let generation = Uuid::new_v4().simple().to_string();
    let mut rows: Vec<DocRow> = Vec::new();
    for (chunk_id, item) in state {
        let doc_id = item
            .get("doc_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let chunk_hash = item
            .get("hash")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if doc_id.is_empty() || chunk_hash.is_empty() {
            continue;
        }
        let mut row = Map::new();
        row.insert(
            "id".to_string(),
            Value::String(stable_row_id(&[
                WIKI_MAP_STATE_COMPILE_KWD.to_string(),
                generation.clone(),
                doc_id.clone(),
                chunk_id.clone(),
            ])),
        );
        row.insert("doc_id".to_string(), Value::String(doc_id));
        row.insert(
            "compile_kwd".to_string(),
            Value::String(WIKI_MAP_STATE_COMPILE_KWD.to_string()),
        );
        row.insert("type_kwd".to_string(), Value::String(generation.clone()));
        row.insert(
            "source_chunk_ids".to_string(),
            Value::Array(vec![Value::String(chunk_id.clone())]),
        );
        row.insert("chunk_hash_kwd".to_string(), Value::String(chunk_hash));
        row.insert(
            "content_with_weight".to_string(),
            Value::String("{}".to_string()),
        );
        row.insert("available_int".to_string(), json!(0));
        rows.push(row);
    }
    if !rows.is_empty() {
        store.insert(&rows, &index_name(tenant_id), kb_id)?;
    }
    let mut marker = Map::new();
    marker.insert(
        "id".to_string(),
        Value::String(stable_row_id(&[
            WIKI_MAP_STATE_META_COMPILE_KWD.to_string(),
            kb_id.to_string(),
        ])),
    );
    marker.insert("doc_id".to_string(), Value::String(String::new()));
    marker.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_MAP_STATE_META_COMPILE_KWD.to_string()),
    );
    marker.insert("type_kwd".to_string(), Value::String(generation.clone()));
    marker.insert(
        "source_chunk_ids".to_string(),
        Value::Array(vec![Value::String("__wiki_map_state__".to_string())]),
    );
    marker.insert(
        "chunk_hash_kwd".to_string(),
        Value::String("committed".to_string()),
    );
    marker.insert(
        "content_with_weight".to_string(),
        Value::String("{}".to_string()),
    );
    marker.insert("available_int".to_string(), json!(0));
    store.insert(&[marker], &index_name(tenant_id), kb_id)?;
    if !previous_generation.is_empty() && previous_generation != generation {
        let mut condition = Map::new();
        condition.insert(
            "compile_kwd".to_string(),
            Value::String(WIKI_MAP_STATE_COMPILE_KWD.to_string()),
        );
        condition.insert(
            "type_kwd".to_string(),
            Value::String(previous_generation.clone()),
        );
        if let Err(err) = store.delete(&condition, &index_name(tenant_id), kb_id) {
            tracing::warn!(
                error = %err,
                generation = %previous_generation,
                "wiki_map: failed to remove inactive state generation"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod wiki_part2_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;

    #[test]
    fn merge_extracts_concats_and_dedups_topics() {
        let merged = merge_extracts(&[
            json!({"entities": [{"name": "A"}], "topics": ["t1", "t2"]}),
            json!({"entities": [{"name": "B"}], "claims": [{"statement": "s"}], "topics": ["t2", "t3"]}),
        ]);
        assert_eq!(merged["entities"].as_array().unwrap().len(), 2);
        assert_eq!(merged["claims"].as_array().unwrap().len(), 1);
        assert_eq!(merged["topics"], json!(["t1", "t2", "t3"]));
    }

    #[test]
    fn resume_doc_shape_roundtrips() {
        let doc = build_resume_doc("c1", "d1", &json!({"topics": ["t"]}), "h1");
        assert_eq!(doc["compile_kwd"], json!("wiki_map_extract"));
        assert_eq!(doc["source_chunk_ids"], json!(["c1"]));
        assert_eq!(doc["chunk_hash_kwd"], json!("h1"));
        assert_eq!(doc["available_int"], json!(0));
        assert_eq!(doc["id"].as_str().unwrap().len(), 16);
        let parsed: Value =
            serde_json::from_str(doc["content_with_weight"].as_str().unwrap()).unwrap();
        assert_eq!(parsed["topics"], json!(["t"]));
    }

    #[test]
    fn persist_and_load_versions_roundtrip() {
        let store = MemoryDocStore::new();
        let mut per_chunk = Map::new();
        per_chunk.insert(
            "c1".to_string(),
            json!({"entities": [{"name": "A"}], "topics": ["t"]}),
        );
        let mut hashes = Map::new();
        hashes.insert("c1".to_string(), Value::String("h1".to_string()));
        persist_extracts(&store, "t1", "kb1", &per_chunk, "d1", Some(&hashes));
        let docs = BTreeSet::from(["d1".to_string()]);
        let versions = load_map_versions(&store, "t1", "kb1", &docs, None);
        let by_hash = versions.get("c1").expect("chunk versions");
        assert_eq!(by_hash["h1"]["entities"][0]["name"], json!("A"));
        let mut wrong = Map::new();
        wrong.insert("c1".to_string(), Value::String("hX".to_string()));
        assert!(load_map_versions(&store, "t1", "kb1", &docs, Some(&wrong)).is_empty());
        let mut right = Map::new();
        right.insert("c1".to_string(), Value::String("h1".to_string()));
        assert!(!load_map_versions(&store, "t1", "kb1", &docs, Some(&right)).is_empty());
    }

    #[test]
    fn scan_state_excludes_compile_rows() {
        let store = MemoryDocStore::new();
        let rows: Vec<DocRow> = vec![
            json!({"id": "c1", "doc_id": "d1", "content_with_weight": "hello", "available_int": 1})
                .as_object()
                .cloned()
                .unwrap(),
            json!({"id": "x1", "doc_id": "d1", "content_with_weight": "cached", "available_int": 1, "compile_kwd": "wiki_map_extract"})
                .as_object()
                .cloned()
                .unwrap(),
        ];
        store.insert(&rows, &index_name("t1"), "kb1").unwrap();
        let state =
            scan_current_chunk_state(&store, "t1", "kb1", &BTreeSet::from(["d1".to_string()]))
                .unwrap();
        assert!(state.contains_key("c1"));
        assert!(!state.contains_key("x1"));
        assert_eq!(state["c1"]["hash"], json!(chunk_hash("hello")));
        assert_eq!(state["c1"]["doc_id"], json!("d1"));
    }

    #[test]
    fn commit_switches_generation_and_cleans_previous() {
        let store = MemoryDocStore::new();
        let mut state = Map::new();
        let mut entry = Map::new();
        entry.insert("doc_id".to_string(), Value::String("d1".to_string()));
        entry.insert("hash".to_string(), Value::String("h1".to_string()));
        state.insert("c1".to_string(), Value::Object(entry));
        commit_active_map_state(&store, "t1", "kb1", &state).unwrap();
        let generation_1 = load_active_map_generation(&store, "t1", "kb1").unwrap();
        assert_eq!(generation_1.len(), 32);
        let loaded = load_active_map_state(&store, "t1", "kb1").unwrap();
        assert_eq!(loaded["c1"]["doc_id"], json!("d1"));
        assert_eq!(loaded["c1"]["hash"], json!("h1"));

        let mut state2 = Map::new();
        let mut entry2 = Map::new();
        entry2.insert("doc_id".to_string(), Value::String("d2".to_string()));
        entry2.insert("hash".to_string(), Value::String("h2".to_string()));
        state2.insert("c2".to_string(), Value::Object(entry2));
        commit_active_map_state(&store, "t1", "kb1", &state2).unwrap();
        let generation_2 = load_active_map_generation(&store, "t1", "kb1").unwrap();
        assert_ne!(generation_1, generation_2);
        let loaded2 = load_active_map_state(&store, "t1", "kb1").unwrap();
        assert!(loaded2.contains_key("c2"));
        assert!(!loaded2.contains_key("c1"));

        let mut stale = Map::new();
        stale.insert(
            "compile_kwd".to_string(),
            Value::String(WIKI_MAP_STATE_COMPILE_KWD.to_string()),
        );
        stale.insert("type_kwd".to_string(), Value::String(generation_1));
        let query = SearchQuery {
            select_fields: vec!["id".to_string()],
            condition: stale,
            match_expressions: Vec::new(),
            offset: 0,
            limit: 10,
            index_names: vec![index_name("t1")],
            dataset_ids: vec!["kb1".to_string()],
            ..Default::default()
        };
        let response = store.search(&query).unwrap();
        assert_eq!(response.total, 0);
    }

    #[test]
    fn extracts_for_state_annotates_selection() {
        let store = MemoryDocStore::new();
        let mut per_chunk = Map::new();
        per_chunk.insert(
            "c1".to_string(),
            json!({"entities": [{"name": "Alpha"}], "topics": ["t"]}),
        );
        let mut hashes = Map::new();
        hashes.insert("c1".to_string(), Value::String("h1".to_string()));
        persist_extracts(&store, "t1", "kb1", &per_chunk, "d1", Some(&hashes));
        let mut state = Map::new();
        let mut entry = Map::new();
        entry.insert("doc_id".to_string(), Value::String("d1".to_string()));
        entry.insert("hash".to_string(), Value::String("h1".to_string()));
        state.insert("c1".to_string(), Value::Object(entry));
        let extracts = load_map_extracts_for_state(&store, "t1", "kb1", &state, None);
        assert_eq!(extracts.len(), 1);
        assert_eq!(extracts[0]["doc_id"], json!("d1"));
        assert_eq!(extracts[0]["_map_version"]["chunk_id"], json!("c1"));
        assert_eq!(extracts[0]["_map_version"]["hash"], json!("h1"));
        assert_eq!(extracts[0]["entities"][0]["name"], json!("Alpha"));
        let none = load_map_extracts_for_state(&store, "t1", "kb1", &state, Some(&BTreeSet::new()));
        assert!(none.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Part 3 — per-batch extraction and the public MAP entry
// (`_wiki_extract_one_batch` .. `wiki_map_from_chunks`).
//
// Adaptation note: upstream fans batches out through `_run_chunked_pipeline`
// with an asyncio semaphore; the Rust closure bounds of
// `run_chunked_pipeline` require `'static` captures, so this port runs the
// packed batches sequentially in order (`max_workers` is accepted and only
// recorded). Generated JSON is cleaned exactly like `struct_gen_json`.
// ---------------------------------------------------------------------------

fn wiki_think_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)^.*</think>").expect("think regex"))
}

fn wiki_fence_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"```(?:json)?\s*|\s*```").expect("fence regex"))
}

/// `gen_json` over `HarnessChat`, with the same think/fence cleaning that
/// `struct_gen_json` applies (json_repair-backed lenient parse fallback).
async fn wiki_gen_json(
    chat: &dyn HarnessChat,
    system: &str,
    user: &str,
    gen_conf: &Value,
) -> Option<Value> {
    let history = vec![json!({"role": "user", "content": user})];
    let raw = chat.chat(system, &history, gen_conf).await.ok()?;
    let stripped = wiki_think_re().replace(&raw, "").to_string();
    let cleaned = wiki_fence_re()
        .replace_all(&stripped, "")
        .trim()
        .to_string();
    serde_json::from_str::<Value>(&cleaned)
        .ok()
        .or_else(|| parse_json_lenient(&cleaned))
}

/// `_wiki_extract_one_batch`: one LLM call for one packed batch.
pub async fn wiki_extract_one_batch(
    chat: &dyn HarnessChat,
    packed: &[PackedEntry],
    doc_id: &str,
    language: &str,
    llm_timeout: i64,
    parser_config: &Value,
    batch_idx: usize,
    total_batches: usize,
    callback: Option<&(dyn Fn(f64, &str) + Send + Sync)>,
) -> Option<Value> {
    let (body, labels) = format_batch_prompt(
        &packed
            .iter()
            .map(|entry| json!({"label": entry.label, "text": entry.text}))
            .collect::<Vec<Value>>(),
    );
    let chunk_id_list = labels
        .iter()
        .map(|label| format!("- {label}"))
        .collect::<Vec<String>>()
        .join("\n");
    let user_prompt = build_user_prompt(
        parser_config,
        language,
        doc_id,
        packed.len(),
        &chunk_id_list,
        &body,
    );
    let gen_conf = knowledge_compile_gen_conf(
        &chat.model_name(),
        Some(&Map::from_iter([("temperature".to_string(), json!(0.1))])),
    );
    let request_conf = Value::Object(gen_conf);
    let timeout = std::time::Duration::from_secs(llm_timeout.max(0) as u64);
    let outcome = tokio::time::timeout(
        timeout,
        wiki_gen_json(chat, WIKI_MAP_SYSTEM, &user_prompt, &request_conf),
    )
    .await;
    match outcome {
        Err(_elapsed) => {
            tracing::warn!(
                seconds = llm_timeout,
                chunks = packed.len(),
                "wiki_map: batch extraction timed out"
            );
            if let Some(callback) = callback {
                callback(
                    (batch_idx + 1) as f64 / (total_batches.max(1)) as f64,
                    &format!(
                        "[ERROR] Wiki MAP batch {}/{} timed out after {}s ({} chunks).",
                        batch_idx + 1,
                        total_batches,
                        llm_timeout,
                        packed.len()
                    ),
                );
            }
            None
        }
        Ok(None) => {
            tracing::warn!(chunks = packed.len(), "wiki_map: batch extraction failed");
            if let Some(callback) = callback {
                callback(
                    (batch_idx + 1) as f64 / (total_batches.max(1)) as f64,
                    &format!(
                        "[ERROR] Wiki MAP batch {}/{} failed ({} chunks).",
                        batch_idx + 1,
                        total_batches,
                        packed.len()
                    ),
                );
            }
            None
        }
        Ok(Some(res)) => {
            let _ = language; // reserved for future localization
            Some(unwrap_extract(&res))
        }
    }
}

/// `_wiki_process_batch`: LLM extract → split by source chunk → persist.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_process_batch(
    chat: &dyn HarnessChat,
    store: &dyn DocStore,
    packed: &[PackedEntry],
    batch_idx: usize,
    total_batches: usize,
    doc_id: &str,
    tenant_id: &str,
    kb_id: &str,
    language: &str,
    llm_timeout: i64,
    callback: Option<&(dyn Fn(f64, &str) + Send + Sync)>,
    parser_config: &Value,
    chunk_hashes: Option<&Map<String, Value>>,
) -> Value {
    if packed.is_empty() {
        return empty_extract();
    }
    let mut label_to_id: HashMap<String, String> = HashMap::new();
    for entry in packed {
        label_to_id.insert(entry.label.clone(), entry.chunk_id.clone());
    }
    let raw_extract = wiki_extract_one_batch(
        chat,
        packed,
        doc_id,
        language,
        llm_timeout,
        parser_config,
        batch_idx,
        total_batches,
        callback,
    )
    .await;
    let Some(raw_extract) = raw_extract else {
        // LLM call failed/timed out: leave no resume hash so the next run
        // re-extracts these chunks instead of locking in an empty result.
        return empty_extract();
    };
    let (merged, per_chunk) = resolve_chunk_ids(&raw_extract, &label_to_id);
    persist_extracts(store, tenant_id, kb_id, &per_chunk, doc_id, chunk_hashes);
    if let Some(callback) = callback {
        let n_items: usize = EXTRACT_LIST_KEYS
            .iter()
            .map(|key| {
                merged
                    .get(*key)
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0)
            })
            .sum();
        callback(
            (batch_idx + 1) as f64 / (total_batches.max(1)) as f64,
            &format!(
                "Wiki MAP {}/{}: {} items from {} chunks",
                batch_idx + 1,
                total_batches,
                n_items,
                packed.len()
            ),
        );
    }
    merged
}

fn wiki_meta(doc_id: &str, requested: usize, cache_hits: usize, extracted: usize) -> Value {
    json!({
        "doc_id": doc_id,
        "requested": requested,
        "cache_hits": cache_hits,
        "extracted": extracted,
    })
}

/// `wiki_map_from_chunks`: MAP phase of the wiki compilation pipeline.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_map_from_chunks(
    store: &dyn DocStore,
    chat: &dyn HarnessChat,
    chunks: &[Value],
    doc_id: &str,
    tenant_id: &str,
    kb_id: &str,
    language: &str,
    max_workers: usize,
    llm_timeout: i64,
    parser_config: &Value,
    batch_size_cap: Option<usize>,
    window_fraction: Option<f64>,
    target_chunk_ids: Option<&BTreeSet<String>>,
    callback: Option<&(dyn Fn(f64, &str) + Send + Sync)>,
) -> Value {
    let _ = max_workers; // sequential execution; kept for signature parity
    if chunks.is_empty() {
        let mut out = empty_extract();
        out.as_object_mut()
            .expect("object")
            .insert("_meta".to_string(), wiki_meta(doc_id, 0, 0, 0));
        return out;
    }

    let mut current_chunk_hashes: Map<String, Value> = Map::new();
    for chunk in chunks {
        let cid = chunk
            .get("id")
            .or_else(|| chunk.get("chunk_id"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if cid.is_empty() {
            continue;
        }
        let text = pick_chunk_text(chunk);
        current_chunk_hashes.insert(cid.to_string(), Value::String(chunk_hash(&text)));
    }

    let mut requested_ids: BTreeSet<String> = current_chunk_hashes.keys().cloned().collect();
    if let Some(target) = target_chunk_ids {
        requested_ids = requested_ids.intersection(target).cloned().collect();
    }

    let mut requested_versions: Map<String, Value> = Map::new();
    for chunk_id in &requested_ids {
        if let Some(hash) = current_chunk_hashes.get(chunk_id) {
            requested_versions.insert(chunk_id.clone(), hash.clone());
        }
    }
    let historical_versions = load_map_versions(
        store,
        tenant_id,
        kb_id,
        &BTreeSet::from([doc_id.to_string()]),
        Some(&requested_versions),
    );
    let mut cache_hits: Vec<Value> = Vec::new();
    let mut cache_hit_ids: BTreeSet<String> = BTreeSet::new();
    for chunk_id in &requested_ids {
        let hash = current_chunk_hashes
            .get(chunk_id)
            .and_then(Value::as_str)
            .unwrap_or("");
        let extract = historical_versions
            .get(chunk_id)
            .and_then(|by_hash| by_hash.get(hash));
        if let Some(extract) = extract {
            cache_hit_ids.insert(chunk_id.clone());
            cache_hits.push(extract.clone());
        }
    }

    let extract_ids: BTreeSet<String> = requested_ids.difference(&cache_hit_ids).cloned().collect();
    // Skip chunks outside this run's delta as well as historical cache hits.
    let resume_set: std::collections::HashSet<String> = current_chunk_hashes
        .keys()
        .filter(|chunk_id| !extract_ids.contains(*chunk_id))
        .cloned()
        .collect();

    // Defensive scrub: chunkers sometimes embed the chunk_id / doc_id into
    // the body; without this the LLM tends to extract the hash as an entity.
    let mut all_known_ids: Vec<String> = Vec::new();
    for chunk in chunks {
        let cid = chunk
            .get("id")
            .or_else(|| chunk.get("chunk_id"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if !cid.is_empty() {
            all_known_ids.push(cid.to_string());
        }
    }
    if !doc_id.is_empty() {
        all_known_ids.push(doc_id.to_string());
    }

    let overhead =
        crate::chunk::tokenizer::token_count(&format!("{WIKI_MAP_SYSTEM}{WIKI_MAP_USER_TEMPLATE}"));
    let mut inputs: Vec<ChunkInput> = Vec::new();
    for chunk in chunks {
        let cid = chunk
            .get("id")
            .or_else(|| chunk.get("chunk_id"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if cid.is_empty() {
            continue;
        }
        inputs.push(ChunkInput {
            id: cid.to_string(),
            text: scrub_known_ids(&pick_chunk_text(chunk), &all_known_ids),
        });
    }
    let (batches, _info) = build_chunk_batches(
        &inputs,
        chat.max_length(),
        overhead,
        Some(&resume_set),
        batch_size_cap,
        window_fraction,
        1024,
    );
    let cached_merged = merge_extracts(&cache_hits);
    if batches.is_empty() {
        let mut out = cached_merged;
        out.as_object_mut().expect("object").insert(
            "_meta".to_string(),
            wiki_meta(doc_id, requested_ids.len(), cache_hit_ids.len(), 0),
        );
        return out;
    }

    let mut extracted_batches: Vec<Value> = Vec::new();
    let total_batches = batches.len();
    for (batch_idx, batch) in batches.iter().enumerate() {
        let merged_batch = wiki_process_batch(
            chat,
            store,
            batch,
            batch_idx,
            total_batches,
            doc_id,
            tenant_id,
            kb_id,
            language,
            llm_timeout,
            callback,
            parser_config,
            Some(&current_chunk_hashes),
        )
        .await;
        extracted_batches.push(merged_batch);
    }
    let extracted = merge_extracts(&extracted_batches);
    let merged = merge_extracts(&[cached_merged, extracted]);
    let counts = |key: &str| {
        merged
            .get(key)
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0)
    };
    tracing::info!(
        doc = doc_id,
        requested = requested_ids.len(),
        cache_hits = cache_hit_ids.len(),
        extracted = extract_ids.len(),
        entities = counts("entities"),
        concepts = counts("concepts"),
        claims = counts("claims"),
        relations = counts("relations"),
        topics = counts("topics"),
        "wiki_map: document processed"
    );
    let mut merged = merged;
    merged.as_object_mut().expect("object").insert(
        "_meta".to_string(),
        wiki_meta(
            doc_id,
            requested_ids.len(),
            cache_hit_ids.len(),
            extract_ids.len(),
        ),
    );
    merged
}

#[cfg(test)]
mod wiki_part3_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;
    use crate::harness::HarnessChat;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct FakeChat {
        reply: String,
        max_length: usize,
        calls: Mutex<Vec<String>>,
    }

    impl FakeChat {
        fn new(reply: &str, max_length: usize) -> Self {
            Self {
                reply: reply.to_string(),
                max_length,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl HarnessChat for FakeChat {
        async fn chat(
            &self,
            _system: &str,
            history: &[Value],
            _gen_conf: &Value,
        ) -> Result<String, String> {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let user = history
                .last()
                .and_then(|message| message.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            self.calls.lock().unwrap().push(user);
            Ok(self.reply.clone())
        }

        fn max_length(&self) -> usize {
            self.max_length
        }
    }

    const REPLY: &str = r#"{
        "entities": [
            {"name": "Alpha", "source_chunk_id": "C1"},
            {"name": "C1", "source_chunk_id": "C2"}
        ],
        "concepts": [
            {"term": "Beta", "definition_excerpt": "d", "source_chunk_id": "C2"}
        ],
        "claims": [],
        "relations": [],
        "topics": ["t"]
    }"#;

    #[tokio::test]
    async fn extract_one_batch_builds_prompt_and_unwraps() {
        let chat = FakeChat::new(REPLY, 100_000);
        let packed = vec![
            PackedEntry {
                label: "C1".to_string(),
                chunk_id: "id1".to_string(),
                text: "a".to_string(),
            },
            PackedEntry {
                label: "C2".to_string(),
                chunk_id: "id2".to_string(),
                text: "b".to_string(),
            },
        ];
        let out = wiki_extract_one_batch(&chat, &packed, "d1", "en", 30, &json!({}), 0, 1, None)
            .await
            .expect("extract");
        assert_eq!(out["entities"].as_array().unwrap().len(), 2);
        let prompt = chat.calls.lock().unwrap()[0].clone();
        assert!(prompt.contains("Document id: d1"));
        assert!(prompt.contains("- C1"));
        assert!(prompt.contains("- C2"));
        assert!(prompt.contains("[CHUNK_ID C1]"));
    }

    #[tokio::test]
    async fn timeout_returns_none_and_fires_callback() {
        let chat = FakeChat::new(REPLY, 100_000);
        let packed = vec![PackedEntry {
            label: "C1".to_string(),
            chunk_id: "id1".to_string(),
            text: "a".to_string(),
        }];
        let fired = std::sync::Arc::new(AtomicBool::new(false));
        let flag = fired.clone();
        let callback = move |_progress: f64, message: &str| {
            if message.contains("[ERROR]") {
                flag.store(true, Ordering::SeqCst);
            }
        };
        let out = wiki_extract_one_batch(
            &chat,
            &packed,
            "d1",
            "en",
            0,
            &json!({}),
            0,
            1,
            Some(&callback),
        )
        .await;
        assert!(out.is_none());
        assert!(fired.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn process_batch_persists_resume_rows() {
        let store = MemoryDocStore::new();
        let chat = FakeChat::new(REPLY, 100_000);
        let packed = vec![
            PackedEntry {
                label: "C1".to_string(),
                chunk_id: "id1".to_string(),
                text: "a".to_string(),
            },
            PackedEntry {
                label: "C2".to_string(),
                chunk_id: "id2".to_string(),
                text: "b".to_string(),
            },
        ];
        let mut hashes = Map::new();
        hashes.insert("id1".to_string(), Value::String("h1".to_string()));
        hashes.insert("id2".to_string(), Value::String("h2".to_string()));
        let merged = wiki_process_batch(
            &chat,
            &store,
            &packed,
            0,
            1,
            "d1",
            "t1",
            "kb1",
            "en",
            30,
            None,
            &json!({}),
            Some(&hashes),
        )
        .await;
        assert_eq!(merged["entities"][0]["chunk_ids"], json!(["id1"]));
        assert_eq!(merged["concepts"][0]["chunk_ids"], json!(["id2"]));
        let versions = load_map_versions(
            &store,
            "t1",
            "kb1",
            &BTreeSet::from(["d1".to_string()]),
            None,
        );
        assert_eq!(versions["id1"]["h1"]["entities"][0]["name"], json!("Alpha"));
    }

    #[tokio::test]
    async fn map_entry_caches_on_second_run() {
        let store = MemoryDocStore::new();
        let chat = FakeChat::new(REPLY, 100_000);
        let chunks = vec![
            json!({"id": "c1", "text": "hello"}),
            json!({"id": "c2", "content_with_weight": "world"}),
        ];
        let out = wiki_map_from_chunks(
            &store,
            &chat,
            &chunks,
            "d1",
            "t1",
            "kb1",
            "en",
            2,
            30,
            &json!({}),
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(out["_meta"]["requested"], json!(2));
        assert_eq!(out["_meta"]["cache_hits"], json!(0));
        assert_eq!(out["_meta"]["extracted"], json!(2));
        assert_eq!(out["entities"].as_array().unwrap().len(), 1);
        assert_eq!(out["concepts"].as_array().unwrap().len(), 1);
        assert_eq!(out["entities"][0]["chunk_ids"], json!(["c1"]));
        let calls_first = chat.calls.lock().unwrap().len();
        assert!(calls_first >= 1);

        let out2 = wiki_map_from_chunks(
            &store,
            &chat,
            &chunks,
            "d1",
            "t1",
            "kb1",
            "en",
            2,
            30,
            &json!({}),
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(out2["_meta"]["cache_hits"], json!(2));
        assert_eq!(out2["_meta"]["extracted"], json!(0));
        assert_eq!(chat.calls.lock().unwrap().len(), calls_first);
        assert_eq!(out2["entities"][0]["name"], json!("Alpha"));
    }

    #[tokio::test]
    async fn map_entry_target_filter_limits_extraction() {
        let store = MemoryDocStore::new();
        let chat = FakeChat::new(REPLY, 100_000);
        let chunks = vec![
            json!({"id": "c1", "text": "hello"}),
            json!({"id": "c2", "text": "world"}),
        ];
        let target = BTreeSet::from(["c2".to_string()]);
        let out = wiki_map_from_chunks(
            &store,
            &chat,
            &chunks,
            "d1",
            "t1",
            "kb1",
            "en",
            2,
            30,
            &json!({}),
            None,
            None,
            Some(&target),
            None,
        )
        .await;
        assert_eq!(out["_meta"]["requested"], json!(1));
        assert_eq!(out["_meta"]["extracted"], json!(1));
        let prompt = chat.calls.lock().unwrap()[0].clone();
        assert!(prompt.contains("[CHUNK_ID C1]"));
        assert!(prompt.contains("world"));
        assert!(!prompt.contains("hello"));
    }
}

// ---------------------------------------------------------------------------
// Part 4 — KB-wide MAP aggregation, input-hash fingerprint and the REDUCE
// resume cache (`_wiki_load_all_map_extracts` .. `_wiki_persist_reduce`).
//
// Adaptation note: upstream resolves disabled documents through
// `DocumentService`; the host injects the disabled-id set here instead.
// The MAP input fingerprint uses xxh3-64 (repo-wide hash divergence).
// ---------------------------------------------------------------------------

/// `WIKI_REDUCE_COMPILE_KWD`.
pub const WIKI_REDUCE_COMPILE_KWD: &str = "wiki_reduce_result";

/// `_wiki_load_all_map_extracts`: merge every `wiki_map_extract` row of the
/// KB into one extract-shaped dict (disabled documents excluded).
pub fn load_all_map_extracts(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    disabled_doc_ids: &BTreeSet<String>,
) -> Value {
    let fields: Vec<String> = ["id", "content_with_weight", "doc_id"]
        .iter()
        .map(|field| field.to_string())
        .collect();
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_MAP_COMPILE_KWD.to_string()),
    );
    let mut merged = empty_extract();
    let mut seen_topics: BTreeSet<String> = BTreeSet::new();
    let mut offset = 0usize;
    loop {
        let rows = match search_page(store, tenant_id, kb_id, &fields, &condition, offset, 1000) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::error!(error = %err, "wiki_reduce: failed to page wiki_map_extract rows");
                break;
            }
        };
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            if !wiki_doc_ids(row.get("doc_id").unwrap_or(&Value::Null))
                .is_disjoint(disabled_doc_ids)
            {
                continue;
            }
            let content = row
                .get("content_with_weight")
                .and_then(Value::as_str)
                .unwrap_or("");
            if content.is_empty() {
                continue;
            }
            let payload: Value = match serde_json::from_str(content) {
                Ok(payload) => payload,
                Err(_) => {
                    tracing::debug!("wiki_reduce: skipping unparseable extract row");
                    continue;
                }
            };
            if !payload.is_object() {
                continue;
            }
            for key in EXTRACT_LIST_KEYS {
                if let Some(Value::Array(items)) = payload.get(key) {
                    if let Some(target) = merged.get_mut(key).and_then(Value::as_array_mut) {
                        target.extend(items.iter().filter(|item| item.is_object()).cloned());
                    }
                }
            }
            if let Some(Value::Array(topics)) = payload.get("topics") {
                for topic in topics {
                    if let Some(text) = topic.as_str() {
                        if !text.is_empty() && seen_topics.insert(text.to_string()) {
                            if let Some(target) =
                                merged.get_mut("topics").and_then(Value::as_array_mut)
                            {
                                target.push(topic.clone());
                            }
                        }
                    }
                }
            }
        }
        if rows.len() < 1000 {
            break;
        }
        offset += 1000;
    }
    merged
}

/// `_wiki_all_map_doc_ids`: distinct `doc_id` across every MAP row.
pub fn all_map_doc_ids(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    disabled_doc_ids: &BTreeSet<String>,
) -> Vec<String> {
    let fields: Vec<String> = ["id", "doc_id"]
        .iter()
        .map(|field| field.to_string())
        .collect();
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_MAP_COMPILE_KWD.to_string()),
    );
    let mut doc_ids: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut offset = 0usize;
    loop {
        let rows = match search_page(store, tenant_id, kb_id, &fields, &condition, offset, 1000) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::error!(
                    error = %err,
                    kb = kb_id,
                    offset,
                    "wiki: failed to scan MAP doc ids"
                );
                break;
            }
        };
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            for doc_id in wiki_doc_ids(row.get("doc_id").unwrap_or(&Value::Null)) {
                if !disabled_doc_ids.contains(&doc_id) && seen.insert(doc_id.clone()) {
                    doc_ids.push(doc_id);
                }
            }
        }
        if rows.len() < 1000 {
            break;
        }
        offset += 1000;
    }
    doc_ids
}

/// `_wiki_compute_map_input_hash`: fingerprint of the current MAP rows.
/// A partial scan returns `""` so REDUCE / PLAN fall through to a full run.
pub fn compute_map_input_hash(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    disabled_doc_ids: &BTreeSet<String>,
) -> String {
    let fields: Vec<String> = ["id", "doc_id", "source_chunk_ids", "chunk_hash_kwd"]
        .iter()
        .map(|field| field.to_string())
        .collect();
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_MAP_COMPILE_KWD.to_string()),
    );
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut offset = 0usize;
    loop {
        let rows = match search_page(store, tenant_id, kb_id, &fields, &condition, offset, 128) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::error!(
                    error = %err,
                    kb = kb_id,
                    offset,
                    "wiki: failed to compute MAP input hash"
                );
                return String::new();
            }
        };
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            if !wiki_doc_ids(row.get("doc_id").unwrap_or(&Value::Null))
                .is_disjoint(disabled_doc_ids)
            {
                continue;
            }
            let chunk_hash = row
                .get("chunk_hash_kwd")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if let Some(Value::Array(source)) = row.get("source_chunk_ids") {
                for cid in source {
                    if let Some(text) = cid.as_str() {
                        if !text.is_empty() {
                            pairs.push((text.to_string(), chunk_hash.clone()));
                        }
                    }
                }
            }
        }
        if rows.len() < 128 {
            break;
        }
        offset += 128;
    }
    pairs.sort();
    let body = pairs
        .iter()
        .map(|(cid, hash)| format!("{cid}:{hash}"))
        .collect::<Vec<String>>()
        .join("|")
        + "|"
        + WIKI_PIPELINE_REV;
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(body.as_bytes()))
}

/// `_wiki_load_reduce_resume`: `(cached_result, stored_input_hash)` or None.
pub fn load_reduce_resume(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
) -> Option<(Value, String)> {
    let fields: Vec<String> = ["id", "content_with_weight", "input_hash_kwd"]
        .iter()
        .map(|field| field.to_string())
        .collect();
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_REDUCE_COMPILE_KWD.to_string()),
    );
    let rows = match search_page(store, tenant_id, kb_id, &fields, &condition, 0, 1) {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!(error = %err, "wiki_reduce: failed to load resume cache");
            return None;
        }
    };
    let row = rows.into_iter().next()?;
    let content = row
        .get("content_with_weight")
        .and_then(Value::as_str)
        .unwrap_or("");
    if content.is_empty() {
        return None;
    }
    let cached: Value = match serde_json::from_str(content) {
        Ok(cached) => cached,
        Err(_) => {
            tracing::debug!("wiki_reduce: cached result unparseable; ignoring");
            return None;
        }
    };
    if !cached.is_object() {
        return None;
    }
    let stored_hash = row
        .get("input_hash_kwd")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Some((cached, stored_hash))
}

/// `_wiki_persist_reduce`: upsert the single KB-scoped REDUCE result row.
pub fn persist_reduce(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    reduced: &Value,
    input_hash: &str,
    source_doc_ids: &[String],
) {
    let kb_id_str = kb_id.to_string();
    let row_id = stable_row_id(&[WIKI_REDUCE_COMPILE_KWD.to_string(), kb_id_str.clone()]);
    let mut doc = Map::new();
    doc.insert("id".to_string(), Value::String(row_id));
    doc.insert("doc_id".to_string(), Value::String(kb_id_str.clone()));
    doc.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_REDUCE_COMPILE_KWD.to_string()),
    );
    doc.insert(
        "source_id".to_string(),
        Value::Array(vec![Value::String(kb_id_str)]),
    );
    doc.insert(
        "source_doc_ids".to_string(),
        Value::Array(
            source_doc_ids
                .iter()
                .map(|doc_id| Value::String(doc_id.clone()))
                .collect(),
        ),
    );
    doc.insert(
        "input_hash_kwd".to_string(),
        Value::String(input_hash.to_string()),
    );
    doc.insert(
        "content_with_weight".to_string(),
        Value::String(reduced.to_string()),
    );
    doc.insert("available_int".to_string(), json!(0));

    let mut delete_condition = Map::new();
    delete_condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_REDUCE_COMPILE_KWD.to_string()),
    );
    if let Err(err) = store.delete(&delete_condition, &index_name(tenant_id), kb_id) {
        tracing::debug!(
            error = %err,
            "wiki_reduce: prior result delete failed; will overwrite by id"
        );
    }
    if let Err(err) = store.insert(&[doc], &index_name(tenant_id), kb_id) {
        tracing::error!(error = %err, "wiki_reduce: failed to persist result row");
    }
}

#[cfg(test)]
mod wiki_part4_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;

    fn insert_docs(store: &MemoryDocStore, docs: &[Value]) {
        let rows: Vec<DocRow> = docs
            .iter()
            .filter_map(|doc| doc.as_object().cloned())
            .collect();
        store.insert(&rows, &index_name("t1"), "kb1").unwrap();
    }

    #[test]
    fn load_all_merges_and_skips_disabled() {
        let store = MemoryDocStore::new();
        insert_docs(
            &store,
            &[
                build_resume_doc(
                    "c1",
                    "d1",
                    &json!({"entities": [{"name": "A"}], "topics": ["t1"]}),
                    "h1",
                ),
                build_resume_doc(
                    "c2",
                    "d2",
                    &json!({"entities": [{"name": "B"}], "claims": [{"statement": "s"}], "topics": ["t1", "t2"]}),
                    "h2",
                ),
                build_resume_doc(
                    "c3",
                    "d3",
                    &json!({"entities": [{"name": "C"}], "topics": ["t9"]}),
                    "h3",
                ),
            ],
        );
        let disabled = BTreeSet::from(["d3".to_string()]);
        let merged = load_all_map_extracts(&store, "t1", "kb1", &disabled);
        assert_eq!(merged["entities"].as_array().unwrap().len(), 2);
        assert_eq!(merged["claims"].as_array().unwrap().len(), 1);
        assert_eq!(merged["topics"], json!(["t1", "t2"]));
    }

    #[test]
    fn all_doc_ids_dedups_and_excludes_disabled() {
        let store = MemoryDocStore::new();
        insert_docs(
            &store,
            &[
                build_resume_doc("c1", "d1", &empty_extract(), "h1"),
                build_resume_doc("c2", "d1", &empty_extract(), "h2"),
                build_resume_doc("c3", "d2", &empty_extract(), "h3"),
                build_resume_doc("c4", "d3", &empty_extract(), "h4"),
            ],
        );
        let disabled = BTreeSet::from(["d3".to_string()]);
        let mut ids = all_map_doc_ids(&store, "t1", "kb1", &disabled);
        ids.sort();
        assert_eq!(ids, vec!["d1".to_string(), "d2".to_string()]);
    }

    #[test]
    fn input_hash_stable_and_row_sensitive() {
        let store = MemoryDocStore::new();
        let disabled = BTreeSet::new();
        let empty = compute_map_input_hash(&store, "t1", "kb1", &disabled);
        let expected = format!(
            "{:016x}",
            xxhash_rust::xxh3::xxh3_64(format!("|{}", WIKI_PIPELINE_REV).as_bytes())
        );
        assert_eq!(empty, expected);
        insert_docs(
            &store,
            &[build_resume_doc("c1", "d1", &empty_extract(), "h1")],
        );
        let one = compute_map_input_hash(&store, "t1", "kb1", &disabled);
        assert_ne!(one, empty);
        assert_eq!(one, compute_map_input_hash(&store, "t1", "kb1", &disabled));
        insert_docs(
            &store,
            &[build_resume_doc("c2", "d1", &empty_extract(), "h2")],
        );
        assert_ne!(compute_map_input_hash(&store, "t1", "kb1", &disabled), one);
    }

    #[test]
    fn reduce_resume_roundtrip_and_single_row() {
        let store = MemoryDocStore::new();
        assert!(load_reduce_resume(&store, "t1", "kb1").is_none());
        persist_reduce(
            &store,
            "t1",
            "kb1",
            &json!({"entities": [{"name": "A"}]}),
            "IH1",
            &["d1".to_string(), "d2".to_string()],
        );
        let (cached, hash) = load_reduce_resume(&store, "t1", "kb1").expect("resume");
        assert_eq!(hash, "IH1");
        assert_eq!(cached["entities"][0]["name"], json!("A"));
        persist_reduce(
            &store,
            "t1",
            "kb1",
            &json!({"entities": [{"name": "B"}]}),
            "IH2",
            &["d1".to_string()],
        );
        let (cached2, hash2) = load_reduce_resume(&store, "t1", "kb1").expect("resume2");
        assert_eq!(hash2, "IH2");
        assert_eq!(cached2["entities"][0]["name"], json!("B"));
        let mut condition = Map::new();
        condition.insert(
            "compile_kwd".to_string(),
            Value::String(WIKI_REDUCE_COMPILE_KWD.to_string()),
        );
        let query = SearchQuery {
            select_fields: vec!["id".to_string()],
            condition,
            match_expressions: Vec::new(),
            offset: 0,
            limit: 10,
            index_names: vec![index_name("t1")],
            dataset_ids: vec!["kb1".to_string()],
            ..Default::default()
        };
        assert_eq!(store.search(&query).unwrap().total, 1);
    }
}

// ---------------------------------------------------------------------------
// Part 5 — REDUCE (dedup) main entry (`wiki_reduce_from_extracts`).
//
// Adaptation note: the host injects `chat` / `embd` (LlmClient / Embedder)
// and the disabled-doc set; the shared `bulk_dedup_items` path carries the
// exact → embedding → LLM pipeline. `llm_timeout` is kept for signature
// parity but the shared dedup path has no timeout knob. On an entity-dedup
// failure the port falls back to the exact-dedup result and warns (upstream
// would raise).
// ---------------------------------------------------------------------------

/// `DEFAULT_WIKI_REDUCE_MERGE_THRESHOLD`.
pub const DEFAULT_WIKI_REDUCE_MERGE_THRESHOLD: f32 = 0.95;
/// `DEFAULT_WIKI_REDUCE_AMBIGUOUS_LOW`.
pub const DEFAULT_WIKI_REDUCE_AMBIGUOUS_LOW: f32 = 0.75;
/// `DEFAULT_WIKI_REDUCE_AMBIGUOUS_BATCH`.
pub const DEFAULT_WIKI_REDUCE_AMBIGUOUS_BATCH: usize = 50;

/// `DEFAULT_WIKI_REDUCE_TIMEOUT` (`_env_int("WIKI_REDUCE_TIMEOUT", 60, minimum=1)`).
pub fn default_wiki_reduce_timeout() -> i64 {
    env_int("WIKI_REDUCE_TIMEOUT", 60, Some(1)).max(1)
}

/// `WIKI_REDUCE_DISAMBIGUATE_SYSTEM`.
pub const WIKI_REDUCE_DISAMBIGUATE_SYSTEM: &str =
    "You are a named-entity resolution assistant. Return only JSON.";

/// `wiki_reduce_from_extracts`: KB-scoped REDUCE (dedup) phase.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_reduce_from_extracts(
    store: &dyn DocStore,
    chat: Option<&LlmClient>,
    embd: Option<&dyn Embedder>,
    tenant_id: &str,
    kb_id: &str,
    disabled_doc_ids: &BTreeSet<String>,
    merge_threshold: f32,
    ambiguous_low: f32,
    ambiguous_batch_size: usize,
    llm_timeout: i64,
    force_rerun: bool,
    callback: Option<&(dyn Fn(f64, &str) + Send + Sync)>,
) -> Value {
    let _ = llm_timeout; // shared dedup path has no timeout knob
    let current_input_hash = compute_map_input_hash(store, tenant_id, kb_id, disabled_doc_ids);
    let reduce_source_doc_ids = all_map_doc_ids(store, tenant_id, kb_id, disabled_doc_ids);

    if !force_rerun {
        if let Some((cached, stored_hash)) = load_reduce_resume(store, tenant_id, kb_id) {
            if !stored_hash.is_empty() && stored_hash == current_input_hash {
                if let Some(callback) = callback {
                    callback(1.0, "wiki REDUCE: cache hit (input unchanged)");
                }
                return cached;
            }
            // Cache present but stale (no hash, or hash mismatch): fall
            // through to a full re-reduce and write a fresh stamp.
        }
    }

    if let Some(callback) = callback {
        callback(0.05, "wiki REDUCE: loading MAP extracts");
    }
    let raw = load_all_map_extracts(store, tenant_id, kb_id, disabled_doc_ids);
    let arr = |key: &str| -> Vec<Value> {
        raw.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let raw_entities = arr("entities");
    let raw_concepts = arr("concepts");
    tracing::info!(
        kb = kb_id,
        entities = raw_entities.len(),
        concepts = raw_concepts.len(),
        claims = arr("claims").len(),
        relations = arr("relations").len(),
        "wiki_reduce: loaded raw items"
    );

    if raw_entities.is_empty() && raw_concepts.is_empty() {
        // Nothing to reduce; persist an empty result so resume can short-circuit.
        let empty = empty_extract();
        persist_reduce(
            store,
            tenant_id,
            kb_id,
            &empty,
            &current_input_hash,
            &reduce_source_doc_ids,
        );
        return empty;
    }

    if let Some(callback) = callback {
        callback(0.25, "wiki REDUCE: dedup (exact + embedding + LLM)");
    }

    // Entities: full three-phase dedup keyed by (normalized name, type).
    let canonical_entities = match bulk_dedup_items(
        raw_entities.clone(),
        "name",
        Some("type"),
        chat,
        embd,
        merge_threshold,
        ambiguous_low,
        ambiguous_batch_size,
        true,
    )
    .await
    {
        Ok(items) => items,
        Err(err) => {
            tracing::warn!(error = %err, "wiki_reduce: entity dedup failed; keeping exact-dedup result");
            exact_dedup_by_key(&raw_entities, "name", Some("type"), None)
        }
    };

    // Concepts: exact-dedup only; keep the longest definition_excerpt per group.
    let concept_extras = |group: &[Value]| -> Option<Value> {
        let mut best = String::new();
        for item in group {
            let definition = item
                .get("definition_excerpt")
                .and_then(Value::as_str)
                .unwrap_or("");
            if definition.len() > best.len() {
                best = definition.to_string();
            }
        }
        Some(json!({"definition_excerpt": best}))
    };
    let canonical_concepts = exact_dedup_by_key(
        &raw_concepts,
        "term",
        None,
        Some(&concept_extras as &dyn Fn(&[Value]) -> Option<Value>),
    );

    tracing::info!(
        entities = canonical_entities.len(),
        concepts = canonical_concepts.len(),
        "wiki_reduce: after dedup"
    );

    let reduced = json!({
        "entities": canonical_entities,
        "concepts": canonical_concepts,
        "claims": arr("claims"),
        "relations": arr("relations"),
        "topics": arr("topics"),
    });

    if let Some(callback) = callback {
        callback(0.9, "wiki REDUCE: persisting result");
    }
    persist_reduce(
        store,
        tenant_id,
        kb_id,
        &reduced,
        &current_input_hash,
        &reduce_source_doc_ids,
    );

    tracing::info!(
        kb = kb_id,
        entities = reduced["entities"].as_array().map(Vec::len).unwrap_or(0),
        concepts = reduced["concepts"].as_array().map(Vec::len).unwrap_or(0),
        claims = reduced["claims"].as_array().map(Vec::len).unwrap_or(0),
        relations = reduced["relations"].as_array().map(Vec::len).unwrap_or(0),
        topics = reduced["topics"].as_array().map(Vec::len).unwrap_or(0),
        "wiki_reduce: done"
    );

    if let Some(callback) = callback {
        callback(1.0, "wiki REDUCE: done");
    }
    reduced
}

#[cfg(test)]
mod wiki_part5_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;

    fn insert_docs(store: &MemoryDocStore, docs: &[Value]) {
        let rows: Vec<DocRow> = docs
            .iter()
            .filter_map(|doc| doc.as_object().cloned())
            .collect();
        store.insert(&rows, &index_name("t1"), "kb1").unwrap();
    }

    fn seed_rows(store: &MemoryDocStore) {
        insert_docs(
            store,
            &[build_resume_doc(
                "c1",
                "d1",
                &json!({
                    "entities": [
                        {"name": "Alpha", "type": "org", "chunk_ids": ["c1"]},
                        {"name": "alpha!", "type": "org", "chunk_ids": ["c2"]}
                    ],
                    "concepts": [
                        {"term": "Beta", "definition_excerpt": "short", "chunk_ids": ["c1"]},
                        {"term": "beta", "definition_excerpt": "a much longer definition", "chunk_ids": ["c2"]}
                    ],
                    "claims": [{"statement": "s", "chunk_ids": ["c1"]}],
                    "relations": [{"from": "Alpha", "to": "Beta", "type": "uses", "chunk_ids": ["c1"]}],
                    "topics": ["t"]
                }),
                "h1",
            )],
        );
    }

    #[tokio::test]
    async fn reduce_dedups_entities_and_concepts() {
        let store = MemoryDocStore::new();
        seed_rows(&store);
        let disabled = BTreeSet::new();
        let reduced = wiki_reduce_from_extracts(
            &store,
            None,
            None,
            "t1",
            "kb1",
            &disabled,
            DEFAULT_WIKI_REDUCE_MERGE_THRESHOLD,
            DEFAULT_WIKI_REDUCE_AMBIGUOUS_LOW,
            DEFAULT_WIKI_REDUCE_AMBIGUOUS_BATCH,
            30,
            false,
            None,
        )
        .await;
        let entities = reduced["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0]["name"], json!("Alpha"));
        assert_eq!(entities[0]["mention_count"], json!(2));
        let mut chunk_ids: Vec<String> = entities[0]["chunk_ids"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect();
        chunk_ids.sort();
        assert_eq!(chunk_ids, vec!["c1".to_string(), "c2".to_string()]);
        let concepts = reduced["concepts"].as_array().unwrap();
        assert_eq!(concepts.len(), 1);
        assert_eq!(concepts[0]["term"], json!("Beta"));
        assert_eq!(
            concepts[0]["definition_excerpt"],
            json!("a much longer definition")
        );
        assert_eq!(reduced["claims"].as_array().unwrap().len(), 1);
        assert_eq!(reduced["relations"].as_array().unwrap().len(), 1);
        assert_eq!(reduced["topics"], json!(["t"]));
    }

    #[tokio::test]
    async fn reduce_cache_hit_and_force_rerun() {
        let store = MemoryDocStore::new();
        seed_rows(&store);
        let disabled = BTreeSet::new();
        let first = wiki_reduce_from_extracts(
            &store,
            None,
            None,
            "t1",
            "kb1",
            &disabled,
            DEFAULT_WIKI_REDUCE_MERGE_THRESHOLD,
            DEFAULT_WIKI_REDUCE_AMBIGUOUS_LOW,
            DEFAULT_WIKI_REDUCE_AMBIGUOUS_BATCH,
            30,
            false,
            None,
        )
        .await;
        let (_, stored_hash) = load_reduce_resume(&store, "t1", "kb1").expect("resume");
        assert_eq!(
            stored_hash,
            compute_map_input_hash(&store, "t1", "kb1", &disabled)
        );
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = flag.clone();
        let callback = move |progress: f64, message: &str| {
            if progress >= 1.0 && message.contains("cache hit") {
                seen.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        };
        let second = wiki_reduce_from_extracts(
            &store,
            None,
            None,
            "t1",
            "kb1",
            &disabled,
            DEFAULT_WIKI_REDUCE_MERGE_THRESHOLD,
            DEFAULT_WIKI_REDUCE_AMBIGUOUS_LOW,
            DEFAULT_WIKI_REDUCE_AMBIGUOUS_BATCH,
            30,
            false,
            Some(&callback),
        )
        .await;
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(second["entities"], first["entities"]);
        let forced = wiki_reduce_from_extracts(
            &store,
            None,
            None,
            "t1",
            "kb1",
            &disabled,
            DEFAULT_WIKI_REDUCE_MERGE_THRESHOLD,
            DEFAULT_WIKI_REDUCE_AMBIGUOUS_LOW,
            DEFAULT_WIKI_REDUCE_AMBIGUOUS_BATCH,
            30,
            true,
            None,
        )
        .await;
        assert_eq!(forced["entities"], first["entities"]);
    }

    #[tokio::test]
    async fn reduce_empty_persists_short_circuit() {
        let store = MemoryDocStore::new();
        let disabled = BTreeSet::new();
        let reduced = wiki_reduce_from_extracts(
            &store,
            None,
            None,
            "t1",
            "kb1",
            &disabled,
            DEFAULT_WIKI_REDUCE_MERGE_THRESHOLD,
            DEFAULT_WIKI_REDUCE_AMBIGUOUS_LOW,
            DEFAULT_WIKI_REDUCE_AMBIGUOUS_BATCH,
            30,
            false,
            None,
        )
        .await;
        assert_eq!(reduced["entities"], json!([]));
        let (cached, _) = load_reduce_resume(&store, "t1", "kb1").expect("resume");
        assert_eq!(cached["concepts"], json!([]));
    }
}

// ---------------------------------------------------------------------------
// Part 6a — PLAN helpers: page-count heuristic, plan-line formatting,
// KB reconciliation and MAYBE resolution (`_wiki_target_page_count` ..
// `_wiki_resolve_maybe_items`).
//
// Adaptation note: the local dense match has no `extra_options` min-score
// filter, so similarity is computed client-side from the returned row vector
// and `sim <= 0` falls back to `maybe_threshold` (mirroring the upstream
// `_score`-missing fallback).
// ---------------------------------------------------------------------------

/// `WIKI_PLAN_COMPILE_KWD`.
pub const WIKI_PLAN_COMPILE_KWD: &str = "wiki_compilation_plan";
/// `WIKI_PAGE_COMPILE_KWD`.
pub const WIKI_PAGE_COMPILE_KWD: &str = "wiki_page";
/// `DEFAULT_WIKI_PLAN_UPDATE_THRESHOLD`.
pub const DEFAULT_WIKI_PLAN_UPDATE_THRESHOLD: f64 = 0.95;
/// `DEFAULT_WIKI_PLAN_MAYBE_THRESHOLD`.
pub const DEFAULT_WIKI_PLAN_MAYBE_THRESHOLD: f64 = 0.60;

/// `DEFAULT_WIKI_PLAN_TIMEOUT` (`_env_int("WIKI_PLAN_TIMEOUT", 600, minimum=1)`).
pub fn default_wiki_plan_timeout() -> i64 {
    env_int("WIKI_PLAN_TIMEOUT", 600, Some(1)).max(1)
}

/// `WIKI_PLAN_RECONCILE_SYSTEM`.
pub const WIKI_PLAN_RECONCILE_SYSTEM: &str = "You are a knowledge base assistant. Return only a JSON boolean array.Keep the user's original language (Chinese/English etc.) for generated data.";

/// `_wiki_target_page_count`: `clamp(8, total // 3, 60)`.
pub fn target_page_count(total_items: i64) -> usize {
    if total_items <= 0 {
        return 8;
    }
    (total_items / 3).clamp(8, 60) as usize
}

/// `_wiki_format_entity_for_plan`.
pub fn format_entity_for_plan(entity: &Value, reconciliation: &Map<String, Value>) -> String {
    let aliases = entity
        .get("aliases")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(3)
                .filter_map(|item| item.as_str())
                .collect::<Vec<&str>>()
                .join(", ")
        })
        .unwrap_or_default();
    let name = entity.get("name").and_then(Value::as_str).unwrap_or("");
    let rec = reconciliation.get(name).cloned().unwrap_or(Value::Null);
    let action = rec
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("CREATE");
    let slug = rec.get("page_slug").and_then(Value::as_str).unwrap_or("");
    let kb_info = format!("→ {action} {slug}");
    let kb_info = kb_info.trim_end();
    let mut line = format!(
        "  - {name} ({}, {} mentions",
        entity.get("type").and_then(Value::as_str).unwrap_or(""),
        entity
            .get("mention_count")
            .and_then(Value::as_i64)
            .unwrap_or(0)
    );
    if !aliases.is_empty() {
        line += &format!(", aliases: {aliases}");
    }
    line += &format!(") {kb_info}");
    line
}

/// `_wiki_format_concept_for_plan`.
pub fn format_concept_for_plan(concept: &Value, reconciliation: &Map<String, Value>) -> String {
    let term = concept.get("term").and_then(Value::as_str).unwrap_or("");
    let rec = reconciliation.get(term).cloned().unwrap_or(Value::Null);
    let action = rec
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("CREATE");
    let slug = rec.get("page_slug").and_then(Value::as_str).unwrap_or("");
    let kb_info = format!("→ {action} {slug}");
    let kb_info = kb_info.trim_end();
    format!(
        "  - {term} ({} mentions) {kb_info}",
        concept
            .get("mention_count")
            .and_then(Value::as_i64)
            .unwrap_or(0)
    )
}

fn create_reconciliation() -> Value {
    json!({
        "action": "CREATE",
        "page_slug": null,
        "page_title": null,
        "page_id": null,
        "similarity": 0.0,
    })
}

fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// `_wiki_reconcile_with_kb`: per-item KNN against `wiki_page` rows.
pub async fn reconcile_with_kb(
    store: &dyn DocStore,
    embd: Option<&dyn Embedder>,
    tenant_id: &str,
    kb_id: &str,
    canonical_entities: &[Value],
    canonical_concepts: &[Value],
    update_threshold: f64,
    maybe_threshold: f64,
) -> Map<String, Value> {
    let mut items: Vec<(String, String, String)> = Vec::new(); // (kind, key, text)
    for entity in canonical_entities {
        if let Some(name) = entity.get("name").and_then(Value::as_str) {
            if !name.is_empty() {
                items.push((
                    "entity".to_string(),
                    name.to_string(),
                    truncate_chars(name, 4000),
                ));
            }
        }
    }
    for concept in canonical_concepts {
        if let Some(term) = concept.get("term").and_then(Value::as_str) {
            if !term.is_empty() {
                let definition = concept
                    .get("definition_excerpt")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let text = if definition.is_empty() {
                    term.to_string()
                } else {
                    format!("{term}: {}", truncate_chars(definition, 200))
                };
                items.push((
                    "concept".to_string(),
                    term.to_string(),
                    truncate_chars(&text, 4000),
                ));
            }
        }
    }
    let mut reconciliation: Map<String, Value> = Map::new();
    if items.is_empty() {
        return reconciliation;
    }
    let Some(embd) = embd else {
        for (_, key, _) in &items {
            reconciliation.insert(key.clone(), create_reconciliation());
        }
        return reconciliation;
    };
    let texts: Vec<&str> = items.iter().map(|(_, _, text)| text.as_str()).collect();
    let vectors = match embd.embed(&texts).await {
        Ok(vectors) => vectors,
        Err(err) => {
            tracing::error!(error = %err, "wiki_plan: reconciliation embedding failed — all items will be CREATE");
            for (_, key, _) in &items {
                reconciliation.insert(key.clone(), create_reconciliation());
            }
            return reconciliation;
        }
    };
    if vectors.len() != items.len() {
        tracing::error!(
            expected = items.len(),
            got = vectors.len(),
            "wiki_plan: reconciliation embedding count mismatch; CREATE all"
        );
        for (_, key, _) in &items {
            reconciliation.insert(key.clone(), create_reconciliation());
        }
        return reconciliation;
    }

    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_PAGE_COMPILE_KWD.to_string()),
    );
    for ((_, key, _), vec) in items.iter().zip(vectors.iter()) {
        if vec.is_empty() {
            reconciliation.insert(key.clone(), create_reconciliation());
            continue;
        }
        let vec_field = format!("q_{}_vec", vec.len());
        let fields: Vec<String> = [
            "id",
            "slug_kwd",
            "title_kwd",
            "page_type_kwd",
            "embedding",
            vec_field.as_str(),
        ]
        .iter()
        .map(|field| field.to_string())
        .collect();
        let query = SearchQuery {
            select_fields: fields.clone(),
            condition: condition.clone(),
            match_expressions: vec![crate::doc_store::MatchExpr::dense(
                &vec_field,
                vec.clone(),
                "cosine",
                1,
            )],
            offset: 0,
            limit: 1,
            index_names: vec![index_name(tenant_id)],
            dataset_ids: vec![kb_id.to_string()],
            ..Default::default()
        };
        let top = match store.search(&query) {
            Ok(response) => store.get_fields(&response, &fields).into_iter().next(),
            Err(err) => {
                tracing::error!(error = %err, key = %key, "wiki_plan: KNN failed");
                reconciliation.insert(key.clone(), create_reconciliation());
                continue;
            }
        };
        let Some((top_id, top_row)) = top else {
            // Local backends drop zero-score dense hits, so rescan the page
            // pool and pick the best row client-side (threshold-gated).
            let query = SearchQuery {
                select_fields: fields.clone(),
                condition: condition.clone(),
                match_expressions: Vec::new(),
                offset: 0,
                limit: 200,
                index_names: vec![index_name(tenant_id)],
                dataset_ids: vec![kb_id.to_string()],
                ..Default::default()
            };
            let mut best: Option<(String, Value, f64)> = None;
            let search_result = match store.search(&query) {
                Ok(response) => Some(response),
                Err(error) => {
                    // The reconciliation fallback silently matched nothing before, so the
                    // page was rebuilt without reconciling existing claims.
                    tracing::error!(%error, key = %key, "wiki_plan: fallback KNN search failed");
                    None
                }
            };
            if let Some(response) = search_result {
                for (row_id, row) in store.get_fields(&response, &fields) {
                    let stored: Vec<f32> = row
                        .get(&vec_field)
                        .or_else(|| row.get("embedding"))
                        .and_then(Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|item| item.as_f64().map(|number| number as f32))
                                .collect()
                        })
                        .unwrap_or_default();
                    if stored.len() != vec.len() {
                        continue;
                    }
                    let sim = crate::merge::cosine_similarity(vec, &stored) as f64;
                    if best
                        .as_ref()
                        .map(|(_, _, best_sim)| sim > *best_sim)
                        .unwrap_or(true)
                    {
                        best = Some((row_id, Value::Object(row), sim));
                    }
                }
            }
            let Some((row_id, row, sim)) = best.filter(|(_, _, sim)| *sim >= maybe_threshold)
            else {
                reconciliation.insert(key.clone(), create_reconciliation());
                continue;
            };
            let slug = row.get("slug_kwd").cloned().unwrap_or(Value::Null);
            let title = row.get("title_kwd").cloned().unwrap_or(Value::Null);
            let action = if sim >= update_threshold {
                "UPDATE"
            } else {
                "MAYBE"
            };
            reconciliation.insert(
                key.clone(),
                json!({
                    "action": action,
                    "page_slug": slug,
                    "page_title": title,
                    "page_id": row_id,
                    "similarity": sim,
                }),
            );
            continue;
        };
        let stored: Vec<f32> = top_row
            .get(&vec_field)
            .or_else(|| top_row.get("embedding"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_f64().map(|number| number as f32))
                    .collect()
            })
            .unwrap_or_default();
        let mut sim = crate::merge::cosine_similarity(vec, &stored) as f64;
        if sim <= 0.0 {
            sim = maybe_threshold;
        }
        let slug = top_row.get("slug_kwd").cloned().unwrap_or(Value::Null);
        let title = top_row.get("title_kwd").cloned().unwrap_or(Value::Null);
        let action = if sim >= update_threshold {
            "UPDATE"
        } else {
            "MAYBE"
        };
        reconciliation.insert(
            key.clone(),
            json!({
                "action": action,
                "page_slug": slug,
                "page_title": title,
                "page_id": top_id,
                "similarity": sim,
            }),
        );
    }
    reconciliation
}

fn set_action(reconciliation: &mut Map<String, Value>, name: &str, action: &str) {
    if let Some(entry) = reconciliation.get_mut(name).and_then(Value::as_object_mut) {
        entry.insert("action".to_string(), Value::String(action.to_string()));
    }
}

/// `_wiki_resolve_maybe_items`: flip MAYBE → UPDATE | CREATE in place.
pub async fn resolve_maybe_items(
    chat: &dyn HarnessChat,
    reconciliation: &mut Map<String, Value>,
    batch_size: usize,
    llm_timeout: i64,
) {
    let maybe_items: Vec<String> = reconciliation
        .iter()
        .filter(|(_, value)| value.get("action").and_then(Value::as_str) == Some("MAYBE"))
        .map(|(key, _)| key.clone())
        .collect();
    if maybe_items.is_empty() {
        return;
    }
    let batch_size = batch_size.max(1);
    for batch in maybe_items.chunks(batch_size) {
        let mut lines: Vec<String> = Vec::new();
        for (idx, name) in batch.iter().enumerate() {
            let rec = reconciliation.get(name).cloned().unwrap_or(Value::Null);
            let title = rec
                .get("page_title")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .or_else(|| rec.get("page_slug").and_then(Value::as_str))
                .unwrap_or("");
            let slug = rec.get("page_slug").and_then(Value::as_str).unwrap_or("");
            let sim = rec.get("similarity").and_then(Value::as_f64).unwrap_or(0.0);
            lines.push(format!(
                "{}. Entity: \"{name}\" — existing wiki page: \"{title}\" (slug: {slug}, similarity: {sim:.2})",
                idx + 1
            ));
        }
        let user_prompt = format!(
            "For each pair below, decide whether the entity refers to the same real-world concept as the existing wiki page (true = UPDATE existing page, false = CREATE new page).\nReturn a JSON array of exactly {} booleans. Return ONLY the JSON array.\n\n{}",
            batch.len(),
            lines.join("\n")
        );
        let gen_conf = knowledge_compile_gen_conf(
            &chat.model_name(),
            Some(&Map::from_iter([("temperature".to_string(), json!(0.0))])),
        );
        let request_conf = Value::Object(gen_conf);
        let timeout = std::time::Duration::from_secs(llm_timeout.max(0) as u64);
        let outcome = tokio::time::timeout(
            timeout,
            wiki_gen_json(
                chat,
                WIKI_PLAN_RECONCILE_SYSTEM,
                &user_prompt,
                &request_conf,
            ),
        )
        .await;
        let res = match outcome {
            Err(_elapsed) => {
                tracing::warn!(
                    pairs = batch.len(),
                    "wiki_plan: MAYBE resolution timed out; defaulting CREATE"
                );
                None
            }
            Ok(None) => {
                tracing::warn!(
                    pairs = batch.len(),
                    "wiki_plan: MAYBE resolution failed; defaulting CREATE"
                );
                None
            }
            Ok(Some(value)) => Some(value),
        };
        let Some(res) = res else {
            for name in batch {
                set_action(reconciliation, name, "CREATE");
            }
            continue;
        };
        let mut decisions: Option<Vec<Value>> = None;
        if let Value::Array(items) = &res {
            decisions = Some(items.clone());
        } else if let Value::Object(map) = &res {
            for value in map.values() {
                if let Value::Array(items) = value {
                    decisions = Some(items.clone());
                    break;
                }
            }
        }
        let Some(decisions) = decisions else {
            tracing::warn!("wiki_plan: MAYBE LLM returned unexpected shape; CREATE all");
            for name in batch {
                set_action(reconciliation, name, "CREATE");
            }
            continue;
        };
        for (idx, name) in batch.iter().enumerate() {
            let verdict = decisions.get(idx).map(json_truthy).unwrap_or(false);
            set_action(
                reconciliation,
                name,
                if verdict { "UPDATE" } else { "CREATE" },
            );
        }
    }
}

#[cfg(test)]
mod wiki_part6_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;
    use crate::embed::Embedder;
    use crate::harness::HarnessChat;
    use std::sync::Mutex;

    struct FakeEmbedder {
        map: HashMap<String, Vec<f32>>,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl Embedder for FakeEmbedder {
        async fn embed(&self, texts: &[&str]) -> crate::Result<Vec<Vec<f32>>> {
            if self.fail {
                return Err(anyhow::anyhow!("embed boom"));
            }
            Ok(texts
                .iter()
                .map(|text| self.map.get(*text).cloned().unwrap_or_default())
                .collect())
        }
    }

    struct FakeChat {
        reply: String,
        delay_ms: u64,
        calls: Mutex<Vec<String>>,
    }

    impl FakeChat {
        fn new(reply: &str) -> Self {
            Self {
                reply: reply.to_string(),
                delay_ms: 0,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl HarnessChat for FakeChat {
        async fn chat(
            &self,
            _system: &str,
            history: &[Value],
            _gen_conf: &Value,
        ) -> Result<String, String> {
            if self.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            }
            let user = history
                .last()
                .and_then(|message| message.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            self.calls.lock().unwrap().push(user);
            Ok(self.reply.clone())
        }

        fn max_length(&self) -> usize {
            100_000
        }
    }

    fn embedder(map: &[(&str, [f32; 2])]) -> FakeEmbedder {
        FakeEmbedder {
            map: map
                .iter()
                .map(|(name, vec)| (name.to_string(), vec.to_vec()))
                .collect(),
            fail: false,
        }
    }

    fn insert_page(store: &MemoryDocStore) {
        let row = json!({
            "id": "p1",
            "compile_kwd": "wiki_page",
            "slug_kwd": "entity/alpha",
            "title_kwd": "Alpha Page",
            "page_type_kwd": "entity",
            "embedding": [1.0, 0.0],
            "q_2_vec": [1.0, 0.0]
        });
        store
            .insert(
                &[row.as_object().cloned().unwrap()],
                &index_name("t1"),
                "kb1",
            )
            .unwrap();
    }

    #[test]
    fn target_page_count_bounds() {
        assert_eq!(target_page_count(0), 8);
        assert_eq!(target_page_count(9), 8);
        assert_eq!(target_page_count(30), 10);
        assert_eq!(target_page_count(1000), 60);
    }

    #[test]
    fn plan_line_formatters() {
        let mut reconciliation = Map::new();
        reconciliation.insert(
            "Alpha".to_string(),
            json!({"action": "UPDATE", "page_slug": "entity/alpha"}),
        );
        let entity = json!({
            "name": "Alpha",
            "type": "org",
            "mention_count": 3,
            "aliases": ["A", "Alpha Inc", "ALPHA", "ignored"]
        });
        assert_eq!(
            format_entity_for_plan(&entity, &reconciliation),
            "  - Alpha (org, 3 mentions, aliases: A, Alpha Inc, ALPHA) → UPDATE entity/alpha"
        );
        let concept = json!({"term": "Beta", "mention_count": 2});
        assert_eq!(
            format_concept_for_plan(&concept, &reconciliation),
            "  - Beta (2 mentions) → CREATE"
        );
        assert_eq!(
            format_concept_for_plan(&concept, &reconciliation),
            "  - Beta (2 mentions) → CREATE"
        );
    }

    #[tokio::test]
    async fn reconcile_classifies_updates_maybes_and_creates() {
        let store = MemoryDocStore::new();
        insert_page(&store);
        let embd = embedder(&[
            ("Alpha", [1.0, 0.0]),
            ("Gamma", [0.0, 1.0]),
            ("Delta", [0.8, 0.6]),
        ]);
        let entities = vec![
            json!({"name": "Alpha", "type": "org"}),
            json!({"name": "Gamma", "type": "org"}),
            json!({"name": "Delta", "type": "org"}),
        ];
        let reconciliation = reconcile_with_kb(
            &store,
            Some(&embd),
            "t1",
            "kb1",
            &entities,
            &[],
            DEFAULT_WIKI_PLAN_UPDATE_THRESHOLD,
            DEFAULT_WIKI_PLAN_MAYBE_THRESHOLD,
        )
        .await;
        assert_eq!(reconciliation["Alpha"]["action"], json!("UPDATE"));
        assert_eq!(reconciliation["Alpha"]["page_slug"], json!("entity/alpha"));
        assert_eq!(reconciliation["Alpha"]["page_id"], json!("p1"));
        assert!(reconciliation["Alpha"]["similarity"].as_f64().unwrap() > 0.99);
        assert_eq!(reconciliation["Gamma"]["action"], json!("CREATE"));
        assert_eq!(reconciliation["Gamma"]["page_slug"], Value::Null);
        assert_eq!(reconciliation["Delta"]["action"], json!("MAYBE"));
        let delta_sim = reconciliation["Delta"]["similarity"].as_f64().unwrap();
        assert!(
            delta_sim > DEFAULT_WIKI_PLAN_MAYBE_THRESHOLD
                && delta_sim < DEFAULT_WIKI_PLAN_UPDATE_THRESHOLD
        );

        let empty_store = MemoryDocStore::new();
        let fresh = reconcile_with_kb(
            &empty_store,
            Some(&embd),
            "t1",
            "kb1",
            &entities,
            &[],
            DEFAULT_WIKI_PLAN_UPDATE_THRESHOLD,
            DEFAULT_WIKI_PLAN_MAYBE_THRESHOLD,
        )
        .await;
        assert_eq!(fresh["Alpha"]["action"], json!("CREATE"));
        assert_eq!(fresh["Alpha"]["page_slug"], Value::Null);
    }

    #[tokio::test]
    async fn reconcile_embed_failure_creates_all() {
        let store = MemoryDocStore::new();
        insert_page(&store);
        let failing = FakeEmbedder {
            map: HashMap::new(),
            fail: true,
        };
        let entities = vec![json!({"name": "Alpha", "type": "org"})];
        let reconciliation = reconcile_with_kb(
            &store,
            Some(&failing),
            "t1",
            "kb1",
            &entities,
            &[],
            DEFAULT_WIKI_PLAN_UPDATE_THRESHOLD,
            DEFAULT_WIKI_PLAN_MAYBE_THRESHOLD,
        )
        .await;
        assert_eq!(reconciliation["Alpha"]["action"], json!("CREATE"));
        let no_embedder = reconcile_with_kb(
            &store,
            None,
            "t1",
            "kb1",
            &entities,
            &[],
            DEFAULT_WIKI_PLAN_UPDATE_THRESHOLD,
            DEFAULT_WIKI_PLAN_MAYBE_THRESHOLD,
        )
        .await;
        assert_eq!(no_embedder["Alpha"]["action"], json!("CREATE"));
    }

    #[tokio::test]
    async fn resolve_maybe_flips_and_defaults() {
        let chat = FakeChat::new("[true, false]");
        let mut reconciliation = Map::new();
        reconciliation.insert(
            "Alpha".to_string(),
            json!({"action": "MAYBE", "page_slug": "entity/alpha", "page_title": "Alpha Page", "similarity": 0.7}),
        );
        reconciliation.insert(
            "Beta".to_string(),
            json!({"action": "MAYBE", "page_slug": "entity/beta", "page_title": "Beta Page", "similarity": 0.65}),
        );
        resolve_maybe_items(&chat, &mut reconciliation, 50, 30).await;
        assert_eq!(reconciliation["Alpha"]["action"], json!("UPDATE"));
        assert_eq!(reconciliation["Beta"]["action"], json!("CREATE"));
        assert!(chat.calls.lock().unwrap()[0].contains("Entity: \"Alpha\""));

        let malformed = FakeChat::new("not json at all");
        let mut map2 = Map::new();
        map2.insert(
            "Gamma".to_string(),
            json!({"action": "MAYBE", "similarity": 0.61}),
        );
        resolve_maybe_items(&malformed, &mut map2, 50, 30).await;
        assert_eq!(map2["Gamma"]["action"], json!("CREATE"));

        let mut slow = FakeChat::new("[true]");
        slow.delay_ms = 50;
        let mut map3 = Map::new();
        map3.insert(
            "Delta".to_string(),
            json!({"action": "MAYBE", "similarity": 0.62}),
        );
        resolve_maybe_items(&slow, &mut map3, 50, 0).await;
        assert_eq!(map3["Delta"]["action"], json!("CREATE"));
    }
}

// ---------------------------------------------------------------------------
// Part 6b — PLAN planning call and plan row I/O (`_wiki_planning_call` ..
// `_wiki_persist_plan`).
//
// Adaptation note: batched planning runs sequentially ('static closure
// bounds); the stored user template uses single braces and the shared
// single-pass `render_template`.
// ---------------------------------------------------------------------------

/// `WIKI_PLAN_PLANNING_SYSTEM`.
pub const WIKI_PLAN_PLANNING_SYSTEM: &str = "You are a knowledge compilation planner. Given extracted entities and their relationship to an existing knowledge base, produce a compilation plan. Return ONLY valid JSON.Keep the user's original language (Chinese/English etc.) for generated data.";

/// `_WIKI_PLAN_MAX_OUTPUT_TOKENS`.
pub const WIKI_PLAN_MAX_OUTPUT_TOKENS: usize = 4096;
/// `_WIKI_PLAN_OUTPUT_SAFETY_TOKENS`.
pub const WIKI_PLAN_OUTPUT_SAFETY_TOKENS: usize = 256;
/// `_WIKI_PLAN_PAGE_TOKEN_ESTIMATE`.
pub const WIKI_PLAN_PAGE_TOKEN_ESTIMATE: usize = 48;
/// `_WIKI_PLAN_ITEMS_PER_BATCH`.
pub const WIKI_PLAN_ITEMS_PER_BATCH: usize = 40;
/// `_WIKI_PLAN_MAX_CONCURRENT_BATCHES` (sequential port).
pub const WIKI_PLAN_MAX_CONCURRENT_BATCHES: usize = 4;

/// `WIKI_PLAN_USER_TEMPLATE` — stored post-`.format` (single braces).
pub const WIKI_PLAN_USER_TEMPLATE: &str = r#"## Knowledge base context
Name: {kb_name}
Description: {kb_description}

## Extracted entities (with mention counts)
{entities_summary}

## Extracted concepts (with mention counts)
{concepts_summary}

## Extracted topics
{topics_summary}

## KB reconciliation results
{kb_reconciliation}

Produce a JSON compilation plan:

{
  "pages": [
    {
      "action": "CREATE",
      "slug": "concept/example-name",
      "title": "Example Page Title",
      "page_type": "entity | concept | topic",
      "topic": "short canonical topic name",
      "entity_names": ["entity or concept name covered by this page"],
      "related_kb_pages": ["existing-slug-1"],
      "priority": 1
    }
  ],
  "estimated_page_count": 5,
  "compilation_notes": "any important notes for the compiler"
}

Rules:
- action must be "CREATE" or "UPDATE".
- For UPDATE, slug MUST be an existing wiki page slug from the KB
  reconciliation list above.
- page_type is one of: entity | concept | topic. Do NOT use "source".
- topic is required for every page. Prefer a topic from the extracted
  topics implied by the entities/concepts. If none fits, create a short
  canonical topic name in the user's language. For topic pages, topic should
  usually match the page title.

# Slug format (CRITICAL — every slug must follow this shape exactly)
- The slug is ``<page_type>/<short-descriptive-name>``. The separator
  between the type and the name MUST be a forward slash ``/``. Do NOT use a
  hyphen here.
- The descriptive part is lowercase, English/Latin only (transliterate
  non-English names), and uses hyphens to join multi-word names. Keep it
  short — 1 to 4 words is ideal.
- The descriptive part MUST be unique to that page's specific subject. Do
  NOT prefix every slug with the same KB-wide topic word. If the KB is
  about logistics, do NOT emit ``concept/logistics-channels``,
  ``concept/logistics-warehousing``, ``concept/logistics-fleet`` — emit
  ``concept/distribution-channels``, ``concept/warehousing``,
  ``concept/fleet-management`` instead.
- Do NOT append numeric suffixes (``-1``, ``-2``, ``-v2``) or random hex
  tags to make slugs distinct. If two candidate slugs collide, rename one
  to use a different descriptive word.

Examples of GOOD slugs:
  - ``entity/jane-doe``               (entity page about a person)
  - ``entity/acme-corp``               (entity page about a company)
  - ``concept/fire-safety``            (concept page about a topic)
  - ``concept/expense-approval``       (concept page about a process)
  - ``topic/water-treatment``          (topic page grouping related items)

Examples of BAD slugs (do NOT produce):
  - ``concept-fire-safety``            (missing the ``/`` between type and name)
  - ``concept/logistics-channels-1``   (numeric suffix to distinguish pages)
  - ``concept/logistics-channels-abc`` (random hex tag)
  - ``logistics/concept-channels``     (type and topic order swapped)
  - ``concept/example-name``           (just duplicate the sample)

# Other rules
- Entity/concept identity is one-to-one with pages: every extracted entity and
  concept must be represented by exactly one canonical page, and each identity
  may appear in only one page's ``entity_names``. Never split an identity into
  multiple pages, page types, thematic sections, aliases, language
  transliterations, or alternate slug spellings. Put all supported sections
  for that identity on its single canonical page.
- A page may represent several closely related low-signal entities/concepts
  (max 3-4 per page), but list every represented identity in ``entity_names``
  and do not repeat any identity on another page. If the page budget is tight,
  group identities rather than omitting one or emitting a second page for it.
- Identity ownership does not limit linking: ``related_kb_pages`` should list
  every directly related canonical page supported by the input (within the
  available-page budget). Never link duplicate or non-canonical slug variants.
- priority 1 = highest importance (process first).
- entity_names must match the names in the entities / concepts lists above.
- Target approximately {target_page_count} total pages (feel free to deviate
  by ±50% if the KB content warrants it).
- Return no more than {max_page_count} page objects. This is a hard limit;
  never continue the JSON beyond this number.
- Return ONLY the JSON object.
"#;

fn plan_slug_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?:entity|concept|topic)/[a-z0-9]+(?:-[a-z0-9]+)*$").expect("plan slug regex")
    })
}

/// `_wiki_planning_call`: single LLM call → Compilation Plan JSON.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_planning_call(
    chat: &dyn HarnessChat,
    canonical_entities: &[Value],
    canonical_concepts: &[Value],
    raw_topics: &[Value],
    reconciliation: &Map<String, Value>,
    kb_name: Option<&str>,
    kb_description: Option<&str>,
    target_page_count: usize,
    llm_timeout: i64,
) -> Value {
    planning_call_inner(
        chat,
        canonical_entities,
        canonical_concepts,
        raw_topics,
        reconciliation,
        kb_name,
        kb_description,
        target_page_count,
        llm_timeout,
        0,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn planning_call_inner(
    chat: &dyn HarnessChat,
    canonical_entities: &[Value],
    canonical_concepts: &[Value],
    raw_topics: &[Value],
    reconciliation: &Map<String, Value>,
    kb_name: Option<&str>,
    kb_description: Option<&str>,
    target_page_count: usize,
    llm_timeout: i64,
    batch_depth: usize,
) -> Value {
    let mention_count = |item: &Value| {
        item.get("mention_count")
            .and_then(Value::as_i64)
            .unwrap_or(0)
    };
    let mut sorted_entities: Vec<Value> = canonical_entities.to_vec();
    sorted_entities.sort_by_key(|item| std::cmp::Reverse(mention_count(item)));
    let mut sorted_concepts: Vec<Value> = canonical_concepts.to_vec();
    sorted_concepts.sort_by_key(|item| std::cmp::Reverse(mention_count(item)));

    let model_context = chat.max_length().max(1);
    let output_tokens =
        WIKI_PLAN_MAX_OUTPUT_TOKENS.min(1024.max((model_context as f64 * 0.4) as usize));
    let output_page_capacity = 1.max(
        output_tokens.saturating_sub(WIKI_PLAN_OUTPUT_SAFETY_TOKENS)
            / WIKI_PLAN_PAGE_TOKEN_ESTIMATE,
    );
    let max_page_count =
        output_page_capacity.min((target_page_count + 8).max(target_page_count * 2));

    let mut all_items: Vec<(bool, Value)> = Vec::new();
    for item in &sorted_entities {
        all_items.push((true, item.clone()));
    }
    for item in &sorted_concepts {
        all_items.push((false, item.clone()));
    }

    if batch_depth == 0 && all_items.len() > WIKI_PLAN_ITEMS_PER_BATCH {
        let total_items = all_items.len();
        let mut batch_plans: Vec<Value> = Vec::new();
        for batch in all_items.chunks(WIKI_PLAN_ITEMS_PER_BATCH) {
            let batch_entities: Vec<Value> = batch
                .iter()
                .filter(|(is_entity, _)| *is_entity)
                .map(|(_, item)| item.clone())
                .collect();
            let batch_concepts: Vec<Value> = batch
                .iter()
                .filter(|(is_entity, _)| !*is_entity)
                .map(|(_, item)| item.clone())
                .collect();
            let mut batch_keys: BTreeSet<String> = BTreeSet::new();
            for (is_entity, item) in batch {
                let key = if *is_entity {
                    item.get("name").and_then(Value::as_str)
                } else {
                    item.get("term").and_then(Value::as_str)
                };
                if let Some(key) = key {
                    batch_keys.insert(key.to_string());
                }
            }
            let batch_reconciliation: Map<String, Value> = reconciliation
                .iter()
                .filter(|(key, _)| batch_keys.contains(*key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            let batch_target = 1.max(
                ((target_page_count as f64) * (batch.len() as f64) / (total_items as f64)).round()
                    as usize,
            );
            let plan = Box::pin(planning_call_inner(
                chat,
                &batch_entities,
                &batch_concepts,
                raw_topics,
                &batch_reconciliation,
                kb_name,
                kb_description,
                batch_target,
                llm_timeout,
                1,
            ))
            .await;
            batch_plans.push(plan);
        }
        let mut merged_pages: Vec<Value> = Vec::new();
        let mut seen_slugs: BTreeSet<String> = BTreeSet::new();
        for batch_plan in &batch_plans {
            if let Some(Value::Array(pages)) = batch_plan.get("pages") {
                for page in pages {
                    if let Some(slug) = page.get("slug").and_then(Value::as_str) {
                        if !slug.is_empty() && seen_slugs.insert(slug.to_string()) {
                            merged_pages.push(page.clone());
                        }
                    }
                }
            }
        }
        tracing::info!(
            items = total_items,
            batches = batch_plans.len(),
            pages = merged_pages.len(),
            "wiki_plan: batched planning"
        );
        return json!({
            "pages": merged_pages,
            "estimated_page_count": merged_pages.len(),
            "compilation_notes": "planned in batches",
        });
    }

    let entities_summary = {
        let lines: Vec<String> = sorted_entities
            .iter()
            .take(200)
            .map(|entity| format_entity_for_plan(entity, reconciliation))
            .collect();
        if lines.is_empty() {
            "  (none)".to_string()
        } else {
            lines.join("\n")
        }
    };
    let concepts_summary = {
        let lines: Vec<String> = sorted_concepts
            .iter()
            .take(200)
            .map(|concept| format_concept_for_plan(concept, reconciliation))
            .collect();
        if lines.is_empty() {
            "  (none)".to_string()
        } else {
            lines.join("\n")
        }
    };
    let topics_summary = {
        let lines: Vec<String> = raw_topics
            .iter()
            .take(200)
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|topic| !topic.is_empty())
            .map(|topic| format!("  - {topic}"))
            .collect();
        if lines.is_empty() {
            "  (none)".to_string()
        } else {
            lines.join("\n")
        }
    };
    let kb_reconciliation = {
        let lines: Vec<String> = reconciliation
            .iter()
            .filter(|(_, rec)| rec.get("action").and_then(Value::as_str) == Some("UPDATE"))
            .filter_map(|(name, rec)| {
                let slug = rec.get("page_slug").and_then(Value::as_str)?;
                if slug.is_empty() {
                    return None;
                }
                let sim = rec.get("similarity").and_then(Value::as_f64).unwrap_or(0.0);
                Some(format!("  - UPDATE: {name} → {slug} (sim={sim:.2})"))
            })
            .collect();
        if lines.is_empty() {
            "  (all items are new)".to_string()
        } else {
            lines.join("\n")
        }
    };

    let target_s = target_page_count.to_string();
    let max_pages_s = max_page_count.to_string();
    let user_prompt = render_template(
        WIKI_PLAN_USER_TEMPLATE,
        &[
            ("kb_name", kb_name.unwrap_or("(unspecified)")),
            (
                "kb_description",
                kb_description.unwrap_or("(no description)"),
            ),
            ("entities_summary", entities_summary.as_str()),
            ("concepts_summary", concepts_summary.as_str()),
            ("topics_summary", topics_summary.as_str()),
            ("kb_reconciliation", kb_reconciliation.as_str()),
            ("target_page_count", target_s.as_str()),
            ("max_page_count", max_pages_s.as_str()),
        ],
    );

    let gen_conf = knowledge_compile_gen_conf(
        &chat.model_name(),
        Some(&Map::from_iter([
            ("temperature".to_string(), json!(0.1)),
            ("max_tokens".to_string(), json!(output_tokens)),
        ])),
    );
    let request_conf = Value::Object(gen_conf);
    let timeout = std::time::Duration::from_secs(llm_timeout.max(0) as u64);
    let outcome = tokio::time::timeout(
        timeout,
        wiki_gen_json(chat, WIKI_PLAN_PLANNING_SYSTEM, &user_prompt, &request_conf),
    )
    .await;
    let res = match outcome {
        Err(_elapsed) => {
            tracing::warn!(
                seconds = llm_timeout,
                "wiki_plan: planning LLM call timed out"
            );
            return json!({
                "pages": [],
                "estimated_page_count": 0,
                "compilation_notes": "planning timeout",
            });
        }
        Ok(None) => {
            tracing::warn!("wiki_plan: planning LLM call failed");
            return json!({
                "pages": [],
                "estimated_page_count": 0,
                "compilation_notes": "planning failed",
            });
        }
        Ok(Some(value)) => value,
    };
    if !res.is_object() {
        return json!({
            "pages": [],
            "estimated_page_count": 0,
            "compilation_notes": "planner returned non-object",
        });
    }

    let mut valid_pages: Vec<Value> = Vec::new();
    if let Some(Value::Array(pages)) = res.get("pages") {
        for page in pages {
            let Some(obj) = page.as_object() else {
                continue;
            };
            let action = obj.get("action").and_then(Value::as_str).unwrap_or("");
            if action != "CREATE" && action != "UPDATE" {
                continue;
            }
            let slug = obj.get("slug").and_then(Value::as_str).unwrap_or("");
            if !plan_slug_re().is_match(slug) {
                tracing::warn!(slug = slug, "wiki_plan: dropped invalid planner slug");
                continue;
            }
            let title_ok = obj
                .get("title")
                .and_then(Value::as_str)
                .map(|title| !title.trim().is_empty())
                .unwrap_or(false);
            if !title_ok {
                tracing::warn!(slug = slug, "wiki_plan: dropped page with missing title");
                continue;
            }
            let page_type = obj.get("page_type").and_then(Value::as_str).unwrap_or("");
            if page_type != "entity" && page_type != "concept" && page_type != "topic" {
                tracing::warn!(
                    slug = slug,
                    "wiki_plan: dropped page with invalid page_type"
                );
                continue;
            }
            if slug.split('/').next().unwrap_or("") != page_type {
                tracing::warn!(
                    slug = slug,
                    page_type = page_type,
                    "wiki_plan: dropped page with mismatched page_type"
                );
                continue;
            }
            if action == "UPDATE"
                && !reconciliation.values().any(|rec| {
                    rec.get("action").and_then(Value::as_str) == Some("UPDATE")
                        && rec.get("page_slug").and_then(Value::as_str) == Some(slug)
                })
            {
                tracing::warn!(
                    slug = slug,
                    "wiki_plan: dropped UPDATE for unreconciled slug"
                );
                continue;
            }
            let topic_ok = obj
                .get("topic")
                .and_then(Value::as_str)
                .map(|topic| !topic.trim().is_empty())
                .unwrap_or(false);
            if !topic_ok {
                tracing::warn!(slug = slug, "wiki_plan: dropped page with missing topic");
                continue;
            }
            valid_pages.push(page.clone());
            if valid_pages.len() >= max_page_count {
                break;
            }
        }
    }
    let mut out = res.as_object().cloned().unwrap_or_default();
    out.insert("pages".to_string(), Value::Array(valid_pages.clone()));
    out.insert("estimated_page_count".to_string(), json!(valid_pages.len()));
    out.entry("compilation_notes".to_string())
        .or_insert(Value::String(String::new()));
    Value::Object(out)
}

/// `_wiki_load_reduce_result` (delegates to the resume reader).
pub fn load_reduce_result(store: &dyn DocStore, tenant_id: &str, kb_id: &str) -> Option<Value> {
    load_reduce_resume(store, tenant_id, kb_id).map(|(cached, _)| cached)
}

/// `_wiki_load_reduce_input_hash`.
pub fn load_reduce_input_hash(store: &dyn DocStore, tenant_id: &str, kb_id: &str) -> String {
    load_reduce_resume(store, tenant_id, kb_id)
        .map(|(_, hash)| hash)
        .unwrap_or_default()
}

/// `_wiki_load_plan_resume`: `(cached_plan, stored_input_hash)` or None.
pub fn load_plan_resume(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
) -> Option<(Value, String)> {
    let fields: Vec<String> = ["id", "content_with_weight", "input_hash_kwd"]
        .iter()
        .map(|field| field.to_string())
        .collect();
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_PLAN_COMPILE_KWD.to_string()),
    );
    let rows = match search_page(store, tenant_id, kb_id, &fields, &condition, 0, 1) {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!(error = %err, "wiki_plan: failed to load cached plan");
            return None;
        }
    };
    let row = rows.into_iter().next()?;
    let content = row
        .get("content_with_weight")
        .and_then(Value::as_str)
        .unwrap_or("");
    if content.is_empty() {
        return None;
    }
    let cached: Value = match serde_json::from_str(content) {
        Ok(cached) => cached,
        Err(_) => {
            tracing::debug!("wiki_plan: cached plan unparseable; ignoring");
            return None;
        }
    };
    if !cached.is_object() {
        return None;
    }
    let stored_hash = row
        .get("input_hash_kwd")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Some((cached, stored_hash))
}

/// `_wiki_persist_plan`: upsert the single KB-scoped plan row.
pub fn persist_plan(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    plan: &Value,
    input_hash: &str,
    source_doc_ids: &[String],
) {
    let kb_id_str = kb_id.to_string();
    let row_id = stable_row_id(&[WIKI_PLAN_COMPILE_KWD.to_string(), kb_id_str.clone()]);
    let mut doc = Map::new();
    doc.insert("id".to_string(), Value::String(row_id));
    doc.insert("doc_id".to_string(), Value::String(kb_id_str.clone()));
    doc.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_PLAN_COMPILE_KWD.to_string()),
    );
    doc.insert(
        "source_id".to_string(),
        Value::Array(vec![Value::String(kb_id_str)]),
    );
    doc.insert(
        "source_doc_ids".to_string(),
        Value::Array(
            source_doc_ids
                .iter()
                .map(|doc_id| Value::String(doc_id.clone()))
                .collect(),
        ),
    );
    doc.insert(
        "input_hash_kwd".to_string(),
        Value::String(input_hash.to_string()),
    );
    doc.insert(
        "content_with_weight".to_string(),
        Value::String(plan.to_string()),
    );
    doc.insert("available_int".to_string(), json!(0));

    let mut delete_condition = Map::new();
    delete_condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_PLAN_COMPILE_KWD.to_string()),
    );
    if let Err(err) = store.delete(&delete_condition, &index_name(tenant_id), kb_id) {
        tracing::debug!(
            error = %err,
            "wiki_plan: prior plan delete failed; will overwrite by id"
        );
    }
    if let Err(err) = store.insert(&[doc], &index_name(tenant_id), kb_id) {
        tracing::error!(error = %err, "wiki_plan: failed to persist plan row");
    }
}

#[cfg(test)]
mod wiki_part6b_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;
    use crate::harness::HarnessChat;
    use std::sync::Mutex;

    struct FakeChat {
        reply: String,
        delay_ms: u64,
        calls: Mutex<Vec<String>>,
    }

    impl FakeChat {
        fn new(reply: &str) -> Self {
            Self {
                reply: reply.to_string(),
                delay_ms: 0,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl HarnessChat for FakeChat {
        async fn chat(
            &self,
            _system: &str,
            history: &[Value],
            _gen_conf: &Value,
        ) -> Result<String, String> {
            if self.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            }
            let user = history
                .last()
                .and_then(|message| message.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            self.calls.lock().unwrap().push(user);
            Ok(self.reply.clone())
        }

        fn max_length(&self) -> usize {
            100_000
        }
    }

    const PLAN_REPLY: &str = r#"{
        "pages": [
            {"action": "CREATE", "slug": "entity/alpha", "title": "Alpha", "page_type": "entity", "topic": "alpha", "entity_names": ["Alpha"], "priority": 1},
            {"action": "CREATE", "slug": "entity-Bad_Slug", "title": "Bad", "page_type": "entity", "topic": "bad"},
            {"action": "CREATE", "slug": "concept/beta", "title": "", "page_type": "concept", "topic": "beta"},
            {"action": "CREATE", "slug": "topic/gamma", "title": "Gamma", "page_type": "entity", "topic": "gamma"},
            {"action": "UPDATE", "slug": "entity/not-reconciled", "title": "Nope", "page_type": "entity", "topic": "nope"},
            {"action": "CREATE", "slug": "concept/delta", "title": "Delta", "page_type": "concept"},
            42
        ],
        "estimated_page_count": 99,
        "compilation_notes": "n"
    }"#;

    #[tokio::test]
    async fn planning_validates_and_drops_bad_pages() {
        let chat = FakeChat::new(PLAN_REPLY);
        let entities = vec![json!({"name": "Alpha", "type": "org", "mention_count": 5})];
        let reconciliation = Map::new();
        let plan = wiki_planning_call(
            &chat,
            &entities,
            &[],
            &[json!("t")],
            &reconciliation,
            Some("KB"),
            Some("desc"),
            8,
            30,
        )
        .await;
        let pages = plan["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0]["slug"], json!("entity/alpha"));
        assert_eq!(plan["estimated_page_count"], json!(1));
        assert_eq!(plan["compilation_notes"], json!("n"));
        let prompt = chat.calls.lock().unwrap()[0].clone();
        assert!(prompt.contains("Name: KB"));
        assert!(prompt.contains("Description: desc"));
        assert!(prompt.contains("Target approximately 8 total pages"));
        assert!(prompt.contains("  - Alpha (org, 5 mentions) → CREATE"));
    }

    #[tokio::test]
    async fn planning_timeout_returns_empty() {
        let mut chat = FakeChat::new(PLAN_REPLY);
        chat.delay_ms = 50;
        let plan = wiki_planning_call(&chat, &[], &[], &[], &Map::new(), None, None, 8, 0).await;
        assert_eq!(plan["pages"], json!([]));
        assert_eq!(plan["compilation_notes"], json!("planning timeout"));
    }

    #[tokio::test]
    async fn planning_batches_and_dedups_slugs() {
        let chat = FakeChat::new(PLAN_REPLY);
        let entities: Vec<Value> = (0..41)
            .map(|idx| json!({"name": format!("E{idx}"), "type": "org", "mention_count": 1}))
            .collect();
        let plan =
            wiki_planning_call(&chat, &entities, &[], &[], &Map::new(), None, None, 8, 30).await;
        let calls = chat.calls.lock().unwrap().len();
        assert_eq!(calls, 2);
        assert_eq!(plan["compilation_notes"], json!("planned in batches"));
        assert_eq!(plan["pages"].as_array().unwrap().len(), 1);
        assert_eq!(plan["estimated_page_count"], json!(1));
    }

    #[test]
    fn plan_resume_and_reduce_hash_io() {
        let store = MemoryDocStore::new();
        assert!(load_plan_resume(&store, "t1", "kb1").is_none());
        assert_eq!(load_reduce_input_hash(&store, "t1", "kb1"), "");
        assert!(load_reduce_result(&store, "t1", "kb1").is_none());
        persist_plan(
            &store,
            "t1",
            "kb1",
            &json!({"pages": [{"slug": "entity/alpha"}]}),
            "PH1",
            &["d1".to_string()],
        );
        let (cached, hash) = load_plan_resume(&store, "t1", "kb1").expect("plan");
        assert_eq!(hash, "PH1");
        assert_eq!(cached["pages"][0]["slug"], json!("entity/alpha"));
        persist_reduce(&store, "t1", "kb1", &json!({"entities": []}), "RH1", &[]);
        assert_eq!(load_reduce_input_hash(&store, "t1", "kb1"), "RH1");
        assert!(load_reduce_result(&store, "t1", "kb1").is_some());
        persist_plan(
            &store,
            "t1",
            "kb1",
            &json!({"pages": [{"slug": "entity/beta"}]}),
            "PH2",
            &[],
        );
        let (cached2, hash2) = load_plan_resume(&store, "t1", "kb1").expect("plan2");
        assert_eq!(hash2, "PH2");
        assert_eq!(cached2["pages"][0]["slug"], json!("entity/beta"));
    }
}

// ---------------------------------------------------------------------------
// Part 7 — PLAN main entry (`wiki_plan_from_reduction`).
//
// Adaptation note: the disabled-doc set is host-injected (upstream resolves
// it through DocumentService); callbacks are plain `Fn` calls.
// ---------------------------------------------------------------------------

/// `DEFAULT_WIKI_PLAN_RECONCILE_BATCH`.
pub const DEFAULT_WIKI_PLAN_RECONCILE_BATCH: usize = 50;

/// `wiki_plan_from_reduction`: KB-scoped PLAN phase.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_plan_from_reduction(
    store: &dyn DocStore,
    chat: &dyn HarnessChat,
    embd: Option<&dyn Embedder>,
    tenant_id: &str,
    kb_id: &str,
    disabled_doc_ids: &BTreeSet<String>,
    kb_name: Option<&str>,
    kb_description: Option<&str>,
    update_threshold: f64,
    maybe_threshold: f64,
    reconcile_batch_size: usize,
    llm_timeout: i64,
    force_rerun: bool,
    callback: Option<&(dyn Fn(f64, &str) + Send + Sync)>,
) -> Value {
    let current_reduce_hash = load_reduce_input_hash(store, tenant_id, kb_id);
    let plan_source_doc_ids = all_map_doc_ids(store, tenant_id, kb_id, disabled_doc_ids);

    if !force_rerun {
        if let Some((cached, stored_hash)) = load_plan_resume(store, tenant_id, kb_id) {
            if !stored_hash.is_empty() && stored_hash == current_reduce_hash {
                if let Some(callback) = callback {
                    callback(1.0, "wiki PLAN: cache hit (REDUCE unchanged)");
                }
                return cached;
            }
        }
    }

    if let Some(callback) = callback {
        callback(0.05, "wiki PLAN: loading REDUCE result");
    }
    let Some(reduced) = load_reduce_result(store, tenant_id, kb_id) else {
        tracing::warn!(
            kb = kb_id,
            "wiki_plan: no wiki_reduce_result found — returning empty plan"
        );
        let empty = json!({
            "pages": [],
            "estimated_page_count": 0,
            "compilation_notes": "no REDUCE result available",
            "_status": "approved",
            "_entities": [],
            "_concepts": [],
            "_claims": [],
            "_relations": [],
            "_topics": [],
            "_reconciliation": {},
        });
        persist_plan(
            store,
            tenant_id,
            kb_id,
            &empty,
            &current_reduce_hash,
            &plan_source_doc_ids,
        );
        return empty;
    };

    let arr = |key: &str| -> Vec<Value> {
        reduced
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let canonical_entities = arr("entities");
    let canonical_concepts = arr("concepts");
    let raw_claims = arr("claims");
    let raw_relations = arr("relations");
    let raw_topics = arr("topics");
    let total_items = canonical_entities.len() + canonical_concepts.len();
    tracing::info!(
        kb = kb_id,
        entities = canonical_entities.len(),
        concepts = canonical_concepts.len(),
        total = total_items,
        "wiki_plan: reduction input"
    );

    if total_items == 0 {
        let empty = json!({
            "pages": [],
            "estimated_page_count": 0,
            "compilation_notes": "no canonical items",
            "_status": "approved",
            "_entities": canonical_entities,
            "_concepts": canonical_concepts,
            "_claims": raw_claims,
            "_relations": raw_relations,
            "_topics": raw_topics,
            "_reconciliation": {},
        });
        persist_plan(
            store,
            tenant_id,
            kb_id,
            &empty,
            &current_reduce_hash,
            &plan_source_doc_ids,
        );
        return empty;
    }

    if let Some(callback) = callback {
        callback(0.25, "wiki PLAN: KB reconciliation");
    }
    let mut reconciliation = reconcile_with_kb(
        store,
        embd,
        tenant_id,
        kb_id,
        &canonical_entities,
        &canonical_concepts,
        update_threshold,
        maybe_threshold,
    )
    .await;

    if let Some(callback) = callback {
        let n_maybe = reconciliation
            .values()
            .filter(|value| value.get("action").and_then(Value::as_str) == Some("MAYBE"))
            .count();
        callback(0.55, &format!("wiki PLAN: resolving {n_maybe} MAYBE items"));
    }

    resolve_maybe_items(chat, &mut reconciliation, reconcile_batch_size, llm_timeout).await;

    if let Some(callback) = callback {
        callback(0.75, "wiki PLAN: planning LLM call");
    }

    let target = target_page_count(total_items as i64);
    let mut plan = wiki_planning_call(
        chat,
        &canonical_entities,
        &canonical_concepts,
        &raw_topics,
        &reconciliation,
        kb_name,
        kb_description,
        target,
        llm_timeout,
    )
    .await;
    if let Some(obj) = plan.as_object_mut() {
        obj.insert("_status".to_string(), Value::String("approved".to_string()));
        obj.insert(
            "_entities".to_string(),
            Value::Array(canonical_entities.clone()),
        );
        obj.insert(
            "_concepts".to_string(),
            Value::Array(canonical_concepts.clone()),
        );
        obj.insert("_claims".to_string(), Value::Array(raw_claims.clone()));
        obj.insert(
            "_relations".to_string(),
            Value::Array(raw_relations.clone()),
        );
        obj.insert("_topics".to_string(), Value::Array(raw_topics.clone()));
        obj.insert(
            "_reconciliation".to_string(),
            Value::Object(reconciliation.clone()),
        );
    }

    if let Some(callback) = callback {
        callback(0.9, "wiki PLAN: persisting plan");
    }
    persist_plan(
        store,
        tenant_id,
        kb_id,
        &plan,
        &current_reduce_hash,
        &plan_source_doc_ids,
    );

    let action_count = |action: &str| {
        reconciliation
            .values()
            .filter(|value| value.get("action").and_then(Value::as_str) == Some(action))
            .count()
    };
    let page_count = plan
        .get("pages")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let updates = action_count("UPDATE");
    let creates = action_count("CREATE");
    tracing::info!(
        kb = kb_id,
        pages = page_count,
        target = target,
        updates = updates,
        creates = creates,
        "wiki_plan: done"
    );

    if let Some(callback) = callback {
        callback(1.0, "wiki PLAN: done");
    }
    plan
}

#[cfg(test)]
mod wiki_part7_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;
    use crate::harness::HarnessChat;
    use std::sync::Mutex;

    struct FakeChat {
        reply: String,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl HarnessChat for FakeChat {
        async fn chat(
            &self,
            _system: &str,
            history: &[Value],
            _gen_conf: &Value,
        ) -> Result<String, String> {
            let user = history
                .last()
                .and_then(|message| message.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            self.calls.lock().unwrap().push(user);
            Ok(self.reply.clone())
        }

        fn max_length(&self) -> usize {
            100_000
        }
    }

    const PLAN_REPLY: &str = r#"{
        "pages": [
            {"action": "CREATE", "slug": "entity/alpha", "title": "Alpha", "page_type": "entity", "topic": "alpha", "entity_names": ["Alpha"], "priority": 1}
        ],
        "estimated_page_count": 1,
        "compilation_notes": "n"
    }"#;

    #[tokio::test]
    async fn plan_without_reduce_result_persists_empty() {
        let store = MemoryDocStore::new();
        let chat = FakeChat {
            reply: PLAN_REPLY.to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let disabled = BTreeSet::new();
        let plan = wiki_plan_from_reduction(
            &store,
            &chat,
            None,
            "t1",
            "kb1",
            &disabled,
            None,
            None,
            DEFAULT_WIKI_PLAN_UPDATE_THRESHOLD,
            DEFAULT_WIKI_PLAN_MAYBE_THRESHOLD,
            DEFAULT_WIKI_PLAN_RECONCILE_BATCH,
            30,
            false,
            None,
        )
        .await;
        assert_eq!(plan["pages"], json!([]));
        assert_eq!(
            plan["compilation_notes"],
            json!("no REDUCE result available")
        );
        assert_eq!(plan["_status"], json!("approved"));
        assert!(load_plan_resume(&store, "t1", "kb1").is_some());
        assert!(chat.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn plan_cache_hit_short_circuits() {
        let store = MemoryDocStore::new();
        persist_reduce(
            &store,
            "t1",
            "kb1",
            &json!({"entities": [], "concepts": []}),
            "RH1",
            &[],
        );
        persist_plan(
            &store,
            "t1",
            "kb1",
            &json!({"pages": [{"slug": "entity/cached"}], "_status": "approved"}),
            "RH1",
            &[],
        );
        let chat = FakeChat {
            reply: PLAN_REPLY.to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let hit = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = hit.clone();
        let callback = move |progress: f64, message: &str| {
            if progress >= 1.0 && message.contains("cache hit") {
                seen.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        };
        let disabled = BTreeSet::new();
        let plan = wiki_plan_from_reduction(
            &store,
            &chat,
            None,
            "t1",
            "kb1",
            &disabled,
            None,
            None,
            DEFAULT_WIKI_PLAN_UPDATE_THRESHOLD,
            DEFAULT_WIKI_PLAN_MAYBE_THRESHOLD,
            DEFAULT_WIKI_PLAN_RECONCILE_BATCH,
            30,
            false,
            Some(&callback),
        )
        .await;
        assert!(hit.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(plan["pages"][0]["slug"], json!("entity/cached"));
        assert!(chat.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn plan_full_flow_reconciles_and_persists() {
        let store = MemoryDocStore::new();
        persist_reduce(
            &store,
            "t1",
            "kb1",
            &json!({
                "entities": [{"name": "Alpha", "type": "org", "mention_count": 5, "chunk_ids": ["c1"]}],
                "concepts": [{"term": "Beta", "mention_count": 2, "chunk_ids": ["c2"]}],
                "claims": [],
                "relations": [],
                "topics": ["t"]
            }),
            "RH2",
            &["d1".to_string()],
        );
        let chat = FakeChat {
            reply: PLAN_REPLY.to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let disabled = BTreeSet::new();
        let plan = wiki_plan_from_reduction(
            &store,
            &chat,
            None,
            "t1",
            "kb1",
            &disabled,
            Some("KB"),
            Some("desc"),
            DEFAULT_WIKI_PLAN_UPDATE_THRESHOLD,
            DEFAULT_WIKI_PLAN_MAYBE_THRESHOLD,
            DEFAULT_WIKI_PLAN_RECONCILE_BATCH,
            30,
            false,
            None,
        )
        .await;
        assert_eq!(plan["_status"], json!("approved"));
        assert_eq!(plan["pages"].as_array().unwrap().len(), 1);
        assert_eq!(plan["_entities"].as_array().unwrap().len(), 1);
        assert_eq!(plan["_reconciliation"]["Alpha"]["action"], json!("CREATE"));
        assert!(!chat.calls.lock().unwrap().is_empty());
        let (_, stored_hash) = load_plan_resume(&store, "t1", "kb1").expect("plan");
        assert_eq!(stored_hash, "RH2");
    }

    #[tokio::test]
    async fn plan_zero_items_persists_short_circuit() {
        let store = MemoryDocStore::new();
        persist_reduce(
            &store,
            "t1",
            "kb1",
            &json!({"entities": [], "concepts": [], "topics": ["t"]}),
            "RH3",
            &[],
        );
        let chat = FakeChat {
            reply: PLAN_REPLY.to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let disabled = BTreeSet::new();
        let plan = wiki_plan_from_reduction(
            &store,
            &chat,
            None,
            "t1",
            "kb1",
            &disabled,
            None,
            None,
            DEFAULT_WIKI_PLAN_UPDATE_THRESHOLD,
            DEFAULT_WIKI_PLAN_MAYBE_THRESHOLD,
            DEFAULT_WIKI_PLAN_RECONCILE_BATCH,
            30,
            false,
            None,
        )
        .await;
        assert_eq!(plan["compilation_notes"], json!("no canonical items"));
        assert_eq!(plan["_topics"], json!(["t"]));
        assert!(chat.calls.lock().unwrap().is_empty());
        let (_, hash) = load_plan_resume(&store, "t1", "kb1").expect("plan");
        assert_eq!(hash, "RH3");
    }
}

// ---------------------------------------------------------------------------
// Part 8 — REFINE prompts and evidence helpers (`WIKI_REFINE_*` constants,
// `_build_refine_writer_system` .. `_wiki_collect_evidence_chunk_ids`).
//
// Adaptation note: word-boundary matching uses Rust `regex` (\b, unicode)
// with `regex::escape` + case-insensitive construction.
// ---------------------------------------------------------------------------

/// `WIKI_DRAFT_COMPILE_KWD`.
pub const WIKI_DRAFT_COMPILE_KWD: &str = "wiki_page_draft";
/// `DEFAULT_WIKI_REFINE_WORKERS` (`_env_int("WIKI_REFINE_WORKERS", 4, minimum=1)`).
pub fn default_wiki_refine_workers() -> usize {
    env_int("WIKI_REFINE_WORKERS", 4, Some(1)).max(1) as usize
}
/// `DEFAULT_WIKI_REFINE_TIMEOUT` (`_env_int("WIKI_REFINE_TIMEOUT", 300, minimum=1)`).
pub fn default_wiki_refine_timeout() -> i64 {
    env_int("WIKI_REFINE_TIMEOUT", 300, Some(1)).max(1)
}
/// `WIKI_REFINE_SOURCE_BUDGET_CHARS`.
pub const WIKI_REFINE_SOURCE_BUDGET_CHARS: usize = 60_000;
/// `WIKI_MERGE_BODY_SHRINK_THRESHOLD`.
pub const WIKI_MERGE_BODY_SHRINK_THRESHOLD: f64 = 0.7;
/// `WIKI_MERGE_TIMEOUT` (`_env_int("WIKI_MERGE_TIMEOUT", 600, minimum=1)`).
pub fn wiki_merge_timeout() -> i64 {
    env_int("WIKI_MERGE_TIMEOUT", 600, Some(1)).max(1)
}

/// `WIKI_TEMPLATE_EXAMPLE`.
pub const WIKI_TEMPLATE_EXAMPLE: &str = "Each page must be a proper encyclopedic article, NOT a flat bullet list:\n1. Opening paragraph (2-4 sentences defining what this is). No heading.\n2. Sections with H2 headings, each starting with prose before sub-bullets.\n   Put every heading on its own line and separate every paragraph with a blank line.\n3. Bold key terms on first use; link them with [[ ]] wikilinks.\n4. Examples or implications where the source provides them.\n5. End with a \"## See also\" section listing wikilinks to highly related pages (less than 12).\n\nPage structure could be as following:\n(Not provided)";

/// `WIKI_REFINE_WRITER_SYSTEM_TEMPLATE` — stored post-`.format` (single braces).
pub const WIKI_REFINE_WRITER_SYSTEM_TEMPLATE: &str = "You are an enterprise knowledge compilation writer. Your job is to write a single, high-quality wiki page by reading the SOURCE TEXT provided and using the evidence checklist as guidance for what to cover.\n\n# Mindset: COMPILE, do NOT summarize\nYou are not writing an executive summary. You are extracting structured knowledge and rewriting it into a reusable wiki page. The output should contain MORE information density than a summary — organized differently, but not condensed. A summary loses specifics. A wiki page preserves them in a queryable structure.\n\n# What to KEEP from the source (do not lose these)\n- Specific numbers: thresholds, dosages, timeframes, dimensions, percentages.\n- Named regulations, laws, articles, code references.\n- Equipment names, model numbers, product specs.\n- Procedure steps in order, with actual actions.\n- Worked examples and exceptions.\n- Named parties, roles, contact paths, escalation chains.\n- Definitions verbatim or near-verbatim if the source is authoritative.\n- Cause-effect statements ('X causes Y because Z') — preserve all three parts.\n\n# What to DROP\n- Marketing language, mission statements, ceremonial filler.\n- Source-specific framing: 'This document explains…', 'In Section 3 below…'.\n- Repeated boilerplate, tables of contents, cover-page metadata.\n- Prose that just rephrases what was already said.\n\n# Language\nWrite in the SAME LANGUAGE as the source text. Never translate content.\n\n# Additional writing instructions — CRITICAL\n{template_instruction}\n\n# Page structure example — CRITICAL\n{template_example}\n\n# What NOT to do\n- Do NOT dump raw bullet points from the source as the entire content.\n- Do NOT omit the opening prose paragraph.\n- Do NOT include Citations / Footnotes sections.\n- Do NOT use [^N] footnote markers.\n- Do NOT translate the content language.\n\n# Wikilinks\n- Use [[slug]] or [[slug|display text]] to cross-link.\n- CRITICAL: You may ONLY link to slugs from the 'Available pages' list.\n  Do NOT invent or hallucinate slugs.\n\n# Minimum depth\n- concept/topic pages: at least 200 words of actual prose+structure.\n- entity pages: at least 100 words.\n";

/// `_build_refine_writer_system`.
pub fn build_refine_writer_system(instruction: Option<&str>, example: Option<&str>) -> String {
    let instruction_body = instruction
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("Follow the page structure and writing requirements below.");
    let example_body = example
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(WIKI_TEMPLATE_EXAMPLE);
    render_template(
        WIKI_REFINE_WRITER_SYSTEM_TEMPLATE,
        &[
            ("template_instruction", instruction_body),
            ("template_example", example_body),
        ],
    )
}

/// `WIKI_REFINE_WRITER_SYSTEM` (default-filled).
pub fn refine_writer_system() -> String {
    build_refine_writer_system(None, None)
}

/// `WIKI_REFINE_WRITER_USER_TEMPLATE` — stored post-`.format` (single braces).
pub const WIKI_REFINE_WRITER_USER_TEMPLATE: &str = "## Task\n{action} the following wiki page.\n\n## Page specification\n- Slug: {slug}\n- Title: {title}\n- Type: {page_type}\n\n## Available pages (ONLY use these slugs for [[wikilinks]])\n{all_plan_slugs}\n\n{existing_section}\n\n## Source document text\nRead this carefully. Extract all relevant facts for this page's topic.\n\n{source_context}\n\n## Evidence checklist ({evidence_count} items)\nThe following items were pre-extracted and should be covered in the page.\nUse them as a checklist — make sure you don't miss any of these facts.\nBut also look for additional relevant information in the source text above.\n\n{evidence_blocks}\n\n## Instructions\nWrite the complete wiki page in markdown based on the source text above.\nPut every heading on its own line and separate every paragraph with a blank line. Do not return the page as one line.\nCross-link to other pages using [[slug]] or [[slug|display text]] — ONLY\nuse slugs from the \"Available pages\" list. Do NOT invent new slugs.\nDo NOT include Citations or Footnotes sections.\nMUST be in the language as the same as the source document text is.\n\nReturn ONLY the markdown content, no other text.\n";

/// `WIKI_REFINE_MERGE_SYSTEM`.
pub const WIKI_REFINE_MERGE_SYSTEM: &str = "You are a wiki page merger. You receive two versions of the same wiki page:\n- EXISTING: the current version in the knowledge base.\n- INCOMING: a new version generated from a different source document.\n\nYour job is to produce a SINGLE unified page that preserves ALL factual content from BOTH versions. Rules:\n\n1. KEEP all facts, numbers, procedures, names from both versions.\n2. REMOVE exact duplicates — if both versions state the same fact, keep it once.\n3. ORGANIZE coherently — clear H2 sections, opening paragraph, ## See also.\n4. PRESERVE [[wikilinks]] from both versions.\n5. Write in the SAME LANGUAGE as the existing content.\n6. Do NOT summarize or condense — the merged page should be AT LEAST as long as the longer of the two inputs.\n7. Do NOT add any facts not present in either version.\n\nReturn ONLY the merged markdown content, no other text.";

/// `_wiki_strip_think`.
pub fn strip_think(raw: &str) -> String {
    wiki_think_re().replace(raw, "").trim().to_string()
}

/// `_wiki_assemble_evidence`.
pub fn assemble_evidence(
    plan_item: &Value,
    claims: &[Value],
    entity_by_name: Option<&Map<String, Value>>,
    concept_by_term: Option<&Map<String, Value>>,
) -> Vec<Value> {
    let raw_names: Vec<String> = plan_item
        .get("entity_names")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if raw_names.is_empty() {
        return Vec::new();
    }
    let names_lower: Vec<String> = raw_names.iter().map(|name| name.to_lowercase()).collect();
    let patterns: Vec<Regex> = raw_names
        .iter()
        .map(|name| {
            regex::RegexBuilder::new(&format!(r"\b{}\b", regex::escape(name)))
                .case_insensitive(true)
                .build()
                .expect("evidence pattern")
        })
        .collect();

    let mut evidence: Vec<Value> = Vec::new();
    for claim in claims {
        let Some(obj) = claim.as_object() else {
            continue;
        };
        let subject_raw = obj
            .get("subject")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if subject_raw.is_empty() {
            continue;
        }
        let subject_lower = subject_raw.to_lowercase();
        let matched = names_lower.contains(&subject_lower)
            || patterns
                .iter()
                .any(|pattern| pattern.is_match(&subject_raw));
        if !matched {
            continue;
        }
        let chunk_ids: Vec<Value> = obj
            .get("chunk_ids")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|item| item.as_str().map(|text| !text.is_empty()).unwrap_or(false))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        evidence.push(json!({
            "statement": obj.get("statement").cloned().unwrap_or(Value::String(String::new())),
            "subject": obj.get("subject").cloned().unwrap_or(Value::String(String::new())),
            "confidence": obj.get("confidence").cloned().unwrap_or(Value::String("explicit".to_string())),
            "chunk_ids": chunk_ids,
        }));
    }
    if !evidence.is_empty() {
        return evidence;
    }

    // Fallback: derive evidence from entity/concept chunk_ids.
    if entity_by_name.is_none() && concept_by_term.is_none() {
        return Vec::new();
    }
    let mut fallback_chunk_ids: Vec<String> = Vec::new();
    let mut matched_names: Vec<String> = Vec::new();
    for (name, name_lower) in raw_names.iter().zip(names_lower.iter()) {
        let mut hit = entity_by_name.and_then(|map| map.get(name_lower));
        if hit.is_none() {
            hit = concept_by_term.and_then(|map| map.get(name_lower));
        }
        let Some(hit) = hit else { continue };
        if hit.is_null() {
            continue;
        }
        if let Some(items) = hit.get("chunk_ids").and_then(Value::as_array) {
            for cid in items {
                if let Some(text) = cid.as_str() {
                    if !text.is_empty() && !fallback_chunk_ids.iter().any(|seen| seen == text) {
                        fallback_chunk_ids.push(text.to_string());
                    }
                }
            }
        }
        matched_names.push(name.clone());
    }
    if fallback_chunk_ids.is_empty() {
        return Vec::new();
    }
    let subject = matched_names
        .first()
        .cloned()
        .unwrap_or_else(|| raw_names[0].clone());
    vec![json!({
        "statement": "",
        "subject": subject,
        "confidence": "inferred",
        "chunk_ids": fallback_chunk_ids,
        "_synthetic": true,
    })]
}

/// `_wiki_format_evidence_blocks`.
pub fn format_evidence_blocks(evidence: &[Value]) -> String {
    let real_evidence: Vec<&Value> = evidence
        .iter()
        .filter(|item| !item.get("_synthetic").map(json_truthy).unwrap_or(false))
        .collect();
    if real_evidence.is_empty() {
        return "(no pre-extracted evidence — extract facts directly from the source document text above)".to_string();
    }
    let mut lines: Vec<String> = Vec::new();
    for (idx, item) in real_evidence.iter().enumerate() {
        let confidence = item
            .get("confidence")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .unwrap_or("explicit")
            .to_uppercase();
        let subject = item.get("subject").and_then(Value::as_str).unwrap_or("");
        let statement = item.get("statement").and_then(Value::as_str).unwrap_or("");
        lines.push(format!(
            "{}. [{confidence}] {subject}\n   {statement}",
            idx + 1
        ));
    }
    lines.join("\n\n")
}

/// `_wiki_collect_evidence_chunk_ids`.
pub fn collect_evidence_chunk_ids(evidence: &[Value]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for item in evidence {
        if let Some(items) = item.get("chunk_ids").and_then(Value::as_array) {
            for cid in items {
                if let Some(text) = cid.as_str() {
                    if !text.is_empty() && !seen.iter().any(|existing| existing == text) {
                        seen.push(text.to_string());
                    }
                }
            }
        }
    }
    seen
}

#[cfg(test)]
mod wiki_part8_tests {
    use super::*;

    #[test]
    fn writer_system_defaults_and_overrides() {
        let default = build_refine_writer_system(None, None);
        assert!(default.contains("Follow the page structure and writing requirements below."));
        assert!(default.contains("Page structure could be as following:"));
        assert!(!default.contains("{template_instruction}"));
        let custom = build_refine_writer_system(Some("  Be terse.  "), Some("Example body"));
        assert!(custom.contains("Be terse."));
        assert!(custom.contains("Example body"));
        assert_eq!(refine_writer_system(), default);
    }

    #[test]
    fn strip_think_removes_leading_block() {
        assert_eq!(strip_think("abc"), "abc");
        assert_eq!(strip_think("secret</think>answer"), "answer");
        assert_eq!(strip_think("secret</think>  \n answer"), "answer");
    }

    #[test]
    fn evidence_matching_exact_and_word_boundary() {
        let plan_item = json!({"entity_names": ["Alpha Corp", "Beta"]});
        let claims = vec![
            json!({"statement": "s1", "subject": "alpha corp", "confidence": "explicit", "chunk_ids": ["c1", ""]}),
            json!({"statement": "s2", "subject": "Alpha Corp Inc", "confidence": "inferred", "chunk_ids": ["c2"]}),
            json!({"statement": "s3", "subject": "Gamma", "chunk_ids": ["c3"]}),
            json!({"statement": "s4", "subject": ""}),
        ];
        let evidence = assemble_evidence(&plan_item, &claims, None, None);
        assert_eq!(evidence.len(), 2);
        assert_eq!(evidence[0]["statement"], json!("s1"));
        assert_eq!(evidence[0]["chunk_ids"], json!(["c1"]));
        assert_eq!(evidence[0]["confidence"], json!("explicit"));
        assert_eq!(evidence[1]["statement"], json!("s2"));
    }

    #[test]
    fn evidence_fallback_synthesizes_stub() {
        let plan_item = json!({"entity_names": ["Alpha"]});
        let mut entities = Map::new();
        entities.insert(
            "alpha".to_string(),
            json!({"name": "Alpha", "chunk_ids": ["c9", "c9", "c8"]}),
        );
        let evidence = assemble_evidence(&plan_item, &[], Some(&entities), None);
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0]["_synthetic"], json!(true));
        assert_eq!(evidence[0]["chunk_ids"], json!(["c9", "c8"]));
        assert_eq!(evidence[0]["subject"], json!("Alpha"));
        assert_eq!(evidence[0]["confidence"], json!("inferred"));
        assert!(assemble_evidence(&plan_item, &[], None, None).is_empty());
    }

    #[test]
    fn evidence_blocks_filter_format_and_collect() {
        let evidence = vec![
            json!({"statement": "one", "subject": "Alpha", "confidence": "explicit", "chunk_ids": ["c1", "c2"]}),
            json!({"statement": "", "subject": "Alpha", "confidence": "inferred", "chunk_ids": ["c3"], "_synthetic": true}),
            json!({"statement": "two", "subject": "Beta", "confidence": "inferred", "chunk_ids": ["c2", "c4"]}),
        ];
        let blocks = format_evidence_blocks(&evidence);
        assert_eq!(
            blocks,
            "1. [EXPLICIT] Alpha\n   one\n\n2. [INFERRED] Beta\n   two"
        );
        assert_eq!(
            collect_evidence_chunk_ids(&evidence),
            vec!["c1", "c2", "c3", "c4"]
        );
        assert_eq!(
            format_evidence_blocks(&[]),
            "(no pre-extracted evidence — extract facts directly from the source document text above)"
        );
        let only_synthetic = vec![json!({"_synthetic": true, "chunk_ids": ["c1"]})];
        assert!(format_evidence_blocks(&only_synthetic).starts_with("(no pre-extracted evidence"));
    }
}

// ---------------------------------------------------------------------------
// Part 9 — chunk loading, source-context assembly and wikilink rewriting
// (`_wiki_load_chunks_by_id` .. `_wiki_collect_doc_ids`).
//
// Adaptation note: the batch `{"id": [ids]}` condition has containment
// semantics in the local store (not ES terms), so chunk fetches use per-id
// `get()` calls, which is also the upstream fallback path. `urlsplit` is a
// hand-rolled parser (scheme/netloc/path with query stripped).
// ---------------------------------------------------------------------------

fn dedup_ids(chunk_ids: &[String]) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut unique: Vec<String> = Vec::new();
    for cid in chunk_ids {
        if cid.is_empty() || seen.contains(cid) {
            continue;
        }
        seen.insert(cid.clone());
        unique.push(cid.clone());
    }
    unique
}

/// `_wiki_load_chunks_by_id`: `{chunk_id: content_with_weight}`.
pub fn load_chunks_by_id(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    chunk_ids: &[String],
) -> Map<String, Value> {
    let mut out: Map<String, Value> = Map::new();
    let unique_ids = dedup_ids(chunk_ids);
    if unique_ids.is_empty() {
        return out;
    }
    let index = index_name(tenant_id);
    for cid in &unique_ids {
        let row = store.get(cid, &index, &[kb_id.to_string()]).ok().flatten();
        if let Some(row) = row {
            let content = row
                .get("content_with_weight")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !content.is_empty() {
                out.insert(cid.clone(), Value::String(content.to_string()));
            }
        }
    }
    let missing: Vec<&String> = unique_ids
        .iter()
        .filter(|cid| !out.contains_key(*cid))
        .collect();
    if !missing.is_empty() {
        tracing::warn!(
            missing = missing.len(),
            total = unique_ids.len(),
            kb = kb_id,
            first = missing[0].as_str(),
            "wiki_refine: chunk fetch missed id(s)"
        );
    }
    out
}

/// `_wiki_build_source_context`: labelled, budget-limited source block.
pub fn build_source_context(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    evidence: &[Value],
    budget: usize,
) -> String {
    let chunk_ids = collect_evidence_chunk_ids(evidence);
    if chunk_ids.is_empty() {
        return "(no source chunks available)".to_string();
    }
    let chunk_map = load_chunks_by_id(store, tenant_id, kb_id, &chunk_ids);
    if chunk_map.is_empty() {
        return "(source chunks could not be loaded)".to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut truncated = 0usize;
    for cid in &chunk_ids {
        let Some(content) = chunk_map.get(cid).and_then(Value::as_str) else {
            continue;
        };
        if content.is_empty() {
            continue;
        }
        let block = format!("[CHUNK {cid}]\n{content}");
        let block_chars = block.chars().count();
        if total + block_chars + 2 > budget {
            let remaining = budget.saturating_sub(total);
            if remaining > 1000 {
                parts.push(format!(
                    "{}\n\n[…chunk truncated…]",
                    truncate_chars(&block, remaining)
                ));
                total += remaining;
            }
            truncated += 1;
            continue;
        }
        parts.push(block);
        total += block_chars + 2;
    }
    if truncated > 0 {
        parts.push(format!(
            "\n\n[…{truncated} chunk(s) omitted to fit context budget…]"
        ));
    }
    parts.join("\n\n")
}

fn wikilink_pipe_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[\[([^\[\]|]+?)\|([^\[\]]+?)\]\]").expect("wikilink pipe"))
}

fn wikilink_simple_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[\[([^\[\]|]+?)\]\]").expect("wikilink simple"))
}

fn wiki_markdown_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").expect("markdown link"))
}

fn split_url(href: &str) -> (String, String, String) {
    let cleaned = href.split(['?', '#']).next().unwrap_or("");
    if let Some(idx) = cleaned.find("://") {
        let scheme = cleaned[..idx].to_string();
        let rest = &cleaned[idx + 3..];
        let (netloc, path) = match rest.find('/') {
            Some(slash) => (rest[..slash].to_string(), rest[slash..].to_string()),
            None => (rest.to_string(), String::new()),
        };
        (scheme, netloc, path)
    } else {
        (String::new(), String::new(), cleaned.to_string())
    }
}

fn python_title(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_alpha = false;
    for ch in text.chars() {
        if ch.is_alphabetic() {
            if prev_alpha {
                out.extend(ch.to_lowercase());
            } else {
                out.extend(ch.to_uppercase());
            }
            prev_alpha = true;
        } else {
            out.push(ch);
            prev_alpha = false;
        }
    }
    out
}

/// `_wiki_transform_links`: `(rendered_md, unique_outlinks)`.
pub fn transform_links(
    content_md: &str,
    kb_id: &str,
    page_titles: Option<&Map<String, Value>>,
    valid_slugs: Option<&BTreeSet<String>>,
) -> (String, Vec<String>) {
    let kb_id_str = kb_id.to_string();
    let seen: std::cell::RefCell<BTreeSet<String>> = std::cell::RefCell::new(BTreeSet::new());
    let outlinks: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());

    let track = |slug: &str| {
        let trimmed = slug.trim();
        if !trimmed.is_empty() && seen.borrow_mut().insert(trimmed.to_string()) {
            outlinks.borrow_mut().push(trimmed.to_string());
        }
    };
    let display_text = |label: &str, slug: &str| -> String {
        let label = label.trim();
        let short = slug.rsplit('/').next().unwrap_or(slug);
        if label != slug && label != short {
            return label.to_string();
        }
        if let Some(title) = page_titles
            .and_then(|map| map.get(slug))
            .and_then(Value::as_str)
            .filter(|title| !title.is_empty())
        {
            return title.to_string();
        }
        let readable = short.replace('-', " ").replace('_', " ").trim().to_string();
        let titled = python_title(&readable);
        if titled.is_empty() {
            label.to_string()
        } else {
            titled
        }
    };
    let is_valid =
        |slug: &str| -> bool { valid_slugs.map(|set| set.contains(slug)).unwrap_or(true) };
    let wiki_slug = |href: &str| -> Option<String> {
        let (scheme, netloc, path) = split_url(href);
        let path = if !scheme.is_empty() || !netloc.is_empty() {
            if netloc != "artifact" {
                return None;
            }
            path
        } else {
            path
        };
        let mut parts: Vec<&str> = path.trim_matches('/').split('/').collect();
        if parts.first() == Some(&"artifact") {
            parts = parts[1..].to_vec();
        }
        if parts.len() < 2 || parts[0] != kb_id_str {
            return None;
        }
        Some(parts[1..].join("/"))
    };

    let rewritten = wiki_markdown_link_re().replace_all(content_md, |caps: &regex::Captures| {
        let whole = caps.get(0).map(|m| m.as_str()).unwrap_or("");
        let href = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        let label = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let Some(slug) = wiki_slug(href) else {
            return whole.to_string();
        };
        if !is_valid(&slug) {
            return display_text(label, &slug);
        }
        track(&slug);
        format!(
            "[{}](artifact/{}/{})",
            display_text(label, &slug),
            kb_id_str,
            slug
        )
    });
    let rewritten = wikilink_pipe_re().replace_all(&rewritten, |caps: &regex::Captures| {
        let slug = caps.get(1).map(|m| m.as_str()).unwrap_or("").trim();
        let text = caps.get(2).map(|m| m.as_str()).unwrap_or("").trim();
        if !is_valid(slug) {
            return text.to_string();
        }
        track(slug);
        format!("[{text}](artifact/{}/{})", kb_id_str, slug)
    });
    let rewritten = wikilink_simple_re().replace_all(&rewritten, |caps: &regex::Captures| {
        let slug = caps.get(1).map(|m| m.as_str()).unwrap_or("").trim();
        if !is_valid(slug) {
            return display_text(slug, slug);
        }
        track(slug);
        format!(
            "[{}](artifact/{}/{})",
            display_text(slug, slug),
            kb_id_str,
            slug
        )
    });
    let outlinks = outlinks.borrow().clone();
    (rewritten.to_string(), outlinks)
}

/// `_wiki_collect_doc_ids`: unique `doc_id`s of the given chunks.
pub fn wiki_collect_doc_ids(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    chunk_ids: &[String],
) -> Vec<String> {
    if chunk_ids.is_empty() {
        return Vec::new();
    }
    let index = index_name(tenant_id);
    let mut out: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut rows_seen = 0usize;
    for cid in chunk_ids {
        if cid.is_empty() {
            continue;
        }
        let row = store.get(cid, &index, &[kb_id.to_string()]).ok().flatten();
        let Some(row) = row else { continue };
        rows_seen += 1;
        for doc_id in wiki_doc_ids(row.get("doc_id").unwrap_or(&Value::Null)) {
            if seen.insert(doc_id.clone()) {
                out.push(doc_id);
            }
        }
    }
    if !chunk_ids.is_empty() && out.is_empty() {
        tracing::warn!(
            chunks = chunk_ids.len(),
            rows = rows_seen,
            kb = kb_id,
            first = chunk_ids[0].as_str(),
            "wiki_refine: doc_id resolution returned 0"
        );
    }
    out
}

#[cfg(test)]
mod wiki_part9_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;

    fn seed_chunks(store: &MemoryDocStore) {
        let rows: Vec<DocRow> = vec![
            json!({"id": "c1", "doc_id": "d1", "content_with_weight": "hello"})
                .as_object()
                .cloned()
                .unwrap(),
            json!({"id": "c2", "doc_id": ["d1", "d2"], "content_with_weight": "world"})
                .as_object()
                .cloned()
                .unwrap(),
        ];
        store.insert(&rows, &index_name("t1"), "kb1").unwrap();
    }

    #[test]
    fn load_chunks_dedups_and_skips_missing() {
        let store = MemoryDocStore::new();
        seed_chunks(&store);
        let ids = vec![
            "c1".to_string(),
            "c1".to_string(),
            "c2".to_string(),
            "ghost".to_string(),
        ];
        let loaded = load_chunks_by_id(&store, "t1", "kb1", &ids);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded["c1"], json!("hello"));
        assert_eq!(loaded["c2"], json!("world"));
        assert!(load_chunks_by_id(&store, "t1", "kb1", &[]).is_empty());
    }

    #[test]
    fn source_context_labels_and_truncates() {
        let store = MemoryDocStore::new();
        seed_chunks(&store);
        let evidence = vec![json!({"subject": "Alpha", "chunk_ids": ["c1", "c2"]})];
        let context = build_source_context(
            &store,
            "t1",
            "kb1",
            &evidence,
            WIKI_REFINE_SOURCE_BUDGET_CHARS,
        );
        assert_eq!(context, "[CHUNK c1]\nhello\n\n[CHUNK c2]\nworld");
        let tight = build_source_context(&store, "t1", "kb1", &evidence, 25);
        assert!(tight.starts_with("[CHUNK c1]\nhello"));
        assert!(tight.contains("omitted to fit context budget"));
        assert_eq!(
            build_source_context(&store, "t1", "kb1", &[], 100),
            "(no source chunks available)"
        );
        let missing = vec![json!({"subject": "Alpha", "chunk_ids": ["ghost"]})];
        assert_eq!(
            build_source_context(&store, "t1", "kb1", &missing, 100),
            "(source chunks could not be loaded)"
        );
    }

    #[test]
    fn transform_links_rewrites_and_filters() {
        let valid = BTreeSet::from(["entity/alpha".to_string(), "concept/beta".to_string()]);
        let mut titles = Map::new();
        titles.insert(
            "entity/alpha".to_string(),
            Value::String("Alpha Page".to_string()),
        );
        let md = "See [[entity/alpha]] and [[concept/beta|the beta item]] and [[entity/ghost]]. Also [x](artifact/kb1/entity/alpha) and [y](artifact/kb1/entity/ghost) and [ext](https://example.com/z).";
        let (rendered, outlinks) = transform_links(md, "kb1", Some(&titles), Some(&valid));
        assert!(rendered.contains("[Alpha Page](artifact/kb1/entity/alpha)"));
        assert!(rendered.contains("[the beta item](artifact/kb1/concept/beta)"));
        assert!(rendered.contains("Ghost"));
        assert!(!rendered.contains("[["));
        assert!(rendered.contains("[x](artifact/kb1/entity/alpha)"));
        assert!(rendered.contains("and y and"));
        assert!(!rendered.contains("artifact/kb1/entity/ghost"));
        assert!(rendered.contains("[ext](https://example.com/z)"));
        assert_eq!(
            outlinks,
            vec!["entity/alpha".to_string(), "concept/beta".to_string()]
        );
        let (plain, plain_out) = transform_links("[[entity/alpha]]", "kb1", None, None);
        assert_eq!(plain, "[Alpha](artifact/kb1/entity/alpha)");
        assert_eq!(plain_out, vec!["entity/alpha".to_string()]);
        let (repeat, repeat_out) =
            transform_links("[[entity/alpha]] [[entity/alpha]]", "kb1", None, None);
        assert_eq!(repeat.matches("artifact/kb1/entity/alpha").count(), 2);
        assert_eq!(repeat_out.len(), 1);
    }

    #[test]
    fn collect_doc_ids_dedups_and_tolerates_shapes() {
        let store = MemoryDocStore::new();
        seed_chunks(&store);
        let ids = vec!["c1".to_string(), "c2".to_string(), "ghost".to_string()];
        assert_eq!(
            wiki_collect_doc_ids(&store, "t1", "kb1", &ids),
            vec!["d1".to_string(), "d2".to_string()]
        );
        assert!(wiki_collect_doc_ids(&store, "t1", "kb1", &[]).is_empty());
    }
}

// ---------------------------------------------------------------------------
// Part 10 — page I/O and draft persistence (`_wiki_get_existing_page` ..
// `_wiki_load_refine_resume`).
//
// Adaptation note: searchable draft rows use `tokenize_for_search`
// space-joined token strings (upstream rag_tokenizer) and store the embedding
// under both `embedding` and `q_<dim>_vec` so local backends can score them.
// ---------------------------------------------------------------------------

/// `_wiki_get_existing_page`.
pub fn wiki_get_existing_page(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    slug: &str,
) -> Option<Value> {
    let fields: Vec<String> = ["id", "content_with_weight", "title_kwd", "page_type_kwd"]
        .iter()
        .map(|field| field.to_string())
        .collect();
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_PAGE_COMPILE_KWD.to_string()),
    );
    condition.insert("slug_kwd".to_string(), Value::String(slug.to_string()));
    let rows = match search_page(store, tenant_id, kb_id, &fields, &condition, 0, 1) {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!(error = %err, slug = slug, "wiki_refine: failed to fetch existing page");
            return None;
        }
    };
    let row = rows.into_iter().next()?;
    let rendered = row
        .get("content_with_weight")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let row_id = row
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let title = row
        .get("title_kwd")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let page_type = row
        .get("page_type_kwd")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .unwrap_or("concept")
        .to_string();
    Some(json!({
        "id": row_id,
        "content_md": rendered,
        "content_md_raw": rendered,
        "title": title,
        "page_type": page_type,
    }))
}

/// `_wiki_chat_text`: single chat call returning raw (think-stripped) text.
pub async fn wiki_chat_text(
    chat: &dyn HarnessChat,
    system_prompt: &str,
    user_prompt: &str,
    temperature: f64,
    llm_timeout: i64,
) -> String {
    let messages = form_message(system_prompt, user_prompt);
    let (_, messages) = message_fit_in(messages, chat.max_length());
    let gen_conf = knowledge_compile_gen_conf(
        &chat.model_name(),
        Some(&Map::from_iter([(
            "temperature".to_string(),
            json!(temperature),
        )])),
    );
    let request_conf = Value::Object(gen_conf);
    let system = messages
        .first()
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or(system_prompt);
    let history: Vec<Value> = messages.iter().skip(1).cloned().collect();
    let timeout = std::time::Duration::from_secs(llm_timeout.max(0) as u64);
    let outcome = tokio::time::timeout(timeout, chat.chat(system, &history, &request_conf)).await;
    let raw = match outcome {
        Err(_elapsed) => {
            tracing::warn!(seconds = llm_timeout, "wiki_refine: chat call timed out");
            return String::new();
        }
        Ok(Err(err)) => {
            tracing::warn!(error = %err, "wiki_refine: chat call failed");
            return String::new();
        }
        Ok(Ok(raw)) => raw,
    };
    strip_think(&raw)
}

/// `_wiki_write_page_simple`: single writer LLM call → markdown content.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_write_page_simple(
    chat: &dyn HarnessChat,
    plan_item: &Value,
    evidence: &[Value],
    existing_md: Option<&str>,
    source_context: &str,
    all_plan_slugs: &[String],
    llm_timeout: i64,
    instruction: Option<&str>,
    example: Option<&str>,
) -> String {
    let own_slug = plan_item
        .get("slug")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let available: Vec<&String> = all_plan_slugs
        .iter()
        .filter(|slug| !slug.is_empty() && **slug != own_slug)
        .collect();
    let slugs_block = if available.is_empty() {
        "(none — this is the only page)".to_string()
    } else {
        available
            .iter()
            .map(|slug| format!("- [[{slug}]]"))
            .collect::<Vec<String>>()
            .join("\n")
    };
    let existing_section = match existing_md {
        Some(existing) if !existing.is_empty() => format!(
            "## Existing page content (UPDATE — integrate new evidence into this)\n\n{existing}\n"
        ),
        _ => String::new(),
    };

    let action = plan_item
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("CREATE");
    let title = plan_item
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or(own_slug.as_str());
    let page_type = plan_item
        .get("page_type")
        .and_then(Value::as_str)
        .unwrap_or("concept");
    let evidence_count = evidence.len().to_string();
    let evidence_blocks = format_evidence_blocks(evidence);
    let user_prompt = render_template(
        WIKI_REFINE_WRITER_USER_TEMPLATE,
        &[
            ("action", action),
            ("slug", own_slug.as_str()),
            ("title", title),
            ("page_type", page_type),
            ("all_plan_slugs", slugs_block.as_str()),
            ("existing_section", existing_section.as_str()),
            ("source_context", source_context),
            ("evidence_count", evidence_count.as_str()),
            ("evidence_blocks", evidence_blocks.as_str()),
        ],
    );
    let system = build_refine_writer_system(instruction, example);
    wiki_chat_text(chat, &system, &user_prompt, 0.15, llm_timeout).await
}

/// `_wiki_merge_page_content`: LLM-merge with shrink-check fallback.
pub async fn wiki_merge_page_content(
    chat: &dyn HarnessChat,
    existing_md: &str,
    new_md: &str,
    slug: &str,
    shrink_threshold: f64,
    llm_timeout: i64,
) -> String {
    if existing_md.is_empty() || existing_md.trim().chars().count() < 50 {
        return new_md.to_string();
    }
    if existing_md.trim() == new_md.trim() {
        return new_md.to_string();
    }
    if new_md.is_empty() {
        return existing_md.to_string();
    }
    let user_prompt = format!(
        "Merge these two versions of wiki page `{slug}`:\n\n## EXISTING VERSION\n\n{existing_md}\n\n---\n\n## INCOMING VERSION\n\n{new_md}\n\n---\n\nProduce the merged page now. Return ONLY the markdown content."
    );
    let merged = wiki_chat_text(
        chat,
        WIKI_REFINE_MERGE_SYSTEM,
        &user_prompt,
        0.1,
        llm_timeout,
    )
    .await;
    if merged.is_empty() {
        return new_md.to_string();
    }
    let max_input_len = existing_md.chars().count().max(new_md.chars().count());
    let min_acceptable = (max_input_len as f64 * shrink_threshold) as usize;
    if merged.chars().count() < min_acceptable {
        tracing::warn!(
            slug = slug,
            merged = merged.chars().count(),
            threshold = min_acceptable,
            max_input = max_input_len,
            "wiki_refine: merge rejected (shrink); falling back to new content"
        );
        return new_md.to_string();
    }
    merged
}

/// `_wiki_extract_summary`: first non-heading paragraph, char-capped.
pub fn wiki_extract_summary(content_md: &str, max_chars: usize) -> String {
    if content_md.trim().is_empty() {
        return String::new();
    }
    let mut buf: Vec<String> = Vec::new();
    for line in content_md.lines() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            if !buf.is_empty() {
                break;
            }
            continue;
        }
        buf.push(stripped.to_string());
        if buf.join(" ").chars().count() >= max_chars {
            break;
        }
    }
    truncate_chars(&buf.join(" "), max_chars)
}

/// `_wiki_draft_row_id`.
pub fn wiki_draft_row_id(kb_id: &str, slug: &str) -> String {
    stable_row_id(&[
        WIKI_DRAFT_COMPILE_KWD.to_string(),
        kb_id.to_string(),
        slug.to_string(),
    ])
}

/// `_wiki_persist_draft`: upsert one resume-cache / searchable page row.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_persist_draft(
    store: &dyn DocStore,
    embd: Option<&dyn Embedder>,
    tenant_id: &str,
    kb_id: &str,
    page: &Value,
    plan_input_hash: &str,
) {
    let Some(page_obj) = page.as_object() else {
        return;
    };
    let slug = page_obj.get("slug").and_then(Value::as_str).unwrap_or("");
    if slug.is_empty() {
        return;
    }
    let mut row = Map::new();
    row.insert(
        "id".to_string(),
        Value::String(wiki_draft_row_id(kb_id, slug)),
    );
    row.insert("doc_id".to_string(), Value::String(kb_id.to_string()));
    row.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_DRAFT_COMPILE_KWD.to_string()),
    );
    row.insert("wiki_slug_kwd".to_string(), Value::String(slug.to_string()));
    row.insert(
        "source_id".to_string(),
        Value::Array(vec![Value::String(kb_id.to_string())]),
    );
    let draft_doc_ids: Vec<Value> = page_obj
        .get("source_doc_ids")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| item.as_str().map(|text| !text.is_empty()).unwrap_or(false))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    row.insert("source_doc_ids".to_string(), Value::Array(draft_doc_ids));
    row.insert(
        "input_hash_kwd".to_string(),
        Value::String(plan_input_hash.to_string()),
    );
    row.insert(
        "content_with_weight".to_string(),
        Value::String(page.to_string()),
    );
    row.insert("available_int".to_string(), json!(0));

    if let Some(embd) = embd {
        let title = page_obj
            .get("title")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .unwrap_or(slug)
            .to_string();
        let body = ["content_md_rendered", "content_md", "content_md_raw"]
            .iter()
            .find_map(|key| page_obj.get(*key).and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        let summary = page_obj
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let (body_ltks, body_sm_ltks) = tokenize_for_search(&body);
        let (title_tks, _) = tokenize_for_search(&title);
        row.insert("docnm_kwd".to_string(), Value::String(title.clone()));
        row.insert("title_kwd".to_string(), Value::String(title.clone()));
        row.insert("title_tks".to_string(), Value::String(title_tks.join(" ")));
        row.insert(
            "content_ltks".to_string(),
            Value::String(body_ltks.join(" ")),
        );
        row.insert(
            "content_sm_ltks".to_string(),
            Value::String(body_sm_ltks.join(" ")),
        );
        let emb_text = if summary.is_empty() {
            format!("{title}\n{body}")
        } else {
            summary.clone()
        };
        let emb_text = truncate_chars(emb_text.trim(), 2048);
        let emb_text = if emb_text.is_empty() {
            title.clone()
        } else {
            emb_text
        };
        match embd.embed(&[emb_text.as_str()]).await {
            Ok(vectors) => {
                if let Some(vector) = vectors.first().filter(|vector| !vector.is_empty()) {
                    row.insert(format!("q_{}_vec", vector.len()), json!(vector));
                    row.insert("embedding".to_string(), json!(vector));
                    row.insert("available_int".to_string(), json!(1));
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, slug = slug, "wiki_refine: draft embedding failed; row stays non-searchable");
            }
        }
    }

    let mut delete_condition = Map::new();
    delete_condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_DRAFT_COMPILE_KWD.to_string()),
    );
    delete_condition.insert("wiki_slug_kwd".to_string(), Value::String(slug.to_string()));
    if let Err(err) = store.delete(&delete_condition, &index_name(tenant_id), kb_id) {
        tracing::debug!(error = %err, "wiki_refine: prior draft delete failed; relying on id upsert");
    }
    if let Err(err) = store.insert(&[row], &index_name(tenant_id), kb_id) {
        tracing::error!(error = %err, slug = slug, "wiki_refine: failed to persist draft");
    }
}

/// `_wiki_load_refine_resume`: `{slug: (page, stored_plan_input_hash)}`.
pub fn wiki_load_refine_resume(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
) -> BTreeMap<String, (Value, String)> {
    let fields: Vec<String> = [
        "id",
        "wiki_slug_kwd",
        "content_with_weight",
        "input_hash_kwd",
    ]
    .iter()
    .map(|field| field.to_string())
    .collect();
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_DRAFT_COMPILE_KWD.to_string()),
    );
    let mut out: BTreeMap<String, (Value, String)> = BTreeMap::new();
    let mut offset = 0usize;
    loop {
        let rows = match search_page(store, tenant_id, kb_id, &fields, &condition, offset, 500) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::error!(error = %err, "wiki_refine: failed to page draft cache");
                break;
            }
        };
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            let slug = row
                .get("wiki_slug_kwd")
                .and_then(Value::as_str)
                .unwrap_or("");
            let content = row
                .get("content_with_weight")
                .and_then(Value::as_str)
                .unwrap_or("");
            if slug.is_empty() || content.is_empty() {
                continue;
            }
            let cached: Value = match serde_json::from_str(content) {
                Ok(cached) => cached,
                Err(_) => continue,
            };
            if !cached.is_object() {
                continue;
            }
            let stored_hash = row
                .get("input_hash_kwd")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            out.insert(slug.to_string(), (cached, stored_hash));
        }
        if rows.len() < 500 {
            break;
        }
        offset += 500;
    }
    out
}

#[cfg(test)]
mod wiki_part10_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;
    use crate::harness::HarnessChat;
    use std::sync::Mutex;

    struct FakeChat {
        reply: String,
        delay_ms: u64,
        calls: Mutex<Vec<String>>,
    }

    impl FakeChat {
        fn new(reply: &str) -> Self {
            Self {
                reply: reply.to_string(),
                delay_ms: 0,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl HarnessChat for FakeChat {
        async fn chat(
            &self,
            _system: &str,
            history: &[Value],
            _gen_conf: &Value,
        ) -> Result<String, String> {
            if self.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            }
            let user = history
                .last()
                .and_then(|message| message.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            self.calls.lock().unwrap().push(user);
            Ok(self.reply.clone())
        }

        fn max_length(&self) -> usize {
            100_000
        }
    }

    struct TestEmb;

    #[async_trait::async_trait]
    impl crate::embed::Embedder for TestEmb {
        async fn embed(&self, texts: &[&str]) -> crate::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }

    #[test]
    fn existing_page_fetch_and_defaults() {
        let store = MemoryDocStore::new();
        let row = json!({
            "id": "p1",
            "compile_kwd": "wiki_page",
            "slug_kwd": "entity/alpha",
            "title_kwd": "Alpha",
            "page_type_kwd": "entity",
            "content_with_weight": "# Alpha\n\nBody"
        });
        store
            .insert(
                &[row.as_object().cloned().unwrap()],
                &index_name("t1"),
                "kb1",
            )
            .unwrap();
        let page = wiki_get_existing_page(&store, "t1", "kb1", "entity/alpha").expect("page");
        assert_eq!(page["id"], json!("p1"));
        assert_eq!(page["content_md"], json!("# Alpha\n\nBody"));
        assert_eq!(page["content_md_raw"], page["content_md"]);
        assert_eq!(page["title"], json!("Alpha"));
        assert_eq!(page["page_type"], json!("entity"));
        assert!(wiki_get_existing_page(&store, "t1", "kb1", "entity/ghost").is_none());
    }

    #[tokio::test]
    async fn chat_text_strips_think_and_handles_timeout() {
        let chat = FakeChat::new("scratch</think>final answer");
        let text = wiki_chat_text(&chat, "sys", "user", 0.1, 30).await;
        assert_eq!(text, "final answer");
        let mut slow = FakeChat::new("late");
        slow.delay_ms = 50;
        assert_eq!(wiki_chat_text(&slow, "sys", "user", 0.1, 0).await, "");
    }

    #[tokio::test]
    async fn write_page_prompt_shapes() {
        let chat = FakeChat::new("content");
        let plan_item = json!({"slug": "entity/alpha", "title": "Alpha", "page_type": "entity", "action": "CREATE"});
        let evidence = vec![json!({"subject": "Alpha", "statement": "s", "chunk_ids": ["c1"]})];
        let slugs = vec!["entity/alpha".to_string(), "concept/beta".to_string()];
        let content = wiki_write_page_simple(
            &chat, &plan_item, &evidence, None, "SOURCE", &slugs, 30, None, None,
        )
        .await;
        assert_eq!(content, "content");
        let prompt = chat.calls.lock().unwrap()[0].clone();
        assert!(prompt.contains("- [[concept/beta]]"));
        assert!(!prompt.contains("- [[entity/alpha]]"));
        assert!(prompt.contains("CREATE the following wiki page"));
        assert!(prompt.contains("Evidence checklist (1 items)"));
        assert!(prompt.contains("[EXPLICIT] Alpha"));
        assert!(!prompt.contains("Existing page content"));

        let chat2 = FakeChat::new("content2");
        wiki_write_page_simple(
            &chat2,
            &plan_item,
            &[],
            Some("OLD BODY"),
            "SOURCE",
            &[],
            30,
            Some("Be terse."),
            None,
        )
        .await;
        let prompt2 = chat2.calls.lock().unwrap()[0].clone();
        assert!(prompt2.contains("Existing page content (UPDATE"));
        assert!(prompt2.contains("OLD BODY"));
        assert!(prompt2.contains("(none — this is the only page)"));
    }

    #[tokio::test]
    async fn merge_short_circuits_and_shrink_guard() {
        let chat = FakeChat::new("m");
        assert_eq!(
            wiki_merge_page_content(&chat, "short", "new body", "s", 0.7, 30).await,
            "new body"
        );
        assert_eq!(
            wiki_merge_page_content(&chat, "", "new body", "s", 0.7, 30).await,
            "new body"
        );
        let long_existing = "x".repeat(200);
        let same = long_existing.clone();
        assert_eq!(
            wiki_merge_page_content(&chat, &long_existing, &same, "s", 0.7, 30).await,
            same
        );
        assert_eq!(
            wiki_merge_page_content(&chat, &long_existing, "", "s", 0.7, 30).await,
            long_existing
        );
        let long_new = "y".repeat(200);
        let merged = wiki_merge_page_content(&chat, &long_existing, &long_new, "s", 0.7, 30).await;
        assert_eq!(merged, long_new);
        let healthy = FakeChat::new(&"z".repeat(300));
        let good = wiki_merge_page_content(&healthy, &long_existing, &long_new, "s", 0.7, 30).await;
        assert_eq!(good.chars().count(), 300);
    }

    #[test]
    fn summary_extraction_shapes() {
        assert_eq!(wiki_extract_summary("", 300), "");
        assert_eq!(
            wiki_extract_summary(
                "# Heading\n\nFirst paragraph.\nSecond line.\n\nNext para",
                300
            ),
            "First paragraph. Second line."
        );
        assert_eq!(wiki_extract_summary("abcdefghij", 5), "abcde");
        assert_eq!(wiki_extract_summary("## Only headings\n### More", 300), "");
    }

    #[tokio::test]
    async fn draft_persist_and_resume_roundtrip() {
        let store = MemoryDocStore::new();
        let page = json!({
            "slug": "entity/alpha",
            "title": "Alpha",
            "content_md": "# Alpha\n\nBody text.",
            "source_doc_ids": ["d1", "d2"]
        });
        wiki_persist_draft(&store, None, "t1", "kb1", &page, "H1").await;
        let resume = wiki_load_refine_resume(&store, "t1", "kb1");
        assert_eq!(resume.len(), 1);
        let (cached, hash) = resume.get("entity/alpha").expect("draft");
        assert_eq!(hash, "H1");
        assert_eq!(cached["title"], json!("Alpha"));

        let emb = TestEmb;
        wiki_persist_draft(&store, Some(&emb), "t1", "kb1", &page, "H2").await;
        let resume2 = wiki_load_refine_resume(&store, "t1", "kb1");
        assert_eq!(resume2.len(), 1);
        assert_eq!(resume2.get("entity/alpha").expect("draft2").1, "H2");
        let mut condition = Map::new();
        condition.insert(
            "compile_kwd".to_string(),
            Value::String(WIKI_DRAFT_COMPILE_KWD.to_string()),
        );
        let query = SearchQuery {
            select_fields: vec!["id".to_string()],
            condition,
            match_expressions: Vec::new(),
            offset: 0,
            limit: 10,
            index_names: vec![index_name("t1")],
            dataset_ids: vec!["kb1".to_string()],
            ..Default::default()
        };
        let response = store.search(&query).unwrap();
        assert_eq!(response.total, 1);
        let stored = response.docs.into_iter().next().unwrap();
        assert_eq!(stored.get("available_int"), Some(&json!(1)));
        assert!(stored.get("q_2_vec").is_some());
        assert!(stored.get("embedding").is_some());
        assert!(stored.get("title_tks").is_some());
    }
}

// ---------------------------------------------------------------------------
// Part 11 — REFINE main entry (`wiki_refine_from_plan`).
//
// Adaptation note: writers run sequentially ('static closure bounds); a
// writer failure logs and skips that page instead of failing the phase;
// the strong-typed signature makes `_ensure_llm_bundle` unnecessary.
// ---------------------------------------------------------------------------

/// `wiki_refine_from_plan`: KB-scoped REFINE phase.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_refine_from_plan(
    store: &dyn DocStore,
    chat: &dyn HarnessChat,
    embd: Option<&dyn Embedder>,
    tenant_id: &str,
    kb_id: &str,
    max_workers: usize,
    llm_timeout: i64,
    source_budget_chars: usize,
    merge_shrink_threshold: f64,
    force_rerun: bool,
    callback: Option<&(dyn Fn(f64, &str) + Send + Sync)>,
    instruction: Option<&str>,
    example: Option<&str>,
) -> Vec<Value> {
    let _ = max_workers; // sequential writes; kept for signature parity
    if let Some(callback) = callback {
        callback(0.02, "wiki REFINE: loading plan");
    }

    let Some((plan, plan_input_hash)) = load_plan_resume(store, tenant_id, kb_id) else {
        tracing::warn!(kb = kb_id, "wiki_refine: no wiki_compilation_plan found");
        return Vec::new();
    };
    if !plan.is_object() {
        tracing::warn!(kb = kb_id, "wiki_refine: cached plan is not an object");
        return Vec::new();
    }
    let pages_raw: Vec<Value> = plan
        .get("pages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if pages_raw.is_empty() {
        tracing::info!(kb = kb_id, "wiki_refine: plan has no pages");
        return Vec::new();
    }

    let priority_of =
        |page: &Value| -> f64 { page.get("priority").and_then(Value::as_f64).unwrap_or(99.0) };
    let mut sorted_spec: Vec<Value> = pages_raw
        .into_iter()
        .filter(|page| {
            page.is_object()
                && page
                    .get("slug")
                    .and_then(Value::as_str)
                    .map(|slug| !slug.is_empty())
                    .unwrap_or(false)
        })
        .collect();
    sorted_spec.sort_by(|a, b| {
        priority_of(a)
            .partial_cmp(&priority_of(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut pages_spec: Vec<Value> = Vec::new();
    let mut seen_slugs: BTreeSet<String> = BTreeSet::new();
    let mut duplicates_dropped = 0usize;
    for page in sorted_spec {
        let slug = page
            .get("slug")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if slug.is_empty() {
            continue;
        }
        if !seen_slugs.insert(slug.clone()) {
            duplicates_dropped += 1;
            continue;
        }
        pages_spec.push(page);
    }
    if duplicates_dropped > 0 {
        tracing::info!(
            dropped = duplicates_dropped,
            kb = kb_id,
            "wiki_refine: dropped duplicate slug entries from plan"
        );
    }

    let all_claims: Vec<Value> = plan
        .get("_claims")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let all_plan_slugs: Vec<String> = pages_spec
        .iter()
        .filter_map(|page| page.get("slug").and_then(Value::as_str).map(str::to_string))
        .collect();
    let mut page_titles: Map<String, Value> = Map::new();
    for page in &pages_spec {
        if let Some(slug) = page.get("slug").and_then(Value::as_str) {
            if let Some(title) = page.get("title").and_then(Value::as_str) {
                let title = title.trim();
                if !title.is_empty() {
                    page_titles.insert(slug.to_string(), Value::String(title.to_string()));
                }
            }
        }
    }

    let mut entity_by_name: Map<String, Value> = Map::new();
    if let Some(entities) = plan.get("_entities").and_then(Value::as_array) {
        for entity in entities {
            if !entity.is_object() {
                continue;
            }
            if let Some(canon) = entity.get("name").and_then(Value::as_str) {
                let canon = canon.trim();
                if !canon.is_empty() {
                    entity_by_name
                        .entry(canon.to_lowercase())
                        .or_insert_with(|| entity.clone());
                }
            }
            if let Some(aliases) = entity.get("aliases").and_then(Value::as_array) {
                for alias in aliases {
                    if let Some(alias) = alias.as_str() {
                        let alias = alias.trim();
                        if !alias.is_empty() {
                            entity_by_name
                                .entry(alias.to_lowercase())
                                .or_insert_with(|| entity.clone());
                        }
                    }
                }
            }
        }
    }
    let mut concept_by_term: Map<String, Value> = Map::new();
    if let Some(concepts) = plan.get("_concepts").and_then(Value::as_array) {
        for concept in concepts {
            if !concept.is_object() {
                continue;
            }
            if let Some(term) = concept.get("term").and_then(Value::as_str) {
                let term = term.trim();
                if !term.is_empty() {
                    concept_by_term
                        .entry(term.to_lowercase())
                        .or_insert_with(|| concept.clone());
                }
            }
            if let Some(aliases) = concept.get("aliases").and_then(Value::as_array) {
                for alias in aliases {
                    if let Some(alias) = alias.as_str() {
                        let alias = alias.trim();
                        if !alias.is_empty() {
                            concept_by_term
                                .entry(alias.to_lowercase())
                                .or_insert_with(|| concept.clone());
                        }
                    }
                }
            }
        }
    }

    let mut cached: BTreeMap<String, Value> = BTreeMap::new();
    if !force_rerun {
        let all_drafts = wiki_load_refine_resume(store, tenant_id, kb_id);
        let mut stale_drafts = 0usize;
        for (slug, (page, stored_hash)) in all_drafts {
            if !plan_input_hash.is_empty()
                && !stored_hash.is_empty()
                && stored_hash == plan_input_hash
            {
                cached.insert(slug, page);
            } else {
                stale_drafts += 1;
            }
        }
        if !cached.is_empty() || stale_drafts > 0 {
            tracing::info!(
                fresh = cached.len(),
                stale = stale_drafts,
                kb = kb_id,
                "wiki_refine: resume drafts"
            );
        }
    }

    let pending: Vec<Value> = pages_spec
        .iter()
        .filter(|page| {
            let slug = page.get("slug").and_then(Value::as_str).unwrap_or("");
            !cached.contains_key(slug)
        })
        .cloned()
        .collect();
    let total = pending.len().max(1);
    if let Some(callback) = callback {
        callback(
            0.1,
            &format!(
                "wiki REFINE: writing {} page(s) (cached={})",
                pending.len(),
                cached.len()
            ),
        );
    }
    let progress_updates = 20usize;
    let report_every = 1.max((total + progress_updates - 1) / progress_updates);

    let valid_slugs: BTreeSet<String> = all_plan_slugs.iter().cloned().collect();
    let mut completed = 0usize;
    let mut new_pages: Vec<Option<Value>> = Vec::new();
    for plan_item in &pending {
        let slug = plan_item
            .get("slug")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let page = write_refine_page(
            store,
            chat,
            embd,
            tenant_id,
            kb_id,
            plan_item,
            &all_claims,
            &all_plan_slugs,
            &valid_slugs,
            &page_titles,
            &entity_by_name,
            &concept_by_term,
            source_budget_chars,
            merge_shrink_threshold,
            llm_timeout,
            instruction,
            example,
            &plan_input_hash,
        )
        .await;
        if page.is_some() {
            completed += 1;
            if let Some(callback) = callback {
                if completed % report_every == 0 || completed == total {
                    let progress = 0.1 + 0.85 * (completed as f64 / total as f64);
                    callback(
                        progress,
                        &format!("wiki REFINE: {completed}/{total} pages completed"),
                    );
                }
            }
        }
        if page.is_none() {
            tracing::warn!(slug = slug.as_str(), "wiki_refine: writer failed for slug");
        }
        new_pages.push(page);
    }

    let mut results: Vec<Value> = Vec::new();
    for spec in &pages_spec {
        let Some(slug) = spec.get("slug").and_then(Value::as_str) else {
            continue;
        };
        if let Some(page) = cached.get(slug) {
            results.push(page.clone());
        } else {
            for candidate in &new_pages {
                if let Some(page) = candidate {
                    if page.get("slug").and_then(Value::as_str) == Some(slug) {
                        results.push(page.clone());
                        break;
                    }
                }
            }
        }
    }

    // Re-render against the pages that actually survived this run so no
    // dangling artifact link reaches persistence.
    let actual_slugs: BTreeSet<String> = results
        .iter()
        .filter_map(|page| page.get("slug").and_then(Value::as_str).map(str::to_string))
        .collect();
    let mut actual_titles: Map<String, Value> = Map::new();
    for page in &results {
        if let Some(slug) = page.get("slug").and_then(Value::as_str) {
            if let Some(title) = page.get("title").and_then(Value::as_str) {
                let title = title.trim();
                if !title.is_empty() {
                    actual_titles.insert(slug.to_string(), Value::String(title.to_string()));
                }
            }
        }
    }
    for page in results.iter_mut() {
        let raw_content = ["content_md_raw", "content_md"]
            .iter()
            .find_map(|key| page.get(*key).and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        let (rendered, outlinks) = transform_links(
            &raw_content,
            kb_id,
            Some(&actual_titles),
            Some(&actual_slugs),
        );
        let summary = {
            let extracted = wiki_extract_summary(&rendered, 300);
            if extracted.is_empty() {
                page.get("title")
                    .or_else(|| page.get("slug"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            } else {
                extracted
            }
        };
        if let Some(obj) = page.as_object_mut() {
            obj.insert("content_md".to_string(), Value::String(rendered.clone()));
            obj.insert("content_md_rendered".to_string(), Value::String(rendered));
            obj.insert(
                "outlinks".to_string(),
                Value::Array(outlinks.into_iter().map(Value::String).collect()),
            );
            obj.insert("summary".to_string(), Value::String(summary));
        }
        let page_snapshot = page.clone();
        wiki_persist_draft(
            store,
            embd,
            tenant_id,
            kb_id,
            &page_snapshot,
            &plan_input_hash,
        )
        .await;
    }

    tracing::info!(
        kb = kb_id,
        written = results.len(),
        cached = cached.len(),
        new = new_pages.iter().filter(|page| page.is_some()).count(),
        "wiki_refine: done"
    );
    if let Some(callback) = callback {
        callback(1.0, "wiki REFINE: done");
    }
    results
}

#[allow(clippy::too_many_arguments)]
async fn write_refine_page(
    store: &dyn DocStore,
    chat: &dyn HarnessChat,
    embd: Option<&dyn Embedder>,
    tenant_id: &str,
    kb_id: &str,
    plan_item: &Value,
    all_claims: &[Value],
    all_plan_slugs: &[String],
    valid_slugs: &BTreeSet<String>,
    page_titles: &Map<String, Value>,
    entity_by_name: &Map<String, Value>,
    concept_by_term: &Map<String, Value>,
    source_budget_chars: usize,
    merge_shrink_threshold: f64,
    llm_timeout: i64,
    instruction: Option<&str>,
    example: Option<&str>,
    plan_input_hash: &str,
) -> Option<Value> {
    let slug = plan_item
        .get("slug")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let action = plan_item
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("CREATE")
        .to_uppercase();
    let title = plan_item
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or(slug.as_str())
        .to_string();
    let page_type = plan_item
        .get("page_type")
        .and_then(Value::as_str)
        .unwrap_or("concept")
        .to_string();

    let evidence = assemble_evidence(
        plan_item,
        all_claims,
        Some(entity_by_name),
        Some(concept_by_term),
    );
    let source_chunk_ids = collect_evidence_chunk_ids(&evidence);
    let source_context =
        build_source_context(store, tenant_id, kb_id, &evidence, source_budget_chars);

    let mut existing_md_raw: Option<String> = None;
    if action == "UPDATE" {
        if let Some(existing) = wiki_get_existing_page(store, tenant_id, kb_id, &slug) {
            existing_md_raw = ["content_md_raw", "content_md"]
                .iter()
                .find_map(|key| existing.get(*key).and_then(Value::as_str))
                .filter(|text| !text.is_empty())
                .map(str::to_string);
        }
    }

    let mut content_md_raw = wiki_write_page_simple(
        chat,
        plan_item,
        &evidence,
        existing_md_raw.as_deref(),
        &source_context,
        all_plan_slugs,
        llm_timeout,
        instruction,
        example,
    )
    .await;
    if content_md_raw.is_empty() {
        content_md_raw = format!("# {title}\n\n(Page generation produced no content.)");
    }
    if let Some(existing) = existing_md_raw.as_deref() {
        content_md_raw = wiki_merge_page_content(
            chat,
            existing,
            &content_md_raw,
            &slug,
            merge_shrink_threshold,
            llm_timeout,
        )
        .await;
    }

    let (content_md_rendered, outlinks) =
        transform_links(&content_md_raw, kb_id, Some(page_titles), Some(valid_slugs));
    let source_doc_ids = wiki_collect_doc_ids(store, tenant_id, kb_id, &source_chunk_ids);
    let summary = {
        let extracted = wiki_extract_summary(&content_md_rendered, 300);
        if extracted.is_empty() {
            title.clone()
        } else {
            extracted
        }
    };
    let topic = plan_item
        .get("topic")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(|text| text.trim().to_string())
        .unwrap_or_else(|| {
            if title.is_empty() {
                slug.clone()
            } else {
                title.clone()
            }
        });
    let entity_names = plan_item
        .get("entity_names")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let related_kb_pages = plan_item
        .get("related_kb_pages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let page = json!({
        "slug": slug,
        "title": title,
        "page_type": page_type,
        "topic": topic,
        "action": action,
        "content_md": content_md_rendered,
        "content_md_rendered": content_md_rendered,
        "content_md_raw": content_md_raw,
        "outlinks": outlinks,
        "summary": summary,
        "entity_names": entity_names,
        "related_kb_pages": related_kb_pages,
        "source_chunk_ids": source_chunk_ids,
        "source_doc_ids": source_doc_ids,
        "kb_id": kb_id,
    });
    wiki_persist_draft(store, embd, tenant_id, kb_id, &page, plan_input_hash).await;
    Some(page)
}

#[cfg(test)]
mod wiki_part11_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;
    use crate::harness::HarnessChat;
    use std::sync::Mutex;

    struct FakeChat {
        reply: String,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl HarnessChat for FakeChat {
        async fn chat(
            &self,
            _system: &str,
            history: &[Value],
            _gen_conf: &Value,
        ) -> Result<String, String> {
            let user = history
                .last()
                .and_then(|message| message.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            self.calls.lock().unwrap().push(user);
            Ok(self.reply.clone())
        }

        fn max_length(&self) -> usize {
            100_000
        }
    }

    fn seed_plan(store: &MemoryDocStore) {
        persist_plan(
            store,
            "t1",
            "kb1",
            &json!({
                "pages": [
                    {"action": "CREATE", "slug": "entity/alpha", "title": "Alpha", "page_type": "entity", "topic": "alpha", "entity_names": ["Alpha"], "priority": 1},
                    {"action": "CREATE", "slug": "concept/beta", "title": "Beta", "page_type": "concept", "topic": "beta", "entity_names": ["Beta"], "priority": 2}
                ],
                "_claims": [{"statement": "s", "subject": "Alpha", "chunk_ids": ["c1"]}],
                "_entities": [{"name": "Alpha", "chunk_ids": ["c1"]}],
                "_concepts": [{"term": "Beta", "chunk_ids": ["c2"], "definition_excerpt": "d"}]
            }),
            "PH",
            &[],
        );
        let rows: Vec<DocRow> = vec![
            json!({"id": "c1", "doc_id": "d1", "content_with_weight": "alpha source text"})
                .as_object()
                .cloned()
                .unwrap(),
        ];
        store.insert(&rows, &index_name("t1"), "kb1").unwrap();
    }

    #[tokio::test]
    async fn refine_without_plan_returns_empty() {
        let store = MemoryDocStore::new();
        let chat = FakeChat {
            reply: "body".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let pages = wiki_refine_from_plan(
            &store,
            &chat,
            None,
            "t1",
            "kb1",
            2,
            30,
            WIKI_REFINE_SOURCE_BUDGET_CHARS,
            0.7,
            false,
            None,
            None,
            None,
        )
        .await;
        assert!(pages.is_empty());
        assert!(chat.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn refine_writes_pages_and_caches() {
        let store = MemoryDocStore::new();
        seed_plan(&store);
        let chat = FakeChat {
            reply: "# Page\n\nSee [[concept/beta]] for more.".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let pages = wiki_refine_from_plan(
            &store,
            &chat,
            None,
            "t1",
            "kb1",
            2,
            30,
            WIKI_REFINE_SOURCE_BUDGET_CHARS,
            0.7,
            false,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0]["slug"], json!("entity/alpha"));
        assert_eq!(pages[1]["slug"], json!("concept/beta"));
        let alpha_md = pages[0]["content_md"].as_str().unwrap();
        assert!(alpha_md.contains("artifact/kb1/concept/beta"));
        assert!(!alpha_md.contains("[["));
        assert!(
            pages[0]["summary"]
                .as_str()
                .unwrap()
                .contains("See [Beta](artifact/kb1/concept/beta)")
        );
        assert_eq!(pages[0]["action"], json!("CREATE"));
        let drafts = wiki_load_refine_resume(&store, "t1", "kb1");
        assert_eq!(drafts.len(), 2);
        let calls_after_first = chat.calls.lock().unwrap().len();
        assert_eq!(calls_after_first, 2);

        let pages2 = wiki_refine_from_plan(
            &store,
            &chat,
            None,
            "t1",
            "kb1",
            2,
            30,
            WIKI_REFINE_SOURCE_BUDGET_CHARS,
            0.7,
            false,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(pages2.len(), 2);
        assert_eq!(chat.calls.lock().unwrap().len(), calls_after_first);
        assert_eq!(pages2[0]["slug"], json!("entity/alpha"));

        let pages3 = wiki_refine_from_plan(
            &store,
            &chat,
            None,
            "t1",
            "kb1",
            2,
            30,
            WIKI_REFINE_SOURCE_BUDGET_CHARS,
            0.7,
            true,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(pages3.len(), 2);
        assert_eq!(chat.calls.lock().unwrap().len(), calls_after_first + 2);
    }

    #[tokio::test]
    async fn refine_update_reads_existing_and_merges() {
        let store = MemoryDocStore::new();
        seed_plan(&store);
        let existing = "# Alpha\n\n".to_string() + &"old detail ".repeat(30);
        let row = json!({
            "id": "p1",
            "compile_kwd": "wiki_page",
            "slug_kwd": "entity/alpha",
            "title_kwd": "Alpha",
            "page_type_kwd": "entity",
            "content_with_weight": existing
        });
        store
            .insert(
                &[row.as_object().cloned().unwrap()],
                &index_name("t1"),
                "kb1",
            )
            .unwrap();
        let short_reply = "# Alpha\n\nnew body only".to_string();
        let chat = FakeChat {
            reply: short_reply.clone(),
            calls: Mutex::new(Vec::new()),
        };
        let plan_override = json!({
            "pages": [{"action": "UPDATE", "slug": "entity/alpha", "title": "Alpha", "page_type": "entity", "topic": "alpha", "entity_names": ["Alpha"], "priority": 1}],
            "_claims": [],
            "_entities": [{"name": "Alpha", "chunk_ids": ["c1"]}],
            "_concepts": []
        });
        persist_plan(&store, "t1", "kb1", &plan_override, "PH2", &[]);
        let pages = wiki_refine_from_plan(
            &store,
            &chat,
            None,
            "t1",
            "kb1",
            2,
            30,
            WIKI_REFINE_SOURCE_BUDGET_CHARS,
            0.7,
            false,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0]["action"], json!("UPDATE"));
        let raw = pages[0]["content_md_raw"].as_str().unwrap();
        assert!(raw.contains("new body only"));
        assert!(chat.calls.lock().unwrap().len() >= 2);
    }
}
