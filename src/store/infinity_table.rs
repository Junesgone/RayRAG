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

use crate::Result;
use crate::settings::{
    INFINITY_CHUNK_DATA_DEFAULT, INFINITY_CHUNK_DATA_FIELD, INFINITY_CHUNK_DATA_TYPE,
    INFINITY_CHUNK_PYTHON_VECTOR_INDEX, INFINITY_CHUNK_VECTOR_FIELD_PREFIX,
    INFINITY_CHUNK_VECTOR_FIELD_SUFFIX, INFINITY_CHUNK_VECTOR_INDEX_EF_CONSTRUCTION,
    INFINITY_CHUNK_VECTOR_INDEX_ENCODING, INFINITY_CHUNK_VECTOR_INDEX_M,
    INFINITY_CHUNK_VECTOR_INDEX_METRIC, infinity_chunk_vector_field, infinity_chunk_vector_index,
};
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
        self.columns
            .insert(dimension, HnswVectorIndex::standard(dimension));
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
    matches!(
        record.get(INFINITY_CHUNK_DATA_FIELD),
        Some(Value::Object(_))
    )
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
            chunk_data.insert(
                "type".into(),
                Value::String(INFINITY_CHUNK_DATA_TYPE.into()),
            );
            chunk_data.insert(
                "default".into(),
                Value::String(INFINITY_CHUNK_DATA_DEFAULT.into()),
            );
            columns.push((INFINITY_CHUNK_DATA_FIELD.into(), chunk_data));
        }
        let mut vector = Map::new();
        vector.insert(
            "type".into(),
            Value::String(format!("vector,{},float", self.vector_index.dimension)),
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


