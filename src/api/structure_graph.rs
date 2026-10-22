//! `GET /api/v1/datasets/{dataset_id}/documents/{document_id}/structure/graph`.
//!
//! Upstream builds this from knowledge-compilation rows: entities and relations tagged with the
//! compilation template that produced them, plus RAPTOR summary blobs, grouped into one graph per
//! template. RayRAG had all of that machinery — `structure_graph_common::{build_bucket, project_entity,
//! project_relation, dedup_entities, filter_entities_with_relations}` and
//! `structure_compile::rebuild_structure_graph_json` — and **no caller**: the endpoint had never been
//! written, so the compiled graphs had no way out of the store. This module is the missing caller.
//!
//! Bucket rules follow upstream `chunk_api.get_document_structure_graph`:
//!
//! * rows are grouped by the template that produced them (`compilation_template_ids`), and rows without
//!   one fall into a `legacy:<compile_kwd>` bucket rather than being dropped;
//! * templates configured on the dataset come first, in the configured order, and `wiki` templates are
//!   skipped because they render elsewhere;
//! * templates with neither entities nor relations are left out;
//! * RAPTOR summary rows (`compile_kwd = "raptor"`) contribute an extra `RAPTOR Summary` bucket.

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use crate::api::structure_graph_common::{
    GRAPH_ALL_FIELDS, build_bucket, dedup_entities, graph_search, normalize_relation_endpoints,
};
use crate::doc_store::{DocRow, OrderByExpr};
use crate::server::{AppState, AuthContext, api_error_code, code, kb_accessible};

/// How many rows the first pass reads to learn which buckets exist.
const BUCKET_SCAN_CAP: usize = 4096;
/// Rows read for the RAPTOR summary bucket, matching upstream's page of 16.
const RAPTOR_ROWS: usize = 16;

#[derive(Debug, serde::Deserialize, Default)]
pub struct GraphQuery {
    /// Upstream narrows the graph to candidates related to a keyword phrase.
    #[serde(default)]
    pub keywords: Option<String>,
}

/// What the blocking store task returns to the handler.
struct Outcome {
    buckets: Vec<(BucketMeta, Vec<Value>, Vec<Value>)>,
    scanned_rows: usize,
    total_matching: usize,
    total_entities: usize,
    total_relations: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BucketMeta {
    template_id: String,
    template_name: String,
    kind: String,
}

/// Upstream's `_compilation_template_kind`: the kind as it should be compared, lower case and without
/// decoration, so `Hypergraph` and `hypergraph` land in the same bucket.
fn normalise_kind(raw: &str) -> String {
    raw.trim().to_ascii_lowercase().replace([' ', '-'], "_")
}

/// The first non-empty template id on a row, as upstream's `_row_template_id` reads it.
fn row_template_id(row: &DocRow) -> Option<String> {
    match row.get("compilation_template_ids") {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .find(|value| !value.is_empty())
            .map(String::from),
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.trim().to_string()),
        _ => None,
    }
}

/// Which bucket a row belongs to, and the filter that selects every row of that bucket.
fn bucket_for_row(row: &DocRow) -> (BucketMeta, Map<String, Value>) {
    let compile_kwd = row
        .get("compile_kwd")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let kind_raw = row
        .get("compilation_template_kind_kwd")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(compile_kwd.as_str())
        .to_string();
    let mut scope = Map::new();
    if let Some(doc_id) = row.get("doc_id").cloned() {
        scope.insert("doc_id".to_string(), doc_id);
    }
    match row_template_id(row) {
        Some(template_id) => {
            scope.insert(
                "compilation_template_ids".to_string(),
                json!([template_id.clone()]),
            );
            (
                BucketMeta {
                    template_name: template_id.clone(),
                    template_id,
                    kind: kind_raw,
                },
                scope,
            )
        }
        // Upstream keeps pre-stamp rows under a synthetic bucket instead of dropping them.
        None => {
            scope.insert("compile_kwd".to_string(), json!(compile_kwd.clone()));
            (
                BucketMeta {
                    template_id: format!("legacy:{compile_kwd}"),
                    template_name: format!("Legacy ({compile_kwd})"),
                    kind: kind_raw,
                },
                scope,
            )
        }
    }
}

/// The templates configured on the dataset that produced this document, in the configured order.
fn configured_templates(
    state: &AppState,
    tenant_id: &str,
    parser_config: &Value,
) -> Vec<BucketMeta> {
    let mut ids: Vec<String> = match parser_config.get("compilation_template_group_id") {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect(),
        Some(Value::String(value)) if !value.trim().is_empty() => vec![value.trim().to_string()],
        _ => Vec::new(),
    };
    ids.dedup();
    let mut configured = Vec::new();
    let mut seen = HashSet::new();
    for group_id in ids {
        let Some(group) = state.compilation_templates.get(tenant_id, &group_id) else {
            continue;
        };
        for template in group.templates {
            if !seen.insert(template.id.clone()) {
                continue;
            }
            // Wiki templates render on their own surface, as upstream skips them here.
            if normalise_kind(&template.kind) == "wiki" {
                continue;
            }
            configured.push(BucketMeta {
                template_name: if template.name.is_empty() {
                    template.id.clone()
                } else {
                    template.name
                },
                template_id: template.id,
                kind: template.kind,
            });
        }
    }
    configured
}

/// Keep the entities whose name or text mentions the keywords, and the relations between survivors.
fn narrow_to_keywords(
    entities: &[Value],
    relations: &[Value],
    keywords: &str,
) -> (Vec<Value>, Vec<Value>) {
    let needles: Vec<String> = keywords
        .to_ascii_lowercase()
        .split_whitespace()
        .map(String::from)
        .collect();
    if needles.is_empty() {
        return (entities.to_vec(), relations.to_vec());
    }
    let matches = |value: &Value| -> bool {
        let text = format!(
            "{} {}",
            value.get("name").and_then(Value::as_str).unwrap_or(""),
            value
                .get("content_with_weight")
                .and_then(Value::as_str)
                .unwrap_or("")
        )
        .to_ascii_lowercase();
        needles.iter().any(|needle| text.contains(needle))
    };
    let kept: Vec<Value> = entities
        .iter()
        .filter(|entity| matches(entity))
        .cloned()
        .collect();
    let names: HashSet<String> = kept
        .iter()
        .filter_map(|entity| entity.get("name").and_then(Value::as_str))
        .map(String::from)
        .collect();
    let kept_relations: Vec<Value> = relations
        .iter()
        .filter(|relation| {
            let from = relation.get("from_entity_kwd").and_then(Value::as_str);
            let to = relation.get("to_entity_kwd").and_then(Value::as_str);
            match (from, to) {
                (Some(from), Some(to)) => names.contains(from) && names.contains(to),
                _ => false,
            }
        })
        .cloned()
        .collect();
    (kept, kept_relations)
}

/// `GET /api/v1/datasets/{dataset_id}/documents/{document_id}/structure/graph`.
pub async fn document_structure_graph(
    State(state): State<std::sync::Arc<AppState>>,
    Path((dataset_id, document_id)): Path<(String, String)>,
    Query(query): Query<GraphQuery>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Response {
    if !kb_accessible(&state, &dataset_id, &auth) {
        return api_error_code(
            StatusCode::OK,
            code::INVALID_OR_MISSING_DATA,
            &format!("You don't own the dataset {dataset_id}."),
        );
    }
    let Some(doc) = state
        .docs
        .get(&document_id)
        .filter(|doc| doc.kb_id == dataset_id)
    else {
        return api_error_code(
            StatusCode::OK,
            code::INVALID_OR_MISSING_DATA,
            &format!("you don't own the document {document_id}"),
        );
    };
    // The dataset owns the template configuration; the document only names its group.
    let dataset = state.kbs.get(&dataset_id);
    let owner = dataset
        .as_ref()
        .map(|dataset| dataset.owner_id.clone())
        .filter(|owner| !owner.is_empty())
        .unwrap_or_else(|| auth.user_id.clone());
    let parser_config: Value = dataset
        .as_ref()
        .and_then(|dataset| serde_json::from_str(&dataset.parser_config).ok())
        .unwrap_or(Value::Null);
    let configured = configured_templates(&state, &owner, &parser_config);

    // The synchronous Postgres client may not be used from a tokio worker at all: it starts its own
    // runtime per call and panics there ("Cannot start a runtime from within a runtime" — first on
    // connect, then on every query). So the whole store interaction runs on a blocking thread, with the
    // async helpers driven by that thread's handle.
    let index = crate::structure_compile::doc_store_index_name(&owner);
    let scanned = crate::api::dataset_navigation::req_store(&state, {
        let dataset_id = dataset_id.clone();
        let document_id = document_id.clone();
        let index = index.clone();
        move |store| -> anyhow::Result<Outcome> {
            {
                let meta_fields: Vec<&str> = {
                    let mut fields: Vec<&str> = GRAPH_ALL_FIELDS.to_vec();
                    fields.extend([
                        "compile_kwd",
                        "compilation_template_ids",
                        "compilation_template_kind_kwd",
                    ]);
                    fields
                };
                let mut scope = Map::new();
                scope.insert("doc_id".to_string(), json!(document_id.clone()));
                scope.insert(
                    "knowledge_graph_kwd".to_string(),
                    json!(["entity", "relation"]),
                );
                let (rows, total_matching) = graph_search(
                    store,
                    &index,
                    &dataset_id,
                    &meta_fields,
                    scope,
                    OrderByExpr::default(),
                    BUCKET_SCAN_CAP,
                    Vec::new(),
                    0,
                )
                .map_err(|error| anyhow::anyhow!(error))?;

                // Group the rows into buckets first, then project each bucket once. The bucket scope is
                // the list of row ids in it: a scope of "doc + compile_kwd" would also match the rows that
                // carry a template id (they keep the same compile key), and each of those would then be
                // counted twice.
                let mut metas: Vec<BucketMeta> = Vec::new();
                let mut bucket_rows: HashMap<String, Vec<String>> = HashMap::new();
                for row in &rows {
                    let (meta, _) = bucket_for_row(row);
                    let ids = bucket_rows.entry(meta.template_id.clone()).or_default();
                    if let Some(id) = row.get("id").and_then(Value::as_str) {
                        ids.push(id.to_string());
                    }
                    if !metas.iter().any(|known| known.template_id == meta.template_id) {
                        metas.push(meta);
                    }
                }

                let excluded: HashSet<String> = HashSet::new();
                let mut buckets: Vec<(BucketMeta, Vec<Value>, Vec<Value>)> = Vec::new();
                let mut total_entities = 0usize;
                let mut total_relations = 0usize;
                for meta in metas {
                    let Some(ids) = bucket_rows.get(&meta.template_id).filter(|ids| !ids.is_empty())
                    else {
                        continue;
                    };
                    let mut bucket_scope = Map::new();
                    bucket_scope.insert("id".to_string(), json!(ids));
                    match build_bucket(store, &index, &dataset_id, &bucket_scope, &excluded) {
                        Ok((entities, relations)) => {
                            total_entities += entities.len();
                            total_relations += relations.len();
                            // A template that produced nothing is not a graph.
                            if !entities.is_empty() || !relations.is_empty() {
                                buckets.push((meta, entities, relations));
                            }
                        }
                        Err(error) => {
                            // One unreadable bucket must not hide the others, and it must not be reported
                            // as empty either: it is named in the log.
                            tracing::warn!(bucket = %meta.template_id, %error, "structure graph bucket failed");
                        }
                    }
                }

                // RAPTOR summaries are stored as blobs rather than entity rows.
                let mut raptor_scope = Map::new();
                raptor_scope.insert("doc_id".to_string(), json!(document_id.clone()));
                raptor_scope.insert("compile_kwd".to_string(), json!("raptor"));
                if let Ok((raptor_rows, _)) = graph_search(
                    store,
                    &index,
                    &dataset_id,
                    &["id", "content_with_weight", "compile_kwd"],
                    raptor_scope,
                    OrderByExpr::default(),
                    RAPTOR_ROWS,
                    Vec::new(),
                    0,
                )
                {
                    let mut entities: Vec<Value> = Vec::new();
                    let mut relations: Vec<Value> = Vec::new();
                    for row in raptor_rows {
                        let Some(payload) = row.get("content_with_weight").and_then(Value::as_str)
                        else {
                            continue;
                        };
                        let Ok(graph) = serde_json::from_str::<Value>(payload) else {
                            continue;
                        };
                        if let Some(found) = graph.get("entities").and_then(Value::as_array) {
                            entities.extend(found.iter().cloned());
                        }
                        if let Some(found) = graph.get("relations").and_then(Value::as_array) {
                            relations.extend(found.iter().cloned());
                        }
                    }
                    total_entities += entities.len();
                    total_relations += relations.len();
                    if !entities.is_empty() || !relations.is_empty() {
                        buckets.push((
                            BucketMeta {
                                template_id: "raptor".to_string(),
                                template_name: "RAPTOR Summary".to_string(),
                                kind: "raptor".to_string(),
                            },
                            entities,
                            relations,
                        ));
                    }
                }
                Ok(Outcome {
                    buckets,
                    scanned_rows: rows.len(),
                    total_matching,
                    total_entities,
                    total_relations,
                })
            }
        }
    })
    .await;
    let Outcome {
        mut buckets,
        scanned_rows,
        total_matching,
        total_entities,
        total_relations,
    } = match scanned {
        Ok(outcome) => outcome,
        Err(response) => return response,
    };

    // Configured order first, then whatever else was found, as upstream orders them.
    let configured_order: HashMap<&str, usize> = configured
        .iter()
        .enumerate()
        .map(|(position, meta)| (meta.template_id.as_str(), position))
        .collect();
    buckets.sort_by_key(|(meta, _, _)| {
        (
            configured_order
                .get(meta.template_id.as_str())
                .copied()
                .unwrap_or(usize::MAX),
            meta.template_id.clone(),
        )
    });
    // A configured template that produced rows keeps the name the user gave it.
    for (meta, _, _) in buckets.iter_mut() {
        if let Some(known) = configured
            .iter()
            .find(|candidate| candidate.template_id == meta.template_id)
        {
            meta.template_name = known.template_name.clone();
            if meta.kind.is_empty() {
                meta.kind = known.kind.clone();
            }
        }
    }

    let keywords = query.keywords.unwrap_or_default();
    let keywords = keywords.trim().to_string();
    let mut returned_entities = 0usize;
    let mut returned_relations = 0usize;
    let templates: Vec<Value> = buckets
        .into_iter()
        .map(|(meta, entities, relations)| {
            // `dedup_entities` de-duplicates nodes; `normalize_relation_endpoints` rewrites edge
            // endpoints to the ids the entities are reported under. `filter_entities_with_relations`
            // returns *entities*, which is easy to mistake for a relation filter — using it as one
            // replaced the relations with a list of nodes.
            let entities = dedup_entities(&entities);
            let relations = normalize_relation_endpoints(&entities, &relations);
            let (entities, relations) = if keywords.is_empty() {
                (entities, relations)
            } else {
                narrow_to_keywords(&entities, &relations, &keywords)
            };
            returned_entities += entities.len();
            returned_relations += relations.len();
            json!({
                "template_id": meta.template_id,
                "template_name": meta.template_name,
                "kind": meta.kind,
                "entities": entities,
                "relations": relations,
            })
        })
        .filter(|bucket| {
            let entities = bucket["entities"].as_array().map(Vec::len).unwrap_or(0);
            let relations = bucket["relations"].as_array().map(Vec::len).unwrap_or(0);
            // Empty templates are filtered out, as upstream does.
            entities > 0 || relations > 0
        })
        .collect();

    let truncated = total_matching > scanned_rows;
    Json(json!({
        "code": 0,
        "data": {
            "total_entities": total_entities,
            "total_relations": total_relations,
            "returned_entities": returned_entities,
            "returned_relations": returned_relations,
            "templates": templates,
            "scanned_rows": scanned_rows,
            "truncated": truncated,
            // An empty answer says why it is empty and what produces the data, instead of looking like a
            // broken page.
            "note": if total_entities == 0 && total_relations == 0 {
                "This document has no compiled structure graph yet: knowledge compilation writes these rows after a document is parsed with a compilation template configured."
            } else {
                "Entities and relations compiled from this document, grouped by compilation template."
            },
        },
        "message": "success",
    }))
    .into_response()
}

/// `DELETE /api/v1/datasets/{dataset_id}/documents/{document_id}/structure/graph`.
///
/// Body: `{"template_id": "<template id> | legacy:<compile_kwd> | raptor"}`. Upstream removes the
/// template's compact graph row and its entity/relation rows; for `raptor` it removes only the graph
/// projection, so the summary chunks stay retrievable.
pub async fn delete_document_structure_graph(
    State(state): State<std::sync::Arc<AppState>>,
    Path((dataset_id, document_id)): Path<(String, String)>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    body: Option<Json<Value>>,
) -> Response {
    if !kb_accessible(&state, &dataset_id, &auth) {
        return api_error_code(
            StatusCode::OK,
            code::INVALID_OR_MISSING_DATA,
            &format!("You don't own the dataset {dataset_id}."),
        );
    }
    if state
        .docs
        .get(&document_id)
        .filter(|doc| doc.kb_id == dataset_id)
        .is_none()
    {
        return api_error_code(
            StatusCode::OK,
            code::INVALID_OR_MISSING_DATA,
            &format!("you don't own the document {document_id}"),
        );
    }
    let template_id = body
        .as_ref()
        .and_then(|Json(value)| value.get("template_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if template_id.is_empty() {
        return api_error_code(
            StatusCode::OK,
            code::INVALID_OR_MISSING_DATA,
            "`template_id` is required",
        );
    }
    let dataset = state.kbs.get(&dataset_id);
    let owner = dataset
        .as_ref()
        .map(|dataset| dataset.owner_id.clone())
        .filter(|owner| !owner.is_empty())
        .unwrap_or_else(|| auth.user_id.clone());
    // Same rule as the read side: the synchronous client may not run on a tokio worker, so every call
    // goes through the blocking helper.
    let index = crate::structure_compile::doc_store_index_name(&owner);
    let requested_template = template_id.clone();
    let outcome = crate::api::dataset_navigation::req_store(&state, {
        let dataset_id = dataset_id.clone();
        let document_id = document_id.clone();
        let index = index.clone();
        move |store| -> anyhow::Result<usize> {
            let mut base = Map::new();
            base.insert("doc_id".to_string(), json!(document_id.clone()));
            let mut remove = |extra: &[(&str, Value)]| -> anyhow::Result<usize> {
                let mut condition = base.clone();
                for (key, value) in extra {
                    condition.insert((*key).to_string(), value.clone());
                }
                store
                    .delete(&condition, &index, &dataset_id)
                    .map_err(|error| anyhow::anyhow!(error))
            };

            // RAPTOR only drops the graph projection, so the summary chunks stay retrievable.
            if template_id == "raptor" {
                return remove(&[("compile_kwd", json!("raptor_graph"))]);
            }

            // A legacy bucket is addressed by its compile key; a real template by its id. Both then lose
            // the compact graph row and the entity/relation rows, counted together.
            let scope: (&str, Value) = match template_id.strip_prefix("legacy:") {
                Some(compile_kwd) => {
                    let compile_kwd = compile_kwd.trim();
                    if compile_kwd.is_empty() {
                        anyhow::bail!("`template_id` is invalid");
                    }
                    ("compile_kwd", json!(compile_kwd))
                }
                None => ("compilation_template_ids", json!([template_id])),
            };
            let mut deleted = 0usize;
            for kind in [json!("graph"), json!(["entity", "relation"])] {
                deleted += remove(&[(scope.0, scope.1.clone()), ("knowledge_graph_kwd", kind)])?;
            }
            Ok(deleted)
        }
    })
    .await;

    match outcome {
        Ok(deleted) => Json(json!({
            "code": 0,
            "data": { "deleted": deleted },
            "message": format!("deleted {deleted} structure graph rows"),
        }))
        .into_response(),
        // The invalid-template refusal is a caller error, not a store failure.
        Err(response) => {
            if requested_template
                .strip_prefix("legacy:")
                .is_some_and(|compile_kwd| compile_kwd.trim().is_empty())
            {
                api_error_code(
                    StatusCode::OK,
                    code::INVALID_OR_MISSING_DATA,
                    "`template_id` is invalid",
                )
            } else {
                response
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_with_a_template_goes_to_that_template_and_a_row_without_one_does_not_disappear() {
        let mut templated = Map::new();
        templated.insert("doc_id".to_string(), json!("doc-1"));
        templated.insert("compilation_template_ids".to_string(), json!(["tpl-9"]));
        templated.insert("compile_kwd".to_string(), json!("knowledge_graph"));
        templated.insert(
            "compilation_template_kind_kwd".to_string(),
            json!("Hypergraph"),
        );
        let (meta, scope) = bucket_for_row(&templated);
        assert_eq!(meta.template_id, "tpl-9");
        assert_eq!(meta.kind, "Hypergraph");
        assert_eq!(scope["compilation_template_ids"], json!(["tpl-9"]));

        let mut legacy = Map::new();
        legacy.insert("doc_id".to_string(), json!("doc-1"));
        legacy.insert("compile_kwd".to_string(), json!("knowledge_graph"));
        let (meta, scope) = bucket_for_row(&legacy);
        assert_eq!(meta.template_id, "legacy:knowledge_graph");
        assert_eq!(meta.template_name, "Legacy (knowledge_graph)");
        assert_eq!(scope["compile_kwd"], json!("knowledge_graph"));

        // The kind normaliser folds the spellings that mean the same thing.
        assert_eq!(normalise_kind("Page Index"), "page_index");
        assert_eq!(normalise_kind("hyper-graph"), "hyper_graph");
    }

    #[test]
    fn keyword_narrowing_keeps_matching_entities_and_only_their_relations() {
        let entities = vec![
            json!({"name": "rust", "content_with_weight": "Rust is a language"}),
            json!({"name": "python", "content_with_weight": "Python is a language"}),
        ];
        let relations = vec![
            json!({"from_entity_kwd": "rust", "to_entity_kwd": "python"}),
            json!({"from_entity_kwd": "python", "to_entity_kwd": "python"}),
        ];
        let (kept_entities, kept_relations) = narrow_to_keywords(&entities, &relations, "RUST");
        assert_eq!(kept_entities.len(), 1, "{kept_entities:?}");
        assert_eq!(kept_entities[0]["name"], "rust");
        // The relation between a survivor and a dropped entity is dropped with it.
        assert!(kept_relations.is_empty(), "{kept_relations:?}");

        // No keywords means no narrowing, not an empty graph.
        let (all_entities, all_relations) = narrow_to_keywords(&entities, &relations, "   ");
        assert_eq!(all_entities.len(), 2);
        assert_eq!(all_relations.len(), 2);
    }
}
