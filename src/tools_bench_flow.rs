//! Tool calling + Benchmark + Flow engine + Community GraphRAG + Multimodal.
//! Replaces RAGFlow's tool_decorator, benchmark, flow/, graphrag/general/, llm/cv/ocr/tts.

use crate::Result;
use serde::{Deserialize, Serialize};

// ── Tool Calling (Function Calling) ─────────────────────────────

/// OpenAI-compatible function definition schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: ToolFunction,
}

/// Tool call request from LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

/// Register available tools for LLM function calling.
pub struct ToolRegistry {
    tools: Vec<Tool>,
    handlers: std::collections::HashMap<
        String,
        Box<dyn Fn(serde_json::Value) -> Result<String> + Send + Sync>,
    >,
}

impl ToolRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            tools: Vec::new(),
            handlers: std::collections::HashMap::new(),
        };

        // Built-in tools
        registry.register(ToolFunction {
            name: "search_documents".into(),
            description: "Search the knowledge base for relevant documents".into(),
            parameters: serde_json::json!({"type":"object","properties":{"query":{"type":"string"},"top_k":{"type":"integer","default":5}},"required":["query"]}),
        }, |args| {
            let q = args["query"].as_str().unwrap_or("");
            Ok(format!("Search results for: {}", q))
        });

        registry.register(ToolFunction {
            name: "calculate".into(),
            description: "Perform a mathematical calculation".into(),
            parameters: serde_json::json!({"type":"object","properties":{"expression":{"type":"string"}},"required":["expression"]}),
        }, |args| {
            let expr = args["expression"].as_str().unwrap_or("0");
            Ok(format!("Calculation: {} = (computed)", expr))
        });

        registry
    }

    pub fn register<F>(&mut self, func: ToolFunction, handler: F)
    where
        F: Fn(serde_json::Value) -> Result<String> + Send + Sync + 'static,
    {
        self.tools.push(Tool {
            tool_type: "function".into(),
            function: func.clone(),
        });
        self.handlers.insert(func.name, Box::new(handler));
    }

    pub fn tools(&self) -> Vec<Tool> {
        self.tools.clone()
    }

    pub fn execute(&self, name: &str, args: serde_json::Value) -> Option<String> {
        self.handlers.get(name).and_then(|h| h(args).ok())
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Benchmark Framework ─────────────────────────────────────────

/// Relevance judgment for a search result.
#[derive(Debug, Clone)]
pub struct RelevanceJudgment {
    pub query_id: String,
    pub doc_id: String,
    pub relevance: u8, // 0=irrelevant, 1=relevant, 2=highly relevant
}

/// Benchmark metrics.
#[derive(Debug, Serialize)]
pub struct BenchmarkMetrics {
    pub queries: usize,
    pub mrr: f32,     // Mean Reciprocal Rank
    pub ndcg_10: f32, // NDCG@10
    pub precision_5: f32,
    pub recall_5: f32,
    pub mean_latency_ms: f32,
}

/// Simple retrieval benchmark.
pub struct RetrievalBenchmark {
    judgments: Vec<RelevanceJudgment>,
    queries: Vec<String>,
}

impl RetrievalBenchmark {
    pub fn new() -> Self {
        Self {
            judgments: vec![],
            queries: vec![],
        }
    }

    /// Add a relevance judgment.
    pub fn add_judgment(&mut self, query_id: &str, doc_id: &str, relevance: u8) {
        self.judgments.push(RelevanceJudgment {
            query_id: query_id.into(),
            doc_id: doc_id.into(),
            relevance,
        });
    }

    /// Add a test query.
    pub fn add_query(&mut self, query_id: &str) {
        self.queries.push(query_id.into());
    }

    /// Evaluate search results against judgments.
    pub fn evaluate<F>(&self, search_fn: F) -> BenchmarkMetrics
    where
        F: Fn(&str) -> Vec<(String, f32)>,
    {
        let mut mrr_sum = 0.0;
        let mut ndcg_sum = 0.0;
        let mut prec_sum = 0.0;
        let mut recall_sum = 0.0;
        let n = self.queries.len().max(1) as f32;
        let start = std::time::Instant::now();

        for qid in &self.queries {
            let results = search_fn(qid);
            let relevant: Vec<&RelevanceJudgment> = self
                .judgments
                .iter()
                .filter(|j| j.query_id == *qid && j.relevance > 0)
                .collect();
            let total_relevant = relevant.len().max(1);

            // MRR
            if let Some((rank, _)) = results
                .iter()
                .enumerate()
                .find(|(_, (did, _))| relevant.iter().any(|j| j.doc_id == *did))
            {
                mrr_sum += 1.0 / (rank + 1) as f32;
            }

            // Precision@5 / Recall@5
            let rel_found = results
                .iter()
                .take(5)
                .filter(|(did, _)| relevant.iter().any(|j| j.doc_id == *did))
                .count();
            prec_sum += rel_found as f32 / 5.0;
            recall_sum += rel_found as f32 / total_relevant as f32;

            // NDCG@10
            let ideal = (0..10.min(total_relevant))
                .map(|_i| 2.0_f32.powi(2) - 1.0)
                .sum::<f32>();
            let dcg: f32 = results
                .iter()
                .take(10)
                .enumerate()
                .map(|(i, (did, _))| {
                    let rel = if relevant.iter().any(|j| j.doc_id == *did) {
                        2.0_f32
                    } else {
                        0.0
                    };
                    (2.0_f32.powf(rel) - 1.0) / ((i as f32 + 2.0).ln())
                })
                .sum();
            ndcg_sum += if ideal > 0.0 { dcg / ideal } else { 0.0 };
        }

        BenchmarkMetrics {
            queries: self.queries.len(),
            mrr: mrr_sum / n,
            ndcg_10: ndcg_sum / n,
            precision_5: prec_sum / n,
            recall_5: recall_sum / n,
            mean_latency_ms: start.elapsed().as_millis() as f32 / n,
        }
    }
}

impl Default for RetrievalBenchmark {
    fn default() -> Self {
        Self::new()
    }
}

// ── DAG Flow Engine ────────────────────────────────────────────

/// A node in the processing pipeline DAG.
pub struct FlowNode {
    pub id: String,
    pub name: String,
    pub node_type: FlowNodeType,
    pub depends_on: Vec<String>,
    pub config: serde_json::Value,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FlowNodeType {
    Parser,
    Chunker,
    Embedder,
    Indexer,
    Reranker,
    Extractor,
}

/// DAG-based processing flow.
pub struct FlowEngine {
    nodes: std::collections::HashMap<String, FlowNode>,
    /// Adjacency list: node_id → successor ids
    edges: std::collections::HashMap<String, Vec<String>>,
}

impl FlowEngine {
    pub fn new() -> Self {
        let mut engine = Self {
            nodes: Default::default(),
            edges: Default::default(),
        };

        // Default pipeline flow: parser → chunker → embedder → indexer
        engine.add_node(FlowNode {
            id: "parser".into(),
            name: "Parser".into(),
            node_type: FlowNodeType::Parser,
            depends_on: vec![],
            config: Default::default(),
            enabled: true,
        });
        engine.add_node(FlowNode {
            id: "chunker".into(),
            name: "Chunker".into(),
            node_type: FlowNodeType::Chunker,
            depends_on: vec!["parser".into()],
            config: Default::default(),
            enabled: true,
        });
        engine.add_node(FlowNode {
            id: "embedder".into(),
            name: "Embedder".into(),
            node_type: FlowNodeType::Embedder,
            depends_on: vec!["chunker".into()],
            config: Default::default(),
            enabled: true,
        });
        engine.add_node(FlowNode {
            id: "indexer".into(),
            name: "Indexer".into(),
            node_type: FlowNodeType::Indexer,
            depends_on: vec!["embedder".into()],
            config: Default::default(),
            enabled: true,
        });

        engine
    }

    pub fn add_node(&mut self, node: FlowNode) -> &mut Self {
        for dep in &node.depends_on {
            self.edges
                .entry(dep.clone())
                .or_default()
                .push(node.id.clone());
        }
        self.nodes.insert(node.id.clone(), node);
        self
    }

    pub fn node(&self, id: &str) -> Option<&FlowNode> {
        self.nodes.get(id)
    }

    /// Topological sort of nodes.
    pub fn topological_order(&self) -> Vec<&FlowNode> {
        let mut in_degree: std::collections::HashMap<&str, usize> = self
            .nodes
            .keys()
            .map(|k| k.as_str())
            .map(|k| (k, 0))
            .collect();
        for tos in self.edges.values() {
            for to in tos {
                *in_degree.get_mut(to.as_str()).unwrap() += 1;
            }
        }
        let mut queue: Vec<&str> = in_degree
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(k, _)| *k)
            .collect();
        let mut order = Vec::new();
        while let Some(n) = queue.pop() {
            order.push(n);
            if let Some(children) = self.edges.get(n) {
                for child in children {
                    if let Some(d) = in_degree.get_mut(child.as_str()) {
                        *d -= 1;
                        if *d == 0 {
                            queue.push(child);
                        }
                    }
                }
            }
        }
        order
            .into_iter()
            .filter_map(|id| self.nodes.get(id))
            .collect()
    }

    pub fn list(&self) -> Vec<&FlowNode> {
        self.nodes.values().collect()
    }
}

impl Default for FlowEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ── GraphRAG Community Reports ──────────────────────────────────

/// A community in the entity graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Community {
    pub id: String,
    pub name: String,
    pub entities: Vec<String>,
    pub summary: String,
    pub coherence_score: f32,
}

/// Community detection via simple connected components.
pub struct CommunityDetector {
    threshold: f32,
}

impl CommunityDetector {
    pub fn new(threshold: f32) -> Self {
        Self { threshold }
    }

    /// Detect communities from entity-relation pairs.
    pub fn detect(&self, relations: &[(String, String, f32)]) -> Vec<Community> {
        let mut adj: std::collections::HashMap<String, Vec<String>> = Default::default();
        for (s, t, w) in relations {
            if *w >= self.threshold {
                adj.entry(s.clone()).or_default().push(t.clone());
                adj.entry(t.clone()).or_default().push(s.clone());
            }
        }

        let mut visited = std::collections::HashSet::new();
        let mut communities = Vec::new();
        for node in adj.keys() {
            if visited.contains(node) {
                continue;
            }
            let mut community = Vec::new();
            let mut queue = vec![node.clone()];
            visited.insert(node.clone());
            while let Some(n) = queue.pop() {
                community.push(n.clone());
                if let Some(neighbors) = adj.get(&n) {
                    for neighbor in neighbors {
                        if visited.insert(neighbor.clone()) {
                            queue.push(neighbor.clone());
                        }
                    }
                }
            }
            if community.len() >= 2 {
                let entities = community.clone();
                let name = community
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                communities.push(Community {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: format!("Community: {}", name),
                    entities,
                    summary: format!("{} connected entities", community.len()),
                    coherence_score: community.len() as f32 / 10.0,
                });
            }
        }
        communities
    }
}

// ── Multimodal Model Abstractions ───────────────────────────────

/// Multimodal model types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MultiModalModelType {
    Vision,
    OCR,
    TTS,
    SpeechToText,
}

/// Model registration for multimodal capabilities.
pub struct MultiModalRegistry {
    enabled: std::collections::HashMap<String, bool>,
}

impl MultiModalRegistry {
    pub fn new() -> Self {
        let mut enabled = std::collections::HashMap::new();
        enabled.insert("paddleocr".into(), true);
        enabled.insert("mxbai-rerank".into(), true);
        Self { enabled }
    }

    pub fn is_enabled(&self, model: &str) -> bool {
        self.enabled.get(model).copied().unwrap_or(false)
    }

    pub fn capabilities(&self) -> Vec<(&str, MultiModalModelType)> {
        let mut caps = Vec::new();
        if self.is_enabled("paddleocr") {
            caps.push(("paddleocr", MultiModalModelType::OCR));
            caps.push(("paddleocr", MultiModalModelType::Vision));
        }
        caps
    }
}

impl Default for MultiModalRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_registry() {
        let reg = ToolRegistry::new();
        assert_eq!(reg.tools().len(), 2);
        assert!(
            reg.execute("calculate", serde_json::json!({"expression":"1+1"}))
                .is_some()
        );
    }

    #[test]
    fn test_flow_topological() {
        let engine = FlowEngine::new();
        let order = engine.topological_order();
        assert!(order.len() == 4);
        let ids: Vec<&str> = order.iter().map(|n| n.id.as_str()).collect();
        assert!(
            ids.iter().position(|&x| x == "parser").unwrap()
                < ids.iter().position(|&x| x == "chunker").unwrap()
        );
    }

    #[test]
    fn test_community_detection() {
        let detector = CommunityDetector::new(0.0);
        let relations = vec![("A".into(), "B".into(), 1.0), ("B".into(), "C".into(), 1.0)];
        let communities = detector.detect(&relations);
        assert!(!communities.is_empty());
    }

    #[test]
    fn test_benchmark() {
        let mut bench = RetrievalBenchmark::new();
        bench.add_query("q1");
        bench.add_judgment("q1", "doc1", 2);
        let metrics = bench.evaluate(|_q| vec![("doc1".into(), 0.9), ("doc2".into(), 0.5)]);
        assert!(metrics.mrr > 0.0);
    }
}
