//! Same-table multi-dimension vector lifecycle for the fixed Infinity chunk
//! mapping.
//!
//! Infinity chunk tables hold the 73 fixed base columns plus one `q_{N}_vec`
//! embedding column per embedding dimension that has ever been written. When
//! a knowledge base is re-embedded with a different model dimension, the
//! connector ADDs the new column and its HNSW index instead of replacing the
//! table, so old and new dimensions coexist until the table is dropped.
//! RayRAG keeps the same lifecycle as a per-table dimension registry, while
//! the JSON engine and the zvec mirror remain the executable storage.

use crate::settings::{
    INFINITY_CHUNK_DATA_FIELD, INFINITY_CHUNK_DATA_TYPE, INFINITY_CHUNK_DATA_DEFAULT,
    INFINITY_CHUNK_PYTHON_VECTOR_INDEX, INFINITY_CHUNK_VECTOR_FIELD_PREFIX,
    INFINITY_CHUNK_VECTOR_FIELD_SUFFIX, INFINITY_CHUNK_VECTOR_INDEX_ENCODING,
    INFINITY_CHUNK_VECTOR_INDEX_EF_CONSTRUCTION, INFINITY_CHUNK_VECTOR_INDEX_M,
    INFINITY_CHUNK_VECTOR_INDEX_METRIC, infinity_chunk_vector_field,
    infinity_chunk_vector_index,
};
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

/// `ParserType.TABLE` value used by the connector to detect the table parser.
pub const TABLE_PARSER_ID: &str = "table";

/// HNSW index descriptor for one embedding dimension, mirroring the fixed
/// Python connector parameters (strings exactly as Infinity's SDK expects).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HnswVectorIndex {
    pub dimension: usize,
    pub m: u8,
    pub ef_construction: u8,
    pub metric: String,
    pub encoding: String,
}

impl HnswVectorIndex {
    pub fn standard(dimension: usize) -> Self {
        Self {
            dimension,
            m: INFINITY_CHUNK_VECTOR_INDEX_M,
            ef_construction: INFINITY_CHUNK_VECTOR_INDEX_EF_CONSTRUCTION,
            metric: INFINITY_CHUNK_VECTOR_INDEX_METRIC.into(),
            encoding: INFINITY_CHUNK_VECTOR_INDEX_ENCODING.into(),
        }
    }

    pub fn column_name(&self) -> String {
        infinity_chunk_vector_field(self.dimension)
    }

    /// The Python connector's fixed index name (`q_vec_idx`), independent of
    /// the dimension.
    pub fn python_index_name(&self) -> String {
        INFINITY_CHUNK_PYTHON_VECTOR_INDEX.into()
    }

    /// The dimensioned index name used by the Go connector.
    pub fn dimensioned_index_name(&self) -> String {
        infinity_chunk_vector_index(self.dimension)
    }

    /// Infinity `IndexInfo` extra options: string-typed values per the SDK.
    pub fn extra_options(&self) -> Map<String, Value> {
        let mut options = Map::new();
        options.insert("M".into(), Value::String(self.m.to_string()));
        options.insert(
            "ef_construction".into(),
            Value::String(self.ef_construction.to_string()),
        );
        options.insert("metric".into(), Value::String(self.metric.clone()));
        options.insert("encode".into(), Value::String(self.encoding.clone()));
        options
    }
}

/// Result of ensuring one vector dimension exists on a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DimensionChange {
    /// True when the `q_{N}_vec` column was newly added.
    pub column_created: bool,
    /// True when the HNSW index over that column was newly created.
    pub index_created: bool,
}

impl DimensionChange {
    pub fn is_noop(&self) -> bool {
        !self.column_created && !self.index_created
    }
}

/// Per-table registry of live embedding dimensions. Serde-stable so the
/// lifecycle survives a restart via the store's JSON persistence helpers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VectorDimensionRegistry {
    columns: BTreeMap<usize, HnswVectorIndex>,
}

impl VectorDimensionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Idempotently register `dimension` (column + HNSW index). Mirrors the
    /// connector's `create_idx` behavior on an already-existing table.
    pub fn ensure_column(&mut self, dimension: usize) -> DimensionChange {
        if self.columns.contains_key(&dimension) {
            return DimensionChange {
                column_created: false,
                index_created: false,
            };
        }
        self.columns.insert(dimension, HnswVectorIndex::standard(dimension));
        DimensionChange {
            column_created: true,
            index_created: true,
        }
    }

    pub fn column(&self, dimension: usize) -> Option<&HnswVectorIndex> {
        self.columns.get(&dimension)
    }

    pub fn dimensions(&self) -> Vec<usize> {
        self.columns.keys().copied().collect()
    }

    /// Drop one dimension column (and its index) from the table.
    pub fn remove_column(&mut self, dimension: usize) -> bool {
        self.columns.remove(&dimension).is_some()
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// Drop the whole table: every dimension column disappears with it.
    pub fn drop_table(&mut self) -> usize {
        let dropped = self.columns.len();
        self.columns.clear();
        dropped
    }
}

/// Infer the embedding dimension from one record's keys (`q_(\d+)_vec`), the
/// connector's table-creation fallback in `insert`.
pub fn infer_vector_size(record: &Map<String, Value>) -> Option<usize> {
    record.keys().find_map(|key| parse_vector_field(key))
}

/// Parse a `q_{N}_vec` field name into its dimension.
pub fn parse_vector_field(field: &str) -> Option<usize> {
    let dimension = field
        .strip_prefix(INFINITY_CHUNK_VECTOR_FIELD_PREFIX)?
        .strip_suffix(INFINITY_CHUNK_VECTOR_FIELD_SUFFIX)?;
    let parsed = dimension.parse::<usize>().ok()?;
    (parsed > 0).then_some(parsed)
}

/// True when a record carries a JSON-object `chunk_data` column — the
/// connector's TABLE-parser signal (`isinstance(documents[0]["chunk_data"],
/// dict)`).
pub fn detect_table_parser(record: &Map<String, Value>) -> bool {
    matches!(record.get(INFINITY_CHUNK_DATA_FIELD), Some(Value::Object(_)))
}

/// Table-creation plan for one vector dimension, mirroring `create_idx`'s
/// schema additions: the dynamic vector column + HNSW index, plus the JSON
/// `chunk_data` column for the TABLE parser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableCreatePlan {
    pub vector_index: HnswVectorIndex,
    pub chunk_data: bool,
}

impl TableCreatePlan {
    pub fn plan(vector_size: usize, parser_id: Option<&str>) -> Result<Self> {
        if vector_size == 0 {
            anyhow::bail!("Cannot infer vector size from documents");
        }
        Ok(Self {
            vector_index: HnswVectorIndex::standard(vector_size),
            chunk_data: parser_id == Some(TABLE_PARSER_ID),
        })
    }

    /// The extra columns this plan adds over the 73 fixed base columns.
    pub fn extra_columns(&self) -> Vec<(String, Map<String, Value>)> {
        let mut columns = Vec::new();
        if self.chunk_data {
            let mut chunk_data = Map::new();
            chunk_data.insert("type".into(), Value::String(INFINITY_CHUNK_DATA_TYPE.into()));
            chunk_data.insert(
                "default".into(),
                Value::String(INFINITY_CHUNK_DATA_DEFAULT.into()),
            );
            columns.push((INFINITY_CHUNK_DATA_FIELD.into(), chunk_data));
        }
        let mut vector = Map::new();
        vector.insert(
            "type".into(),
            Value::String(format!(
                "vector,{},float",
                self.vector_index.dimension
            )),
        );
        columns.push((self.vector_index.column_name(), vector));
        columns
    }
}

/// Fill zero vectors for every registered embedding column a record lacks —
/// the connector's `embedding columns can't have a default value` insertion
/// loop. Returns the number of columns filled.
pub fn plan_embedding_fill(
    record: &mut Map<String, Value>,
    registry: &VectorDimensionRegistry,
) -> usize {
    let mut filled = 0;
    for dimension in registry.dimensions() {
        let column = infinity_chunk_vector_field(dimension);
        if record.contains_key(&column) {
            continue;
        }
        record.insert(column, Value::Array(vec![Value::from(0.0); dimension]));
        filled += 1;
    }
    filled
}

/// Persist one table's dimension lifecycle next to the snapshot.
pub fn save_registry(path: &Path, registry: &VectorDimensionRegistry) -> Result<()> {
    crate::persistence::save_json(path, registry)
}

/// Load a persisted dimension lifecycle; a missing file is fresh state.
pub fn load_registry(path: &Path) -> Result<VectorDimensionRegistry> {
    Ok(crate::persistence::load_json(path)?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_field_parsing_accepts_only_dimensioned_names() {
        assert_eq!(parse_vector_field("q_1024_vec"), Some(1024));
        assert_eq!(parse_vector_field("q_2_vec"), Some(2));
        assert_eq!(parse_vector_field("q_0_vec"), None);
        assert_eq!(parse_vector_field("q_1024"), None);
        assert_eq!(parse_vector_field("content_ltks"), None);
        assert_eq!(parse_vector_field("q_1024_vec_idx"), None);
    }

    #[test]
    fn infer_vector_size_scans_record_keys_like_python_regex() {
        let record: Map<String, Value> = [
            ("id".into(), Value::String("c1".into())),
            ("content".into(), Value::String("text".into())),
            ("q_768_vec".into(), Value::Array(vec![])),
        ]
        .into_iter()
        .collect();
        assert_eq!(infer_vector_size(&record), Some(768));
        let without: Map<String, Value> = [("id".into(), Value::String("c1".into()))]
            .into_iter()
            .collect();
        assert_eq!(infer_vector_size(&without), None);
    }

    #[test]
    fn table_parser_detection_requires_json_object_chunk_data() {
        let with_table: Map<String, Value> = [(
            INFINITY_CHUNK_DATA_FIELD.into(),
            serde_json::json!({"cells": ["a"]}),
        )]
        .into_iter()
        .collect();
        assert!(detect_table_parser(&with_table));
        let without: Map<String, Value> = [
            (INFINITY_CHUNK_DATA_FIELD.into(), Value::String("{}".into())),
        ]
        .into_iter()
        .collect();
        assert!(!detect_table_parser(&without));
        let absent: Map<String, Value> = Map::new();
        assert!(!detect_table_parser(&absent));
    }

    #[test]
    fn hnsw_descriptor_matches_the_fixed_python_connector() {
        let index = HnswVectorIndex::standard(1024);
        assert_eq!(index.column_name(), "q_1024_vec");
        assert_eq!(index.python_index_name(), "q_vec_idx");
        assert_eq!(index.dimensioned_index_name(), "q_1024_vec_idx");
        assert_eq!(index.m, 16);
        assert_eq!(index.ef_construction, 50);
        assert_eq!(index.metric, "cosine");
        assert_eq!(index.encoding, "lvq");
        assert_eq!(
            index.extra_options(),
            serde_json::json!({
                "M": "16",
                "ef_construction": "50",
                "metric": "cosine",
                "encode": "lvq"
            })
            .as_object()
            .cloned()
            .unwrap()
        );
        assert_eq!(crate::settings::INFINITY_CHUNK_VECTOR_INDEX_TYPE, "hnsw");
    }

    #[test]
    fn registry_keeps_multiple_dimensions_in_one_table() {
        let mut registry = VectorDimensionRegistry::new();
        assert_eq!(
            registry.ensure_column(768),
            DimensionChange {
                column_created: true,
                index_created: true
            }
        );
        assert!(registry.ensure_column(768).is_noop());
        registry.ensure_column(1024);
        assert_eq!(registry.dimensions(), vec![768, 1024]);
        assert!(registry.column(1024).is_some());
        // Old dimensions coexist: re-embedding never removes a column.
        assert!(registry.remove_column(768));
        assert_eq!(registry.dimensions(), vec![1024]);
        assert!(!registry.remove_column(768));
    }

    #[test]
    fn table_drop_removes_every_dimension_column() {
        let mut registry = VectorDimensionRegistry::new();
        registry.ensure_column(384);
        registry.ensure_column(1536);
        assert_eq!(registry.drop_table(), 2);
        assert!(registry.is_empty());
    }

    #[test]
    fn table_create_plan_adds_vector_and_optional_chunk_data() {
        let plan = TableCreatePlan::plan(1024, Some(TABLE_PARSER_ID)).unwrap();
        assert!(plan.chunk_data);
        assert_eq!(plan.vector_index.dimension, 1024);
        let columns = plan.extra_columns();
        assert_eq!(columns.len(), 2);
        assert_eq!(columns[0].0, "chunk_data");
        assert_eq!(columns[0].1["type"], "json");
        assert_eq!(columns[1].0, "q_1024_vec");
        assert_eq!(columns[1].1["type"], "vector,1024,float");

        let plain = TableCreatePlan::plan(768, Some("naive")).unwrap();
        assert!(!plain.chunk_data);
        assert_eq!(plain.extra_columns().len(), 1);
        assert!(TableCreatePlan::plan(0, None).is_err());
    }

    #[test]
    fn embedding_fill_zeroes_only_missing_dimension_columns() {
        let registry = {
            let mut registry = VectorDimensionRegistry::new();
            registry.ensure_column(2);
            registry.ensure_column(3);
            registry
        };
        let mut record: Map<String, Value> = [(
            "q_2_vec".into(),
            serde_json::json!([1.0, 0.5]),
        )]
        .into_iter()
        .collect();
        assert_eq!(plan_embedding_fill(&mut record, &registry), 1);
        assert_eq!(record["q_2_vec"], serde_json::json!([1.0, 0.5]));
        assert_eq!(record["q_3_vec"], serde_json::json!([0.0, 0.0, 0.0]));
        assert_eq!(plan_embedding_fill(&mut record, &registry), 0);
    }

    #[test]
    fn registry_persists_across_restarts_and_missing_files_are_fresh() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-infinity-table-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("dimensions.json");
        let mut registry = VectorDimensionRegistry::new();
        registry.ensure_column(384);
        registry.ensure_column(1024);
        save_registry(&path, &registry).unwrap();

        let loaded = load_registry(&path).unwrap();
        assert_eq!(loaded, registry);

        let fresh = load_registry(&root.join("absent.json")).unwrap();
        assert!(fresh.is_empty());
        std::fs::remove_dir_all(root).ok();
    }
}
