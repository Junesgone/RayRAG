//! Low mode: direct single-pass search — RAGFlow v0.27.2
//! `rag/advanced_rag/harness/orchestrator/direct.py`.
//!
//! One hybrid search whose result is merged into the central `kbinfos` pool,
//! with the entity-weighted retrieval query attached (a problem-level search
//! over the bare question is exactly where the entity must dominate the
//! ranking).

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::harness::stats::StatsHandle;

/// Mutable retrieval pool (`tools.kbinfos`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Kbinfos {
    pub chunks: Vec<Value>,
    pub doc_aggs: Vec<Value>,
    /// `kbinfos["pre_summary"]` — the intermediate draft reviewed by the SCA.
    pub pre_summary: Option<String>,
}

impl Kbinfos {
    /// `_has_chunks(tools)`.
    pub fn has_chunks(&self) -> bool {
        !self.chunks.is_empty()
    }

    pub fn as_json(&self) -> Value {
        json!({"chunks": self.chunks, "doc_aggs": self.doc_aggs})
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map(|n| n != 0.0).unwrap_or(true),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

fn value_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `_chunk_key`: `chunk_id` or `id` (Python truthiness), else an anonymous key.
/// Upstream falls back to `str(id(ck))` (object identity); the positional key
/// keeps distinct anonymous chunks distinct, which is the observable contract.
fn chunk_key(chunk: &Value, index: usize) -> String {
    for key in ["chunk_id", "id"] {
        if let Some(value) = chunk.get(key)
            && truthy(value)
        {
            return value_str(value);
        }
    }
    format!("anon:{index}")
}

/// `_merge_kbinfos`: append unseen chunks and document aggregates.
pub fn merge_kbinfos(kbinfos: &mut Kbinfos, result: &Kbinfos) {
    let mut seen: std::collections::HashSet<String> = kbinfos
        .chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| chunk_key(chunk, index))
        .collect();
    for (index, chunk) in result.chunks.iter().enumerate() {
        let key = chunk_key(chunk, index);
        if seen.insert(key) {
            kbinfos.chunks.push(chunk.clone());
        }
    }
    let mut doc_seen: std::collections::HashSet<Option<String>> = kbinfos
        .doc_aggs
        .iter()
        .map(|doc| {
            doc.get("doc_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    for doc in &result.doc_aggs {
        let key = doc
            .get("doc_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        if doc_seen.insert(key) {
            kbinfos.doc_aggs.push(doc.clone());
        }
    }
}

/// The `hybrid_search` call the direct path performs.
#[derive(Debug, Clone, Default)]
pub struct HybridSearchRequest {
    pub query: String,
    pub keywords: String,
    pub retrieval_query: String,
    pub use_compiled: bool,
}

/// The `RAGTools` surface `direct_search` drives.
#[async_trait]
pub trait DirectTools: Send + Sync {
    /// `tools._extract_keywords_weighted(question)`.
    async fn extract_keywords_weighted(&self, question: &str) -> Result<(String, String), String>;

    /// `hybrid_search(tools, query=…, keywords=…, retrieval_query=…, use_compiled=True)`.
    async fn hybrid_search(&self, request: HybridSearchRequest) -> Result<Kbinfos, String>;
}

/// `direct_search`: single hybrid search → merge into kbinfos.
///
/// Returns the upstream state delta: `{"empty_result": true, "kbinfos": …}`
/// when nothing matched, otherwise `{"kbinfos": …}`.
pub async fn direct_search(
    state: &Value,
    tools: &dyn DirectTools,
    kbinfos: &mut Kbinfos,
    stats: &StatsHandle,
) -> Value {
    let _phase = stats.enter_phase("direct");
    let question = state
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let keywords = state
        .get("keywords")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    // Entity/qualifier-weighted retrieval query: a problem-level search over
    // the bare question is exactly where the entity must dominate the
    // ranking, so the weighted query (entity x3, qualifier x3) is attached.
    let mut retrieval_query = String::new();
    if let Ok((query, _)) = tools.extract_keywords_weighted(&question).await {
        retrieval_query = query;
    }

    let result = tools
        .hybrid_search(HybridSearchRequest {
            query: question,
            keywords,
            retrieval_query,
            use_compiled: true,
        })
        .await
        .unwrap_or_default();
    merge_kbinfos(kbinfos, &result);

    if !kbinfos.has_chunks() {
        return json!({"empty_result": true, "kbinfos": kbinfos.as_json()});
    }
    json!({"kbinfos": kbinfos.as_json()})
}


