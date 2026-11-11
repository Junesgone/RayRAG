//! Retrieval benchmark harness mirroring `rag/benchmark.py`.
//!
//! The Python module evaluates a RAGFlow knowledge base against the
//! MS MARCO v1.1 / TriviaQA / MIRACL datasets and reports `ndcg@10`,
//! `map@5`, `mrr@10` (via `ranx`). This Rust port keeps the same dataset
//! vocabulary, the same per-query retrieval flow (ranked chunks with
//! similarity scores, queries with empty results dropped), the same metrics,
//! the same per-query latency capture, and the same result artifacts
//! (`<dataset>result.md`, `<dataset>.qrels.json`, `<dataset>.run.json`).
//!
//! Rank order is preserved: the Python `run` dict iterates in insertion
//! order (retrieval rank), so runs here are `Vec<(doc_id, score)>` in rank
//! order rather than a sorted map.

use std::collections::BTreeMap;
use std::time::Instant;

/// A qrels / run column: `(doc_id, score)` pairs. Qrels use 0/1 relevance;
/// runs keep retrieval rank order with the similarity score.
pub type ScorePairs = Vec<(String, f64)>;
/// `query -> ranked pairs` (mirrors `defaultdict(dict)` in the Python).
pub type ScoreTable = BTreeMap<String, ScorePairs>;

/// Datasets supported by `rag/benchmark.py::Benchmark.__call__`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchmarkDataset {
    MsMarcoV11,
    TriviaQa,
    Miracl,
}

/// MIRACL languages iterated by the Python benchmark (`__call__`).
pub const MIRACL_LANGUAGES: &[&str] = &[
    "ar", "bn", "de", "en", "es", "fa", "fi", "fr", "hi", "id", "ja", "ko", "ru", "sw", "te", "th",
    "yo", "zh",
];

impl BenchmarkDataset {
    pub fn parse(value: &str) -> Option<BenchmarkDataset> {
        match value {
            "ms_marco_v1.1" => Some(BenchmarkDataset::MsMarcoV11),
            "trivia_qa" => Some(BenchmarkDataset::TriviaQa),
            "miracl" => Some(BenchmarkDataset::Miracl),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            BenchmarkDataset::MsMarcoV11 => "ms_marco_v1.1",
            BenchmarkDataset::TriviaQa => "trivia_qa",
            BenchmarkDataset::Miracl => "miracl",
        }
    }
}

/// CLI-style benchmark configuration (`benchmark.py <max_docs> <kb_id>
/// <dataset> <dataset_path> [<miracl_corpus_path>]`).
#[derive(Debug, Clone, PartialEq)]
pub struct BenchmarkConfig {
    pub max_docs: usize,
    pub kb_id: String,
    pub dataset: BenchmarkDataset,
    pub dataset_path: String,
    pub miracl_corpus_path: String,
}

impl BenchmarkConfig {
    pub fn new(
        max_docs: usize,
        kb_id: impl Into<String>,
        dataset: BenchmarkDataset,
        dataset_path: impl Into<String>,
        miracl_corpus_path: impl Into<String>,
    ) -> Self {
        Self {
            max_docs,
            kb_id: kb_id.into(),
            dataset,
            dataset_path: dataset_path.into(),
            miracl_corpus_path: miracl_corpus_path.into(),
        }
    }

    /// Validate the on-disk layout the Python `__call__` checks before
    /// running: qrels/topics directories per MIRACL language and the corpus
    /// directory. Pure path checks; returns the first missing directory.
    pub fn validate_dataset_layout(&self) -> Result<(), String> {
        match self.dataset {
            BenchmarkDataset::MsMarcoV11 | BenchmarkDataset::TriviaQa => {
                if self.max_docs == 0 {
                    return Err("max_docs must be a positive integer".into());
                }
                Ok(())
            }
            BenchmarkDataset::Miracl => {
                for lang in MIRACL_LANGUAGES {
                    let base = format!("{}/miracl-v1.0-{}", self.dataset_path, lang);
                    for required in ["qrels", "topics"] {
                        let dir = format!("{base}/{required}");
                        if !std::path::Path::new(&dir).is_dir() {
                            return Err(format!("Directory: {dir} not found!"));
                        }
                    }
                    let corpus = format!("{}/miracl-corpus-v1.0-{}", self.miracl_corpus_path, lang);
                    if !std::path::Path::new(&corpus).is_dir() {
                        return Err(format!("Directory: {corpus} not found!"));
                    }
                }
                Ok(())
            }
        }
    }
}

/// One ranked retrieval result for a query (chunk id + similarity), the
/// shape `ranks["chunks"]` carries in `_get_retrieval`.
#[derive(Debug, Clone, PartialEq)]
pub struct RankedChunk {
    pub chunk_id: String,
    pub similarity: f64,
}

/// Abstraction over the retrieval backend so the benchmark can run against a
/// live store or a stub in tests (`settings.retriever.retrieval(...)` in the
/// Python, which also sleeps 20s for index readiness before the first query —
/// that wait is the caller's concern here).
#[async_trait::async_trait]
pub trait RetrievalRunner: Send + Sync {
    async fn retrieve(&self, query: &str, top_k: usize) -> Vec<RankedChunk>;
}

/// Per-query retrieval latency captured by the benchmark loop.
#[derive(Debug, Clone, PartialEq)]
pub struct LatencyStats {
    pub count: usize,
    pub avg_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

impl Default for LatencyStats {
    fn default() -> Self {
        Self {
            count: 0,
            avg_ms: 0.0,
            p50_ms: 0.0,
            p95_ms: 0.0,
            max_ms: 0.0,
        }
    }
}

impl LatencyStats {
    pub fn from_millis(mut samples: Vec<f64>) -> LatencyStats {
        let count = samples.len();
        if count == 0 {
            return LatencyStats::default();
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let avg_ms = samples.iter().sum::<f64>() / count as f64;
        let percentile = |p: f64| {
            let idx = ((count as f64 - 1.0) * p).round() as usize;
            samples[idx.min(count - 1)]
        };
        LatencyStats {
            count,
            avg_ms,
            p50_ms: percentile(0.50),
            p95_ms: percentile(0.95),
            max_ms: samples[count - 1],
        }
    }
}

/// Look up a doc's qrel relevance (0.0 when absent).
fn qrel_of(qrels: &[(String, f64)], doc: &str) -> f64 {
    qrels
        .iter()
        .find(|(id, _)| id == doc)
        .map(|(_, relevance)| *relevance)
        .unwrap_or(0.0)
}

/// DCG@k with the standard `log2(i+2)` discount (ranx semantics), iterating
/// the ranked relevance scores in rank order.
pub fn dcg_at_k(ranked: &[f64], k: usize) -> f64 {
    ranked
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, relevance)| relevance / (i as f64 + 2.0).log2())
        .sum()
}

/// `ndcg@k` — DCG of the run (qrel relevance in rank order) over DCG of the
/// ideal (relevance-descending) ordering. Zero when there is no relevant
/// document in the top-k.
pub fn ndcg_at_k(qrels: &ScorePairs, run: &ScorePairs, k: usize) -> f64 {
    let mut ideal: Vec<f64> = qrels.iter().map(|(_, relevance)| *relevance).collect();
    ideal.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let idcg = dcg_at_k(&ideal, k);
    if idcg <= 0.0 {
        return 0.0;
    }
    let run_relevance: Vec<f64> = run
        .iter()
        .take(k)
        .map(|(doc, _)| qrel_of(qrels, doc))
        .collect();
    dcg_at_k(&run_relevance, k) / idcg
}

/// Average precision at k for one query (`map@k` component), rank order.
pub fn average_precision_at_k(qrels: &ScorePairs, run: &ScorePairs, k: usize) -> f64 {
    let relevant_total = qrels.iter().filter(|(_, r)| *r > 0.0).count();
    if relevant_total == 0 {
        return 0.0;
    }
    let mut hits = 0usize;
    let mut sum = 0.0;
    for (rank, (doc, _)) in run.iter().take(k).enumerate() {
        if qrel_of(qrels, doc) > 0.0 {
            hits += 1;
            sum += hits as f64 / (rank as f64 + 1.0);
        }
    }
    sum / relevant_total.min(k) as f64
}

/// `mrr@k` — reciprocal rank of the first relevant document within top-k.
pub fn reciprocal_rank_at_k(qrels: &ScorePairs, run: &ScorePairs, k: usize) -> f64 {
    for (rank, (doc, _)) in run.iter().take(k).enumerate() {
        if qrel_of(qrels, doc) > 0.0 {
            return 1.0 / (rank as f64 + 1.0);
        }
    }
    0.0
}

/// Overall metrics, averaged over queries (`ranx` `evaluate` equivalent).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BenchmarkMetrics {
    pub ndcg10: f64,
    pub map5: f64,
    pub mrr10: f64,
    pub queries: usize,
}

impl BenchmarkMetrics {
    pub fn evaluate(qrels: &ScoreTable, run: &ScoreTable) -> BenchmarkMetrics {
        let mut metrics = BenchmarkMetrics::default();
        for (query, qrel_pairs) in qrels {
            let Some(run_pairs) = run.get(query) else {
                continue;
            };
            metrics.queries += 1;
            metrics.ndcg10 += ndcg_at_k(qrel_pairs, run_pairs, 10);
            metrics.map5 += average_precision_at_k(qrel_pairs, run_pairs, 5);
            metrics.mrr10 += reciprocal_rank_at_k(qrel_pairs, run_pairs, 10);
        }
        if metrics.queries > 0 {
            let n = metrics.queries as f64;
            metrics.ndcg10 /= n;
            metrics.map5 /= n;
            metrics.mrr10 /= n;
        }
        metrics
    }
}

/// One query's per-query ndcg@10 row, mirroring the `save_results` table.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryScoreRow {
    pub query: String,
    pub ndcg10: f64,
    /// Top-10 run entries: (doc text, qrel score).
    pub top_passages: Vec<(String, f64)>,
}

/// Full benchmark run output, mirroring the artifacts written by
/// `save_results` plus latency capture (the latency reporting is the Rust
/// addition the port asks for).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BenchmarkReport {
    pub dataset: String,
    pub metrics: BenchmarkMetrics,
    pub latency: LatencyStats,
    pub queries_deleted: usize,
    pub rows: Vec<QueryScoreRow>,
}

impl BenchmarkReport {
    /// `save_results` markdown: "## Score For Every Query" with per-query
    /// ndcg@10 sorted ascending, then the top-10 passages.
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("## Score For Every Query\n");
        let mut rows = self.rows.clone();
        rows.sort_by(|a, b| {
            a.ndcg10
                .partial_cmp(&b.ndcg10)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for row in &rows {
            out.push_str(&format!(
                "### query: {} ndcg@10:{}\n",
                row.query, row.ndcg10
            ));
            for (text, qrel) in &row.top_passages {
                out.push_str(&format!("- text: {}\t qrel: {}\n", text, qrel));
            }
        }
        out
    }
}

/// Run the benchmark: for every query in `qrels`, retrieve top-k chunks,
/// drop queries with empty results (mirroring `_get_retrieval`), measure
/// latency, then score.
pub async fn run_retrieval_benchmark(
    runner: &dyn RetrievalRunner,
    qrels: &ScoreTable,
    texts: &BTreeMap<String, String>,
    top_k: usize,
) -> BenchmarkReport {
    let mut run: ScoreTable = BTreeMap::new();
    let mut deleted = 0usize;
    let mut latencies = Vec::with_capacity(qrels.len());
    for (query, qrel_pairs) in qrels {
        let started = Instant::now();
        let ranked = runner.retrieve(query, top_k).await;
        latencies.push(started.elapsed().as_secs_f64() * 1000.0);
        if ranked.is_empty() {
            deleted += 1;
            continue;
        }
        run.insert(
            query.clone(),
            ranked
                .iter()
                .map(|chunk| (chunk.chunk_id.clone(), chunk.similarity))
                .collect(),
        );
        let _ = qrel_pairs;
    }
    let metrics = BenchmarkMetrics::evaluate(qrels, &run);
    let rows = qrels
        .keys()
        .filter(|query| run.contains_key(*query))
        .map(|query| QueryScoreRow {
            query: query.clone(),
            ndcg10: ndcg_at_k(&qrels[query], &run[query], 10),
            top_passages: run[query]
                .iter()
                .take(10)
                .map(|(doc, _)| {
                    (
                        texts.get(doc).cloned().unwrap_or_default(),
                        qrel_of(&qrels[query], doc),
                    )
                })
                .collect(),
        })
        .collect();
    BenchmarkReport {
        dataset: String::new(),
        metrics,
        latency: LatencyStats::from_millis(latencies),
        queries_deleted: deleted,
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build rank-ordered pairs in the given (insertion) order, like the
    /// Python `run` dict.
    fn pairs(entries: &[(&str, f64)]) -> ScorePairs {
        entries
            .iter()
            .map(|(doc, score)| (doc.to_string(), *score))
            .collect()
    }

    #[test]
    fn ndcg_mrr_and_map_match_ranx_on_a_small_example() {
        let qrels = pairs(&[("d1", 1.0), ("d2", 1.0), ("d3", 0.0), ("d4", 1.0)]);
        // Perfect ordering: d1, d2, d4 relevant at ranks 1..3.
        let perfect = pairs(&[("d1", 0.9), ("d2", 0.8), ("d4", 0.7), ("d3", 0.1)]);
        assert!((ndcg_at_k(&qrels, &perfect, 10) - 1.0).abs() < 1e-9);
        assert!((reciprocal_rank_at_k(&qrels, &perfect, 10) - 1.0).abs() < 1e-9);
        assert!((average_precision_at_k(&qrels, &perfect, 5) - 1.0).abs() < 1e-9);

        // First relevant document at rank 2: mrr = 0.5, ndcg = dcg/idcg.
        let run = pairs(&[("d3", 0.9), ("d1", 0.8), ("d2", 0.7), ("d4", 0.1)]);
        assert!((reciprocal_rank_at_k(&qrels, &run, 10) - 0.5).abs() < 1e-9);
        // Rank order relevance: d3→0, d1→1, d2→1, d4→1.
        let dcg =
            0.0 / 2.0f64.log2() + 1.0 / 3.0f64.log2() + 1.0 / 4.0f64.log2() + 1.0 / 5.0f64.log2();
        let idcg = 1.0 / 2.0f64.log2() + 1.0 / 3.0f64.log2() + 1.0 / 4.0f64.log2();
        assert!((ndcg_at_k(&qrels, &run, 10) - dcg / idcg).abs() < 1e-9);

        // No relevant document in the run → zero across the board.
        let empty = pairs(&[("d9", 0.5)]);
        assert_eq!(ndcg_at_k(&qrels, &empty, 10), 0.0);
        assert_eq!(reciprocal_rank_at_k(&qrels, &empty, 10), 0.0);
        assert_eq!(average_precision_at_k(&qrels, &empty, 5), 0.0);
    }

    #[test]
    fn metrics_average_over_queries_like_ranx_evaluate() {
        let qrels: ScoreTable = [
            ("q1".to_owned(), pairs(&[("a", 1.0), ("b", 1.0)])),
            ("q2".to_owned(), pairs(&[("c", 1.0)])),
        ]
        .into_iter()
        .collect();
        let run: ScoreTable = [
            ("q1".to_owned(), pairs(&[("a", 0.9), ("b", 0.8)])),
            // q2's run puts an irrelevant doc at rank 1 (z), c at rank 2.
            ("q2".to_owned(), pairs(&[("z", 0.9), ("c", 0.7)])),
        ]
        .into_iter()
        .collect();
        let metrics = BenchmarkMetrics::evaluate(&qrels, &run);
        assert_eq!(metrics.queries, 2);
        // q1: ndcg = 1; q2: c at rank 2 → dcg = 1/log2(3), idcg = 1.
        let expected_ndcg = (1.0 + 1.0 / 3.0f64.log2()) / 2.0;
        assert!((metrics.ndcg10 - expected_ndcg).abs() < 1e-9);
        // q1: ap@5 = 1; q2: hit at rank 2 → 1/2 → mean 0.75.
        assert!((metrics.map5 - 0.75).abs() < 1e-9);
        // q1: rr = 1; q2: rr = 1/2 → mean 0.75.
        assert!((metrics.mrr10 - 0.75).abs() < 1e-9);
    }

    #[test]
    fn latency_stats_compute_avg_p50_p95_and_max() {
        let stats = LatencyStats::from_millis(vec![10.0, 20.0, 30.0, 40.0, 50.0]);
        assert_eq!(stats.count, 5);
        assert!((stats.avg_ms - 30.0).abs() < 1e-9);
        assert!((stats.p50_ms - 30.0).abs() < 1e-9);
        assert!((stats.p95_ms - 50.0).abs() < 1e-9);
        assert!((stats.max_ms - 50.0).abs() < 1e-9);
        let empty = LatencyStats::from_millis(vec![]);
        assert_eq!(empty.count, 0);
        assert_eq!(empty.avg_ms, 0.0);
    }

    #[test]
    fn benchmark_loop_drops_empty_queries_and_scores_the_rest() {
        struct StubRunner;
        #[async_trait::async_trait]
        impl RetrievalRunner for StubRunner {
            async fn retrieve(&self, query: &str, top_k: usize) -> Vec<RankedChunk> {
                if query == "q2" {
                    return vec![];
                }
                vec![
                    RankedChunk {
                        chunk_id: "a".into(),
                        similarity: 0.9,
                    },
                    RankedChunk {
                        chunk_id: "b".into(),
                        similarity: 0.8,
                    },
                ]
                .into_iter()
                .take(top_k)
                .collect()
            }
        }
        let qrels: ScoreTable = [
            ("q1".to_owned(), pairs(&[("a", 1.0), ("b", 1.0)])),
            ("q2".to_owned(), pairs(&[("c", 1.0)])),
        ]
        .into_iter()
        .collect();
        let texts: BTreeMap<String, String> = [
            ("a".to_owned(), "doc a".to_owned()),
            ("b".to_owned(), "doc b".to_owned()),
        ]
        .into_iter()
        .collect();
        let report = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run_retrieval_benchmark(&StubRunner, &qrels, &texts, 30));
        // q2 was dropped from the run, so only q1 is scored.
        assert_eq!(report.queries_deleted, 1);
        assert_eq!(report.metrics.queries, 1);
        assert!((report.metrics.ndcg10 - 1.0).abs() < 1e-9);
        assert_eq!(report.rows.len(), 1);
        let markdown = report.to_markdown();
        assert!(markdown.starts_with("## Score For Every Query\n"));
        assert!(markdown.contains("ndcg@10:1"));
        assert!(markdown.contains("doc a"));
    }

    #[test]
    fn dataset_parse_and_layout_validation_match_python() {
        assert_eq!(
            BenchmarkDataset::parse("ms_marco_v1.1"),
            Some(BenchmarkDataset::MsMarcoV11)
        );
        assert_eq!(
            BenchmarkDataset::parse("trivia_qa"),
            Some(BenchmarkDataset::TriviaQa)
        );
        assert_eq!(
            BenchmarkDataset::parse("miracl"),
            Some(BenchmarkDataset::Miracl)
        );
        assert_eq!(BenchmarkDataset::parse("nonsense"), None);
        assert_eq!(BenchmarkDataset::MsMarcoV11.name(), "ms_marco_v1.1");
        assert_eq!(MIRACL_LANGUAGES.len(), 18);
        assert!(MIRACL_LANGUAGES.contains(&"zh"));
        assert!(MIRACL_LANGUAGES.contains(&"en"));

        let config = BenchmarkConfig::new(0, "kb", BenchmarkDataset::MsMarcoV11, "/tmp/bench", "");
        assert!(config.validate_dataset_layout().is_err());
        let config = BenchmarkConfig::new(
            100,
            "kb",
            BenchmarkDataset::Miracl,
            "/tmp/does-not-exist",
            "/tmp/corpus-does-not-exist",
        );
        let err = config.validate_dataset_layout().unwrap_err();
        assert!(err.contains("miracl-v1.0-ar/qrels"), "unexpected: {err}");
    }
}
