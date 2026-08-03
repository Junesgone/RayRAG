//! Enhanced GraphRAG — entity recognition, resolution, graph search.
//! Replaces RAGFlow's `rag/graphrag/ner/graph_extractor.py` + `entity_resolution.py`.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ── Named Entity Recognition ────────────────────────────────────

/// Recognized entity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NerEntity {
    pub name: String,
    pub entity_type: EntityType,
    pub start: usize,
    pub end: usize,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EntityType {
    Person,
    Organization,
    Location,
    Date,
    Technology,
    Product,
    Event,
    Concept,
    Unknown,
}

/// Regex-based NER using common patterns.
pub struct NerExtractor {
    patterns: Vec<(regex::Regex, EntityType)>,
}

impl NerExtractor {
    pub fn new() -> Self {
        let patterns = [
            (
                r"\b(?:Dr\.|Mr\.|Mrs\.|Ms\.|Prof\.)\s+[A-Z][a-z]+\s+[A-Z][a-z]+\b",
                EntityType::Person,
            ),
            (r"\b[A-Z][a-z]+ [A-Z][a-z]+\b", EntityType::Person),
            (r"\b[A-Z]{2,6}\b", EntityType::Organization),
            (
                r"\b[A-Z][a-zA-Z&\. ]+(?:Inc\.?|Ltd\.?|Corp\.?|LLC|GmbH|Co\.?|Corporation|Limited|Group|Technologies|Systems)\b",
                EntityType::Organization,
            ),
            (
                r"\b(?:Beijing|Shanghai|Tokyo|London|Paris|New York|San Francisco|Berlin|Singapore|Sydney|Toronto)\b",
                EntityType::Location,
            ),
            (
                r"\b(?:China|Japan|USA|UK|Germany|France|Canada|Australia)\b",
                EntityType::Location,
            ),
            (r"\b\d{4}-\d{2}-\d{2}\b", EntityType::Date),
            (
                r"\b(?:January|February|March|April|May|June|July|August|September|October|November|December)\s+\d{1,2},?\s+\d{4}\b",
                EntityType::Date,
            ),
            (
                r"\b(?:Rust|Python|JavaScript|TypeScript|Go|Java|C\+\+|Kubernetes|Docker|TensorFlow|PyTorch|React|Vue|Angular|LLM|GPT|BERT|Transformer)\b",
                EntityType::Technology,
            ),
            (
                r"\b(?:RAGFlow|RayRAG|Elasticsearch|OpenSearch|PostgreSQL|MySQL|MongoDB|Redis|Kafka)\b",
                EntityType::Product,
            ),
        ]
        .into_iter()
        .map(|(pattern, entity_type)| (regex::Regex::new(pattern).unwrap(), entity_type))
        .collect();

        Self { patterns }
    }

    /// Extract entities from text.
    pub fn extract(&self, text: &str) -> Vec<NerEntity> {
        let mut entities = Vec::new();
        let mut seen = HashSet::new();

        for (re, entity_type) in &self.patterns {
            for m in re.find_iter(text) {
                let name = m.as_str().to_string();
                let key = format!("{:?}:{}", entity_type, name);
                if seen.contains(&key) {
                    continue;
                }
                seen.insert(key);
                entities.push(NerEntity {
                    name,
                    entity_type: entity_type.clone(),
                    start: m.start(),
                    end: m.end(),
                    confidence: 0.85,
                });
            }
        }

        // Sort by position
        entities.sort_by_key(|e| e.start);
        entities
    }

    /// Extract entities grouped by type.
    pub fn extract_grouped(&self, text: &str) -> HashMap<String, Vec<String>> {
        let entities = self.extract(text);
        let mut grouped: HashMap<String, Vec<String>> = HashMap::new();

        for e in entities {
            let type_name = format!("{:?}", e.entity_type);
            grouped.entry(type_name).or_default().push(e.name);
        }

        grouped
    }
}

impl Default for NerExtractor {
    fn default() -> Self {
        Self::new()
    }
}

// ── Entity Resolution (deduplication) ───────────────────────────

/// Resolve and deduplicate similar entities.
pub struct EntityResolver {
    /// Threshold for fuzzy matching (0.0-1.0)
    threshold: f32,
}

impl EntityResolver {
    pub fn new(threshold: f32) -> Self {
        Self { threshold }
    }

    /// Resolve a list of entity names, grouping similar ones.
    pub fn resolve(&self, entities: &[String]) -> Vec<(String, Vec<String>)> {
        let mut groups: Vec<(String, Vec<String>)> = Vec::new();

        for entity in entities {
            let mut found = false;
            for (canonical, variants) in &mut groups {
                if similarity(entity, canonical) >= self.threshold {
                    if entity != canonical {
                        variants.push(entity.clone());
                    }
                    found = true;
                    break;
                }
            }
            if !found {
                groups.push((entity.clone(), vec![]));
            }
        }

        groups
    }
}

/// Simple character-level similarity (Jaro-Winkler approximation).
fn similarity(a: &str, b: &str) -> f32 {
    let a = a.to_lowercase();
    let b = b.to_lowercase();
    if a == b {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    // Jaccard similarity of character bigrams
    let bigrams_a: HashSet<String> = a
        .chars()
        .collect::<Vec<_>>()
        .windows(2)
        .map(|w| w.iter().collect())
        .collect();
    let bigrams_b: HashSet<String> = b
        .chars()
        .collect::<Vec<_>>()
        .windows(2)
        .map(|w| w.iter().collect())
        .collect();

    let intersection = bigrams_a.intersection(&bigrams_b).count();
    let union = bigrams_a.len() + bigrams_b.len() - intersection;

    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

// ── Graph Search ────────────────────────────────────────────────

/// Entity graph node.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GraphNode {
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    entity_type: EntityType,
    #[serde(default)]
    #[allow(dead_code)]
    description: String,
    connections: Vec<(String, f32)>, // (target_name, weight)
}

/// Simple entity graph with search.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityGraph {
    nodes: HashMap<String, GraphNode>,
}

impl EntityGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
        }
    }

    /// Add entities as nodes.
    pub fn add_entities(&mut self, entities: &[NerEntity]) {
        for e in entities {
            self.nodes
                .entry(e.name.clone())
                .or_insert_with(|| GraphNode {
                    name: e.name.clone(),
                    entity_type: e.entity_type.clone(),
                    description: String::new(),
                    connections: Vec::new(),
                });
        }
    }

    /// Set a node description (community reports render it into the prompt).
    pub fn set_description(&mut self, name: &str, description: impl Into<String>) {
        if let Some(node) = self.nodes.get_mut(name) {
            node.description = description.into();
        }
    }

    /// Node names present in the graph.
    pub fn node_names(&self) -> Vec<String> {
        self.nodes.keys().cloned().collect()
    }

    /// Degree (connection count) for a node — RAGFlow computes `rank` from
    /// graph.degree before community detection.
    pub fn degree(&self, name: &str) -> usize {
        self.nodes.get(name).map(|n| n.connections.len()).unwrap_or(0)
    }

    /// Neighbor names of a node.
    pub fn neighbors(&self, name: &str) -> Vec<String> {
        self.nodes
            .get(name)
            .map(|n| n.connections.iter().map(|(t, _)| t.clone()).collect())
            .unwrap_or_default()
    }

    /// Description of a node, if set.
    pub fn description_of(&self, name: &str) -> Option<&str> {
        self.nodes.get(name).map(|n| n.description.as_str())
    }

    /// Description of the edge between two nodes (either direction), if any.
    pub fn edge_description(&self, a: &str, b: &str) -> Option<&str> {
        // RayRAG edges carry only weight; return a generic edge description
        // (None means no edge). We synthesize "connected" so community
        // reports render a relation row per edge.
        if self
            .nodes
            .get(a)
            .is_some_and(|n| n.connections.iter().any(|(t, _)| t == b))
        {
            Some("connected")
        } else {
            None
        }
    }

    /// Add a relation between two entities.
    pub fn add_relation(&mut self, source: &str, target: &str, weight: f32) {
        if let Some(node) = self.nodes.get_mut(source) {
            node.connections.push((target.to_string(), weight));
        }
        if let Some(node) = self.nodes.get_mut(target) {
            node.connections.push((source.to_string(), weight));
        }
    }

    /// Merge a persisted document graph into a KB-level graph.
    pub fn merge(&mut self, other: &Self) {
        for (name, node) in &other.nodes {
            let target = self.nodes.entry(name.clone()).or_insert_with(|| GraphNode {
                name: node.name.clone(),
                entity_type: node.entity_type.clone(),
                description: node.description.clone(),
                connections: Vec::new(),
            });
            for connection in &node.connections {
                if !target.connections.contains(connection) {
                    target.connections.push(connection.clone());
                }
            }
        }
    }

    /// Produce concise graph context for entities mentioned in a query.
    pub fn context_for_query(&self, query: &str, max_items: usize) -> Vec<String> {
        let query = query.to_lowercase();
        let mut context = Vec::new();
        for (name, node) in &self.nodes {
            if !query.contains(&name.to_lowercase()) {
                continue;
            }
            for (target, weight) in &node.connections {
                context.push(format!("{name} -> {target} ({weight:.2})"));
                if context.len() >= max_items {
                    return context;
                }
            }
        }
        context
    }

    /// Number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Search for entities connected to a query entity.
    pub fn search_connected(&self, entity_name: &str, max_depth: usize) -> Vec<String> {
        let mut visited = HashSet::new();
        let mut results = Vec::new();
        self.dfs(entity_name, 0, max_depth, &mut visited, &mut results);
        results
    }

    fn dfs(
        &self,
        name: &str,
        depth: usize,
        max_depth: usize,
        visited: &mut HashSet<String>,
        results: &mut Vec<String>,
    ) {
        if depth > max_depth || visited.contains(name) {
            return;
        }
        visited.insert(name.to_string());

        if let Some(node) = self.nodes.get(name) {
            if depth > 0 {
                results.push(name.to_string());
            }
            for (target, _) in &node.connections {
                self.dfs(target, depth + 1, max_depth, visited, results);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ner_extraction() {
        let ner = NerExtractor::new();
        let entities = ner.extract("John Smith works at Google Inc in San Francisco since 2023.");
        assert!(entities.iter().any(|e| e.name == "John Smith"));
        assert!(entities.iter().any(|e| e.name.contains("Google")));
        assert!(entities.iter().any(|e| e.name == "San Francisco"));
    }

    #[test]
    fn test_entity_resolution() {
        let resolver = EntityResolver::new(0.3); // Lower threshold for short names
        let groups = resolver.resolve(&[
            "Google Inc".into(),
            "Google".into(),
            "Microsoft Corp".into(),
        ]);
        assert!(groups.len() == 2); // Google Inc + Google merged, Microsoft separate
    }

    #[test]
    fn test_graph_search() {
        let mut graph = EntityGraph::new();
        // Use entity names that match our patterns
        let entities = NerExtractor::new().extract("John Smith works at Google Inc and uses Rust.");
        graph.add_entities(&entities);
        graph.add_relation("John Smith", "Google Inc", 1.0);
        graph.add_relation("Google Inc", "Rust", 1.0);

        let connected = graph.search_connected("John Smith", 2);
        assert!(!connected.is_empty());
    }
}
