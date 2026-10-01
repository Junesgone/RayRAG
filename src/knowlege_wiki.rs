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
use crate::structure_compile::{cfg_get, env_int, localize};
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;

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
    previous: &HashMap<String, Value>,
    current: &HashMap<String, Value>,
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
) -> (Value, HashMap<String, Value>) {
    let mut per_chunk: HashMap<String, Value> = HashMap::new();
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
        let mut previous = HashMap::new();
        previous.insert("a".to_string(), json!({"hash": "1"}));
        previous.insert("b".to_string(), json!({"hash": "2"}));
        previous.insert("c".to_string(), json!({"hash": "3"}));
        let mut current = HashMap::new();
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
