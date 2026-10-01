//! Dataset-level navigation clustering — RAGFlow v0.27.2
//! `rag/advanced_rag/knowlege_compile/dataset_nav.py`.
//!
//! Incremental clustering for dataset-level navigation: each new document is
//! embedded and placed into the nearest `nav_cluster` via layered KNN search
//! plus threshold-based merge/create. Storage keeps one row per `nav_cluster`
//! or `nav_doc` node; the tree is encoded via `parent_kwd` (no tree blob).
//!
//! Port progress: part 1 (constants, ids, tree-summary helper, index/vector
//! helpers, doc-store get/search wrappers). Store I/O runs through the local
//! `DocStore`; row ids use the crate row-id helper over the upstream seed
//! (xxh3-64; upstream xxh64 — established divergence).

use serde_json::Value;

/// `_COMPILE_KWD`.
pub const COMPILE_KWD: &str = "dataset_nav";

/// `_MERGE_THRESHOLD`.
pub const MERGE_THRESHOLD: f64 = 0.80;
/// `_RECURSE_THRESHOLD`.
pub const RECURSE_THRESHOLD: f64 = 0.65;
/// `_MIN_SIM`.
pub const MIN_SIM: f64 = 0.50;
/// `_MAX_FANOUT`.
pub const MAX_FANOUT: usize = 64;
/// `_MAX_DOCS_PER_CLUSTER`.
pub const MAX_DOCS_PER_CLUSTER: usize = 50;
/// `_LOCK_TIMEOUT_S`.
pub const LOCK_TIMEOUT_S: u64 = 30;
/// `_LOCK_BLOCKING_TIMEOUT_S`.
pub const LOCK_BLOCKING_TIMEOUT_S: u64 = 5;
/// `_KNN_TOP_K`.
pub const KNN_TOP_K: usize = 5;
/// `_NAV_HYBRID_DENSE_W`.
pub const NAV_HYBRID_DENSE_W: f64 = 0.5;
/// `_NAV_TREE_MIN_SCORE`.
pub const NAV_TREE_MIN_SCORE: f64 = 0.1;

/// `_NAV_SEARCH_FIELDS`.
pub const NAV_SEARCH_FIELDS: [&str; 7] = [
    "id",
    "content_with_weight",
    "name",
    "doc_id",
    "type_kwd",
    "doc_ids_kwd",
    "doc_count_int",
];

/// `_NAV_STOP_WORDS`.
pub const NAV_STOP_WORDS: [&str; 30] = [
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "with", "at", "is", "are", "was",
    "were", "be", "been", "being", "this", "that", "these", "those", "it", "its", "as", "by",
    "from", "about", "into",
];

/// `_nav_doc_id`: stable row id for a nav_doc (deterministic by doc_id).
pub fn nav_doc_id(doc_id: &str) -> String {
    crate::structure_compile::stable_row_id(&[format!("dataset_nav:doc:{doc_id}")])
}

/// `_nav_cluster_id`: stable row id for a nav_cluster (kb_id + name).
pub fn nav_cluster_id(kb_id: &str, name: &str) -> String {
    crate::structure_compile::stable_row_id(&[format!("dataset_nav:{kb_id}:cluster:{name}")])
}

/// `_nav_lock_key`: Redis lock key for a KB's nav tree.
pub fn nav_lock_key(kb_id: &str) -> String {
    format!("dataset_nav:{kb_id}")
}

/// `_extract_root_summary_from_tree`: the doc-level summary from a RAPTOR tree
/// (or a bare string value).
pub fn extract_root_summary_from_tree(tree: &Value) -> String {
    let Some(map) = tree.as_object() else {
        return String::new();
    };
    if let Some(title) = map.get("title").and_then(Value::as_str) {
        if !title.trim().is_empty() {
            return title.trim().to_string();
        }
    }
    for key in ["summary", "content_with_weight", "content"] {
        if let Some(text) = map.get(key).and_then(Value::as_str) {
            if !text.trim().is_empty() {
                return text.trim().to_string();
            }
        }
    }
    String::new()
}

/// `_index_name(tenant_id)`.
pub fn index_name(tenant_id: &str) -> String {
    crate::structure_compile::doc_store_index_name(tenant_id)
}

/// `_vec_field(dim)`.
pub fn vec_field(dim: usize) -> String {
    format!("q_{dim}_vec")
}

/// `_store_get`: one row by id.
pub fn store_get(
    store: &dyn crate::doc_store::DocStore,
    tenant_id: &str,
    kb_id: &str,
    row_id: &str,
) -> Option<Value> {
    let index = index_name(tenant_id);
    let row = store.get(row_id, &index, &[kb_id.to_string()]).ok()??;
    Some(Value::Object(row))
}

/// `_store_search`: rows matching a conjunctive condition.
pub fn store_search(
    store: &dyn crate::doc_store::DocStore,
    tenant_id: &str,
    kb_id: &str,
    condition: &Value,
    fields: &[String],
    limit: usize,
) -> Vec<Value> {
    let index = index_name(tenant_id);
    let mut query = crate::doc_store::SearchQuery {
        select_fields: fields.to_vec(),
        condition: condition.as_object().cloned().unwrap_or_default(),
        offset: 0,
        limit,
        index_names: vec![index],
        dataset_ids: vec![kb_id.to_string()],
        ..Default::default()
    };
    query.match_expressions = Vec::new();
    let Ok(response) = store.search(&query) else {
        return Vec::new();
    };
    store
        .get_fields(&response, fields)
        .into_values()
        .map(Value::Object)
        .collect()
}
#[cfg(test)]
mod dataset_nav_tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    #[test]
    fn ids_are_stable_and_scoped() {
        assert_eq!(nav_doc_id("d1"), nav_doc_id("d1"));
        assert_ne!(nav_doc_id("d1"), nav_doc_id("d2"));
        assert_ne!(nav_cluster_id("kb1", "A"), nav_cluster_id("kb1", "B"));
        assert_ne!(nav_cluster_id("kb1", "A"), nav_cluster_id("kb2", "A"));
        assert_eq!(nav_lock_key("kb1"), "dataset_nav:kb1");
        assert_eq!(index_name("t1"), "ragflow_t1");
        assert_eq!(vec_field(1024), "q_1024_vec");
        assert_eq!(COMPILE_KWD, "dataset_nav");
    }

    #[test]
    fn root_summary_variants() {
        assert_eq!(
            extract_root_summary_from_tree(&json!({"title": " Root "})),
            "Root"
        );
        assert_eq!(
            extract_root_summary_from_tree(&json!({"summary": "S"})),
            "S"
        );
        assert_eq!(
            extract_root_summary_from_tree(&json!({"content_with_weight": "C"})),
            "C"
        );
        assert_eq!(extract_root_summary_from_tree(&json!({"title": "  "})), "");
        assert_eq!(extract_root_summary_from_tree(&json!(null)), "");
        assert_eq!(extract_root_summary_from_tree(&json!("bare")), "");
    }

    #[derive(Default)]
    struct MockStore {
        rows: Vec<Value>,
        gets: Mutex<usize>,
    }

    impl crate::doc_store::DocStore for MockStore {
        fn db_type(&self) -> &'static str {
            "memory"
        }
        fn health(&self) -> crate::Result<crate::doc_store::HealthStatus> {
            Ok(crate::doc_store::HealthStatus::green("test"))
        }
        fn create_idx(&self, _i: &str, _d: &str, _v: usize) -> crate::Result<()> {
            Ok(())
        }
        fn delete_idx(&self, _i: &str, _d: &str) -> crate::Result<()> {
            Ok(())
        }
        fn index_exist(&self, _i: &str, _d: &str) -> crate::Result<bool> {
            Ok(true)
        }
        fn insert(
            &self,
            _rows: &[crate::doc_store::DocRow],
            _i: &str,
            _d: &str,
        ) -> crate::Result<Vec<String>> {
            Ok(Vec::new())
        }
        fn get(
            &self,
            _data_id: &str,
            _i: &str,
            _d: &[String],
        ) -> crate::Result<Option<crate::doc_store::DocRow>> {
            *self.gets.lock().unwrap() += 1;
            Ok(None)
        }
        fn update(
            &self,
            _c: &crate::doc_store::FilterCondition,
            _n: &crate::doc_store::DocRow,
            _i: &str,
            _d: &str,
        ) -> crate::Result<bool> {
            Ok(false)
        }
        fn delete(
            &self,
            _c: &crate::doc_store::FilterCondition,
            _i: &str,
            _d: &str,
        ) -> crate::Result<usize> {
            Ok(0)
        }
        fn search(
            &self,
            _q: &crate::doc_store::SearchQuery,
        ) -> crate::Result<crate::doc_store::SearchResponse> {
            let docs: Vec<crate::doc_store::DocRow> = self
                .rows
                .iter()
                .filter_map(|row| row.as_object().cloned())
                .collect();
            Ok(crate::doc_store::SearchResponse {
                total: docs.len(),
                docs,
                ..Default::default()
            })
        }
        fn sql(&self, _s: &str, _f: usize) -> crate::Result<Vec<Value>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn store_wrappers_delegate() {
        let store = MockStore {
            rows: vec![json!({"id": "r1", "name": "A"})],
            ..MockStore::default()
        };
        let fields = vec!["id".to_string(), "name".to_string()];
        let rows = store_search(
            &store,
            "t1",
            "kb1",
            &json!({"compile_kwd": ["dataset_nav"]}),
            &fields,
            100,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], json!("A"));
        assert!(store_get(&store, "t1", "kb1", "r1").is_none());
        assert_eq!(*store.gets.lock().unwrap(), 1, "get was attempted");
    }
}
