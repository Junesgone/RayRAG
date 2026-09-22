//! Vector storage backed by either the built-in JSON index or zvec.
//!
//! The JSON backend is the portable default. Build with `zvec-backend` and set
//! `RAYRAG_VECTOR_BACKEND=zvec` to use the native `zvec-rust` collection.

pub mod infinity_chunk;
pub mod infinity_query;
pub mod infinity_table;
pub mod search_mapping;

use crate::search::{IndexedChunk, SearchEngine};
use crate::{Chunk, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[cfg(feature = "zvec-backend")]
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
#[cfg(feature = "zvec-backend")]
use std::sync::{Arc, Mutex};

const VECTOR_BACKEND_ENV: &str = "RAYRAG_VECTOR_BACKEND";

/// zvec is RayRAG's vector backend, so it is the default as soon as the native
/// library is compiled in; a build without `zvec-backend` keeps using the
/// portable JSON index (which is also how you roll back).
pub fn default_vector_backend() -> &'static str {
    if cfg!(feature = "zvec-backend") {
        "zvec"
    } else {
        "json"
    }
}
#[cfg(feature = "zvec-backend")]
const ZVEC_ROOT_ENV: &str = "RAYRAG_ZVEC_DIR";

enum VectorBackend {
    Json {
        engine: RwLock<SearchEngine>,
        index_path: PathBuf,
    },
    #[cfg(feature = "zvec-backend")]
    Zvec(zvec_backend::ZvecCollection),
}

/// Vector store with a stable API across the portable and native backends.
pub struct ZvecStore {
    collection: String,
    dimension: usize,
    backend: VectorBackend,
}

/// Search result with score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub chunk: Chunk,
    pub score: f32,
}

/// Optional native zvec mirror for the HTTP search index.
///
/// RayRAG keeps the JSON `SearchEngine` as the hybrid-search source because it
/// also owns BM25, metadata filters, and rank features. When enabled, this
/// mirror writes the same vector-bearing chunks into one zvec collection per
/// knowledge base so different embedding spaces never share a collection.
pub struct OnlineVectorMirror {
    #[cfg(feature = "zvec-backend")]
    root: Option<PathBuf>,
    #[cfg(feature = "zvec-backend")]
    collections: Mutex<HashMap<String, Arc<zvec_backend::ZvecCollection>>>,
}

impl OnlineVectorMirror {
    pub fn from_env() -> Result<Self> {
        Self::from_env_subdirectory("online")
    }

    /// Build the independent native mirror used by the Skill index. Keeping
    /// Skill collections under their own root prevents the online knowledge
    /// base reconciliation pass from treating them as stale collections.
    pub(crate) fn for_skill_index() -> Result<Self> {
        Self::from_env_subdirectory("skills")
    }

    fn from_env_subdirectory(_subdirectory: &str) -> Result<Self> {
        let backend = std::env::var(VECTOR_BACKEND_ENV)
            .unwrap_or_else(|_| default_vector_backend().to_string())
            .trim()
            .to_ascii_lowercase();
        match backend.as_str() {
            "json" => Ok(Self::disabled()),
            "zvec" => {
                #[cfg(feature = "zvec-backend")]
                {
                    let root = std::env::var(ZVEC_ROOT_ENV)
                        .map(PathBuf::from)
                        .unwrap_or_else(|_| PathBuf::from("zvec-data"));
                    let root = root.join(_subdirectory);
                    std::fs::create_dir_all(&root)?;
                    if !std::fs::metadata(&root)?.is_dir() {
                        anyhow::bail!("zvec root is not a directory: {}", root.display());
                    }
                    Ok(Self {
                        root: Some(root),
                        collections: Mutex::new(HashMap::new()),
                    })
                }
                #[cfg(not(feature = "zvec-backend"))]
                {
                    anyhow::bail!("RAYRAG_VECTOR_BACKEND=zvec requires --features zvec-backend")
                }
            }
            value => {
                anyhow::bail!("Unsupported vector backend '{value}'; expected 'json' or 'zvec'")
            }
        }
    }

    pub fn disabled() -> Self {
        Self {
            #[cfg(feature = "zvec-backend")]
            root: None,
            #[cfg(feature = "zvec-backend")]
            collections: Mutex::new(HashMap::new()),
        }
    }

    pub fn is_enabled(&self) -> bool {
        #[cfg(feature = "zvec-backend")]
        {
            self.root.is_some()
        }
        #[cfg(not(feature = "zvec-backend"))]
        {
            false
        }
    }

    /// Validate the zvec storage directory and every currently opened native
    /// collection. Returns `Ok(false)` when the optional backend is disabled.
    pub fn health(&self) -> Result<bool> {
        #[cfg(feature = "zvec-backend")]
        {
            let Some(root) = self.root.as_ref() else {
                return Ok(false);
            };
            if !std::fs::metadata(root)?.is_dir() {
                anyhow::bail!("zvec root is not a directory: {}", root.display());
            }
            let collections = self
                .collections
                .lock()
                .map_err(|_| anyhow::anyhow!("zvec online collection cache lock poisoned"))?;
            for collection in collections.values() {
                collection.len()?;
            }
            Ok(true)
        }
        #[cfg(not(feature = "zvec-backend"))]
        {
            Ok(false)
        }
    }

    /// Synchronize the native mirror to `current`. On failure, every affected
    /// collection is restored to `previous` before the error is returned.
    pub fn sync_snapshot(&self, previous: &[IndexedChunk], current: &[IndexedChunk]) -> Result<()> {
        #[cfg(feature = "zvec-backend")]
        {
            if self.root.is_none() {
                return Ok(());
            }
            let previous_by_kb = chunks_by_kb(previous)?;
            let current_by_kb = chunks_by_kb(current)?;
            let kb_ids: HashSet<_> = previous_by_kb
                .keys()
                .chain(current_by_kb.keys())
                .cloned()
                .collect();
            let mut applied: Vec<String> = Vec::new();
            for kb_id in kb_ids {
                let before = previous_by_kb.get(&kb_id).map(Vec::as_slice).unwrap_or(&[]);
                let after = current_by_kb.get(&kb_id).map(Vec::as_slice).unwrap_or(&[]);
                if let Err(error) = self.replace_collection(&kb_id, before, after) {
                    if let Err(rollback_error) = self.replace_collection(&kb_id, after, before) {
                        tracing::error!(
                            kb_id = %kb_id,
                            %rollback_error,
                            "Failed to roll back failing zvec online collection"
                        );
                    }
                    for applied_kb in applied.into_iter().rev() {
                        let applied_before = previous_by_kb
                            .get(&applied_kb)
                            .map(Vec::as_slice)
                            .unwrap_or(&[]);
                        let applied_after = current_by_kb
                            .get(&applied_kb)
                            .map(Vec::as_slice)
                            .unwrap_or(&[]);
                        if let Err(rollback_error) =
                            self.replace_collection(&applied_kb, applied_after, applied_before)
                        {
                            tracing::error!(
                                kb_id = %applied_kb,
                                %rollback_error,
                                "Failed to roll back zvec online collection"
                            );
                        }
                    }
                    return Err(error);
                }
                applied.push(kb_id);
            }
            Ok(())
        }
        #[cfg(not(feature = "zvec-backend"))]
        {
            let _ = (previous, current);
            Ok(())
        }
    }

    /// Rebuild native collections represented by the JSON source of truth.
    /// This removes stale primary keys left by an interrupted process before
    /// the HTTP server starts accepting requests.
    pub fn reconcile_snapshot(&self, current: &[IndexedChunk]) -> Result<()> {
        #[cfg(feature = "zvec-backend")]
        {
            let Some(root) = self.root.as_ref() else {
                return Ok(());
            };
            let current_by_kb = chunks_by_kb(current)?;
            let current_dirs: HashSet<_> = current_by_kb
                .keys()
                .map(|kb_id| safe_collection_name(kb_id))
                .collect();
            let cached_directories: HashSet<_> = {
                let collections = self
                    .collections
                    .lock()
                    .map_err(|_| anyhow::anyhow!("zvec online collection cache lock poisoned"))?;
                for (kb_id, collection) in collections.iter() {
                    if !current_by_kb.contains_key(kb_id) {
                        collection.delete_all()?;
                        collection.flush()?;
                    }
                }
                collections
                    .keys()
                    .map(|kb_id| safe_collection_name(kb_id))
                    .collect()
            };
            if root.exists() {
                for entry in std::fs::read_dir(root)? {
                    let entry = entry?;
                    if !entry.file_type()?.is_dir() {
                        continue;
                    }
                    let directory = entry.file_name().to_string_lossy().into_owned();
                    if current_dirs.contains(&directory) || cached_directories.contains(&directory)
                    {
                        continue;
                    }
                    let collection = zvec_backend::ZvecCollection::open(
                        &entry.path(),
                        &format!("stale_{directory}"),
                        1,
                    )?;
                    collection.delete_all()?;
                    collection.flush()?;
                }
            }
            for (kb_id, chunks) in current_by_kb {
                let Some(dimension) = vector_dimension(&chunks) else {
                    continue;
                };
                validate_dimensions(&kb_id, &chunks, dimension)?;
                let collection = self.collection(&kb_id, dimension)?;
                collection.delete_all()?;
                collection.upsert(&chunks)?;
                collection.flush()?;
            }
            Ok(())
        }
        #[cfg(not(feature = "zvec-backend"))]
        {
            let _ = current;
            Ok(())
        }
    }

    #[cfg(all(test, feature = "zvec-backend"))]
    fn native(root: PathBuf) -> Self {
        Self {
            root: Some(root),
            collections: Mutex::new(HashMap::new()),
        }
    }

    #[cfg(feature = "zvec-backend")]
    fn replace_collection(
        &self,
        kb_id: &str,
        previous: &[IndexedChunk],
        current: &[IndexedChunk],
    ) -> Result<()> {
        let Some(dimension) = vector_dimension(current).or_else(|| vector_dimension(previous))
        else {
            return Ok(());
        };
        validate_dimensions(kb_id, previous, dimension)?;
        validate_dimensions(kb_id, current, dimension)?;
        let collection = self.collection(kb_id, dimension)?;
        let current_ids: HashSet<_> = current.iter().map(|chunk| chunk.id.as_str()).collect();
        let removed: Vec<_> = previous
            .iter()
            .filter(|chunk| !current_ids.contains(chunk.id.as_str()))
            .map(|chunk| chunk.id.as_str())
            .collect();
        if !removed.is_empty() {
            collection.delete(&removed)?;
        }
        // 仅 upsert 新增或内容/向量变更的 chunk（避免全量重写）
        let before_ids: HashSet<_> = previous.iter().map(|chunk| chunk.id.as_str()).collect();
        let changed: Vec<IndexedChunk> = current
            .iter()
            .filter(|chunk| {
                !before_ids.contains(chunk.id.as_str())
                    || previous
                        .iter()
                        .find(|before| before.id == chunk.id)
                        .map(|before| {
                            before.doc_name != chunk.doc_name
                                || before.content != chunk.content
                                || before.embedding != chunk.embedding
                                || before.token_count != chunk.token_count
                                || before.position != chunk.position
                                || before.metadata != chunk.metadata
                        })
                        .unwrap_or(true)
            })
            .cloned()
            .collect();
        if !changed.is_empty() {
            collection.upsert(&changed)?;
        }
        collection.flush()
    }

    #[cfg(feature = "zvec-backend")]
    fn collection(
        &self,
        kb_id: &str,
        dimension: usize,
    ) -> Result<Arc<zvec_backend::ZvecCollection>> {
        let mut collections = self
            .collections
            .lock()
            .map_err(|_| anyhow::anyhow!("zvec online collection cache lock poisoned"))?;
        if let Some(collection) = collections.get(kb_id) {
            return Ok(collection.clone());
        }
        let root = self.root.as_ref().expect("enabled zvec mirror has a root");
        let path = root.join(safe_collection_name(kb_id));
        let collection = Arc::new(zvec_backend::ZvecCollection::open(
            &path,
            &format!("kb_{kb_id}"),
            dimension,
        )?);
        collections.insert(kb_id.into(), collection.clone());
        Ok(collection)
    }
}

pub fn persist_online_index(
    engine: &mut SearchEngine,
    index_path: &str,
    mirror: &OnlineVectorMirror,
    previous: Vec<IndexedChunk>,
) -> Result<()> {
    let current = engine.to_vec();
    search_mapping::validate_replacement_snapshot(&current)?;
    if let Err(error) = mirror.sync_snapshot(&previous, &current) {
        *engine = SearchEngine::from_chunks(previous);
        return Err(error);
    }
    if let Err(error) = engine.save(index_path) {
        if let Err(rollback_error) = mirror.sync_snapshot(&current, &previous) {
            tracing::error!(%rollback_error, "Failed to roll back zvec after JSON index failure");
        }
        *engine = SearchEngine::from_chunks(previous);
        return Err(error);
    }
    Ok(())
}

pub fn rollback_online_index(
    engine: &mut SearchEngine,
    index_path: &str,
    mirror: &OnlineVectorMirror,
    previous: Vec<IndexedChunk>,
    reason: &str,
) {
    let current = engine.to_vec();
    if let Err(error) = mirror.sync_snapshot(&current, &previous) {
        tracing::error!(%error, %reason, "Failed to roll back zvec online index");
    }
    *engine = SearchEngine::from_chunks(previous);
    if let Err(error) = engine.save(index_path) {
        tracing::error!(%error, %reason, "Failed to persist JSON index rollback");
    }
}

#[cfg(feature = "zvec-backend")]
fn chunks_by_kb(chunks: &[IndexedChunk]) -> Result<HashMap<String, Vec<IndexedChunk>>> {
    let mut grouped: HashMap<String, Vec<IndexedChunk>> = HashMap::new();
    for chunk in chunks.iter().filter(|chunk| !chunk.embedding.is_empty()) {
        let kb_id = chunk
            .metadata
            .get("kb_id")
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Chunk {} is missing kb_id metadata", chunk.id))?;
        grouped.entry(kb_id.into()).or_default().push(chunk.clone());
    }
    Ok(grouped)
}

#[cfg(feature = "zvec-backend")]
fn vector_dimension(chunks: &[IndexedChunk]) -> Option<usize> {
    chunks
        .iter()
        .find(|chunk| !chunk.embedding.is_empty())
        .map(|chunk| chunk.embedding.len())
}

#[cfg(feature = "zvec-backend")]
fn validate_dimensions(kb_id: &str, chunks: &[IndexedChunk], dimension: usize) -> Result<()> {
    if let Some(chunk) = chunks
        .iter()
        .find(|chunk| !chunk.embedding.is_empty() && chunk.embedding.len() != dimension)
    {
        anyhow::bail!(
            "Knowledge base {kb_id} has mixed vector dimensions: expected {dimension}, chunk {} has {}",
            chunk.id,
            chunk.embedding.len()
        );
    }
    Ok(())
}

impl ZvecStore {
    /// Create a store. The selected backend is controlled by
    /// `RAYRAG_VECTOR_BACKEND=json|zvec` and defaults to zvec whenever the
    /// `zvec-backend` feature is compiled in (the shipped build), falling back
    /// to the portable JSON index only when the native library is absent.
    pub fn new(collection: &str, dimension: usize) -> Result<Self> {
        Self::open(collection, dimension, None)
    }

    /// Load a portable JSON index, or open the configured zvec collection.
    pub fn from_file(collection: &str, dimension: usize, path: &str) -> Result<Self> {
        Self::open(collection, dimension, Some(Path::new(path)))
    }

    fn open(collection: &str, dimension: usize, json_path: Option<&Path>) -> Result<Self> {
        if collection.trim().is_empty() {
            anyhow::bail!("Vector collection name must not be empty");
        }
        if dimension == 0 {
            anyhow::bail!("Vector dimension must be greater than zero");
        }

        let backend = std::env::var(VECTOR_BACKEND_ENV)
            .unwrap_or_else(|_| default_vector_backend().to_string())
            .trim()
            .to_ascii_lowercase();
        match backend.as_str() {
            "json" => {
                let index_path = json_path
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from(format!("{collection}.index.json")));
                let engine = if index_path.exists() {
                    SearchEngine::from_file(path_str(&index_path)?)?
                } else {
                    SearchEngine::new()
                };
                Ok(Self {
                    collection: collection.into(),
                    dimension,
                    backend: VectorBackend::Json {
                        engine: RwLock::new(engine),
                        index_path,
                    },
                })
            }
            "zvec" => {
                #[cfg(feature = "zvec-backend")]
                {
                    let root = std::env::var(ZVEC_ROOT_ENV)
                        .map(PathBuf::from)
                        .unwrap_or_else(|_| PathBuf::from("zvec-data"));
                    let path = root.join(safe_collection_name(collection));
                    let native = zvec_backend::ZvecCollection::open(&path, collection, dimension)?;
                    Ok(Self {
                        collection: collection.into(),
                        dimension,
                        backend: VectorBackend::Zvec(native),
                    })
                }
                #[cfg(not(feature = "zvec-backend"))]
                {
                    anyhow::bail!("RAYRAG_VECTOR_BACKEND=zvec requires --features zvec-backend")
                }
            }
            value => {
                anyhow::bail!("Unsupported vector backend '{value}'; expected 'json' or 'zvec'")
            }
        }
    }

    /// Validate and report the active backend.
    pub fn init(&self) -> Result<()> {
        tracing::info!(
            collection = %self.collection,
            dimension = self.dimension,
            backend = self.backend_name(),
            "Vector store initialized"
        );
        Ok(())
    }

    /// Insert or replace chunks with embeddings.
    pub fn insert(&self, chunks: &[Chunk]) -> Result<usize> {
        let indexed = indexed_chunks(chunks, &self.collection, self.dimension)?;
        if indexed.is_empty() {
            return Ok(0);
        }
        let count = indexed.len();
        match &self.backend {
            VectorBackend::Json { engine, index_path } => {
                let mut engine = engine
                    .write()
                    .map_err(|_| anyhow::anyhow!("JSON vector index lock poisoned"))?;
                let previous = engine.to_vec();
                for chunk in indexed {
                    engine.remove_chunk(&chunk.id);
                    engine.add(chunk);
                }
                if let Err(error) = engine.save(path_str(index_path)?) {
                    *engine = SearchEngine::from_chunks(previous);
                    return Err(error);
                }
            }
            #[cfg(feature = "zvec-backend")]
            VectorBackend::Zvec(collection) => collection.upsert(&indexed)?,
        }
        Ok(count)
    }

    /// Search for similar chunks by embedding vector.
    pub fn search(&self, query_embedding: &[f32], top_k: usize) -> Result<Vec<SearchResult>> {
        if query_embedding.len() != self.dimension {
            anyhow::bail!(
                "Query vector dimension mismatch: expected {}, got {}",
                self.dimension,
                query_embedding.len()
            );
        }
        if top_k == 0 {
            return Ok(Vec::new());
        }
        match &self.backend {
            VectorBackend::Json { engine, .. } => {
                let engine = engine
                    .read()
                    .map_err(|_| anyhow::anyhow!("JSON vector index lock poisoned"))?;
                Ok(engine
                    .search(query_embedding, top_k)
                    .into_iter()
                    .map(|result| SearchResult {
                        chunk: indexed_to_chunk(&result.chunk),
                        score: result.score,
                    })
                    .collect())
            }
            #[cfg(feature = "zvec-backend")]
            VectorBackend::Zvec(collection) => collection.search(query_embedding, top_k),
        }
    }

    /// Flush the active backend to durable storage.
    pub fn save(&self) -> Result<()> {
        match &self.backend {
            VectorBackend::Json { engine, index_path } => engine
                .read()
                .map_err(|_| anyhow::anyhow!("JSON vector index lock poisoned"))?
                .save(path_str(index_path)?),
            #[cfg(feature = "zvec-backend")]
            VectorBackend::Zvec(collection) => collection.flush(),
        }
    }

    pub fn len(&self) -> usize {
        match &self.backend {
            VectorBackend::Json { engine, .. } => {
                engine.read().map(|value| value.len()).unwrap_or(0)
            }
            #[cfg(feature = "zvec-backend")]
            VectorBackend::Zvec(collection) => collection.len().unwrap_or(0),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            VectorBackend::Json { .. } => "json",
            #[cfg(feature = "zvec-backend")]
            VectorBackend::Zvec(_) => "zvec",
        }
    }
}

fn indexed_chunks(
    chunks: &[Chunk],
    collection: &str,
    dimension: usize,
) -> Result<Vec<IndexedChunk>> {
    chunks
        .iter()
        .filter_map(|chunk| chunk.embedding.as_ref().map(|embedding| (chunk, embedding)))
        .map(|(chunk, embedding)| {
            if embedding.len() != dimension {
                anyhow::bail!(
                    "Chunk {} vector dimension mismatch: expected {}, got {}",
                    chunk.id,
                    dimension,
                    embedding.len()
                );
            }
            let record = infinity_chunk::encode_chunk_record(chunk, collection)?;
            let mut metadata = chunk.metadata.clone();
            metadata.extend(infinity_chunk::sparse_record_metadata(&record));
            metadata.insert("collection".into(), collection.into());
            metadata
                .entry("doc_id".into())
                .or_insert_with(|| chunk.doc_id.to_string());
            metadata
                .entry("content_type".into())
                .or_insert_with(|| chunk.content_type.clone());
            let doc_name = record
                .get("docnm")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            Ok(IndexedChunk {
                id: chunk.id.clone(),
                doc_name,
                content: chunk.content.clone(),
                embedding: embedding.clone(),
                token_count: chunk.token_count,
                position: chunk.position,
                metadata,
            })
        })
        .collect()
}

fn indexed_to_chunk(chunk: &IndexedChunk) -> Chunk {
    let metadata = infinity_chunk::decode_sparse_metadata(&chunk.metadata)
        .unwrap_or_else(|_| chunk.metadata.clone());
    Chunk {
        id: chunk.id.clone(),
        content: chunk.content.clone(),
        content_type: chunk
            .metadata
            .get("content_type")
            .cloned()
            .unwrap_or_else(|| "text".into()),
        doc_id: chunk
            .metadata
            .get("doc_id")
            .and_then(|value| uuid::Uuid::parse_str(value).ok())
            .unwrap_or_else(uuid::Uuid::nil),
        position: chunk.position,
        token_count: chunk.token_count,
        embedding: Some(chunk.embedding.clone()),
        metadata,
    }
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow::anyhow!("Path is not valid UTF-8: {}", path.display()))
}

#[cfg(feature = "zvec-backend")]
fn safe_collection_name(collection: &str) -> String {
    collection
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

/// Convert a Chunk to a flat key-value metadata map for integrations.
pub fn chunk_to_fields(chunk: &Chunk) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    fields.insert("chunk_id".to_string(), chunk.id.clone());
    fields.insert("content".to_string(), chunk.content.clone());
    fields.insert("content_type".to_string(), chunk.content_type.clone());
    fields.insert("doc_id".to_string(), chunk.doc_id.to_string());
    fields.insert("position".to_string(), chunk.position.to_string());
    fields.insert("token_count".to_string(), chunk.token_count.to_string());
    for (key, value) in &chunk.metadata {
        fields.insert(format!("meta_{key}"), value.clone());
    }
    fields
}

#[cfg(feature = "zvec-backend")]
mod zvec_backend {
    use super::{IndexedChunk, Path, Result, SearchResult, indexed_to_chunk};
    use std::sync::{Mutex, OnceLock};
    use zvec_rust::{
        Collection, CollectionSchema, DataType, Doc, FieldSchema, IndexParams, MetricType,
        SearchQuery,
    };

    static INITIALIZE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    pub struct ZvecCollection {
        collection: Collection,
    }

    impl ZvecCollection {
        pub fn open(path: &Path, name: &str, dimension: usize) -> Result<Self> {
            initialize()?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let path_text = path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid zvec path: {}", path.display()))?;
            let collection = if path.exists() {
                Collection::open(path_text, None)?
            } else {
                let schema = CollectionSchema::builder(name)
                    .add_field(FieldSchema::new("content", DataType::String, true, 0)?)
                    .add_field(FieldSchema::new("doc_name", DataType::String, true, 0)?)
                    .add_field(FieldSchema::new(
                        "metadata_json",
                        DataType::String,
                        false,
                        0,
                    )?)
                    .add_field(FieldSchema::new("position", DataType::Uint64, false, 0)?)
                    .add_field(FieldSchema::new("token_count", DataType::Uint64, false, 0)?)
                    .add_vector_field(
                        "embedding",
                        DataType::VectorFp32,
                        u32::try_from(dimension)?,
                        IndexParams::hnsw(MetricType::Cosine, 16, 50)?,
                    )
                    .build()?;
                Collection::create_and_open(path_text, &schema, None)?
            };
            Ok(Self { collection })
        }

        pub fn upsert(&self, chunks: &[IndexedChunk]) -> Result<()> {
            if chunks.is_empty() {
                return Ok(());
            }
            let mut docs = Vec::with_capacity(chunks.len());
            for chunk in chunks {
                let mut doc = Doc::new()?;
                doc.set_pk(&chunk.id);
                doc.add_string("content", &chunk.content)?;
                doc.add_string("doc_name", &chunk.doc_name)?;
                doc.add_string("metadata_json", &serde_json::to_string(&chunk.metadata)?)?;
                doc.add_u64("position", u64::try_from(chunk.position)?)?;
                doc.add_u64("token_count", u64::try_from(chunk.token_count)?)?;
                doc.add_vector_f32("embedding", &chunk.embedding)?;
                docs.push(doc);
            }
            let refs: Vec<&Doc> = docs.iter().collect();
            let result = self.collection.upsert(&refs)?;
            if result.error_count > 0 {
                let details = result
                    .results
                    .iter()
                    .filter(|result| !result.success)
                    .map(|result| result.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                anyhow::bail!(
                    "zvec upsert failed for {} chunks: {details}",
                    result.error_count
                );
            }
            self.collection.flush()?;
            Ok(())
        }

        pub fn delete(&self, chunk_ids: &[&str]) -> Result<()> {
            if chunk_ids.is_empty() {
                return Ok(());
            }
            let result = self.collection.delete(chunk_ids)?;
            if result.error_count > 0 {
                let details = result
                    .results
                    .iter()
                    .filter(|result| !result.success)
                    .map(|result| result.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                anyhow::bail!(
                    "zvec delete failed for {} chunks: {details}",
                    result.error_count
                );
            }
            Ok(())
        }

        pub fn delete_all(&self) -> Result<()> {
            self.collection.delete_by_filter("position >= 0")?;
            Ok(())
        }

        pub fn search(&self, embedding: &[f32], top_k: usize) -> Result<Vec<SearchResult>> {
            let top_k = i32::try_from(top_k)?;
            let query = SearchQuery::builder()
                .field_name("embedding")
                .vector(embedding)
                .topk(top_k)
                .output_fields(&[
                    "content",
                    "doc_name",
                    "metadata_json",
                    "position",
                    "token_count",
                ])
                .build()?;
            self.collection
                .query(&query)?
                .into_iter()
                .map(|doc| {
                    let metadata = doc
                        .get_string("metadata_json")?
                        .map(|value| serde_json::from_str(&value))
                        .transpose()?
                        .unwrap_or_default();
                    let indexed = IndexedChunk {
                        id: doc.get_pk().unwrap_or_default().to_string(),
                        doc_name: doc.get_string("doc_name")?.unwrap_or_default(),
                        content: doc.get_string("content")?.unwrap_or_default(),
                        embedding: embedding.to_vec(),
                        token_count: usize::try_from(doc.get_u64("token_count")?.unwrap_or(0))?,
                        position: usize::try_from(doc.get_u64("position")?.unwrap_or(0))?,
                        metadata,
                    };
                    Ok(SearchResult {
                        chunk: indexed_to_chunk(&indexed),
                        score: doc.get_score(),
                    })
                })
                .collect()
        }

        pub fn flush(&self) -> Result<()> {
            self.collection.flush()?;
            Ok(())
        }

        pub fn len(&self) -> Result<usize> {
            Ok(usize::try_from(self.collection.stats()?.doc_count)?)
        }
    }

    fn initialize() -> Result<()> {
        let lock = INITIALIZE_LOCK.get_or_init(|| Mutex::new(()));
        let _guard = lock
            .lock()
            .map_err(|_| anyhow::anyhow!("zvec initialization lock poisoned"))?;
        if !zvec_rust::is_initialized() {
            zvec_rust::initialize(None)?;
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::collections::HashMap;

        #[test]
        fn native_collection_upserts_queries_and_reopens() {
            let root =
                std::env::temp_dir().join(format!("rayrag-zvec-native-{}", uuid::Uuid::new_v4()));
            let path = root.join("collection");
            let collection = ZvecCollection::open(&path, "native_test", 2).unwrap();
            let chunk = IndexedChunk {
                id: "native-chunk".into(),
                doc_name: "native.txt".into(),
                content: "native zvec search".into(),
                embedding: vec![1.0, 0.0],
                token_count: 3,
                position: 0,
                metadata: HashMap::from([
                    ("doc_id".into(), uuid::Uuid::new_v4().to_string()),
                    ("content_type".into(), "text".into()),
                    ("position_int".into(), "[[1,10,100,20,40]]".into()),
                    ("page_num_int".into(), "[1]".into()),
                    ("top_int".into(), "[20]".into()),
                ]),
            };
            collection.upsert(&[chunk]).unwrap();
            let results = collection.search(&[1.0, 0.0], 1).unwrap();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].chunk.id, "native-chunk");
            assert_eq!(collection.len().unwrap(), 1);
            drop(collection);

            let reopened = ZvecCollection::open(&path, "native_test", 2).unwrap();
            assert_eq!(reopened.len().unwrap(), 1);
            let results = reopened.search(&[1.0, 0.0], 1).unwrap();
            assert_eq!(results[0].chunk.content, "native zvec search");
            assert_eq!(
                results[0]
                    .chunk
                    .metadata
                    .get("position_int")
                    .map(String::as_str),
                Some("[[1,10,100,20,40]]")
            );
            assert_eq!(
                results[0]
                    .chunk
                    .metadata
                    .get("page_num_int")
                    .map(String::as_str),
                Some("[1]")
            );
            assert_eq!(
                results[0].chunk.metadata.get("top_int").map(String::as_str),
                Some("[20]")
            );
            drop(reopened);
            std::fs::remove_dir_all(root).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "zvec-backend")]
    fn indexed_chunk(id: &str, kb_id: &str, embedding: Vec<f32>) -> IndexedChunk {
        IndexedChunk {
            id: id.into(),
            doc_name: format!("{id}.txt"),
            content: format!("content {id}"),
            embedding,
            token_count: 2,
            position: 0,
            metadata: HashMap::from([
                ("kb_id".into(), kb_id.into()),
                ("doc_id".into(), format!("doc-{id}")),
                ("content_type".into(), "text".into()),
            ]),
        }
    }

    #[cfg(feature = "zvec-backend")]
    #[test]
    fn online_native_mirror_isolates_kbs_deletes_and_reconciles() {
        let root =
            std::env::temp_dir().join(format!("rayrag-zvec-online-{}", uuid::Uuid::new_v4()));
        let mirror = OnlineVectorMirror::native(root.clone());
        let first = indexed_chunk("a", "kb-a", vec![1.0, 0.0]);
        let second = indexed_chunk("b", "kb-b", vec![0.0, 1.0]);
        mirror
            .sync_snapshot(&[], &[first.clone(), second.clone()])
            .unwrap();
        assert_eq!(mirror.collection("kb-a", 2).unwrap().len().unwrap(), 1);
        assert_eq!(mirror.collection("kb-b", 2).unwrap().len().unwrap(), 1);

        mirror
            .sync_snapshot(
                &[first.clone(), second.clone()],
                std::slice::from_ref(&second),
            )
            .unwrap();
        assert_eq!(mirror.collection("kb-a", 2).unwrap().len().unwrap(), 0);
        assert_eq!(mirror.collection("kb-b", 2).unwrap().len().unwrap(), 1);

        mirror
            .sync_snapshot(
                std::slice::from_ref(&second),
                &[first.clone(), second.clone()],
            )
            .unwrap();
        mirror
            .reconcile_snapshot(std::slice::from_ref(&second))
            .unwrap();
        assert_eq!(mirror.collection("kb-a", 2).unwrap().len().unwrap(), 0);
        assert_eq!(mirror.collection("kb-b", 2).unwrap().len().unwrap(), 1);
        drop(mirror);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    /// The compiled-in default decides the backend when the environment does
    /// not: the shipped build (with `zvec-backend`) uses the native collection,
    /// a build without it keeps the portable JSON index.
    #[test]
    fn default_backend_follows_the_compiled_features() {
        let expected = if cfg!(feature = "zvec-backend") {
            "zvec"
        } else {
            "json"
        };
        assert_eq!(default_vector_backend(), expected);
        // An explicit environment value always wins over the default.
        if cfg!(feature = "zvec-backend") {
            unsafe { std::env::set_var("RAYRAG_VECTOR_BACKEND", "json") };
            let store = ZvecStore::new("default-backend-probe", 4).unwrap();
            assert_eq!(store.backend_name(), "json");
            unsafe { std::env::remove_var("RAYRAG_VECTOR_BACKEND") };
        }
    }

    fn json_backend_is_immediately_searchable_after_insert() {
        let root = std::env::temp_dir().join(format!("rayrag-vector-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("index.json");
        let store = ZvecStore::from_file("test", 2, path.to_str().unwrap()).unwrap();
        let chunk = Chunk {
            id: "chunk-1".into(),
            content: "hello".into(),
            content_type: "text".into(),
            doc_id: uuid::Uuid::new_v4(),
            position: 0,
            token_count: 1,
            embedding: Some(vec![1.0, 0.0]),
            metadata: HashMap::from([
                ("position_int".into(), "[[1,10,100,20,40]]".into()),
                ("page_num_int".into(), "[1]".into()),
                ("top_int".into(), "[20]".into()),
            ]),
        };
        store.insert(&[chunk]).unwrap();
        let persisted: Vec<IndexedChunk> =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(persisted.len(), 1);
        assert_eq!(
            persisted[0].metadata.get("kb_id").map(String::as_str),
            Some("test")
        );
        assert_eq!(
            persisted[0]
                .metadata
                .get("available_int")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            persisted[0]
                .metadata
                .get("position_int")
                .map(String::as_str),
            Some("00000001_0000000a_00000064_00000014_00000028")
        );
        let results = store.search(&[1.0, 0.0], 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].chunk.id, "chunk-1");
        assert_eq!(
            results[0]
                .chunk
                .metadata
                .get("position_int")
                .map(String::as_str),
            Some("[[1,10,100,20,40]]")
        );
        assert_eq!(
            results[0]
                .chunk
                .metadata
                .get("page_num_int")
                .map(String::as_str),
            Some("[1]")
        );
        assert_eq!(
            results[0].chunk.metadata.get("top_int").map(String::as_str),
            Some("[20]")
        );
        assert_eq!(store.len(), 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn insert_rejects_mixed_vector_dimensions_without_partial_write() {
        let root = std::env::temp_dir().join(format!("rayrag-vector-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("index.json");
        let store = ZvecStore::from_file("test", 2, path.to_str().unwrap()).unwrap();
        let chunk = Chunk {
            id: "bad".into(),
            content: "bad".into(),
            content_type: "text".into(),
            doc_id: uuid::Uuid::new_v4(),
            position: 0,
            token_count: 1,
            embedding: Some(vec![1.0]),
            metadata: HashMap::new(),
        };
        assert!(store.insert(&[chunk]).is_err());
        assert!(store.is_empty());
        assert!(!path.exists());
        std::fs::remove_dir_all(root).ok();
    }
}
