//! RAGFlow `rag/res/` resource tables — `ner.json` + `synonym.json`.
//!
//! Mirrors RAGFlow's resource loading semantics:
//! - `rag/nlp/term_weight.py` `Dealer.load_dict`: `ner.json` maps a term to
//!   its NER category (`"stock"`, `"corp"`, `"loca"`, ...); the file lives
//!   under `{project_base}/rag/res/` and a failed load only logs a warning,
//!   never aborts parsing.
//! - `rag/nlp/synonym.py` `Dealer.__init__`: `synonym.json` maps a term to
//!   its canonical synonym; lookups are exact-key (the shipped file stores
//!   both directions, and [`DeepDocResources::load`] additionally mirrors
//!   value → key so single-direction files behave the same way).
//!
//! The tables are intentionally plain `HashMap`s: downstream consumers
//! (`nlp::TermWeightComputer::with_resources`,
//! `nlp::SynonymDict::from_synonym_json`) can adopt them directly via
//! [`DeepDocResources::ner_map`] / [`DeepDocResources::synonym_map`].

use crate::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Default resource directory, relative to the working directory —
/// RAGFlow's `get_project_base_directory()/rag/res`.
pub const DEFAULT_RES_DIR: &str = "rag/res";

/// Environment variable consulted first by [`DeepDocResources::from_env`]
/// (RayRAG-specific alias; takes precedence over the RAGFlow name).
pub const RAYRAG_RES_PATH_ENV: &str = "RAYRAG_RES_PATH";

/// Environment variable consulted by [`DeepDocResources::from_env`],
/// matching RAGFlow deployments that point `rag/res` elsewhere.
pub const RAGFLOW_RES_PATH_ENV: &str = "RAGFLOW_RES_PATH";

/// `rag/res/` resource tables (`ner.json` + `synonym.json`).
#[derive(Debug, Clone, Default)]
pub struct DeepDocResources {
    /// ner.json: term → NER category (`"stock"`, `"corp"`, `"loca"`, ...).
    ner: HashMap<String, String>,
    /// synonym.json: term → canonical synonym (mirrored both directions).
    synonym: HashMap<String, String>,
}

impl DeepDocResources {
    /// Load both resource files from `dir`. Missing files and unparsable
    /// JSON degrade to empty tables (RAGFlow warns and continues), so this
    /// only fails on I/O errors that are not `NotFound`.
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let ner = load_string_map(&dir.join("ner.json"))?;
        let mut synonym = load_string_map(&dir.join("synonym.json"))?;
        // Mirror value → key so single-direction files behave like the
        // shipped bidirectional synonym.json (nlp::SynonymDict does the
        // same when built from a map).
        let mirrored: Vec<(String, String)> = synonym
            .iter()
            .map(|(key, value)| (value.clone(), key.clone()))
            .collect();
        for (key, value) in mirrored {
            synonym.entry(key).or_insert(value);
        }
        Ok(Self { ner, synonym })
    }

    /// Resolve the resource directory from the environment
    /// (`RAYRAG_RES_PATH` > `RAGFLOW_RES_PATH` > `rag/res`) and load the
    /// tables. `Ok(None)` when the directory does not exist — callers that
    /// treat the resources as optional can skip enrichment entirely.
    pub fn from_env() -> Result<Option<Self>> {
        let dir = resolve_res_dir();
        if !dir.is_dir() {
            return Ok(None);
        }
        Ok(Some(Self::load(&dir)?))
    }

    /// NER category of `term` (`None` when unknown — `Dealer.ner` then
    /// falls back to the default weight of 1.0).
    pub fn ner_category(&self, term: &str) -> Option<&str> {
        self.ner.get(term).map(String::as_str)
    }

    /// Whether `term` has a NER category entry.
    pub fn is_ner(&self, term: &str) -> bool {
        self.ner.contains_key(term)
    }

    /// Canonical synonym of `term` (exact-key lookup, mirroring
    /// `synonym.py Dealer.lookup`).
    pub fn synonym(&self, term: &str) -> Option<&str> {
        self.synonym.get(term).map(String::as_str)
    }

    /// Borrow the NER table (term → category) for
    /// `nlp::TermWeightComputer::with_resources`.
    pub fn ner_map(&self) -> &HashMap<String, String> {
        &self.ner
    }

    /// Borrow the synonym table (term → canonical, mirrored) for
    /// `nlp::SynonymDict::from_synonym_json`.
    pub fn synonym_map(&self) -> &HashMap<String, String> {
        &self.synonym
    }

    /// Consume the resources, returning `(ner, synonym)` owned maps.
    pub fn into_parts(self) -> (HashMap<String, String>, HashMap<String, String>) {
        (self.ner, self.synonym)
    }
}

/// `RAYRAG_RES_PATH` > `RAGFLOW_RES_PATH` > `DEFAULT_RES_DIR`.
fn resolve_res_dir() -> PathBuf {
    std::env::var(RAYRAG_RES_PATH_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var(RAGFLOW_RES_PATH_ENV)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_RES_DIR))
}

/// Read a `{term: category|synonym}` string map; `NotFound` → empty map,
/// unparsable JSON → empty map (RAGFlow logs a warning and continues).
fn load_string_map(path: &Path) -> Result<HashMap<String, String>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(HashMap::new());
        }
        Err(error) => return Err(error.into()),
    };
    match serde_json::from_str::<HashMap<String, String>>(&raw) {
        Ok(map) => Ok(map),
        Err(_) => Ok(HashMap::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a scratch `rag/res`-style directory with small resource files.
    fn scratch_resources() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let res = dir.path().join("res");
        std::fs::create_dir_all(&res).expect("create res dir");
        std::fs::write(
            res.join("ner.json"),
            r#"{"贵州茅台":"stock","腾讯控股":"stock","北京":"loca"}"#,
        )
        .expect("write ner.json");
        // Single-direction file on purpose: value→key mirroring is the
        // loader's job, matching the shipped bidirectional synonym.json.
        std::fs::write(
            res.join("synonym.json"),
            r#"{"阿为特":"873693","卓兆点胶":"873726"}"#,
        )
        .expect("write synonym.json");
        (dir, res)
    }

    #[test]
    fn load_parses_ner_and_mirrors_synonyms() {
        let (_keep, res) = scratch_resources();
        let resources = DeepDocResources::load(&res).expect("load resources");

        assert_eq!(resources.ner_category("贵州茅台"), Some("stock"));
        assert_eq!(resources.ner_category("北京"), Some("loca"));
        assert_eq!(resources.ner_category("未知术语"), None);
        assert!(resources.is_ner("腾讯控股"));
        assert!(!resources.is_ner("腾讯"));

        // Direct key lookup...
        assert_eq!(resources.synonym("阿为特"), Some("873693"));
        // ...and the mirrored reverse direction.
        assert_eq!(resources.synonym("873693"), Some("阿为特"));
        assert_eq!(resources.synonym("873726"), Some("卓兆点胶"));
        assert_eq!(resources.synonym("missing"), None);

        let (ner, synonym) = resources.into_parts();
        assert_eq!(ner.len(), 3);
        assert_eq!(synonym.len(), 4); // 2 entries + 2 mirrored
    }

    #[test]
    fn missing_and_malformed_files_degrades_to_empty_tables() {
        let dir = tempfile::tempdir().expect("tempdir");

        // No files at all → empty tables, not an error.
        let empty = DeepDocResources::load(dir.path()).expect("empty dir loads");
        assert!(empty.ner_map().is_empty());
        assert!(empty.synonym_map().is_empty());

        // Malformed JSON → empty table for that file, others still load.
        std::fs::write(dir.path().join("ner.json"), "{not json").expect("write bad ner");
        std::fs::write(dir.path().join("synonym.json"), r#"{"a":"b"}"#)
            .expect("write good synonym");
        let degraded = DeepDocResources::load(dir.path()).expect("degraded load");
        assert!(degraded.ner_map().is_empty());
        assert_eq!(degraded.synonym("a"), Some("b"));
    }

    #[test]
    fn from_env_returns_none_when_res_dir_missing() {
        // Point at a directory that does not exist → None (optional
        // enrichment skipped), never an error.
        let bogus = std::env::temp_dir().join(format!("rayrag-res-none-{}", std::process::id()));
        unsafe {
            std::env::set_var(RAYRAG_RES_PATH_ENV, &bogus);
        }
        let result = DeepDocResources::from_env().expect("from_env never errors");
        unsafe {
            std::env::remove_var(RAYRAG_RES_PATH_ENV);
        }
        assert!(result.is_none());
    }

    #[test]
    fn resolve_res_dir_prefers_rayrag_env_over_ragflow_env() {
        unsafe {
            std::env::set_var(RAYRAG_RES_PATH_ENV, "/tmp/rayrag-res");
            std::env::set_var(RAGFLOW_RES_PATH_ENV, "/tmp/ragflow-res");
        }
        let resolved = resolve_res_dir();
        unsafe {
            std::env::remove_var(RAYRAG_RES_PATH_ENV);
            std::env::remove_var(RAGFLOW_RES_PATH_ENV);
        }
        assert_eq!(resolved, PathBuf::from("/tmp/rayrag-res"));

        unsafe {
            std::env::set_var(RAGFLOW_RES_PATH_ENV, "/tmp/ragflow-res");
        }
        let resolved = resolve_res_dir();
        unsafe {
            std::env::remove_var(RAGFLOW_RES_PATH_ENV);
        }
        assert_eq!(resolved, PathBuf::from("/tmp/ragflow-res"));

        // Neither set → the default relative directory.
        assert_eq!(resolve_res_dir(), PathBuf::from(DEFAULT_RES_DIR));
    }
}
