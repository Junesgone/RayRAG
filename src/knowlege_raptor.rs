//! Knowledge-compile RAPTOR — recursive abstractive processing for
//! tree-organized retrieval. RAGFlow v0.27.2
//! `rag/advanced_rag/knowlege_compile/raptor.py` (the 1D-watershed
//! `RecursiveAbstractiveProcessing4TreeOrganizedRetrieval` variant; the
//! older `rag/raptor.py` capability lives in `src/raptor.rs`).
//!
//! Builds summary layers with the classic 1D-watershed clustering strategy:
//! adjacent chunk embeddings are compared (O(N)), the split threshold comes
//! from a percentile of the adjacent-similarity distribution and is lowered
//! when the cluster-count cap demands it. Summaries are produced per cluster
//! and appended until one root remains.
//!
//! Port mapping: LLM/embedding calls arrive through [`RaptorBackend`] (the
//! host owns caching, retries, timeouts and the `chat_limiter`); numpy vector
//! math is implemented directly (L2 norm, dot product, percentile with linear
//! interpolation, sort/unique). Divergence: cluster summaries run sequentially
//! (Python gathers them concurrently).

use serde_json::Value;

/// RAPTOR failures (`TaskCanceledException` / fatal error count).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RaptorError {
    Cancelled,
    Aborted(String),
}

/// One chunk in the RAPTOR pipeline (normalized 3-or-4 tuple).
#[derive(Debug, Clone, PartialEq)]
pub struct RaptorChunk {
    pub text: String,
    pub vector: Vec<f32>,
    pub source_chunk_ids: Vec<String>,
    /// Summary title (empty for layer-0 originals).
    pub title: String,
}

/// LLM + embedding surface the RAPTOR class drives.
#[async_trait::async_trait]
pub trait RaptorBackend: Send + Sync {
    /// `self._chat(...)`: returns the raw assistant text; `None` = failure
    /// (the host owns the 20-minute bound, cache and the 3-attempt retry).
    async fn chat(&self, system: &str, history: &[Value], gen_conf: &Value) -> Option<String>;
    /// `_embedding_encode`: `None` = empty/failed embedding.
    async fn encode(&self, text: &str) -> Option<Vec<f32>>;
    /// `self._llm_model.max_length`.
    fn max_length(&self) -> usize;
    /// `self._llm_model.llm_name` (feeds `knowledge_compile_gen_conf`).
    fn model_name(&self) -> String;
    /// `has_canceled(task_id)`.
    fn is_canceled(&self, task_id: &str) -> bool;
}

/// `RecursiveAbstractiveProcessing4TreeOrganizedRetrieval`.
pub struct Raptor4Tree {
    pub max_cluster: usize,
    pub small_layer_collapse: usize,
    pub clustering_threshold: f64,
    pub clustering_ratio: f64,
    pub prompt: String,
    pub max_token: usize,
    pub max_errors: usize,
    pub error_count: usize,
}

impl Raptor4Tree {
    /// `__init__` (max_cluster / prompt are required; the rest default).
    pub fn new(max_cluster: usize, prompt: &str) -> Self {
        Self {
            max_cluster,
            small_layer_collapse: 8,
            clustering_threshold: 0.3,
            clustering_ratio: 0.5,
            prompt: prompt.to_string(),
            max_token: 512,
            max_errors: 3,
            error_count: 0,
        }
    }

    /// Configure `max_token` (clamped to `[512, 2048]` like upstream).
    pub fn with_max_token(mut self, max_token: usize) -> Self {
        self.max_token = max_token.clamp(512, 2048);
        self
    }

    pub fn with_small_layer_collapse(mut self, value: usize) -> Self {
        self.small_layer_collapse = value;
        self
    }

    pub fn with_clustering(mut self, threshold: f64, ratio: f64) -> Self {
        self.clustering_threshold = threshold;
        self.clustering_ratio = ratio;
        self
    }

    /// `_check_task_canceled`.
    fn check_canceled(
        &self,
        backend: &dyn RaptorBackend,
        task_id: &str,
    ) -> std::result::Result<(), RaptorError> {
        if !task_id.is_empty() && backend.is_canceled(task_id) {
            return Err(RaptorError::Cancelled);
        }
        Ok(())
    }

    /// `_get_clusters_ahc`: 1D-watershed segmentation over adjacent cosine
    /// similarities (O(N)); adapts the threshold to the layer's similarity
    /// range and lowers it when the cluster-count cap demands it.
    pub fn get_clusters_ahc(&self, embeddings: &[Vec<f32>], task_id: &str) -> Vec<usize> {
        let n = embeddings.len();
        if n <= 1 {
            return vec![0; n];
        }
        let _ = task_id;
        let mut normalized: Vec<Vec<f32>> = Vec::with_capacity(n);
        for vector in embeddings {
            let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
            let norm = if norm == 0.0 { 1.0 } else { norm };
            normalized.push(vector.iter().map(|value| value / norm).collect());
        }
        let mut adj_sims: Vec<f32> = Vec::with_capacity(n - 1);
        for index in 0..n - 1 {
            let sim: f32 = normalized[index]
                .iter()
                .zip(normalized[index + 1].iter())
                .map(|(a, b)| a * b)
                .sum();
            adj_sims.push(sim);
        }
        let mut sorted_sims = adj_sims.clone();
        sorted_sims.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let max_clusters = ((n as f64 * self.clustering_ratio).round() as usize).max(1);
        let watershed = |threshold: f32| -> Vec<usize> {
            let mut labels = vec![0usize; n];
            let mut cid = 0usize;
            for i in 1..n {
                if adj_sims[i - 1] >= threshold {
                    labels[i] = cid;
                } else {
                    cid += 1;
                    labels[i] = cid;
                }
            }
            labels
        };
        let pct = ((self.clustering_threshold * 100.0).round() as i64).clamp(1, 99) as usize;
        let mut threshold = percentile(&sorted_sims, pct);
        let mut labels = watershed(threshold);
        let mut n_clusters = unique_count(&labels);
        if n_clusters > max_clusters && sorted_sims.len() >= max_clusters {
            let adjusted = sorted_sims[(max_clusters - 1).min(sorted_sims.len() - 1)];
            if adjusted < threshold {
                threshold = adjusted;
                labels = watershed(threshold);
                n_clusters = unique_count(&labels);
            }
        }
        let _ = n_clusters;
        labels
    }

    /// `clustering`: dense relabeling of the watershed labels.
    pub fn clustering(&self, embeddings: &[Vec<f32>], task_id: &str) -> (usize, Vec<usize>) {
        if embeddings.is_empty() {
            return (0, Vec::new());
        }
        let labels = self.get_clusters_ahc(embeddings, task_id);
        if labels.is_empty() {
            return (0, Vec::new());
        }
        let mut unique: Vec<usize> = Vec::new();
        for label in &labels {
            if !unique.contains(label) {
                unique.push(*label);
            }
        }
        if unique.len() <= 1 {
            return (1, vec![0; labels.len()]);
        }
        let mapped: Vec<usize> = labels
            .iter()
            .map(|label| unique.iter().position(|value| value == label).unwrap_or(0))
            .collect();
        (unique.len(), mapped)
    }

    /// `_summarize_texts`: one cluster → `(title, summary, embedding)`.
    async fn summarize_texts(
        &mut self,
        backend: &dyn RaptorBackend,
        texts: &[String],
        callback: &(dyn Fn(&str) + Send + Sync),
        task_id: &str,
    ) -> std::result::Result<Option<(String, String, Vec<f32>)>, RaptorError> {
        self.check_canceled(backend, task_id)?;
        let div = texts.len().max(1);
        let len_per_chunk = (backend.max_length().saturating_sub(self.max_token)) / div;
        let cluster_content = texts
            .iter()
            .map(|text| truncate_tokens(text, len_per_chunk.max(1)))
            .collect::<Vec<_>>()
            .join("\n");
        let system = format!(
            "You're a helpful assistant.\n\nHelp me with the following task.\n\n{}",
            self.prompt.replace("{cluster_content}", &cluster_content)
        );
        let user =
            "Beside the summarization, give a title at the first line of your summarization. \
Must be in the same language as the paragraphs. \
Keep the summary concise and target approximately the configured token budget."
                .to_string();
        let gen_conf =
            crate::structure_compile::knowledge_compile_gen_conf(&backend.model_name(), None);
        let history = vec![serde_json::json!({"role": "user", "content": user})];
        self.check_canceled(backend, task_id)?;
        let raw = backend
            .chat(&system, &history, &Value::Object(gen_conf))
            .await;
        let Some(raw) = raw else {
            return self.record_error(texts.len(), "chat failed", callback);
        };
        let think = regex::Regex::new(r"(?s)^.*</think>").expect("think regex");
        let mut cleaned = think.replace(&raw, "").to_string();
        if cleaned.contains("**ERROR**") {
            return self.record_error(texts.len(), "model reported **ERROR**", callback);
        }
        let truncation = regex::Regex::new(
            "(······\n由于长度的原因，回答被截断了，要继续吗？|For the content length reason, it stopped, continue?)",
        )
        .expect("truncation regex");
        cleaned = truncation.replace_all(&cleaned, "").to_string();
        let cleaned = cleaned.trim().to_string();
        self.check_canceled(backend, task_id)?;
        let Some(vector) = backend.encode(&cleaned).await else {
            return self.record_error(texts.len(), "empty embedding", callback);
        };
        let title = cleaned.lines().next().unwrap_or("").trim().to_string();
        Ok(Some((title, cleaned, vector)))
    }

    fn record_error(
        &mut self,
        chunk_count: usize,
        message: &str,
        callback: &(dyn Fn(&str) + Send + Sync),
    ) -> std::result::Result<Option<(String, String, Vec<f32>)>, RaptorError> {
        self.error_count += 1;
        callback(&format!(
            "[RAPTOR] Skip cluster ({chunk_count} chunks) due to error: {message}"
        ));
        if self.error_count >= self.max_errors {
            return Err(RaptorError::Aborted(format!(
                "RAPTOR aborted after {} errors. Last error: {message}",
                self.error_count
            )));
        }
        Ok(None)
    }

    /// `__call__`: build summary chunks and layer boundaries (flat result), or
    /// materialize the hierarchical tree when `is_tree`.
    pub async fn call(
        &mut self,
        backend: &dyn RaptorBackend,
        input: Vec<(String, Vec<f32>, Vec<String>)>,
        callback: &(dyn Fn(&str) + Send + Sync),
        task_id: &str,
        is_tree: bool,
    ) -> std::result::Result<RaptorCallOutput, RaptorError> {
        if input.len() <= 1 {
            return Ok(if is_tree {
                RaptorCallOutput::Tree(Value::Null)
            } else {
                RaptorCallOutput::Flat {
                    chunks: Vec::new(),
                    layers: Vec::new(),
                }
            });
        }
        let mut chunks: Vec<RaptorChunk> = Vec::new();
        for (text, vector, source) in input {
            if text.is_empty() || vector.is_empty() {
                continue;
            }
            let source: Vec<String> = source.into_iter().filter(|id| !id.is_empty()).collect();
            chunks.push(RaptorChunk {
                text,
                vector,
                source_chunk_ids: source,
                title: String::new(),
            });
        }
        if chunks.len() <= 1 {
            return Ok(if is_tree {
                RaptorCallOutput::Tree(Value::Null)
            } else {
                RaptorCallOutput::Flat {
                    layers: vec![(0, chunks.len())],
                    chunks,
                }
            });
        }
        let n_originals = chunks.len();
        let mut parent_child_map: std::collections::HashMap<usize, Vec<usize>> =
            std::collections::HashMap::new();
        let mut layers: Vec<(usize, usize)> = vec![(0, chunks.len())];
        let mut start = 0usize;
        let mut end = chunks.len();
        while end > start + 1 {
            self.check_canceled(backend, task_id)?;
            let embeddings: Vec<Vec<f32>> = chunks[start..end]
                .iter()
                .map(|chunk| chunk.vector.clone())
                .collect();
            if end - start <= self.small_layer_collapse {
                let indices: Vec<usize> = (start..end).collect();
                self.summarize_cluster(
                    backend,
                    &mut chunks,
                    &mut parent_child_map,
                    &indices,
                    callback,
                    task_id,
                )
                .await?;
                let produced = chunks.len() - end;
                if produced == 0 {
                    break;
                }
                layers.push((end, chunks.len()));
                callback(&format!(
                    "Cluster one layer: {} -> {} (small-N collapse)",
                    end - start,
                    produced
                ));
                break;
            }
            let (mut n_clusters, mut labels) = self.clustering(&embeddings, task_id);
            if n_clusters >= embeddings.len() {
                n_clusters = 1;
                labels = vec![0; embeddings.len()];
            }
            for cluster in 0..n_clusters {
                let indices: Vec<usize> = labels
                    .iter()
                    .enumerate()
                    .filter(|(_, label)| **label == cluster)
                    .map(|(index, _)| index + start)
                    .collect();
                if indices.is_empty() {
                    continue;
                }
                self.check_canceled(backend, task_id)?;
                self.summarize_cluster(
                    backend,
                    &mut chunks,
                    &mut parent_child_map,
                    &indices,
                    callback,
                    task_id,
                )
                .await?;
            }
            let produced = chunks.len() - end;
            if produced < n_clusters {
                callback(&format!(
                    "RAPTOR layer produced {produced}/{n_clusters} cluster summaries"
                ));
            }
            if produced == 0 {
                break;
            }
            layers.push((end, chunks.len()));
            callback(&format!("Cluster one layer: {} -> {produced}", end - start));
            start = end;
            end = chunks.len();
        }
        if is_tree {
            Ok(RaptorCallOutput::Tree(materialize_tree(
                &chunks,
                &layers,
                &parent_child_map,
                n_originals,
            )))
        } else {
            Ok(RaptorCallOutput::Flat { chunks, layers })
        }
    }

    async fn summarize_cluster(
        &mut self,
        backend: &dyn RaptorBackend,
        chunks: &mut Vec<RaptorChunk>,
        parent_child_map: &mut std::collections::HashMap<usize, Vec<usize>>,
        indices: &[usize],
        callback: &(dyn Fn(&str) + Send + Sync),
        task_id: &str,
    ) -> std::result::Result<(), RaptorError> {
        let texts: Vec<String> = indices
            .iter()
            .map(|index| chunks[*index].text.clone())
            .collect();
        match self
            .summarize_texts(backend, &texts, callback, task_id)
            .await?
        {
            Some((title, text, vector)) => {
                let mut merged_ids: Vec<String> = Vec::new();
                for index in indices {
                    for source in &chunks[*index].source_chunk_ids {
                        if !merged_ids.iter().any(|existing| existing == source) {
                            merged_ids.push(source.clone());
                        }
                    }
                }
                parent_child_map.insert(chunks.len(), indices.to_vec());
                chunks.push(RaptorChunk {
                    text,
                    vector,
                    source_chunk_ids: merged_ids,
                    title,
                });
            }
            None => {}
        }
        Ok(())
    }
}

/// RAPTOR result shape.
#[derive(Debug, Clone, PartialEq)]
pub enum RaptorCallOutput {
    Flat {
        chunks: Vec<RaptorChunk>,
        layers: Vec<(usize, usize)>,
    },
    Tree(Value),
}

/// `_materialize_tree`: walk the parent-child map from the top layer down.
pub fn materialize_tree(
    chunks: &[RaptorChunk],
    layers: &[(usize, usize)],
    parent_child_map: &std::collections::HashMap<usize, Vec<usize>>,
    n_originals: usize,
) -> Value {
    if layers.is_empty() || chunks.is_empty() {
        return Value::Null;
    }
    let (top_start, top_end) = layers[layers.len() - 1];
    if top_end <= top_start {
        return Value::Null;
    }
    fn build_node(
        index: usize,
        chunks: &[RaptorChunk],
        parent_child_map: &std::collections::HashMap<usize, Vec<usize>>,
        n_originals: usize,
    ) -> Value {
        let empty: Vec<usize> = Vec::new();
        let children = parent_child_map.get(&index).unwrap_or(&empty);
        let title = chunks[index].title.clone();
        let description = chunks[index].text.clone();
        if !children.is_empty() && children.iter().all(|child| *child < n_originals) {
            let mut source_chunk_ids: Vec<String> = Vec::new();
            for child in children {
                for id in &chunks[*child].source_chunk_ids {
                    if !source_chunk_ids.iter().any(|existing| existing == id) {
                        source_chunk_ids.push(id.clone());
                    }
                }
            }
            return serde_json::json!({
                "title": title,
                "source_chunk_ids": source_chunk_ids,
                "description": description,
            });
        }
        let nodes: Vec<Value> = children
            .iter()
            .map(|child| build_node(*child, chunks, parent_child_map, n_originals))
            .collect();
        serde_json::json!({
            "children": nodes,
            "title": title,
            "description": description,
        })
    }
    let top_nodes: Vec<Value> = (top_start..top_end)
        .map(|index| build_node(index, chunks, parent_child_map, n_originals))
        .collect();
    if top_nodes.len() == 1 {
        top_nodes.into_iter().next().unwrap_or(Value::Null)
    } else {
        serde_json::json!({"title": "(root)", "children": top_nodes})
    }
}

/// `np.percentile` with linear interpolation over an ascending slice.
fn percentile(sorted_asc: &[f32], pct: usize) -> f32 {
    let m = sorted_asc.len();
    if m == 0 {
        return 0.0;
    }
    if m == 1 {
        return sorted_asc[0];
    }
    let rank = (pct as f64 / 100.0) * (m as f64 - 1.0);
    let low = rank.floor() as usize;
    let high = rank.ceil().min((m - 1) as f64) as usize;
    let frac = rank - low as f64;
    (sorted_asc[low] as f64 * (1.0 - frac) + sorted_asc[high] as f64 * frac) as f32
}

fn unique_count(labels: &[usize]) -> usize {
    let mut unique: Vec<usize> = Vec::new();
    for label in labels {
        if !unique.contains(label) {
            unique.push(*label);
        }
    }
    unique.len()
}

/// `common.token_utils.truncate`: keep the longest prefix whose token count
/// fits `max_tokens` (binary search over char boundaries under the crate
/// tokenizer; upstream uses tiktoken).
fn truncate_tokens(text: &str, max_tokens: usize) -> String {
    if max_tokens == 0 {
        return String::new();
    }
    if crate::chunk::tokenizer::token_count(text) <= max_tokens {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut low = 0usize;
    let mut high = chars.len();
    while low < high {
        let mid = (low + high + 1) / 2;
        let candidate: String = chars[..mid].iter().collect();
        if crate::chunk::tokenizer::token_count(&candidate) <= max_tokens {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    chars[..low].iter().collect()
}
#[cfg(test)]
mod raptor_tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    struct MockBackend {
        summaries: Mutex<Vec<String>>,
        canceled: bool,
    }

    #[async_trait::async_trait]
    impl RaptorBackend for MockBackend {
        async fn chat(
            &self,
            _system: &str,
            _history: &[Value],
            _gen_conf: &Value,
        ) -> Option<String> {
            let mut queue = self.summaries.lock().unwrap();
            if queue.is_empty() {
                None
            } else {
                Some(queue.remove(0))
            }
        }
        async fn encode(&self, text: &str) -> Option<Vec<f32>> {
            // Deterministic pseudo-embedding from the text length.
            let base = text.chars().count() as f32;
            Some(vec![base, base + 1.0, base + 2.0])
        }
        fn max_length(&self) -> usize {
            4096
        }
        fn model_name(&self) -> String {
            "gpt-4o".to_string()
        }
        fn is_canceled(&self, _task_id: &str) -> bool {
            self.canceled
        }
    }

    fn vec_of(values: [f32; 3]) -> Vec<f32> {
        values.to_vec()
    }

    #[test]
    fn watershed_splits_on_low_similarity() {
        let raptor = Raptor4Tree::new(4, "p");
        // Two groups of nearly identical vectors.
        let embeddings = vec![
            vec_of([1.0, 0.0, 0.0]),
            vec_of([0.99, 0.01, 0.0]),
            vec_of([0.0, 1.0, 0.0]),
            vec_of([0.01, 0.99, 0.0]),
        ];
        let (n_clusters, labels) = raptor.clustering(&embeddings, "");
        assert_eq!(n_clusters, 2);
        assert_eq!(labels[0], labels[1]);
        assert_eq!(labels[2], labels[3]);
        assert_ne!(labels[0], labels[2]);
    }

    #[test]
    fn watershed_single_and_empty_inputs() {
        let raptor = Raptor4Tree::new(4, "p");
        assert_eq!(raptor.clustering(&[], ""), (0, Vec::new()));
        let one = vec![vec_of([1.0, 0.0, 0.0])];
        assert_eq!(raptor.clustering(&one, ""), (1, vec![0]));
    }

    #[tokio::test]
    async fn call_flat_builds_layers_and_merges_source_ids() {
        let mut raptor =
            Raptor4Tree::new(4, "summarize: {cluster_content}").with_small_layer_collapse(8);
        let backend = MockBackend {
            summaries: Mutex::new(vec!["Root summary\nbody".to_string()]),
            canceled: false,
        };
        let input = vec![
            (
                "alpha".to_string(),
                vec_of([1.0, 0.0, 0.0]),
                vec!["c1".to_string()],
            ),
            (
                "beta".to_string(),
                vec_of([0.99, 0.01, 0.0]),
                vec!["c2".to_string()],
            ),
        ];
        let progress = |_msg: &str| {};
        let output = raptor
            .call(&backend, input, &progress, "", false)
            .await
            .unwrap();
        match output {
            RaptorCallOutput::Flat { chunks, layers } => {
                assert_eq!(chunks.len(), 3, "two originals + one summary");
                assert_eq!(layers, vec![(0, 2), (2, 3)]);
                let summary = &chunks[2];
                assert_eq!(summary.title, "Root summary");
                assert_eq!(
                    summary.source_chunk_ids,
                    vec!["c1".to_string(), "c2".to_string()]
                );
            }
            other => panic!("expected flat output, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn call_tree_materializes_single_root() {
        let mut raptor = Raptor4Tree::new(4, "{cluster_content}");
        let backend = MockBackend {
            summaries: Mutex::new(vec!["Top\ndetail".to_string()]),
            canceled: false,
        };
        let input = vec![
            (
                "alpha".to_string(),
                vec_of([1.0, 0.0, 0.0]),
                vec!["c1".to_string()],
            ),
            (
                "beta".to_string(),
                vec_of([0.98, 0.02, 0.0]),
                vec!["c2".to_string()],
            ),
        ];
        let progress = |_msg: &str| {};
        let output = raptor
            .call(&backend, input, &progress, "", true)
            .await
            .unwrap();
        match output {
            RaptorCallOutput::Tree(tree) => {
                assert_eq!(tree["title"], json!("Top"));
                assert_eq!(tree["source_chunk_ids"], json!(["c1", "c2"]));
            }
            other => panic!("expected tree output, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancellation_and_error_budget() {
        let mut raptor = Raptor4Tree::new(4, "{cluster_content}");
        let canceled_backend = MockBackend {
            summaries: Mutex::new(vec![]),
            canceled: true,
        };
        let progress = |_msg: &str| {};
        let input = vec![
            ("a".to_string(), vec_of([1.0, 0.0, 0.0]), vec![]),
            ("b".to_string(), vec_of([0.0, 1.0, 0.0]), vec![]),
        ];
        assert_eq!(
            raptor
                .call(&canceled_backend, input.clone(), &progress, "t1", false)
                .await,
            Err(RaptorError::Cancelled)
        );

        let mut failing = Raptor4Tree::new(4, "{cluster_content}");
        failing.max_errors = 1;
        let failing_backend = MockBackend {
            summaries: Mutex::new(vec![]),
            canceled: false,
        };
        let result = failing
            .call(&failing_backend, input, &progress, "", false)
            .await;
        assert!(
            matches!(result, Err(RaptorError::Aborted(_))),
            "chat failure exhausts the error budget"
        );
    }

    #[test]
    fn truncate_respects_token_budget() {
        let text = "word ".repeat(2000);
        let cut = truncate_tokens(&text, 50);
        assert!(crate::chunk::tokenizer::token_count(&cut) <= 50);
        assert!(cut.len() < text.len());
        assert_eq!(truncate_tokens("short", 50), "short");
        assert!(truncate_tokens("anything", 0).is_empty());
    }
}
