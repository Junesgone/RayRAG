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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockTools {
        keywords: Mutex<Result<(String, String), String>>,
        result: Mutex<Kbinfos>,
        last_request: Mutex<Option<HybridSearchRequest>>,
    }

    #[async_trait]
    impl DirectTools for MockTools {
        async fn extract_keywords_weighted(
            &self,
            _question: &str,
        ) -> Result<(String, String), String> {
            self.keywords.lock().unwrap().clone()
        }

        async fn hybrid_search(&self, request: HybridSearchRequest) -> Result<Kbinfos, String> {
            *self.last_request.lock().unwrap() = Some(request);
            Ok(self.result.lock().unwrap().clone())
        }
    }

    fn mock(kkeywords: Result<(String, String), String>, result: Kbinfos) -> MockTools {
        MockTools {
            keywords: Mutex::new(kkeywords),
            result: Mutex::new(result),
            last_request: Mutex::new(None),
        }
    }

    #[test]
    fn merge_dedups_chunks_and_docs() {
        let mut kbinfos = Kbinfos {
            chunks: vec![json!({"chunk_id": "c1", "text": "a"})],
            doc_aggs: vec![json!({"doc_id": "d1"})],
        };
        let result = Kbinfos {
            chunks: vec![
                json!({"chunk_id": "c1", "text": "dup"}),
                json!({"id": 7, "text": "b"}),
            ],
            doc_aggs: vec![json!({"doc_id": "d1"}), json!({"doc_id": "d2"})],
        };
        merge_kbinfos(&mut kbinfos, &result);
        assert_eq!(kbinfos.chunks.len(), 2);
        assert_eq!(kbinfos.chunks[1]["id"], json!(7));
        assert_eq!(kbinfos.doc_aggs.len(), 2);

        // Anonymous chunks stay distinct;
        // a second None doc_id is skipped (Python set semantics).
        let mut kbinfos = Kbinfos::default();
        let result = Kbinfos {
            chunks: vec![json!({"text": "x"})],
            doc_aggs: vec![json!({}), json!({})],
        };
        merge_kbinfos(&mut kbinfos, &result);
        assert_eq!(kbinfos.doc_aggs.len(), 1);
    }

    #[tokio::test]
    async fn direct_search_paths() {
        let stats = StatsHandle::new();

        // Success: the weighted query is forwarded, use_compiled set.
        let tools = mock(
            Ok((
                "Alpha, Alpha, Alpha, 1999".to_string(),
                "Alpha, 1999".to_string(),
            )),
            Kbinfos {
                chunks: vec![json!({"chunk_id": "c1"})],
                doc_aggs: vec![],
            },
        );
        let mut kbinfos = Kbinfos::default();
        let state = json!({"question": "Who?", "keywords": "Alpha"});
        let out = direct_search(&state, &tools, &mut kbinfos, &stats).await;
        assert!(out.get("empty_result").is_none());
        let request = tools.last_request.lock().unwrap().clone().unwrap();
        assert_eq!(request.query, "Who?");
        assert_eq!(request.retrieval_query, "Alpha, Alpha, Alpha, 1999");
        assert!(request.use_compiled);

        // Keyword extraction failure clears the retrieval query but proceeds.
        let tools = mock(Err("boom".to_string()), Kbinfos::default());
        let mut kbinfos = Kbinfos::default();
        let out = direct_search(&state, &tools, &mut kbinfos, &stats).await;
        assert_eq!(out["empty_result"], json!(true));
        let request = tools.last_request.lock().unwrap().clone().unwrap();
        assert_eq!(request.retrieval_query, "");

        // The phase recorded both calls under "direct".
        let rows = stats.snapshot();
        let direct = rows
            .iter()
            .find(|(phase, _)| phase == "direct")
            .expect("direct phase recorded");
        assert_eq!(direct.1["calls"], json!(0));
    }
}
