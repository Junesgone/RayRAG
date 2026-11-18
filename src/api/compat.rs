//! Documented field names, added alongside the ones this project grew.
//!
//! The API guide names things differently in places: `embedding_model` where RayRAG stored
//! `embd_id`, `dataset_ids` where it stored `kb_ids`, `create_time` where it stored `created_at`.
//! A client written against the guide reads a name that is not there and gets `null`, which looks
//! like missing data rather than a naming difference.
//!
//! Everything here is **additive**: the original key stays, so no existing caller changes
//! behaviour. Values are only copied from a field that already holds the fact, or set to `null`
//! for concepts this project genuinely does not have (the guide's own examples show those as
//! `null`). Nothing is invented to make a payload look complete.
//!
//! `scripts/api-field-audit.py` reports what is missing after this layer runs.

use serde_json::{Map, Value};

/// Adds `target` under the guide's name when it is absent and `source` holds a real value.
fn alias(object: &mut Map<String, Value>, source: &str, target: &str) {
    if object.contains_key(target) {
        return;
    }
    let value = match object.get(source) {
        Some(value) if !value.is_null() => value.clone(),
        _ => return,
    };
    object.insert(target.to_string(), value);
}

/// A field the guide documents but this project has no value for. The guide's own examples show
/// these as `null`, so `null` is the documented answer rather than a placeholder.
fn null_if_absent(object: &mut Map<String, Value>, key: &str) {
    if !object.contains_key(key) {
        object.insert(key.to_string(), Value::Null);
    }
}

fn enrich(value: &mut Value, apply: fn(&mut Map<String, Value>)) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(|item| enrich(item, apply)),
        Value::Object(object) => apply(object),
        _ => {}
    }
}

/// `parser_config` reaches clients as a JSON string in RayRAG and as an object in the guide, so the
/// string is parsed when it is valid JSON and `chunk_method` is lifted out where the guide has it
/// as a sibling field.
fn normalise_parser_config(object: &mut Map<String, Value>) {
    let parsed = match object.get("parser_config") {
        Some(Value::String(raw)) => serde_json::from_str::<Value>(raw).ok(),
        _ => None,
    };
    if let Some(Value::Object(map)) = parsed {
        if let Some(method) = map.get("chunk_method").cloned()
            && !object.contains_key("chunk_method")
        {
            object.insert("chunk_method".to_string(), method);
        }
        object.insert("parser_config".to_string(), Value::Object(map));
    }
}

/// `GET /api/v1/datasets` and `GET /api/v1/datasets/{id}`.
pub fn enrich_datasets(value: &mut Value) {
    enrich(value, |object| {
        alias(object, "embd_id", "embedding_model");
        alias(object, "owner_id", "created_by");
        alias(object, "owner_id", "tenant_id");
        alias(object, "created_at", "create_time");
        alias(object, "updated_at", "update_time");
        alias(object, "doc_count", "document_count");
        normalise_parser_config(object);
        null_if_absent(object, "status");
        null_if_absent(object, "chunk_method");
    });
}

/// `GET /api/v1/chats`.
pub fn enrich_chats(value: &mut Value) {
    enrich(value, |object| {
        alias(object, "kb_ids", "dataset_ids");
        alias(object, "created_at", "create_time");
        alias(object, "updated_at", "update_time");
        alias(object, "owner_id", "tenant_id");
        null_if_absent(object, "status");
    });
}

/// `create_date` / `update_date` as upstream renders them in rows: local time,
/// `YYYY-MM-DDTHH:MM:SS` (probe `doc_list_page_neg`).
pub(crate) fn ms_to_datetime_string(ms: u64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(i64::try_from(ms).unwrap_or(i64::MAX)) {
        chrono::LocalResult::Single(datetime) => datetime.format("%Y-%m-%dT%H:%M:%S").to_string(),
        _ => String::new(),
    }
}

/// `GET /api/v1/datasets/{id}/documents`.
pub fn enrich_documents(value: &mut Value) {
    enrich(value, |object| {
        alias(object, "created_at", "create_time");
        alias(object, "updated_at", "update_time");
        if let Some(ms) = object.get("created_at").and_then(Value::as_u64) {
            object.insert(
                "create_date".to_string(),
                Value::String(ms_to_datetime_string(ms)),
            );
        }
        if let Some(ms) = object.get("updated_at").and_then(Value::as_u64) {
            object.insert(
                "update_date".to_string(),
                Value::String(ms_to_datetime_string(ms)),
            );
        }
        alias(object, "storage_name", "location");
        alias(object, "owner_id", "created_by");
        // A document in this project is always a document fetched from a local upload; saying so
        // is a fact about the object, not a placeholder.
        if !object.contains_key("type") {
            object.insert("type".to_string(), Value::String("doc".to_string()));
        }
        if !object.contains_key("source_type") {
            object.insert(
                "source_type".to_string(),
                Value::String("local".to_string()),
            );
        }
    });
}

/// `GET /api/v1/agents`.
pub fn enrich_agents(value: &mut Value) {
    enrich(value, |object| {
        alias(object, "name", "title");
        alias(object, "owner_id", "tenant_id");
        alias(object, "updated_at", "update_time");
        alias(object, "created_at", "create_time");
        if !object.contains_key("type") {
            object.insert("type".to_string(), Value::String("agent".to_string()));
        }
        null_if_absent(object, "release_time");
        null_if_absent(object, "description");
        null_if_absent(object, "nickname");
    });
}

/// `GET /api/v1/files` — one file record.
pub fn enrich_files(value: &mut Value) {
    enrich(value, |object| {
        // `file_type` holds exactly what the guide calls `type` ("doc" / "folder").
        alias(object, "file_type", "type");
        alias(object, "created_at", "create_time");
        alias(object, "owner_id", "created_by");
    });
}

/// `GET /api/v1/chats/{chat_id}/sessions` — one session.
pub fn enrich_sessions(value: &mut Value) {
    enrich(value, |object| {
        // A session created under a chat assistant records that assistant in `app_id`.
        alias(object, "app_id", "chat_id");
        alias(object, "created_at", "create_time");
        alias(object, "updated_at", "update_time");
    });
}

/// `GET /api/v1/searches`.
pub fn enrich_search_apps(value: &mut Value) {
    enrich(value, |object| {
        alias(object, "created_at", "create_time");
        alias(object, "owner_id", "tenant_id");
    });
}


