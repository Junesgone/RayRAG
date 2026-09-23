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

/// Primary keys deleted per page when a collection is cleared. Bounded so clearing
/// a large collection never materialises it in memory.
const ZVEC_CLEAR_PAGE: usize = 512;

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
    /// Synchronize the native mirror to `current`. On failure, every affected
    /// collection is restored to `previous` before the error is returned.
    ///
    /// Both snapshots are **borrowed**: only the chunks an owner actually changed
    /// are copied, for the native `upsert`. The earlier version cloned every chunk
    /// of both snapshots (embeddings included) into two `HashMap`s before doing any
    /// work, so a commit cost several times the index size in transient memory —
    /// memory that grew with the corpus on every single document.
    pub fn sync_snapshot(&self, previous: &[IndexedChunk], current: &[IndexedChunk]) -> Result<()> {
        #[cfg(feature = "zvec-backend")]
        {
            if self.root.is_none() {
                return Ok(());
            }
            let previous_by_kb = owners_by_kb(previous)?;
            let current_by_kb = owners_by_kb(current)?;
            // Deterministic order keeps a repair pass reproducible and its log
            // readable; the owner list itself holds no chunk data.
            let mut kb_ids: Vec<&str> = previous_by_kb
                .keys()
                .chain(current_by_kb.keys())
                .copied()
                .collect();
            kb_ids.sort_unstable();
            kb_ids.dedup();
            let mut applied: Vec<String> = Vec::new();
            for kb_id in kb_ids {
                let before = previous_by_kb.get(kb_id).map(Vec::as_slice).unwrap_or(&[]);
                let after = current_by_kb.get(kb_id).map(Vec::as_slice).unwrap_or(&[]);
                if let Err(error) = self.replace_collection(kb_id, before, after) {
                    if let Err(rollback_error) = self.replace_collection(kb_id, after, before) {
                        tracing::error!(
                            kb_id = %kb_id,
                            %rollback_error,
                            "Failed to roll back failing zvec online collection"
                        );
                    }
                    for applied_kb in applied.into_iter().rev() {
                        let applied_before = previous_by_kb
                            .get(applied_kb.as_str())
                            .map(Vec::as_slice)
                            .unwrap_or(&[]);
                        let applied_after = current_by_kb
                            .get(applied_kb.as_str())
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
                applied.push(kb_id.to_string());
            }
            Ok(())
        }
        #[cfg(not(feature = "zvec-backend"))]
        {
            let _ = (previous, current);
            Ok(())
        }
    }

    /// Reclaim the collection directories the index does not carry.
    ///
    /// This is the only part of reconciliation that runs during startup, and it
    /// opens nothing: a directory whose owner left the index holds no reachable
    /// vector, so it is removed from disk. Leaving them behind (the previous
    /// behaviour opened, emptied and flushed each one on *every* boot) had grown
    /// the deployment to 42 directories / 1.3 GB and put minutes of native opens on
    /// the startup path.
    pub fn reclaim_unindexed_collections(&self, counts: &HashMap<String, usize>) -> Result<usize> {
        #[cfg(feature = "zvec-backend")]
        {
            let Some(root) = self.root.as_ref() else {
                return Ok(0);
            };
            let expected_directories: HashSet<String> = counts
                .keys()
                .map(|kb_id| safe_collection_name(kb_id))
                .collect();
            let open_directories: HashSet<String> = {
                let collections = self
                    .collections
                    .lock()
                    .map_err(|_| anyhow::anyhow!("zvec online collection cache lock poisoned"))?;
                // An owner that left the index while its collection is open cannot
                // have its directory removed from under the handle; it is emptied
                // through the handle instead (paged, so a large collection costs one
                // page of ids), and the directory goes on the next start.
                for (kb_id, collection) in collections.iter() {
                    if !counts.contains_key(kb_id) {
                        collection.clear(ZVEC_CLEAR_PAGE)?;
                        collection.flush()?;
                    }
                }
                collections
                    .keys()
                    .map(|kb_id| safe_collection_name(kb_id))
                    .collect()
            };
            let mut reclaimed = 0;
            if root.exists() {
                for entry in std::fs::read_dir(root)? {
                    let entry = entry?;
                    if !entry.file_type()?.is_dir() {
                        continue;
                    }
                    let directory = entry.file_name().to_string_lossy().into_owned();
                    if expected_directories.contains(&directory)
                        || open_directories.contains(&directory)
                    {
                        continue;
                    }
                    let path = entry.path();
                    match std::fs::remove_dir_all(&path) {
                        Ok(()) => {
                            reclaimed += 1;
                            tracing::info!(
                                path = %path.display(),
                                "Reclaimed zvec collection directory the index does not carry"
                            );
                        }
                        // A leftover directory must never keep the server from
                        // starting: report it and retry on the next boot.
                        Err(error) => tracing::warn!(
                            path = %path.display(),
                            %error,
                            "Failed to reclaim zvec collection directory"
                        ),
                    }
                }
            }
            Ok(reclaimed)
        }
        #[cfg(not(feature = "zvec-backend"))]
        {
            let _ = counts;
            Ok(0)
        }
    }

    /// Owners whose collection does not hold exactly the rows the index carries,
    /// in a deterministic order. Only a collection's row count is read here — no
    /// chunk is loaded, and nothing is rewritten.
    pub fn drifted_owners(&self, counts: &HashMap<String, usize>) -> Result<Vec<String>> {
        #[cfg(feature = "zvec-backend")]
        {
            let Some(root) = self.root.as_ref() else {
                return Ok(Vec::new());
            };
            let mut owners: Vec<&String> = counts.keys().collect();
            owners.sort();
            // A collection this process already holds open must be inspected through
            // that handle: the native layer refuses a second open of the same path
            // (`Can't lock read-write collection`), which would read as drift and
            // send a healthy collection into a rewrite.
            let cached: HashMap<String, Arc<zvec_backend::ZvecCollection>> = self
                .collections
                .lock()
                .map_err(|_| anyhow::anyhow!("zvec online collection cache lock poisoned"))?
                .iter()
                .map(|(kb_id, collection)| (kb_id.clone(), Arc::clone(collection)))
                .collect();
            let mut drifted = Vec::new();
            for kb_id in owners {
                let Some(expected) = counts.get(kb_id).copied() else {
                    continue;
                };
                let actual = match cached.get(kb_id) {
                    Some(collection) => collection.len()?,
                    None => {
                        let path = root.join(safe_collection_name(kb_id));
                        if path.exists() {
                            match zvec_backend::ZvecCollection::open(&path, kb_id, 1) {
                                Ok(collection) => collection.len()?,
                                Err(error) => {
                                    tracing::warn!(
                                        kb_id = %kb_id,
                                        %error,
                                        "Cannot open zvec collection; it will be rebuilt"
                                    );
                                    0
                                }
                            }
                        } else {
                            0
                        }
                    }
                };
                if actual != expected {
                    drifted.push(kb_id.clone());
                }
            }
            Ok(drifted)
        }
        #[cfg(not(feature = "zvec-backend"))]
        {
            let _ = counts;
            Ok(Vec::new())
        }
    }

    /// Bring the native mirror in line with the JSON source of truth.
    ///
    /// `counts` is the number of chunks the index holds per owner, and `fetch`
    /// returns one owner's chunks on demand. Both exist so this pass never copies
    /// the whole corpus: the previous version cloned the entire index (embeddings
    /// included) into a `HashMap`, then **rebuilt every collection** with
    /// `delete_all` + full `upsert` + `flush` on every boot. On the deployment that
    /// meant rewriting 1.3 GB of native collections at every start — the phase that
    /// spent minutes in a single boot and drove memory into the tens of gigabytes.
    ///
    /// The pass is therefore **bounded and on demand**: owners are visited in order,
    /// one at a time, and only those whose collection does not already agree with
    /// the index are loaded and rewritten. The caller runs it outside the startup
    /// path (`server::start_mirror_repair`), so a large repair can never block or
    /// spike a boot, and `skip` lets the caller keep an owner that failed to repair
    /// from being retried forever.
    pub fn reconcile_snapshot<F>(&self, counts: &HashMap<String, usize>, fetch: F) -> Result<usize>
    where
        F: Fn(&str) -> Vec<IndexedChunk>,
    {
        #[cfg(feature = "zvec-backend")]
        {
            if self.root.is_none() {
                return Ok(0);
            }
            let mut repaired = 0;
            for kb_id in self.drifted_owners(counts)? {
                let owned = fetch(&kb_id);
                if owned.is_empty() {
                    // The index counts chunks this reader cannot produce: leave the
                    // collection alone rather than deleting data we cannot rebuild.
                    tracing::warn!(
                        kb_id = %kb_id,
                        "Index reports chunks the reader cannot fetch; skipping zvec rebuild"
                    );
                    continue;
                }
                if self.repair_owner(&kb_id, &owned)? {
                    repaired += 1;
                }
                // No accumulated state crosses owners: `owned` is dropped here,
                // before the next owner is even inspected.
            }
            Ok(repaired)
        }
        #[cfg(not(feature = "zvec-backend"))]
        {
            let _ = (counts, fetch);
            Ok(0)
        }
    }

    /// Rewrite exactly one owner's collection from `chunks`.
    ///
    /// Returns `false` when there is nothing to write (no vector to derive a
    /// dimension from). This is the unit of work the background repair pass runs —
    /// one owner per pass, so peak memory is bounded by that owner alone.
    pub fn repair_owner(&self, kb_id: &str, owned: &[IndexedChunk]) -> Result<bool> {
        #[cfg(feature = "zvec-backend")]
        {
            if self.root.is_none() {
                return Ok(false);
            }
            let chunks: Vec<&IndexedChunk> = owned.iter().collect();
            let Some(dimension) = vector_dimension(&chunks) else {
                return Ok(false);
            };
            validate_dimensions(kb_id, &chunks, dimension)?;
            tracing::info!(
                kb_id = %kb_id,
                chunks = owned.len(),
                "Rewriting zvec collection that drifted from the index"
            );
            // The rewrite happens in place: the native layer keeps state for paths
            // it has opened, so removing a collection directory is reserved for the
            // leftovers the startup reclaim pass deletes.
            let collection = self.collection(kb_id, dimension)?;
            collection.clear(ZVEC_CLEAR_PAGE)?;
            collection.upsert(owned)?;
            collection.flush()?;
            Ok(true)
        }
        #[cfg(not(feature = "zvec-backend"))]
        {
            let _ = (kb_id, owned);
            Ok(false)
        }
    }

    /// Reconcile against an in-memory snapshot (the Skill index keeps its whole
    /// state in memory already, so the snapshot is the natural input).
    pub fn reconcile_chunks(&self, current: &[IndexedChunk]) -> Result<usize> {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for chunk in current.iter().filter(|chunk| !chunk.embedding.is_empty()) {
            if let Some(kb_id) = chunk
                .metadata
                .get("kb_id")
                .map(String::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                *counts.entry(kb_id.to_string()).or_default() += 1;
            }
        }
        self.reconcile_snapshot(&counts, |kb_id| {
            current
                .iter()
                .filter(|chunk| chunk.metadata.get("kb_id").map(String::as_str) == Some(kb_id))
                .cloned()
                .collect()
        })
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
        previous: &[&IndexedChunk],
        current: &[&IndexedChunk],
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
        let changed = Self::changed_chunks(previous, current);
        if !changed.is_empty() {
            collection.upsert(&changed)?;
        }
        collection.flush()
    }

    /// One owner's chunks in `current` that `previous` does not already hold with
    /// the same content — the only chunks a commit has to copy for the native
    /// `upsert`.
    #[cfg(feature = "zvec-backend")]
    fn changed_chunks<'a>(
        previous: &[&IndexedChunk],
        current: &[&'a IndexedChunk],
    ) -> Vec<IndexedChunk> {
        let before: HashMap<&str, &IndexedChunk> = previous
            .iter()
            .map(|chunk| (chunk.id.as_str(), *chunk))
            .collect();
        current
            .iter()
            .filter(|chunk| {
                before.get(chunk.id.as_str()).is_none_or(|known| {
                    known.doc_name != chunk.doc_name
                        || known.content != chunk.content
                        || known.embedding != chunk.embedding
                        || known.token_count != chunk.token_count
                        || known.position != chunk.position
                        || known.metadata != chunk.metadata
                })
            })
            .map(|chunk| (*chunk).clone())
            .collect()
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
/// Group a snapshot by owning knowledge base **without copying any chunk**: the
/// map holds positions into the caller's slice, so a commit costs O(index) tiny
/// indices instead of a full clone of every embedding.
#[cfg(feature = "zvec-backend")]
fn owners_by_kb(chunks: &[IndexedChunk]) -> Result<HashMap<&str, Vec<&IndexedChunk>>> {
    let mut grouped: HashMap<&str, Vec<&IndexedChunk>> = HashMap::new();
    for chunk in chunks.iter().filter(|chunk| !chunk.embedding.is_empty()) {
        let kb_id = chunk
            .metadata
            .get("kb_id")
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Chunk {} is missing kb_id metadata", chunk.id))?;
        grouped.entry(kb_id).or_default().push(chunk);
    }
    Ok(grouped)
}

#[cfg(feature = "zvec-backend")]
fn vector_dimension(chunks: &[&IndexedChunk]) -> Option<usize> {
    chunks
        .iter()
        .find(|chunk| !chunk.embedding.is_empty())
        .map(|chunk| chunk.embedding.len())
}

#[cfg(feature = "zvec-backend")]
fn validate_dimensions(kb_id: &str, chunks: &[&IndexedChunk], dimension: usize) -> Result<()> {
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
        Collection, CollectionOptions, CollectionSchema, DataType, Doc, FieldSchema, IndexParams,
        MetricType, SearchQuery,
    };

    /// Ceiling for one collection's native write buffer (`RAYRAG_ZVEC_MAX_BUFFER_BYTES`).
    fn zvec_max_buffer_bytes() -> u64 {
        std::env::var("RAYRAG_ZVEC_MAX_BUFFER_BYTES")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value >= 1024 * 1024)
            .unwrap_or(64 * 1024 * 1024)
    }

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
            // Bound the native layer's buffer for every collection this process
            // opens: an unbounded buffer is memory the process cannot account for,
            // and a collection that grows past it must degrade, not take the host
            // with it. `RAYRAG_ZVEC_MAX_BUFFER_BYTES` overrides the 64 MiB default.
            let mut options = CollectionOptions::new()?;
            options.set_max_buffer_size(zvec_max_buffer_bytes())?;
            let collection = if path.exists() {
                Collection::open(path_text, Some(&options))?
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
                Collection::create_and_open(path_text, &schema, Some(&options))?
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

        /// Delete every document in pages of primary keys.
        ///
        /// `delete_by_filter("position >= 0")` makes the native layer collect every
        /// matching key before deleting, so one call on the deployment's 523 MB
        /// collection grew past the container's 6 GB ceiling and was OOM-killed.
        /// Paging keeps peak memory at one page of ids: the iterator is asked for
        /// primary keys only (`Some(&[])`, no vectors), a page is deleted, and the
        /// next page is read from the shrunken collection.
        pub fn clear(&self, page: usize) -> Result<usize> {
            let page = page.max(1);
            let mut removed = 0usize;
            loop {
                let mut ids: Vec<String> = Vec::with_capacity(page);
                {
                    let iterator = self.collection.iter_with_options(Some(&[]), false)?;
                    for doc in iterator {
                        let doc = doc?;
                        if let Some(pk) = doc.get_pk() {
                            ids.push(pk.to_string());
                        }
                        if ids.len() >= page {
                            break;
                        }
                    }
                }
                if ids.is_empty() {
                    break;
                }
                let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                self.delete(&refs)?;
                removed += ids.len();
                if ids.len() < page {
                    break;
                }
            }
            Ok(removed)
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

            // `clear` must empty a collection whose row count far exceeds one page:
            // it deletes page by page instead of asking the native layer for every
            // key at once (the call that grew past the container ceiling).
            let many: Vec<IndexedChunk> = (0..37)
                .map(|index| IndexedChunk {
                    id: format!("page-chunk-{index}"),
                    doc_name: "pages.txt".into(),
                    content: format!("page {index}"),
                    embedding: vec![1.0, 0.0],
                    token_count: 1,
                    position: index,
                    metadata: HashMap::from([("doc_id".into(), "pages".into())]),
                })
                .collect();
            reopened.upsert(&many).unwrap();
            assert_eq!(reopened.len().unwrap(), 38);
            // A page smaller than the collection forces several rounds.
            assert_eq!(reopened.clear(5).unwrap(), 38);
            assert_eq!(reopened.len().unwrap(), 0);
            assert_eq!(
                reopened.clear(5).unwrap(),
                0,
                "clearing an empty collection is a no-op"
            );
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
    fn online_native_mirror_isolates_kbs_and_applies_partial_updates() {
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

        // Dropping one owner's chunk deletes it from that owner's collection only.
        mirror
            .sync_snapshot(
                &[first.clone(), second.clone()],
                std::slice::from_ref(&second),
            )
            .unwrap();
        assert_eq!(mirror.collection("kb-a", 2).unwrap().len().unwrap(), 0);
        assert_eq!(mirror.collection("kb-b", 2).unwrap().len().unwrap(), 1);

        // Re-adding it upserts again without touching the other owner.
        mirror
            .sync_snapshot(
                std::slice::from_ref(&second),
                &[first.clone(), second.clone()],
            )
            .unwrap();
        assert_eq!(mirror.collection("kb-a", 2).unwrap().len().unwrap(), 1);
        assert_eq!(mirror.collection("kb-b", 2).unwrap().len().unwrap(), 1);
        drop(mirror);
        std::fs::remove_dir_all(root).ok();
    }

    /// Startup reclaims the collection directories the index no longer carries
    /// instead of opening, emptying and flushing each one on every boot — that loop
    /// paid a native open per leftover directory and never got rid of any of them
    /// (the deployment had accumulated 42 directories / 1.3 GB).
    #[cfg(feature = "zvec-backend")]
    #[test]
    fn online_native_mirror_reclaims_directories_the_index_dropped() {
        let root =
            std::env::temp_dir().join(format!("rayrag-zvec-reclaim-{}", uuid::Uuid::new_v4()));
        let mirror = OnlineVectorMirror::native(root.clone());
        let kept = indexed_chunk("a", "kb-live", vec![1.0, 0.0]);
        let dropped = indexed_chunk("b", "kb-dropped", vec![0.0, 1.0]);
        mirror
            .sync_snapshot(&[], &[kept.clone(), dropped.clone()])
            .unwrap();
        let dropped_dir = root.join(safe_collection_name("kb-dropped"));
        let live_dir = root.join(safe_collection_name("kb-live"));
        assert!(dropped_dir.is_dir(), "the dropped collection exists first");
        assert!(live_dir.is_dir(), "the live collection exists first");
        drop(mirror);

        // A fresh process starts against an index that only carries kb-live.
        let mirror = OnlineVectorMirror::native(root.clone());
        let counts: HashMap<String, usize> = [("kb-live".to_string(), 1)].into_iter().collect();
        assert_eq!(mirror.reclaim_unindexed_collections(&counts).unwrap(), 1);
        assert!(
            !dropped_dir.exists(),
            "a collection the index dropped must be reclaimed from disk"
        );
        assert!(live_dir.is_dir(), "a collection the index carries stays");
        assert_eq!(mirror.collection("kb-live", 2).unwrap().len().unwrap(), 1);

        // An orphan directory nobody owns is reclaimed as well, without ever being
        // opened as a native collection, and a second pass has nothing left to do.
        let orphan = root.join("orphan-collection");
        std::fs::create_dir_all(&orphan).unwrap();
        std::fs::write(orphan.join("junk.bin"), b"not a collection").unwrap();
        assert_eq!(mirror.reclaim_unindexed_collections(&counts).unwrap(), 1);
        assert!(!orphan.exists(), "an unowned directory must be reclaimed");
        assert_eq!(mirror.reclaim_unindexed_collections(&counts).unwrap(), 0);

        // A collection this process holds open keeps its storage: the reclaim pass
        // never removes a directory out from under a live handle.
        let open_dir = root.join(safe_collection_name("kb-open"));
        mirror
            .collection("kb-open", 2)
            .unwrap()
            .upsert(std::slice::from_ref(&dropped))
            .unwrap();
        assert!(open_dir.is_dir());
        assert_eq!(mirror.reclaim_unindexed_collections(&counts).unwrap(), 0);
        assert!(
            open_dir.is_dir(),
            "an open collection must not lose its storage underneath it"
        );
        drop(mirror);
        std::fs::remove_dir_all(root).ok();
    }

    /// Reconciliation loads a knowledge base's chunks **only** when its collection
    /// drifted: the steady state (a collection that already holds exactly the rows
    /// the index carries) must not read a single chunk. That is what keeps a normal
    /// boot flat instead of rewriting every collection.
    #[cfg(feature = "zvec-backend")]
    #[test]
    fn online_native_mirror_loads_chunks_only_for_drifted_collections() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let root = std::env::temp_dir().join(format!("rayrag-zvec-lazy-{}", uuid::Uuid::new_v4()));
        let mirror = OnlineVectorMirror::native(root.clone());
        let first = indexed_chunk("a", "kb-a", vec![1.0, 0.0]);
        let second = indexed_chunk("b", "kb-b", vec![0.0, 1.0]);
        mirror
            .sync_snapshot(&[], &[first.clone(), second.clone()])
            .unwrap();
        drop(mirror);

        // A fresh process sees collections that already match the index.
        let mirror = OnlineVectorMirror::native(root.clone());
        let counts: HashMap<String, usize> = [("kb-a".to_string(), 1), ("kb-b".to_string(), 1)]
            .into_iter()
            .collect();
        let loads = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&loads);
        mirror
            .reconcile_snapshot(&counts, move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                Vec::new()
            })
            .unwrap();
        assert_eq!(
            loads.load(Ordering::SeqCst),
            0,
            "a mirror that agrees with the index must not load any chunk"
        );

        // One owner drifts (the index knows two chunks it does not hold): exactly
        // that owner is loaded and rebuilt.
        let counts: HashMap<String, usize> = [("kb-a".to_string(), 2), ("kb-b".to_string(), 1)]
            .into_iter()
            .collect();
        let loads = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&loads);
        mirror
            .reconcile_snapshot(&counts, move |kb_id| {
                counter.fetch_add(1, Ordering::SeqCst);
                if kb_id == "kb-a" {
                    vec![first.clone(), indexed_chunk("c", "kb-a", vec![0.5, 0.5])]
                } else {
                    Vec::new()
                }
            })
            .unwrap();
        assert_eq!(
            loads.load(Ordering::SeqCst),
            1,
            "only the drifted owner may be loaded"
        );
        assert_eq!(mirror.collection("kb-a", 2).unwrap().len().unwrap(), 2);
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
