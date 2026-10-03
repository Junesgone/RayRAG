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

/// `GET /api/v1/datasets/{id}/documents`.
pub fn enrich_documents(value: &mut Value) {
    enrich(value, |object| {
        alias(object, "created_at", "create_time");
        alias(object, "updated_at", "update_time");
        alias(object, "created_at", "create_date");
        alias(object, "updated_at", "update_date");
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

/// `GET /api/v1/searches`.
pub fn enrich_search_apps(value: &mut Value) {
    enrich(value, |object| {
        alias(object, "created_at", "create_time");
        alias(object, "owner_id", "tenant_id");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn datasets_gain_the_documented_names_without_losing_the_old_ones() {
        let mut dataset = json!({
            "id": "kb-1",
            "embd_id": "BAAI/bge-large-zh-v1.5@BAAI",
            "owner_id": "user-1",
            "created_at": 1729763127646_u64,
            "updated_at": 1729763127647_u64,
            "doc_count": 2,
            "parser_config": "{\"chunk_method\":\"naive\",\"chunk_token_num\":512}",
        });
        enrich_datasets(&mut dataset);
        assert_eq!(
            dataset["embd_id"], "BAAI/bge-large-zh-v1.5@BAAI",
            "original kept"
        );
        assert_eq!(dataset["embedding_model"], "BAAI/bge-large-zh-v1.5@BAAI");
        assert_eq!(dataset["created_by"], "user-1");
        assert_eq!(dataset["tenant_id"], "user-1");
        assert_eq!(dataset["create_time"], 1729763127646_u64);
        assert_eq!(dataset["update_time"], 1729763127647_u64);
        assert_eq!(dataset["document_count"], 2);
        assert_eq!(dataset["chunk_method"], "naive", "lifted out of the config");
        assert!(
            dataset["parser_config"].is_object(),
            "the guide documents an object: {}",
            dataset["parser_config"]
        );
    }

    #[test]
    fn chats_gain_dataset_ids_and_timestamps() {
        let mut chat = json!({
            "id": "c-1",
            "kb_ids": ["kb-1", "kb-2"],
            "created_at": 1729232406637_u64,
            "updated_at": 1729232406638_u64,
        });
        enrich_chats(&mut chat);
        assert_eq!(chat["kb_ids"], json!(["kb-1", "kb-2"]));
        assert_eq!(chat["dataset_ids"], json!(["kb-1", "kb-2"]));
        assert_eq!(chat["create_time"], 1729232406637_u64);
        assert_eq!(chat["update_time"], 1729232406638_u64);
    }

    #[test]
    fn a_real_value_is_never_overwritten() {
        let mut dataset = json!({"embd_id": "old", "embedding_model": "explicit"});
        enrich_datasets(&mut dataset);
        assert_eq!(dataset["embedding_model"], "explicit");
    }

    #[test]
    fn missing_concepts_are_null_not_invented() {
        let mut agent = json!({"id": "a-1", "name": "Helper"});
        enrich_agents(&mut agent);
        assert_eq!(agent["title"], "Helper");
        assert_eq!(agent["type"], "agent");
        assert_eq!(
            agent["release_time"],
            Value::Null,
            "the guide shows null here"
        );
        assert!(agent.get("document_count").is_none(), "no invented numbers");
    }

    #[test]
    fn a_broken_parser_config_string_is_left_alone_rather_than_dropped() {
        let mut dataset = json!({"parser_config": "not json"});
        enrich_datasets(&mut dataset);
        assert_eq!(dataset["parser_config"], "not json");
    }

    #[test]
    fn lists_are_enriched_element_by_element() {
        let mut list = json!([{"name": "a"}, {"name": "b"}]);
        enrich_agents(&mut list);
        assert_eq!(list[0]["title"], "a");
        assert_eq!(list[1]["title"], "b");
    }
}
