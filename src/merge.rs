//! LLM-judged structured-knowledge merging — ported from RAGFlow's
//! `rag/advanced_rag/knowlege_compile/structure.py` merge phase.
//!
//! Mirrors `_struct_merge_pair` (LLM decides whether two entity/relation
//! payloads are the same logical item and merges them when they are),
//! `_struct_apply_merge_invariants` (relation source/target must not change
//! across a merge) and the merge pipeline's similarity pre-filter
//! (pairwise cosine similarity above the threshold before asking the LLM).

use crate::Result;
use crate::llm::LlmClient;
use serde_json::{Value, json};

/// `MERGE_SYSTEM_PROMPT` — verbatim port.
pub const MERGE_SYSTEM_PROMPT: &str = "You are an intelligent data merging assistant.\n\
You will merge two JSON objects representing the same entity: Item A (existing) and Item B (incoming).\n\n\
Merge strategy:\n\
1. Combine information from both items.\n\
2. If fields conflict, use your best judgment to pick the more detailed or recent-looking value.\n\
3. If one item has a null/missing value and the other has data, keep the data.\n\
4. For list fields, combine unique elements from both.\n\
5. Do not invent new information not present in the inputs.\n\
6. Return the result in the exact JSON format of the input items.";

/// `MERGE_USER_PROMPT` — verbatim port with `{item_existing}` / `{item_incoming}`.
pub const MERGE_USER_PROMPT: &str =
    "Item A (existing):\n{item_existing}\n\nItem B (incoming):\n{item_incoming}";

/// `MERGE_DECISION_INSTRUCTION` — verbatim port; appended to the system
/// prompt so the LLM branches on duplication before merging.
pub const MERGE_DECISION_INSTRUCTION: &str = "First decide whether Item A and Item B refer to \
the same logical entity (for entities) or the same logical relation (for relations). \
Use the merge strategy above only if they are the same.\n\n\
Return ONLY a JSON object with this exact structure (no markdown fences, no commentary):\n\
{\n  \"duplicated\": <true | false>,\n  \"merged\": <merged JSON object using the same keys as the inputs when duplicated=true; otherwise null>\n}";

/// Default cosine-similarity threshold above which the LLM is consulted
/// (RAGFlow `similarity_threshold` default 0.9).
pub const DEFAULT_MERGE_THRESHOLD: f32 = 0.9;

/// Cosine similarity over two vectors (empty/zero vectors → 0.0).
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += (*x as f64) * (*y as f64);
        na += (*x as f64) * (*x as f64);
        nb += (*y as f64) * (*y as f64);
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    (dot / (na.sqrt() * nb.sqrt())) as f32
}

/// `_struct_apply_merge_invariants`: for relations, force source/target
/// (and src/from, tgt/to aliases) back to the existing payload's values —
/// they must not change across a merge.
pub fn apply_merge_invariants(existing: &Value, mut merged: Value) -> Value {
    if !existing.is_object() || !merged.is_object() {
        return merged;
    }
    let is_relation = ["source", "src", "from"]
        .iter()
        .any(|k| existing.get(*k).is_some());
    if !is_relation {
        return merged;
    }
    if let Some(map) = merged.as_object_mut() {
        for field in ["source", "src", "from"] {
            if let Some(value) = existing.get(field) {
                map.insert(field.to_string(), value.clone());
            }
        }
        for field in ["target", "tgt", "to"] {
            if let Some(value) = existing.get(field) {
                map.insert(field.to_string(), value.clone());
            }
        }
    }
    merged
}

/// `_struct_merge_pair`: LLM-judged merge of two payloads. Returns the merged
/// payload when the LLM decides they are duplicates, else None. temperature 0.0.
pub async fn merge_pair(
    existing: &Value,
    incoming: &Value,
    client: &LlmClient,
) -> Result<Option<Value>> {
    let user_prompt = MERGE_USER_PROMPT
        .replace("{item_existing}", &serde_json::to_string(existing)?)
        .replace("{item_incoming}", &serde_json::to_string(incoming)?);
    let system_prompt = format!("{MERGE_SYSTEM_PROMPT}\n\n{MERGE_DECISION_INSTRUCTION}");
    let res = crate::hypergraph::gen_json_with_temperature(
        client,
        &system_prompt,
        &user_prompt,
        Some(0.0),
    )
    .await?;
    if !res
        .get("duplicated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(None);
    }
    let merged = res.get("merged").cloned().unwrap_or(Value::Null);
    if !merged.is_object() {
        return Ok(None);
    }
    Ok(Some(apply_merge_invariants(existing, merged)))
}

/// Merge two payloads when their similarity (name equality when embeddings
/// are absent, else cosine similarity of `embedding`/`q_*_vec`) passes the
/// threshold. Returns the merged payload when duplicates, else None.
pub async fn maybe_merge(
    existing: &Value,
    incoming: &Value,
    client: &LlmClient,
    threshold: f32,
) -> Result<Option<Value>> {
    if payloads_similar(existing, incoming, threshold) {
        merge_pair(existing, incoming, client).await
    } else {
        Ok(None)
    }
}

/// Similarity pre-filter: embeddings (preferred) or name equality fallback.
fn payloads_similar(a: &Value, b: &Value, threshold: f32) -> bool {
    if let (Some(va), Some(vb)) = (embedding_of(a), embedding_of(b)) {
        return cosine_similarity(&va, &vb) >= threshold;
    }
    // Fallback: exact name match.
    let name_a = a
        .get("name")
        .or_else(|| a.get("source"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let name_b = b
        .get("name")
        .or_else(|| b.get("source"))
        .and_then(Value::as_str)
        .unwrap_or("");
    !name_a.is_empty() && name_a == name_b
}

fn embedding_of(value: &Value) -> Option<Vec<f32>> {
    for key in ["embedding", "q_vec"] {
        if let Some(v) = value.get(key).and_then(Value::as_array) {
            let vec: Vec<f32> = v
                .iter()
                .filter_map(|x| x.as_f64().map(|f| f as f32))
                .collect();
            if !vec.is_empty() {
                return Some(vec);
            }
        }
    }
    // Any q_<dim>_vec key (RAGFlow vector field naming).
    if let Some(map) = value.as_object() {
        for (key, val) in map {
            if key.ends_with("_vec") {
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

/// Merge a list of entity/relation payloads: greedy pairwise pass — for each
/// incoming item, first matching existing item (similarity above threshold)
/// is LLM-judged; on duplicate the existing payload is replaced by the
/// merged payload (unioning `source_chunk_ids`), else the item is appended.
pub async fn deduplicate_items(
    items: Vec<Value>,
    client: &LlmClient,
    threshold: f32,
) -> Result<Vec<Value>> {
    let mut out: Vec<Value> = Vec::new();
    for incoming in items {
        let mut merged_into: Option<usize> = None;
        for (idx, existing) in out.iter().enumerate() {
            if payloads_similar(existing, &incoming, threshold)
                && let Some(mut merged) = merge_pair(existing, &incoming, client).await? {
                    union_chunk_ids(&mut merged, existing);
                    out[idx] = merged;
                    merged_into = Some(idx);
                    break;
                }
        }
        if merged_into.is_none() {
            out.push(incoming);
        }
    }
    Ok(out)
}

/// Union `source_chunk_ids` from the existing payload into the merged one
/// when the merge result lacks them.
fn union_chunk_ids(merged: &mut Value, existing: &Value) {
    if !merged.is_object() {
        return;
    }
    let mut ids: Vec<String> = merged
        .get("source_chunk_ids")
        .and_then(Value::as_array)
        .map(|v| {
            v.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    if let Some(list) = existing.get("source_chunk_ids").and_then(Value::as_array) {
        for v in list {
            if let Some(s) = v.as_str()
                && !ids.iter().any(|x| x == s) {
                    ids.push(s.to_string());
                }
        }
    }
    if let Some(map) = merged.as_object_mut() {
        map.insert("source_chunk_ids".into(), json!(ids));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cosine_similarity_basic() {
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]), 1.0);
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        assert_eq!(cosine_similarity(&[], &[1.0]), 0.0);
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[1.0]), 0.0);
        let v = cosine_similarity(&[1.0, 2.0, 3.0], &[2.0, 4.0, 6.0]);
        assert!((v - 1.0).abs() < 1e-6, "parallel vectors: {v}");
    }

    #[test]
    fn merge_invariants_preserve_relation_endpoints() {
        let existing = json!({
            "source": "A", "target": "B", "type": "works_at",
            "description": "old"
        });
        let merged = json!({
            "source": "X", "target": "Y", "type": "works_at",
            "description": "new evidence"
        });
        let out = apply_merge_invariants(&existing, merged);
        assert_eq!(out["source"], "A");
        assert_eq!(out["target"], "B");
        assert_eq!(out["description"], "new evidence");
        // Entities are untouched.
        let ent = json!({"name": "A", "description": "d"});
        let out2 = apply_merge_invariants(&ent, json!({"name": "A", "description": "d2"}));
        assert_eq!(out2["description"], "d2");
    }

    #[test]
    fn embedding_fallback_and_threshold() {
        let a = json!({"name": "张三", "embedding": [1.0, 0.0, 0.0]});
        let b = json!({"name": "张三", "embedding": [0.98, 0.02, 0.0]});
        let c = json!({"name": "李四", "embedding": [0.0, 1.0, 0.0]});
        assert!(payloads_similar(&a, &b, DEFAULT_MERGE_THRESHOLD));
        assert!(!payloads_similar(&a, &c, DEFAULT_MERGE_THRESHOLD));
        // q_<dim>_vec naming.
        let d = json!({"name": "王五", "q_3_vec": [1.0, 0.0, 0.0]});
        assert!(payloads_similar(&a, &d, DEFAULT_MERGE_THRESHOLD));
        // Name-equality fallback without vectors.
        let e = json!({"name": "张三"});
        let f = json!({"name": "张三"});
        assert!(payloads_similar(&e, &f, DEFAULT_MERGE_THRESHOLD));
    }

    #[test]
    fn union_chunk_ids_dedupes() {
        let mut merged = json!({"name": "A", "source_chunk_ids": ["c1", "c2"]});
        let existing = json!({"name": "A", "source_chunk_ids": ["c2", "c3"]});
        union_chunk_ids(&mut merged, &existing);
        let ids: Vec<&str> = merged["source_chunk_ids"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(ids, vec!["c1", "c2", "c3"]);
    }

    // Real GPU merge judgement (ignored; requires RAYRAG_TEST_LLM_BASE/MODEL).
    #[tokio::test]
    #[ignore]
    async fn gpu_merge_pair_judges_duplicate_and_merges() {
        let base = std::env::var("RAYRAG_TEST_LLM_BASE").expect("RAYRAG_TEST_LLM_BASE");
        let model = std::env::var("RAYRAG_TEST_LLM_MODEL").expect("RAYRAG_TEST_LLM_MODEL");
        let client = crate::llm::LlmClient::new(crate::llm::LlmConfig {
            api_base: base,
            api_key: String::new(),
            model,
            generation: Default::default(),
            system_prompt: String::new(),
        });
        // Same logical entity, different descriptions.
        let existing =
            json!({"type": "organization", "name": "百鲤居水产养殖场", "description": "位于中山"});
        let incoming = json!({"type": "organization", "name": "百鲤居水产养殖场", "description": "主营四大家鱼"});
        let merged = merge_pair(&existing, &incoming, &client)
            .await
            .expect("merge")
            .expect("duplicated");
        assert_eq!(merged["name"], "百鲤居水产养殖场");
        // Different entities → None.
        let other = json!({"type": "person", "name": "李锦澎", "description": "负责人"});
        assert!(
            merge_pair(&existing, &other, &client)
                .await
                .unwrap()
                .is_none(),
            "unrelated items must not merge"
        );
    }
}
