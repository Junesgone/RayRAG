//! RAPTOR — Recursive Abstractive Processing for Tree-Organized Retrieval.
//!
//! Based on RAGFlow's `rag/raptor.py`. Implements hierarchical clustering:
//! 1. Group similar chunks using cosine similarity
//! 2. Create summary nodes for each cluster
//! 3. Recursively build a tree
//!
//! For retrieval, traverse the tree to find the most relevant level.
//!
//! RAGFlow's `RaptorService.build_doc_tree` summarizes each cluster with a
//! chat LLM (`chat_mdl`); RayRAG mirrors that with the optional
//! `ClusterSummarizer` passed to [`RaptorTree::build_with_llm`]. When no
//! summarizer is available (or the LLM call fails), the extractive fallback
//! is used so the tree still builds.

use crate::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A node in the RAPTOR tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaptorNode {
    /// Node ID
    pub id: String,
    /// Node text content (summary for non-leaf, chunk text for leaf)
    pub content: String,
    /// Embedding vector (for similarity search)
    pub embedding: Vec<f32>,
    /// Child node IDs
    pub children: Vec<String>,
    /// Node level in the tree (0 = leaf, higher = more abstract)
    pub level: usize,
    /// Child count (original chunks collapsed into this node)
    pub chunk_count: usize,
    /// Metadata
    pub metadata: HashMap<String, String>,
}

/// Summarizes a cluster of chunks into a single summary text.
///
/// Mirrors RAGFlow's `chat_mdl` in `RaptorService.build_doc_tree`: the LLM
/// receives the cluster texts (optionally via a prompt template) and returns
/// a concise summary that becomes the parent node's content. The `prompt`
/// follows RAGFlow's contract — a template containing a `{cluster_content}`
/// placeholder that the implementation substitutes with the joined,
/// per-chunk-truncated cluster texts. RAGFlow also asks the model to give a
/// title on the first line in the same language as the paragraphs and caps
/// output at `max(max_token, 512)` tokens.
#[async_trait]
pub trait ClusterSummarizer: Send + Sync {
    /// Summarize `texts` into one summary string using `prompt` (a
    /// `{cluster_content}` template) and `max_token` output budget.
    async fn summarize(&self, texts: &[String], prompt: &str, max_token: usize) -> Result<String>;
}

/// RAPTOR tree for hierarchical document retrieval.
pub struct RaptorTree {
    /// All nodes indexed by ID
    nodes: HashMap<String, RaptorNode>,
    /// Root node ID
    root_id: Option<String>,
    /// Clustering threshold (cosine similarity, default 0.7)
    threshold: f32,
    /// Max cluster size before splitting
    max_cluster_size: usize,
    /// Max tree depth
    max_depth: usize,
}

impl RaptorTree {
    /// Create a new RAPTOR tree.
    pub fn new(threshold: f32) -> Self {
        Self {
            nodes: HashMap::new(),
            root_id: None,
            threshold,
            max_cluster_size: 10,
            max_depth: 5,
        }
    }

    /// Limit the number of summary layers generated above leaf chunks.
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth.max(1);
        self
    }

    /// Build the tree from a set of chunks (each with text + embedding).
    pub fn build(&mut self, chunks: &[(String, Vec<f32>)]) -> Result<&str> {
        if chunks.is_empty() {
            anyhow::bail!("No chunks provided");
        }

        // Level 0: leaf nodes (one per chunk)
        let mut current_level: Vec<RaptorNode> = chunks
            .iter()
            .enumerate()
            .map(|(i, (text, emb))| RaptorNode {
                id: format!("L0-{}", i),
                content: text.clone(),
                embedding: emb.clone(),
                children: vec![],
                level: 0,
                chunk_count: 1,
                metadata: HashMap::new(),
            })
            .collect();

        // Insert all leaf nodes
        for node in &current_level {
            self.nodes.insert(node.id.clone(), node.clone());
        }

        let mut level = 0;
        // Recursively cluster and summarize
        while current_level.len() > 1 && level < self.max_depth {
            level += 1;
            let clusters = self.cluster(&current_level);

            let mut next_level = Vec::new();
            for cluster in &clusters {
                if cluster.is_empty() {
                    continue;
                }
                if cluster.len() == 1 {
                    // Single node — promote directly
                    let mut node = cluster[0].clone();
                    node.id = format!("L{}-{}", level, next_level.len());
                    node.level = level;
                    node.chunk_count = 1;
                    self.nodes.insert(node.id.clone(), node.clone());
                    next_level.push(node);
                } else {
                    // Multiple nodes — create summary node
                    let summary = Self::extractive_summarize(cluster);
                    let avg_embedding = Self::average_embedding(cluster);
                    let child_ids: Vec<String> = cluster.iter().map(|n| n.id.clone()).collect();
                    let total_chunks: usize = cluster.iter().map(|n| n.chunk_count).sum();

                    let node = RaptorNode {
                        id: format!("L{}-{}", level, next_level.len()),
                        content: summary,
                        embedding: avg_embedding,
                        children: child_ids,
                        level,
                        chunk_count: total_chunks,
                        metadata: HashMap::new(),
                    };
                    self.nodes.insert(node.id.clone(), node.clone());
                    next_level.push(node);
                }
            }

            current_level = next_level;
        }

        // Root is the last remaining node
        if let Some(root) = current_level.first() {
            self.root_id = Some(root.id.clone());
            // Insert root if not already inserted
            if !self.nodes.contains_key(&root.id) {
                self.nodes.insert(root.id.clone(), root.clone());
            }
        }

        tracing::info!(
            "RAPTOR tree built: {} nodes, {} levels",
            self.nodes.len(),
            level + 1
        );

        Ok(self.root_id.as_deref().unwrap_or(""))
    }

    /// Build the tree using an LLM summarizer for cluster summaries.
    ///
    /// Mirrors RAGFlow's `RaptorService.build_doc_tree(chunks, chat_mdl,
    /// embd_mdl, tree_builder="raptor", clustering_method="gmm")`. The
    /// summarizer is invoked for each multi-chunk cluster with the RAGFlow
    /// default prompt template; a summarizer error falls back to the
    /// extractive summary so the build continues.
    pub async fn build_with_llm(
        &mut self,
        chunks: &[(String, Vec<f32>)],
        summarizer: &dyn ClusterSummarizer,
    ) -> Result<&str> {
        const DEFAULT_PROMPT: &str =
            "Please write a concise summary of the following texts:\n{cluster_content}";
        const DEFAULT_MAX_TOKEN: usize = 512;
        self.build_with_llm_prompt(chunks, summarizer, DEFAULT_PROMPT, DEFAULT_MAX_TOKEN)
            .await
    }

    /// [`Self::build_with_llm`] with an explicit RAGFlow-style prompt
    /// template (`{cluster_content}` placeholder) and max-token budget.
    pub async fn build_with_llm_prompt(
        &mut self,
        chunks: &[(String, Vec<f32>)],
        summarizer: &dyn ClusterSummarizer,
        prompt: &str,
        max_token: usize,
    ) -> Result<&str> {
        if chunks.is_empty() {
            anyhow::bail!("No chunks provided");
        }

        // Level 0: leaf nodes (one per chunk)
        let mut current_level: Vec<RaptorNode> = chunks
            .iter()
            .enumerate()
            .map(|(i, (text, emb))| RaptorNode {
                id: format!("L0-{}", i),
                content: text.clone(),
                embedding: emb.clone(),
                children: vec![],
                level: 0,
                chunk_count: 1,
                metadata: HashMap::new(),
            })
            .collect();

        for node in &current_level {
            self.nodes.insert(node.id.clone(), node.clone());
        }

        let mut level = 0;
        while current_level.len() > 1 && level < self.max_depth {
            level += 1;
            let clusters = self.cluster(&current_level);

            let mut next_level = Vec::new();
            for cluster in &clusters {
                if cluster.is_empty() {
                    continue;
                }
                if cluster.len() == 1 {
                    let mut node = cluster[0].clone();
                    node.id = format!("L{}-{}", level, next_level.len());
                    node.level = level;
                    node.chunk_count = 1;
                    self.nodes.insert(node.id.clone(), node.clone());
                    next_level.push(node);
                } else {
                    // LLM summary with extractive fallback.
                    let texts: Vec<String> = cluster.iter().map(|n| n.content.clone()).collect();
                    let summary = match summarizer.summarize(&texts, prompt, max_token).await {
                        Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
                        _ => Self::extractive_summarize(cluster),
                    };
                    let avg_embedding = Self::average_embedding(cluster);
                    let child_ids: Vec<String> = cluster.iter().map(|n| n.id.clone()).collect();
                    let total_chunks: usize = cluster.iter().map(|n| n.chunk_count).sum();

                    let node = RaptorNode {
                        id: format!("L{}-{}", level, next_level.len()),
                        content: summary,
                        embedding: avg_embedding,
                        children: child_ids,
                        level,
                        chunk_count: total_chunks,
                        metadata: HashMap::new(),
                    };
                    self.nodes.insert(node.id.clone(), node.clone());
                    next_level.push(node);
                }
            }

            current_level = next_level;
        }

        if let Some(root) = current_level.first() {
            self.root_id = Some(root.id.clone());
            if !self.nodes.contains_key(&root.id) {
                self.nodes.insert(root.id.clone(), root.clone());
            }
        }

        Ok(self.root_id.as_deref().unwrap_or(""))
    }

    /// Search the tree for the most relevant content at the appropriate abstraction level.
    /// Returns nodes sorted by relevance (cosine similarity).
    pub fn search(&self, query_embedding: &[f32], top_k: usize) -> Vec<&RaptorNode> {
        let mut scored: Vec<(f32, &RaptorNode)> = self
            .nodes
            .values()
            .filter(|n| !n.embedding.is_empty())
            .map(|n| {
                let score = cosine_similarity(query_embedding, &n.embedding);
                (score, n)
            })
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        scored.into_iter().map(|(_, n)| n).collect()
    }

    /// Get a node by ID.
    pub fn get(&self, id: &str) -> Option<&RaptorNode> {
        self.nodes.get(id)
    }

    /// Get the root node.
    pub fn root(&self) -> Option<&RaptorNode> {
        self.root_id.as_ref().and_then(|id| self.nodes.get(id))
    }

    /// Number of nodes in the tree.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Return only generated summary nodes. Leaf nodes remain ordinary chunks.
    pub fn summary_nodes(&self) -> Vec<&RaptorNode> {
        let mut nodes: Vec<_> = self.nodes.values().filter(|node| node.level > 0).collect();
        nodes.sort_by(|left, right| {
            left.level
                .cmp(&right.level)
                .then_with(|| left.id.cmp(&right.id))
        });
        nodes
    }

    // ── Internal methods ──────────────────────────────────────

    /// Cluster nodes by cosine similarity (agglomerative).
    fn cluster(&self, nodes: &[RaptorNode]) -> Vec<Vec<RaptorNode>> {
        let n = nodes.len();
        if n == 0 {
            return vec![];
        }

        // Compute pairwise similarity matrix (cache in HashMap for sparse)
        let mut clusters: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();

        // Simple greedy clustering: merge most similar pairs above threshold
        loop {
            let mut best_sim = 0.0f32;
            let mut best_pair = None;

            for i in 0..clusters.len() {
                for j in (i + 1)..clusters.len() {
                    // Check combined size
                    if clusters[i].len() + clusters[j].len() > self.max_cluster_size {
                        continue;
                    }
                    let sim =
                        Self::cluster_similarity(&clusters[i], &clusters[j], nodes, self.threshold);
                    if sim > best_sim && sim >= self.threshold {
                        best_sim = sim;
                        best_pair = Some((i, j));
                    }
                }
            }

            match best_pair {
                Some((i, j)) => {
                    // Merge cluster j into i
                    let mut merged = clusters[i].clone();
                    merged.extend(clusters[j].clone());
                    clusters[i] = merged;
                    clusters.remove(j);
                }
                None => break, // No more pairs above threshold
            }
        }

        // Convert index clusters to node clusters
        clusters
            .into_iter()
            .map(|indices| indices.into_iter().map(|idx| nodes[idx].clone()).collect())
            .collect()
    }

    /// Average cosine similarity between two clusters.
    fn cluster_similarity(a: &[usize], b: &[usize], nodes: &[RaptorNode], _threshold: f32) -> f32 {
        let mut total = 0.0f32;
        let mut count = 0u32;
        for &ai in a {
            for &bi in b {
                total += cosine_similarity(&nodes[ai].embedding, &nodes[bi].embedding);
                count += 1;
            }
        }
        if count == 0 {
            0.0
        } else {
            total / count as f32
        }
    }

    /// Simple extractive summarization: pick first 2 sentences as summary.
    fn extractive_summarize(nodes: &[RaptorNode]) -> String {
        // Collect representative sentences from each node
        let mut sentences = Vec::new();
        for node in nodes.iter().take(5) {
            // Take first sentence from each chunk
            if let Some(first_sentence) = node.content.split(['.', '。', '!', '?', '\n']).next() {
                let trimmed = first_sentence.trim();
                if !trimmed.is_empty() && trimmed.len() > 5 {
                    sentences.push(trimmed.to_string());
                }
            }
        }
        if sentences.is_empty() {
            // Fallback: take first 200 chars
            nodes
                .first()
                .map(|n| n.content.chars().take(200).collect())
                .unwrap_or_default()
        } else {
            sentences.join(". ") + "."
        }
    }

    /// Compute the average embedding vector for a cluster.
    fn average_embedding(nodes: &[RaptorNode]) -> Vec<f32> {
        if nodes.is_empty() {
            return vec![];
        }
        let dim = nodes[0].embedding.len();
        let mut avg = vec![0.0f32; dim];
        for node in nodes {
            for (i, &v) in node.embedding.iter().enumerate() {
                avg[i] += v;
            }
        }
        for v in avg.iter_mut() {
            *v /= nodes.len() as f32;
        }
        avg
    }
}

// ── GraphRAG: Simple entity-relation extraction ─────────────────

/// A named entity extracted from text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    /// Entity name
    pub name: String,
    /// Entity type label
    pub entity_type: String,
    /// Source chunk ID
    pub source: String,
}

/// A relation between two entities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    /// Source entity name
    pub source: String,
    /// Target entity name
    pub target: String,
    /// Relation label
    pub relation: String,
    /// Source chunk ID
    pub evidence: String,
}

/// Simple entity-relation graph extracted from text.
pub struct KnowledgeGraph {
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
}

impl KnowledgeGraph {
    /// Extract entities and relations from text using simple pattern matching.
    ///
    /// Pattern rules (simplified from RAGFlow's graphrag):
    /// - Capitalized words → Person/Organization
    /// - "X is a Y" → IS-A relation
    /// - "X works at Y" / "X founded Y" → WORKS-AT / FOUNDED
    pub fn extract(chunks: &[(String, String)]) -> Self {
        let mut entities = Vec::new();
        let mut relations = Vec::new();

        for (chunk_id, text) in chunks {
            // Simple Named Entity patterns
            for word in text.split_whitespace() {
                let trimmed = word.trim_matches(|c: char| !c.is_alphanumeric());
                if trimmed.len() > 1
                    && trimmed.chars().next().is_some_and(|c| c.is_uppercase())
                    && !trimmed.chars().all(|c| c.is_uppercase())
                {
                    // Avoid duplicates
                    if !entities.iter().any(|e: &Entity| e.name == trimmed) {
                        entities.push(Entity {
                            name: trimmed.to_string(),
                            entity_type: "Unknown".into(),
                            source: chunk_id.clone(),
                        });
                    }
                }
            }

            // Simple relation patterns
            for pattern in &[" is a ", " works at ", " founded ", " created ", " wrote "] {
                if let Some(pos) = text.find(pattern) {
                    let subject = text[..pos].split_whitespace().last().unwrap_or("");
                    let object = text[pos + pattern.len()..]
                        .split_whitespace()
                        .next()
                        .unwrap_or("");

                    if !subject.is_empty() && !object.is_empty() {
                        let rel_label = match *pattern {
                            " is a " => "IS_A",
                            " works at " => "WORKS_AT",
                            " founded " => "FOUNDED",
                            " created " => "CREATED",
                            " wrote " => "WROTE",
                            _ => "RELATED_TO",
                        };

                        relations.push(Relation {
                            source: subject.to_string(),
                            target: object.to_string(),
                            relation: rel_label.into(),
                            evidence: chunk_id.clone(),
                        });
                    }
                }
            }
        }

        Self {
            entities,
            relations,
        }
    }
}

// ── Cosine similarity helper ────────────────────────────────────

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// LLM-backed [`ClusterSummarizer`] that mirrors RAGFlow's RAPTOR summary
/// protocol (`RecursiveAbstractiveProcessing4TreeOrganizedRetrieval.
/// _summarize_texts`):
/// - per-chunk truncation so the combined cluster fits the model budget
/// - prompt template with a `{cluster_content}` placeholder as the system
///   message
/// - user message asking for a same-language title line
/// - output capped at `max(max_token, 512)` tokens
pub struct LlmClusterSummarizer {
    llm: crate::llm::LlmClient,
    /// Model context window (tokens). Used only for truncation budgeting.
    max_length: usize,
}

impl LlmClusterSummarizer {
    pub fn new(llm: crate::llm::LlmClient, max_length: usize) -> Self {
        Self { llm, max_length }
    }
}

#[async_trait]
impl ClusterSummarizer for LlmClusterSummarizer {
    async fn summarize(&self, texts: &[String], prompt: &str, max_token: usize) -> Result<String> {
        if texts.is_empty() {
            anyhow::bail!("empty cluster");
        }
        // Per-chunk truncation budget, mirroring RAGFlow:
        // len_per_chunk = (max_length - max_token) / len(texts)
        let budget = self.max_length.saturating_sub(max_token.max(1));
        let len_per_chunk = (budget / texts.len()).max(1);
        let cluster_content = texts
            .iter()
            .map(|t| {
                if t.chars().count() > len_per_chunk {
                    t.chars().take(len_per_chunk).collect::<String>()
                } else {
                    t.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let system = prompt.replace("{cluster_content}", &cluster_content);
        let user = "Beside the summarization, give a title at the first line of your summarization. Must be in the same language as the paragraphs.";
        let messages = vec![
            crate::llm::ChatMessage::new("system", system),
            crate::llm::ChatMessage::new("user", user),
        ];
        let patch = crate::generation_params::GenerationParamsPatch {
            max_tokens: Some(max_token.max(512) as u32),
            ..Default::default()
        };
        let completion = self
            .llm
            .chat_completion_with_generation(&messages, patch)
            .await?;
        let content = completion.content;
        if content.trim().is_empty() {
            anyhow::bail!("empty LLM summary");
        }
        Ok(content)
    }
}

/// Outcome of running one tree-kind compilation template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeTemplateResult {
    /// Template id the tree was built for.
    pub template_id: String,
    /// Root node id of the built tree (empty when skipped/failed).
    pub root_id: String,
    /// Number of nodes in the tree.
    pub node_count: usize,
    /// Entity/relation projection of the built tree (empty when skipped).
    #[serde(default)]
    pub graph: TreeGraph,
}

/// Entity/relation graph projection of a RAPTOR tree.
///
/// Mirrors RAGFlow `chunk_post_processor.raptor_tree_to_graph`: every tree
/// node becomes an entity (type `tree_node`, description from the node
/// content) and every parent→child edge becomes a `child` relation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TreeGraph {
    pub entities: Vec<TreeGraphEntity>,
    pub relations: Vec<TreeGraphRelation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeGraphEntity {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub description: String,
    pub mention_count: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_chunk_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeGraphRelation {
    #[serde(rename = "from")]
    pub from: String,
    #[serde(rename = "to")]
    pub to: String,
    #[serde(rename = "type")]
    pub kind: String,
}

impl RaptorTree {
    /// Project the tree onto `{entities, relations}` in RAGFlow's
    /// document-structure graph shape. Leaves carry their chunk ids as
    /// `source_chunk_ids`; internal nodes use their content (title line) as
    /// the entity name.
    pub fn to_graph(&self) -> TreeGraph {
        let mut graph = TreeGraph::default();
        let Some(root_id) = &self.root_id else {
            return graph;
        };
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![(root_id.clone(), None::<String>)];
        while let Some((node_id, parent_id)) = stack.pop() {
            if !seen.insert(node_id.clone()) {
                continue;
            }
            let Some(node) = self.nodes.get(&node_id) else {
                continue;
            };
            let name = node
                .content
                .lines()
                .next()
                .unwrap_or(&node.id)
                .trim()
                .to_string();
            graph.entities.push(TreeGraphEntity {
                name: name.clone(),
                kind: "tree_node".into(),
                description: node.content.clone(),
                mention_count: 1,
                source_chunk_ids: if node.level == 0 {
                    vec![node.id.clone()]
                } else {
                    Vec::new()
                },
            });
            if let Some(parent) = parent_id {
                graph.relations.push(TreeGraphRelation {
                    from: parent,
                    to: name.clone(),
                    kind: "child".into(),
                });
            }
            for child in &node.children {
                stack.push((child.clone(), Some(name.clone())));
            }
        }
        graph
    }
}

/// Run RAGFlow-style `tree`-kind compilation templates over a document's
/// chunks.
///
/// Mirrors `chunk_post_processor.run_tree_templates`: every template whose
/// kind is `tree` builds a RAPTOR tree over the same chunk set using the
/// template's `raptor` config (prompt / max_token / threshold / max_cluster
/// keys) and the shared summarizer. A failing template is skipped so one bad
/// template never aborts the rest.
pub async fn run_tree_templates(
    chunks: &[(String, Vec<f32>)],
    templates: &[(String, serde_json::Value)],
    summarizer: &dyn ClusterSummarizer,
) -> Vec<TreeTemplateResult> {
    if chunks.is_empty() || templates.is_empty() {
        return vec![];
    }
    let mut results = Vec::with_capacity(templates.len());
    for (template_id, config) in templates {
        let raptor_cfg = config.get("raptor").and_then(serde_json::Value::as_object);
        let prompt = raptor_cfg
            .and_then(|c| c.get("prompt"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Please write a concise summary of the following texts:\n{cluster_content}");
        let max_token = raptor_cfg
            .and_then(|c| c.get("max_token"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(512) as usize;
        let threshold = raptor_cfg
            .and_then(|c| c.get("threshold"))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.1) as f32;
        let max_cluster = raptor_cfg
            .and_then(|c| c.get("max_cluster"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(64) as usize;

        let mut tree = RaptorTree::new(threshold);
        tree.max_cluster_size = max_cluster.max(1);
        let (root_id, graph) = match tree
            .build_with_llm_prompt(chunks, summarizer, prompt, max_token)
            .await
        {
            Ok(root) if !root.is_empty() => (root.to_string(), tree.to_graph()),
            _ => {
                tracing::warn!("tree-template {template_id}: RAPTOR build skipped");
                (String::new(), TreeGraph::default())
            }
        };
        results.push(TreeTemplateResult {
            template_id: template_id.to_string(),
            node_count: tree.len(),
            root_id,
            graph,
        });
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeSummarizer;

    #[async_trait]
    impl ClusterSummarizer for FakeSummarizer {
        async fn summarize(
            &self,
            texts: &[String],
            prompt: &str,
            max_token: usize,
        ) -> Result<String> {
            let rendered = prompt.replace("{cluster_content}", &texts.join("\n"));
            Ok(format!(
                "LLM-SUMMARY({})[{}][{}]",
                texts.len(),
                rendered.chars().count(),
                max_token
            ))
        }
    }

    struct FailingSummarizer;

    #[async_trait]
    impl ClusterSummarizer for FailingSummarizer {
        async fn summarize(
            &self,
            _texts: &[String],
            _prompt: &str,
            _max_token: usize,
        ) -> Result<String> {
            anyhow::bail!("llm unavailable")
        }
    }

    #[tokio::test]
    async fn test_raptor_build_with_llm_uses_summarizer() {
        let chunks = vec![
            ("Rust is fast".into(), vec![1.0, 0.0]),
            ("Rust is safe".into(), vec![0.9, 0.1]),
            ("Python is slow".into(), vec![0.0, 1.0]),
        ];
        let mut tree = RaptorTree::new(0.5);
        tree.build_with_llm(&chunks, &FakeSummarizer).await.unwrap();
        // At least one non-leaf node carries the LLM summary.
        let has_llm_summary = tree
            .search(&[1.0, 0.0], 10)
            .iter()
            .any(|n| n.level > 0 && n.content.starts_with("LLM-SUMMARY("));
        assert!(has_llm_summary, "expected an LLM summary node");
        assert!(tree.len() >= 3);
    }

    #[tokio::test]
    async fn test_raptor_build_with_llm_falls_back_on_error() {
        let chunks = vec![
            ("Rust is fast".into(), vec![1.0, 0.0]),
            ("Rust is safe".into(), vec![0.9, 0.1]),
            ("Python is slow".into(), vec![0.0, 1.0]),
        ];
        let mut tree = RaptorTree::new(0.5);
        tree.build_with_llm(&chunks, &FailingSummarizer)
            .await
            .unwrap();
        // Tree still builds; root exists.
        assert!(tree.root().is_some());
        assert!(tree.len() >= 3);
    }

    #[tokio::test]
    async fn test_run_tree_templates_builds_one_tree_per_template() {
        let chunks = vec![
            ("Rust is fast".into(), vec![1.0, 0.0]),
            ("Rust is safe".into(), vec![0.9, 0.1]),
            ("Python is slow".into(), vec![0.0, 1.0]),
        ];
        let templates = vec![
            (
                "tpl-a".to_string(),
                serde_json::json!({"raptor": {"prompt": "Summarize:\n{cluster_content}", "max_token": 256, "threshold": 0.3, "max_cluster": 8}}),
            ),
            (
                "tpl-b".to_string(),
                serde_json::json!({"raptor": {"threshold": 0.9}}), // high threshold → few merges
            ),
        ];
        let results = run_tree_templates(&chunks, &templates, &FakeSummarizer).await;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].template_id, "tpl-a");
        assert!(results[0].node_count >= 3, "tree should hold all leaves");
        assert!(!results[0].root_id.is_empty(), "tree A should build");
        assert_eq!(results[1].template_id, "tpl-b");
        assert!(!results[1].root_id.is_empty(), "tree B should build");
    }

    #[tokio::test]
    async fn test_run_tree_templates_skips_empty_inputs() {
        let chunks: Vec<(String, Vec<f32>)> = vec![];
        let templates = vec![("tpl".to_string(), serde_json::json!({}))];
        let results = run_tree_templates(&chunks, &templates, &FakeSummarizer).await;
        assert!(results.is_empty());

        let chunks2 = vec![("x".into(), vec![1.0])];
        let templates2: Vec<(String, serde_json::Value)> = vec![];
        let results2 = run_tree_templates(&chunks2, &templates2, &FakeSummarizer).await;
        assert!(results2.is_empty());
    }

    #[test]
    fn tree_to_graph_projects_entities_and_child_relations() {
        let mut tree = RaptorTree::new(0.7);
        // Manual 3-node tree: root "r" with children "a", "b".
        tree.nodes.insert(
            "r".into(),
            RaptorNode {
                id: "r".into(),
                content: "Root summary".into(),
                embedding: vec![1.0],
                children: vec!["a".into(), "b".into()],
                level: 1,
                chunk_count: 2,
                metadata: Default::default(),
            },
        );
        tree.nodes.insert(
            "a".into(),
            RaptorNode {
                id: "a".into(),
                content: "Chunk A".into(),
                embedding: vec![1.0],
                children: vec![],
                level: 0,
                chunk_count: 1,
                metadata: Default::default(),
            },
        );
        tree.nodes.insert(
            "b".into(),
            RaptorNode {
                id: "b".into(),
                content: "Chunk B".into(),
                embedding: vec![1.0],
                children: vec![],
                level: 0,
                chunk_count: 1,
                metadata: Default::default(),
            },
        );
        tree.root_id = Some("r".into());

        let graph = tree.to_graph();
        assert_eq!(graph.entities.len(), 3);
        assert_eq!(graph.relations.len(), 2);
        let root_ent = graph
            .entities
            .iter()
            .find(|e| e.name == "Root summary")
            .unwrap();
        assert_eq!(root_ent.kind, "tree_node");
        assert!(root_ent.source_chunk_ids.is_empty());
        let leaf = graph.entities.iter().find(|e| e.name == "Chunk A").unwrap();
        assert_eq!(leaf.source_chunk_ids, vec!["a".to_string()]);
        let rels: Vec<_> = graph
            .relations
            .iter()
            .map(|r| (r.from.as_str(), r.to.as_str(), r.kind.as_str()))
            .collect();
        assert!(rels.contains(&("Root summary", "Chunk A", "child")));
        assert!(rels.contains(&("Root summary", "Chunk B", "child")));
    }

    // Real GPU RAPTOR LLM summary loop (ignored by default; requires
    // RAYRAG_TEST_LLM_BASE/MODEL pointing at an OpenAI-compatible chat
    // endpoint — the host 8088 Qwen3.5-9B).
    #[tokio::test]
    #[ignore]
    async fn gpu_raptor_llm_summary_loop() {
        let api_base = std::env::var("RAYRAG_TEST_LLM_BASE").expect("RAYRAG_TEST_LLM_BASE");
        let model = std::env::var("RAYRAG_TEST_LLM_MODEL").expect("RAYRAG_TEST_LLM_MODEL");
        let config = crate::llm::LlmConfig {
            api_base,
            api_key: std::env::var("RAYRAG_TEST_LLM_KEY").unwrap_or_default(),
            model,
            generation: crate::generation_params::GenerationParams::default(),
            system_prompt: String::new(),
        };
        let summarizer = LlmClusterSummarizer::new(crate::llm::LlmClient::new(config), 8192);
        // Probe the summarizer directly before building the tree.
        let probe = summarizer
            .summarize(
                &["中山市百鲤居水产养殖场主营四大家鱼养殖。".to_string()],
                "Please write a concise summary of the following texts:\n{cluster_content}",
                512,
            )
            .await
            .expect("summarizer probe");
        assert!(!probe.trim().is_empty(), "probe summary empty");
        tracing::info!("RAPTOR probe summary: {probe}");
        // Three related Chinese chunks so clustering forms one cluster and
        // the LLM must produce a same-language titled summary.
        let chunks = vec![
            (
                "中山市百鲤居水产养殖场主营四大家鱼养殖。".into(),
                vec![1.0, 0.0, 0.0],
            ),
            (
                "水产养殖需要关注水质、溶氧和饲料管理。".into(),
                vec![0.9, 0.1, 0.0],
            ),
            (
                "科学养殖可以提高鱼类的成活率与产量。".into(),
                vec![0.8, 0.2, 0.0],
            ),
        ];
        let mut tree = RaptorTree::new(0.5);
        let root = tree
            .build_with_llm(&chunks, &summarizer)
            .await
            .expect("build");
        assert!(!root.is_empty());
        // Root (or a level-1 node) must carry a real LLM summary, not the
        // extractive fallback prefix.
        let root_node = tree.root().expect("root node");
        assert!(root_node.level >= 1, "expected a summarized tree root");
        assert!(
            root_node.content.chars().count() > 10,
            "LLM summary too short: {:?}",
            root_node.content
        );
        // The extractive fallback would join the chunk first sentences
        // verbatim ("中山市百鲤居水产养殖场主营四大家鱼养殖。水产养殖
        // 需要关注水质、溶氧和饲料管理。"). The LLM rewrite adds connective
        // structure ("通过...管理...提升..."), so require the root content
        // to be a real paraphrase, not the verbatim extractive join.
        let extractive_join =
            "中山市百鲤居水产养殖场主营四大家鱼养殖。水产养殖需要关注水质、溶氧和饲料管理。";
        assert!(
            !root_node.content.starts_with(extractive_join),
            "content looks extractive, LLM summary missing; root={:?} nodes={}",
            root_node.content,
            tree.len()
        );
        tracing::info!("RAPTOR LLM root summary: {}", root_node.content);
    }

    #[test]
    fn test_raptor_tree_build() {
        let chunks = vec![
            ("Rust is fast".into(), vec![1.0, 0.0]),
            ("Rust is safe".into(), vec![0.9, 0.1]),
            ("Python is slow".into(), vec![0.0, 1.0]),
        ];
        let mut tree = RaptorTree::new(0.5);
        let root = tree.build(&chunks).unwrap();
        assert!(!root.is_empty());
        assert!(tree.len() >= 3); // at least 3 leaf nodes
    }

    #[test]
    fn test_graph_extraction() {
        let chunks = vec![
            ("c1".into(), "Alice works at Google".into()),
            ("c2".into(), "Bob is a engineer".into()),
        ];
        let kg = KnowledgeGraph::extract(&chunks);
        assert!(kg.entities.len() >= 2);
        assert!(!kg.relations.is_empty());
    }
}
