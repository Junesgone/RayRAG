//! Document store abstraction — RAGFlow `common/doc_store` port.
//!
//! Mirrors `doc_store_base.py` (the `DocStoreConnection` contract: index
//! management, CRUD, search with match expressions, result helpers and SQL)
//! and `es_conn_pool.py` (lazy singleton connection pool with bounded retries,
//! health-checked acquire/refresh) on top of RayRAG's own backends:
//! - `MemoryDocStore` — portable in-memory default;
//! - `PostgresDocStore` — durable JSONB store (`--features postgres-backend`);
//! - `ZvecDocStore` — native zvec dense mirror (`--features zvec-backend`).
//!
//! Search scoring at this layer is deliberately simple (term-hit + cosine +
//! rank-feature boost); RAGFlow's BM25/fusion refinement lives one level up in
//! `src/search.rs` (`ragflow_rerank`), which stays the source of truth for
//! hybrid ranking.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

/// Default top-N for dense match expressions (doc_store_base constant).
pub const DEFAULT_MATCH_VECTOR_TOPN: usize = 10;
/// Default top-N for sparse match expressions (doc_store_base constant).
pub const DEFAULT_MATCH_SPARSE_TOPN: usize = 10;
/// Connection attempts during pool warm-up (es_conn_pool `ATTEMPT_TIME`).
pub const ATTEMPT_TIME: usize = 2;
/// Seconds to wait between connection attempts (es_conn_pool).
pub const CONNECT_RETRY_SLEEP_SECS: u64 = 5;
/// Minimum PostgreSQL major version (RayRAG deployment baseline is 18.4).
pub const POSTGRES_MIN_MAJOR_VERSION: i32 = 14;

/// A document row: an arbitrary JSON object (a chunk document).
pub type DocRow = Map<String, Value>;

/// Conjunctive equality filter used by `update`/`delete`/`search`.
pub type FilterCondition = DocRow;

/// Stable row id (`doc_store_base` chunk `id` field).
pub fn row_id(row: &DocRow) -> Option<String> {
    row.get("id").and_then(Value::as_str).map(String::from)
}

/// Embedding vector extracted from a row's `embedding` JSON array.
pub fn row_embedding(row: &DocRow) -> Vec<f32> {
    row.get("embedding")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|value| value.as_f64().map(|f| f as f32))
                .collect()
        })
        .unwrap_or_default()
}

/// Sparse vector (doc_store_base.SparseVector): parallel index/value arrays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SparseVector {
    pub indices: Vec<i64>,
    pub values: Option<Vec<f64>>,
}

impl SparseVector {
    pub fn new(indices: Vec<i64>, values: Option<Vec<f64>>) -> Self {
        if let Some(values) = &values {
            assert_eq!(
                indices.len(),
                values.len(),
                "SparseVector indices/values length mismatch"
            );
        }
        Self { indices, values }
    }

    /// Dense dict form `{"0": v0, "3": v1, ...}` (doc_store_base.to_dict).
    pub fn to_dict(&self) -> Value {
        let mut map = Map::new();
        if let Some(values) = &self.values {
            for (index, value) in self.indices.iter().zip(values) {
                map.insert(index.to_string(), json!(value));
            }
        }
        Value::Object(map)
    }

    pub fn from_dict(d: &Value) -> Self {
        let indices = d
            .get("indices")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default();
        let values = d
            .get("values")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_f64).collect());
        Self::new(indices, values)
    }
}

/// Match expressions accepted by `DocStore::search` (doc_store_base
/// MatchTextExpr / MatchDenseExpr / MatchSparseExpr / FusionExpr).
#[derive(Debug, Clone)]
pub enum MatchExpr {
    Text {
        fields: Vec<String>,
        matching_text: String,
        topn: usize,
    },
    Dense {
        vector_column_name: String,
        embedding_data: Vec<f32>,
        embedding_data_type: String,
        distance_type: String,
        topn: usize,
    },
    Sparse {
        vector_column_name: String,
        sparse_data: SparseVector,
        distance_type: String,
        topn: usize,
    },
    Fusion {
        method: String,
        topn: usize,
    },
}

impl MatchExpr {
    pub fn text(fields: &[&str], matching_text: &str, topn: usize) -> Self {
        Self::Text {
            fields: fields.iter().map(|f| (*f).to_string()).collect(),
            matching_text: matching_text.to_string(),
            topn,
        }
    }

    pub fn dense(
        vector_column_name: &str,
        embedding_data: Vec<f32>,
        distance_type: &str,
        topn: usize,
    ) -> Self {
        Self::Dense {
            vector_column_name: vector_column_name.to_string(),
            embedding_data,
            embedding_data_type: "float".into(),
            distance_type: distance_type.to_string(),
            topn,
        }
    }

    pub fn fusion(method: &str, topn: usize) -> Self {
        Self::Fusion {
            method: method.to_string(),
            topn,
        }
    }
}

/// Order-by builder (doc_store_base.OrderByExpr): (field, descending) pairs.
#[derive(Debug, Clone, Default)]
pub struct OrderByExpr {
    pub fields: Vec<(String, bool)>,
}

impl OrderByExpr {
    pub fn asc(mut self, field: &str) -> Self {
        self.fields.push((field.to_string(), false));
        self
    }

    pub fn desc(mut self, field: &str) -> Self {
        self.fields.push((field.to_string(), true));
        self
    }
}

/// Search request (doc_store_base.DocStoreConnection.search signature).
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub select_fields: Vec<String>,
    pub highlight_fields: Vec<String>,
    pub condition: FilterCondition,
    pub match_expressions: Vec<MatchExpr>,
    pub order_by: OrderByExpr,
    pub offset: usize,
    pub limit: usize,
    pub index_names: Vec<String>,
    pub dataset_ids: Vec<String>,
    pub agg_fields: Vec<String>,
    pub rank_feature: Option<HashMap<String, f32>>,
}

/// Search response with total, paged docs and per-field aggregations.
#[derive(Debug, Clone, Default)]
pub struct SearchResponse {
    pub total: usize,
    pub docs: Vec<DocRow>,
    pub aggregations: HashMap<String, Vec<(String, usize)>>,
}

/// Health payload returned by `DocStore::health`.
#[derive(Debug, Clone, Serialize)]
pub struct HealthStatus {
    pub status: &'static str, // "green" | "red"
    pub version: Option<String>,
    pub error: Option<String>,
}

impl HealthStatus {
    pub fn green(version: impl Into<String>) -> Self {
        Self {
            status: "green",
            version: Some(version.into()),
            error: None,
        }
    }

    pub fn red(error: impl Into<String>) -> Self {
        Self {
            status: "red",
            version: None,
            error: Some(error.into()),
        }
    }
}

/// Document store contract — the Rust counterpart of
/// `doc_store_base.DocStoreConnection`.
pub trait DocStore: Send + Sync {
    /// Database type name ("memory", "postgresql", "zvec").
    fn db_type(&self) -> &'static str;
    /// Health status of the underlying database.
    fn health(&self) -> crate::Result<HealthStatus>;

    // ── Index operations ────────────────────────────────────────
    fn create_idx(
        &self,
        index_name: &str,
        dataset_id: &str,
        vector_size: usize,
    ) -> crate::Result<()>;
    fn delete_idx(&self, index_name: &str, dataset_id: &str) -> crate::Result<()>;
    fn index_exist(&self, index_name: &str, dataset_id: &str) -> crate::Result<bool>;

    // ── CRUD operations ─────────────────────────────────────────
    /// Bulk upsert; returns the ids of the stored rows.
    fn insert(
        &self,
        rows: &[DocRow],
        index_name: &str,
        dataset_id: &str,
    ) -> crate::Result<Vec<String>>;
    /// Get a single row by id.
    fn get(
        &self,
        data_id: &str,
        index_name: &str,
        dataset_ids: &[String],
    ) -> crate::Result<Option<DocRow>>;
    /// Update rows matching the conjunctive condition; returns whether any matched.
    fn update(
        &self,
        condition: &FilterCondition,
        new_value: &DocRow,
        index_name: &str,
        dataset_id: &str,
    ) -> crate::Result<bool>;
    /// Delete rows matching the conjunctive condition; returns the deleted count.
    fn delete(
        &self,
        condition: &FilterCondition,
        index_name: &str,
        dataset_id: &str,
    ) -> crate::Result<usize>;
    /// Search with match expressions, filtering, ordering and pagination.
    fn search(&self, query: &SearchQuery) -> crate::Result<SearchResponse>;

    /// Run raw SQL (only supported by the SQL backends).
    fn sql(&self, sql: &str, fetch_size: usize) -> crate::Result<Vec<Value>>;

    // ── Search-result helpers (doc_store_base) ──────────────────
    fn get_total(&self, res: &SearchResponse) -> usize {
        res.total
    }

    fn get_doc_ids(&self, res: &SearchResponse) -> Vec<String> {
        res.docs.iter().filter_map(row_id).collect()
    }

    fn get_fields(&self, res: &SearchResponse, fields: &[String]) -> HashMap<String, DocRow> {
        let mut out = HashMap::new();
        for doc in &res.docs {
            let Some(id) = row_id(doc) else {
                continue;
            };
            let mut selected = Map::new();
            for field in fields {
                if let Some(value) = doc.get(field) {
                    selected.insert(field.clone(), value.clone());
                }
            }
            out.insert(id, selected);
        }
        out
    }

    fn get_highlight(
        &self,
        res: &SearchResponse,
        keywords: &[String],
        field_name: &str,
    ) -> HashMap<String, String> {
        let docs: Vec<Value> = res
            .docs
            .iter()
            .map(|row| Value::Object(row.clone()))
            .collect();
        crate::memory::get_highlight_from_messages(&docs, keywords, field_name, None)
            .into_iter()
            .collect()
    }

    fn get_aggregation(&self, res: &SearchResponse, field_name: &str) -> Vec<(String, usize)> {
        let docs: Vec<Value> = res
            .docs
            .iter()
            .map(|row| Value::Object(row.clone()))
            .collect();
        crate::memory::aggregate_by_field(&docs, field_name)
    }
}

/// Internal stored row shared by the in-memory and PostgreSQL backends.
#[derive(Debug, Clone)]
struct StoredRow {
    id: String,
    dataset_id: String,
    index_name: String,
    row: DocRow,
    embedding: Vec<f32>,
}

fn condition_matches(row: &DocRow, condition: &FilterCondition) -> bool {
    condition
        .iter()
        .all(|(key, value)| row.get(key) == Some(value))
}

/// Lexical + dense + rank-feature scoring for a single row.
fn score_doc(row: &DocRow, embedding: &[f32], query: &SearchQuery) -> f32 {
    let mut score = 0.0f32;
    for expr in &query.match_expressions {
        match expr {
            MatchExpr::Text {
                fields,
                matching_text,
                topn: _,
            } => {
                if matching_text.is_empty() {
                    continue;
                }
                let terms: Vec<&str> = matching_text
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|term| !term.is_empty())
                    .collect();
                if terms.is_empty() {
                    continue;
                }
                for field in fields {
                    let Some(text) = row.get(field).and_then(Value::as_str) else {
                        continue;
                    };
                    let lower = text.to_lowercase();
                    for term in &terms {
                        score += lower.matches(&term.to_lowercase()).count() as f32;
                    }
                }
            }
            MatchExpr::Dense {
                embedding_data,
                distance_type,
                ..
            } => {
                if !embedding_data.is_empty() && embedding_data.len() == embedding.len() {
                    let similarity = cosine_similarity(embedding_data, embedding);
                    // distance_type "cosine": higher is better; "l2": invert.
                    score += if distance_type == "l2" {
                        1.0 / (1.0 + similarity.max(0.0))
                    } else {
                        similarity
                    };
                }
            }
            MatchExpr::Sparse { .. } => {
                // Sparse term scoring happens in the vector backend.
            }
            MatchExpr::Fusion { .. } => {
                // Fusion (RRF/weighted) is applied by the caller (search.rs
                // ragflow_rerank) across per-expression result lists.
            }
        }
    }
    if let Some(rank_feature) = &query.rank_feature {
        for key in ["kb_id", "id"] {
            if let Some(value) = row.get(key).and_then(Value::as_str)
                && let Some(boost) = rank_feature.get(value) {
                    score += boost;
                }
        }
    }
    score
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();
    let left_norm: f32 = left.iter().map(|v| v * v).sum::<f32>().sqrt();
    let right_norm: f32 = right.iter().map(|v| v * v).sum::<f32>().sqrt();
    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        dot / (left_norm * right_norm)
    }
}

/// Sort scored rows, apply pagination, then build the response (shared by
/// every backend so search semantics stay identical across them).
fn finalize_search(mut scored: Vec<(f32, StoredRow)>, query: &SearchQuery) -> SearchResponse {
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    let total = scored.len();
    let mut aggregations = HashMap::new();
    for field in &query.agg_fields {
        let docs: Vec<Value> = scored
            .iter()
            .map(|(_, row)| Value::Object(row.row.clone()))
            .collect();
        aggregations.insert(
            field.clone(),
            crate::memory::aggregate_by_field(&docs, field),
        );
    }
    let limit = if query.limit == 0 {
        usize::MAX
    } else {
        query.limit
    };
    let docs: Vec<DocRow> = scored
        .into_iter()
        .skip(query.offset)
        .take(limit)
        .map(|(_, row)| row.row)
        .collect();
    SearchResponse {
        total,
        docs,
        aggregations,
    }
}

// ── In-memory backend ───────────────────────────────────────────

/// In-memory document store — the portable default backend.
pub struct MemoryDocStore {
    rows: Mutex<Vec<StoredRow>>,
    indexes: Mutex<HashSet<(String, String)>>, // (index_name, dataset_id)
}

impl MemoryDocStore {
    pub fn new() -> Self {
        Self {
            rows: Mutex::new(Vec::new()),
            indexes: Mutex::new(HashSet::new()),
        }
    }

    pub fn len(&self) -> usize {
        self.rows.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for MemoryDocStore {
    fn default() -> Self {
        Self::new()
    }
}

impl DocStore for MemoryDocStore {
    fn db_type(&self) -> &'static str {
        "memory"
    }

    fn health(&self) -> crate::Result<HealthStatus> {
        Ok(HealthStatus::green("memory-1.0"))
    }

    fn create_idx(
        &self,
        index_name: &str,
        dataset_id: &str,
        _vector_size: usize,
    ) -> crate::Result<()> {
        self.indexes
            .lock()
            .unwrap()
            .insert((index_name.to_string(), dataset_id.to_string()));
        Ok(())
    }

    fn delete_idx(&self, index_name: &str, dataset_id: &str) -> crate::Result<()> {
        self.indexes
            .lock()
            .unwrap()
            .remove(&(index_name.to_string(), dataset_id.to_string()));
        Ok(())
    }

    fn index_exist(&self, index_name: &str, dataset_id: &str) -> crate::Result<bool> {
        Ok(self
            .indexes
            .lock()
            .unwrap()
            .contains(&(index_name.to_string(), dataset_id.to_string())))
    }

    fn insert(
        &self,
        rows: &[DocRow],
        index_name: &str,
        dataset_id: &str,
    ) -> crate::Result<Vec<String>> {
        let mut store = self.rows.lock().unwrap();
        let mut ids = Vec::with_capacity(rows.len());
        for row in rows {
            let id = row_id(row).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let embedding = row_embedding(row);
            store.retain(|stored| {
                !(stored.id == id
                    && stored.dataset_id == dataset_id
                    && stored.index_name == index_name)
            });
            store.push(StoredRow {
                id: id.clone(),
                dataset_id: dataset_id.to_string(),
                index_name: index_name.to_string(),
                row: row.clone(),
                embedding,
            });
            ids.push(id);
        }
        Ok(ids)
    }

    fn get(
        &self,
        data_id: &str,
        index_name: &str,
        dataset_ids: &[String],
    ) -> crate::Result<Option<DocRow>> {
        let store = self.rows.lock().unwrap();
        Ok(store
            .iter()
            .find(|stored| {
                stored.id == data_id
                    && stored.index_name == index_name
                    && (dataset_ids.is_empty() || dataset_ids.contains(&stored.dataset_id))
            })
            .map(|stored| stored.row.clone()))
    }

    fn update(
        &self,
        condition: &FilterCondition,
        new_value: &DocRow,
        index_name: &str,
        dataset_id: &str,
    ) -> crate::Result<bool> {
        let mut store = self.rows.lock().unwrap();
        let mut changed = false;
        for stored in store.iter_mut() {
            if stored.index_name == index_name
                && stored.dataset_id == dataset_id
                && condition_matches(&stored.row, condition)
            {
                for (key, value) in new_value {
                    stored.row.insert(key.clone(), value.clone());
                }
                changed = true;
            }
        }
        Ok(changed)
    }

    fn delete(
        &self,
        condition: &FilterCondition,
        index_name: &str,
        dataset_id: &str,
    ) -> crate::Result<usize> {
        let mut store = self.rows.lock().unwrap();
        let before = store.len();
        store.retain(|stored| {
            !(stored.index_name == index_name
                && stored.dataset_id == dataset_id
                && condition_matches(&stored.row, condition))
        });
        Ok(before - store.len())
    }

    fn search(&self, query: &SearchQuery) -> crate::Result<SearchResponse> {
        let store = self.rows.lock().unwrap();
        let scored: Vec<(f32, StoredRow)> = store
            .iter()
            .filter(|stored| {
                (query.dataset_ids.is_empty() || query.dataset_ids.contains(&stored.dataset_id))
                    && (query.index_names.is_empty()
                        || query
                            .index_names
                            .iter()
                            .any(|name| name == &stored.index_name))
                    && condition_matches(&stored.row, &query.condition)
            })
            .map(|stored| {
                (
                    score_doc(&stored.row, &stored.embedding, query),
                    stored.clone(),
                )
            })
            .filter(|(score, _)| query.match_expressions.is_empty() || *score > 0.0)
            .collect();
        Ok(finalize_search(scored, query))
    }

    fn sql(&self, _sql: &str, _fetch_size: usize) -> crate::Result<Vec<Value>> {
        anyhow::bail!("SQL is not supported by the in-memory doc store")
    }
}

// ── PostgreSQL backend ──────────────────────────────────────────

/// PostgreSQL-backed doc store — the durable backend.
///
/// Rows live in `doc_store_chunks` as JSONB with an embedding column; index
/// bookkeeping lives in `doc_store_indexes`. The client is guarded by a mutex
/// so the pool can health-check and refresh it like `es_conn_pool` does with
/// the single Elasticsearch connection.
#[cfg(feature = "postgres-backend")]
pub struct PostgresDocStore {
    client: Mutex<postgres::Client>,
}

#[cfg(feature = "postgres-backend")]
impl PostgresDocStore {
    pub fn from_config(url: &str) -> crate::Result<Self> {
        let mut client = postgres::Client::connect(url, postgres::NoTls)?;
        client.batch_execute(
            "CREATE TABLE IF NOT EXISTS doc_store_chunks (
                id TEXT NOT NULL,
                dataset_id TEXT NOT NULL,
                index_name TEXT NOT NULL,
                row JSONB NOT NULL,
                embedding JSONB,
                PRIMARY KEY (dataset_id, index_name, id)
             );
             CREATE TABLE IF NOT EXISTS doc_store_indexes (
                index_name TEXT NOT NULL,
                dataset_id TEXT NOT NULL,
                vector_size INT NOT NULL,
                created_at BIGINT NOT NULL DEFAULT 0,
                PRIMARY KEY (index_name, dataset_id)
             );",
        )?;
        Ok(Self {
            client: Mutex::new(client),
        })
    }

    pub fn from_env() -> crate::Result<Self> {
        let url = std::env::var("RAYRAG_POSTGRES_URL")?;
        Self::from_config(&url)
    }

    /// Shared fetch path: `SELECT id, dataset_id, index_name, row, embedding ...`.
    fn query_rows(
        &self,
        sql: &str,
        params: &[&(dyn postgres::types::ToSql + Sync)],
    ) -> crate::Result<Vec<StoredRow>> {
        let mut client = self.client.lock().unwrap();
        let rows = client.query(sql, params)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.get(0);
            let dataset_id: String = row.get(1);
            let index_name: String = row.get(2);
            let row_value: Value = row.get(3);
            let embedding: Option<Value> = row.get(4);
            let embedding = embedding
                .and_then(|value| {
                    value.as_array().map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_f64().map(|f| f as f32))
                            .collect()
                    })
                })
                .unwrap_or_default();
            out.push(StoredRow {
                id,
                dataset_id,
                index_name,
                row: row_value.as_object().cloned().unwrap_or_default(),
                embedding,
            });
        }
        Ok(out)
    }
}

#[cfg(feature = "postgres-backend")]
impl DocStore for PostgresDocStore {
    fn db_type(&self) -> &'static str {
        "postgresql"
    }

    fn health(&self) -> crate::Result<HealthStatus> {
        let mut client = self.client.lock().unwrap();
        let version: String = client.query_one("SHOW server_version", &[])?.get(0);
        let major = version
            .split('.')
            .next()
            .and_then(|part| part.parse::<i32>().ok())
            .unwrap_or(0);
        if major < POSTGRES_MIN_MAJOR_VERSION {
            return Ok(HealthStatus::red(format!(
                "PostgreSQL {major} < required {POSTGRES_MIN_MAJOR_VERSION}"
            )));
        }
        Ok(HealthStatus::green(format!("postgresql-{version}")))
    }

    fn create_idx(
        &self,
        index_name: &str,
        dataset_id: &str,
        vector_size: usize,
    ) -> crate::Result<()> {
        let mut client = self.client.lock().unwrap();
        client.execute(
            "INSERT INTO doc_store_indexes (index_name, dataset_id, vector_size, created_at)
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
            &[
                &index_name,
                &dataset_id,
                &(vector_size as i64),
                &(std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0)),
            ],
        )?;
        Ok(())
    }

    fn delete_idx(&self, index_name: &str, dataset_id: &str) -> crate::Result<()> {
        let mut client = self.client.lock().unwrap();
        client.execute(
            "DELETE FROM doc_store_indexes WHERE index_name = $1 AND dataset_id = $2",
            &[&index_name, &dataset_id],
        )?;
        client.execute(
            "DELETE FROM doc_store_chunks WHERE index_name = $1 AND dataset_id = $2",
            &[&index_name, &dataset_id],
        )?;
        Ok(())
    }

    fn index_exist(&self, index_name: &str, dataset_id: &str) -> crate::Result<bool> {
        let mut client = self.client.lock().unwrap();
        let exists: bool = client.query_one(
            "SELECT EXISTS(SELECT 1 FROM doc_store_indexes WHERE index_name = $1 AND dataset_id = $2)",
            &[&index_name, &dataset_id],
        )?.get(0);
        Ok(exists)
    }

    fn insert(
        &self,
        rows: &[DocRow],
        index_name: &str,
        dataset_id: &str,
    ) -> crate::Result<Vec<String>> {
        let mut client = self.client.lock().unwrap();
        let mut ids = Vec::with_capacity(rows.len());
        for row in rows {
            let id = row_id(row).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let embedding = serde_json::to_value(row_embedding(row))?;
            let row_value = Value::Object(row.clone());
            client.execute(
                "INSERT INTO doc_store_chunks (id, dataset_id, index_name, row, embedding)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (dataset_id, index_name, id)
                 DO UPDATE SET row = EXCLUDED.row, embedding = EXCLUDED.embedding",
                &[&id, &dataset_id, &index_name, &row_value, &embedding],
            )?;
            ids.push(id);
        }
        Ok(ids)
    }

    fn get(
        &self,
        data_id: &str,
        index_name: &str,
        dataset_ids: &[String],
    ) -> crate::Result<Option<DocRow>> {
        let mut client = self.client.lock().unwrap();
        let row = client.query_opt(
            "SELECT row FROM doc_store_chunks WHERE id = $1 AND index_name = $2 AND dataset_id = ANY($3)",
            &[&data_id, &index_name, &dataset_ids],
        )?;
        Ok(row
            .and_then(|row| row.try_get::<_, Value>(0).ok())
            .and_then(|value| value.as_object().cloned()))
    }

    fn update(
        &self,
        condition: &FilterCondition,
        new_value: &DocRow,
        index_name: &str,
        dataset_id: &str,
    ) -> crate::Result<bool> {
        let condition_value = Value::Object(condition.clone());
        let new_value = Value::Object(new_value.clone());
        let mut client = self.client.lock().unwrap();
        let affected = client.execute(
            "UPDATE doc_store_chunks
             SET row = row || $4::jsonb
             WHERE dataset_id = $1 AND index_name = $2 AND row @> $3::jsonb",
            &[&dataset_id, &index_name, &condition_value, &new_value],
        )?;
        Ok(affected > 0)
    }

    fn delete(
        &self,
        condition: &FilterCondition,
        index_name: &str,
        dataset_id: &str,
    ) -> crate::Result<usize> {
        let condition_value = Value::Object(condition.clone());
        let mut client = self.client.lock().unwrap();
        let affected = client.execute(
            "DELETE FROM doc_store_chunks WHERE dataset_id = $1 AND index_name = $2 AND row @> $3::jsonb",
            &[&dataset_id, &index_name, &condition_value],
        )?;
        Ok(affected as usize)
    }

    fn search(&self, query: &SearchQuery) -> crate::Result<SearchResponse> {
        let condition_value = Value::Object(query.condition.clone());
        let rows = if query.index_names.is_empty() {
            self.query_rows(
                "SELECT id, dataset_id, index_name, row, embedding
                 FROM doc_store_chunks
                 WHERE dataset_id = ANY($1) AND row @> $2::jsonb",
                &[&query.dataset_ids, &condition_value],
            )?
        } else {
            self.query_rows(
                "SELECT id, dataset_id, index_name, row, embedding
                 FROM doc_store_chunks
                 WHERE dataset_id = ANY($1) AND index_name = ANY($2) AND row @> $3::jsonb",
                &[&query.dataset_ids, &query.index_names, &condition_value],
            )?
        };
        let scored: Vec<(f32, StoredRow)> = rows
            .into_iter()
            .map(|stored| (score_doc(&stored.row, &stored.embedding, query), stored))
            .filter(|(score, _)| query.match_expressions.is_empty() || *score > 0.0)
            .collect();
        Ok(finalize_search(scored, query))
    }

    fn sql(&self, sql: &str, fetch_size: usize) -> crate::Result<Vec<Value>> {
        let mut client = self.client.lock().unwrap();
        let rows = client.query(sql, &[])?;
        let mut out = Vec::new();
        for row in rows.iter().take(fetch_size.max(1)) {
            let mut map = Map::new();
            for (index, column) in row.columns().iter().enumerate() {
                let value: Value = row.try_get(index).unwrap_or(Value::Null);
                map.insert(column.name().to_string(), value);
            }
            out.push(Value::Object(map));
        }
        Ok(out)
    }
}

// ── zvec backend ────────────────────────────────────────────────

/// zvec-backed doc store — mirrors rows into a native zvec collection for
/// dense search while keeping the JSON metadata contract in the row cache.
#[cfg(feature = "zvec-backend")]
pub struct ZvecDocStore {
    memory: MemoryDocStore,
    zvec: Mutex<crate::store::ZvecStore>,
    dimension: usize,
}

#[cfg(feature = "zvec-backend")]
impl ZvecDocStore {
    pub fn new(collection: &str, dimension: usize) -> crate::Result<Self> {
        Ok(Self {
            memory: MemoryDocStore::new(),
            zvec: Mutex::new(crate::store::ZvecStore::new(collection, dimension)?),
            dimension,
        })
    }
}

#[cfg(feature = "zvec-backend")]
fn row_to_chunk(row: &DocRow, _dimension: usize) -> Option<crate::Chunk> {
    let id = row_id(row)?;
    let content = row
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let doc_id = row
        .get("doc_id")
        .and_then(Value::as_str)
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .unwrap_or_else(uuid::Uuid::nil);
    let position = row.get("position").and_then(Value::as_u64).unwrap_or(0) as usize;
    let token_count = row.get("token_count").and_then(Value::as_u64).unwrap_or(0) as usize;
    let embedding = row_embedding(row);
    let embedding = (!embedding.is_empty()).then_some(embedding);
    let metadata = row
        .iter()
        .filter_map(|(key, value)| value.as_str().map(|text| (key.clone(), text.to_string())))
        .collect();
    Some(crate::Chunk {
        id,
        content,
        content_type: "text".into(),
        doc_id,
        position,
        token_count,
        embedding,
        metadata,
    })
}

#[cfg(feature = "zvec-backend")]
impl DocStore for ZvecDocStore {
    fn db_type(&self) -> &'static str {
        "zvec"
    }

    fn health(&self) -> crate::Result<HealthStatus> {
        let zvec = self.zvec.lock().unwrap();
        Ok(HealthStatus::green(format!("zvec-{}", zvec.backend_name())))
    }

    fn create_idx(
        &self,
        index_name: &str,
        dataset_id: &str,
        vector_size: usize,
    ) -> crate::Result<()> {
        self.memory.create_idx(index_name, dataset_id, vector_size)
    }

    fn delete_idx(&self, index_name: &str, dataset_id: &str) -> crate::Result<()> {
        self.memory.delete_idx(index_name, dataset_id)
    }

    fn index_exist(&self, index_name: &str, dataset_id: &str) -> crate::Result<bool> {
        self.memory.index_exist(index_name, dataset_id)
    }

    fn insert(
        &self,
        rows: &[DocRow],
        index_name: &str,
        dataset_id: &str,
    ) -> crate::Result<Vec<String>> {
        let ids = self.memory.insert(rows, index_name, dataset_id)?;
        let chunks: Vec<crate::Chunk> = rows
            .iter()
            .filter_map(|row| row_to_chunk(row, self.dimension))
            .collect();
        if !chunks.is_empty() {
            let zvec = self.zvec.lock().unwrap();
            zvec.insert(&chunks)?;
        }
        Ok(ids)
    }

    fn get(
        &self,
        data_id: &str,
        index_name: &str,
        dataset_ids: &[String],
    ) -> crate::Result<Option<DocRow>> {
        self.memory.get(data_id, index_name, dataset_ids)
    }

    fn update(
        &self,
        condition: &FilterCondition,
        new_value: &DocRow,
        index_name: &str,
        dataset_id: &str,
    ) -> crate::Result<bool> {
        self.memory
            .update(condition, new_value, index_name, dataset_id)
    }

    fn delete(
        &self,
        condition: &FilterCondition,
        index_name: &str,
        dataset_id: &str,
    ) -> crate::Result<usize> {
        self.memory.delete(condition, index_name, dataset_id)
    }

    fn search(&self, query: &SearchQuery) -> crate::Result<SearchResponse> {
        // Boost dense scores with the native zvec index when a dense
        // expression is present; fall back to the in-memory scoring otherwise.
        let dense = query.match_expressions.iter().find_map(|expr| match expr {
            MatchExpr::Dense { embedding_data, .. } if !embedding_data.is_empty() => {
                Some(embedding_data)
            }
            _ => None,
        });
        if let Some(embedding) = dense {
            let zvec = self.zvec.lock().unwrap();
            if let Ok(hits) = zvec.search(embedding, query.limit.max(64)) {
                let mut boosted = query.clone();
                let mut features = boosted.rank_feature.take().unwrap_or_default();
                for hit in hits {
                    features.insert(hit.chunk.id.clone(), hit.score);
                }
                boosted.rank_feature = Some(features);
                return self.memory.search(&boosted);
            }
        }
        self.memory.search(query)
    }

    fn sql(&self, _sql: &str, _fetch_size: usize) -> crate::Result<Vec<Value>> {
        anyhow::bail!("SQL is not supported by the zvec doc store")
    }
}

// ── Connection pool (es_conn_pool semantics) ────────────────────

/// Selectable pool backend.
#[derive(Debug, Clone)]
pub enum DocStoreBackend {
    Memory,
    Postgres { url: String },
}

/// Lazy connection pool mirroring `es_conn_pool.ElasticSearchConnectionPool`:
/// a single shared connection created on first use, `ATTEMPT_TIME` retries
/// with `CONNECT_RETRY_SLEEP_SECS` between attempts, a mandatory health check
/// before the connection is handed out, and an explicit `refresh_conn` that
/// reconnects when the current connection is unhealthy.
pub struct DocStorePool {
    backend: DocStoreBackend,
    inner: RwLock<Option<Arc<dyn DocStore>>>,
}

impl DocStorePool {
    pub fn memory() -> Self {
        Self {
            backend: DocStoreBackend::Memory,
            inner: RwLock::new(None),
        }
    }

    /// Backend from the environment: `RAYRAG_POSTGRES_URL` selects PostgreSQL
    /// (when built with `--features postgres-backend`), otherwise memory.
    pub fn from_env() -> Self {
        match std::env::var("RAYRAG_POSTGRES_URL") {
            Ok(url) if !url.trim().is_empty() => Self {
                backend: DocStoreBackend::Postgres { url },
                inner: RwLock::new(None),
            },
            _ => Self::memory(),
        }
    }

    pub fn backend_name(&self) -> &'static str {
        match &self.backend {
            DocStoreBackend::Memory => "memory",
            DocStoreBackend::Postgres { .. } => "postgresql",
        }
    }

    fn connect(&self) -> crate::Result<Arc<dyn DocStore>> {
        match &self.backend {
            DocStoreBackend::Memory => Ok(Arc::new(MemoryDocStore::new())),
            DocStoreBackend::Postgres { url: _url } => {
                #[cfg(feature = "postgres-backend")]
                {
                    Ok(Arc::new(PostgresDocStore::from_config(_url)?))
                }
                #[cfg(not(feature = "postgres-backend"))]
                {
                    anyhow::bail!(
                        "RayRAG was built without --features postgres-backend; \
                         cannot open the PostgreSQL doc store"
                    )
                }
            }
        }
    }

    /// Get the shared connection, connecting on first use with bounded retries.
    pub fn get_conn(&self) -> crate::Result<Arc<dyn DocStore>> {
        if let Some(conn) = self.inner.read().unwrap().as_ref()
            && conn.health().map(|h| h.status == "green").unwrap_or(false) {
                return Ok(conn.clone());
            }
        let mut guard = self.inner.write().unwrap();
        if let Some(conn) = guard.as_ref()
            && conn.health().map(|h| h.status == "green").unwrap_or(false) {
                return Ok(conn.clone());
            }
        let mut last_error = None;
        for attempt in 0..ATTEMPT_TIME {
            match self.connect() {
                Ok(conn) => match conn.health() {
                    Ok(health) if health.status == "green" => {
                        *guard = Some(conn.clone());
                        tracing::info!(
                            backend = self.backend_name(),
                            "doc store connection established"
                        );
                        return Ok(conn);
                    }
                    Ok(health) => {
                        last_error =
                            Some(anyhow::anyhow!("doc store unhealthy: {:?}", health.error));
                    }
                    Err(error) => last_error = Some(error),
                },
                Err(error) => last_error = Some(error),
            }
            if attempt + 1 < ATTEMPT_TIME {
                std::thread::sleep(std::time::Duration::from_secs(CONNECT_RETRY_SLEEP_SECS));
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("failed to connect doc store")))
    }

    /// Close and reconnect when the current connection is unhealthy
    /// (es_conn_pool.refresh_conn).
    pub fn refresh_conn(&self) -> crate::Result<Arc<dyn DocStore>> {
        {
            let guard = self.inner.read().unwrap();
            if let Some(conn) = guard.as_ref()
                && conn.health().map(|h| h.status == "green").unwrap_or(false) {
                    return Ok(conn.clone());
                }
        }
        self.inner.write().unwrap().take();
        self.get_conn()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, content: &str) -> DocRow {
        let mut row = Map::new();
        row.insert("id".into(), json!(id));
        row.insert("content".into(), json!(content));
        row
    }

    fn text_query(terms: &str) -> SearchQuery {
        SearchQuery {
            match_expressions: vec![MatchExpr::text(&["content"], terms, 10)],
            limit: 10,
            ..Default::default()
        }
    }

    #[test]
    fn memory_doc_store_insert_search_delete() {
        let store = MemoryDocStore::new();
        let ids = store
            .insert(
                &[
                    row("c1", "RayRAG is a RAG engine written in Rust."),
                    row("c2", "Vector search over embedded chunks."),
                    row("c3", "RayRAG highlights matches with em tags."),
                ],
                "idx1",
                "kb1",
            )
            .unwrap();
        assert_eq!(ids, vec!["c1", "c2", "c3"]);

        // Dataset isolation: only kb1 rows are visible.
        let mut query = text_query("rayrag");
        query.dataset_ids = vec!["kb1".into()];
        let res = store.search(&query).unwrap();
        assert_eq!(res.total, 2);
        assert_eq!(res.docs[0]["id"], json!("c1")); // more term hits -> first

        let mut query = text_query("rayrag");
        query.dataset_ids = vec!["kb2".into()];
        assert_eq!(store.search(&query).unwrap().total, 0);

        // Index bookkeeping.
        store.create_idx("idx1", "kb1", 384).unwrap();
        assert!(store.index_exist("idx1", "kb1").unwrap());
        store.delete_idx("idx1", "kb1").unwrap();
        assert!(!store.index_exist("idx1", "kb1").unwrap());

        // Delete by conjunctive condition.
        let mut condition = Map::new();
        condition.insert("id".into(), json!("c2"));
        assert_eq!(store.delete(&condition, "idx1", "kb1").unwrap(), 1);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn memory_doc_store_update_and_get() {
        let store = MemoryDocStore::new();
        store
            .insert(&[row("c1", "original")], "idx1", "kb1")
            .unwrap();

        let mut condition = Map::new();
        condition.insert("id".into(), json!("c1"));
        let mut update = Map::new();
        update.insert("content".into(), json!("updated"));
        assert!(store.update(&condition, &update, "idx1", "kb1").unwrap());
        assert_eq!(
            store.get("c1", "idx1", &["kb1".into()]).unwrap().unwrap()["content"],
            json!("updated")
        );

        // Non-matching condition -> false, no change.
        let mut missed = Map::new();
        missed.insert("id".into(), json!("nope"));
        assert!(!store.update(&missed, &update, "idx1", "kb1").unwrap());

        // Dataset-scoped get.
        assert!(
            store
                .get("c1", "idx1", &["other".into()])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn search_response_helpers_match_doc_store_base() {
        let store = MemoryDocStore::new();
        let res = SearchResponse {
            total: 2,
            docs: vec![
                row("c1", "Rust is a systems language."),
                row("c2", "Nothing relevant here."),
            ],
            aggregations: HashMap::new(),
        };

        assert_eq!(store.get_total(&res), 2);
        assert_eq!(store.get_doc_ids(&res), vec!["c1", "c2"]);

        let fields = store.get_fields(&res, &["content".into()]);
        assert_eq!(
            fields["c1"]["content"],
            json!("Rust is a systems language.")
        );

        let highlights = store.get_highlight(&res, &["rust".into()], "content");
        assert_eq!(highlights.len(), 1);
        assert!(highlights["c1"].contains("<em>Rust</em>"));

        let mut tagged = res;
        tagged.docs[0].insert("category".into(), json!("tech"));
        tagged.docs[1].insert("category".into(), json!("other"));
        let mut third = row("c3", "Another Rust chunk.");
        third.insert("category".into(), json!("tech"));
        tagged.docs.push(third);
        let aggregation = store.get_aggregation(&tagged, "category");
        assert_eq!(
            aggregation,
            vec![("tech".to_string(), 2), ("other".to_string(), 1)]
        );
    }

    #[test]
    fn doc_store_pool_memory_backend_acquire_and_refresh() {
        let pool = DocStorePool::memory();
        assert_eq!(pool.backend_name(), "memory");
        let conn = pool.get_conn().unwrap();
        assert_eq!(conn.db_type(), "memory");
        conn.insert(&[row("c1", "pooled row")], "idx1", "kb1")
            .unwrap();

        // Same shared connection is returned on repeated acquire.
        let again = pool.get_conn().unwrap();
        assert!(Arc::ptr_eq(&conn, &again));
        assert_eq!(
            again.get("c1", "idx1", &["kb1".into()]).unwrap().is_some(),
            true
        );

        // refresh_conn keeps a healthy connection untouched.
        let refreshed = pool.refresh_conn().unwrap();
        assert_eq!(
            refreshed
                .get("c1", "idx1", &["kb1".into()])
                .unwrap()
                .is_some(),
            true
        );
    }
}
