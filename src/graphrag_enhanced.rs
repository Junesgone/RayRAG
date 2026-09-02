//! Enhanced GraphRAG — entity recognition, resolution, graph search.
//! Replaces RAGFlow's `rag/graphrag/ner/graph_extractor.py` + `entity_resolution.py`.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

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
    #[serde(default)]
    #[allow(dead_code)]
    source_id: Vec<String>,
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
                    source_id: Vec::new(),
                    connections: Vec::new(),
                });
        }
    }

    /// Add (or update) a node with description + source id — mirrors
    /// generate_subgraph's `subgraph.add_node(ent["entity_name"], **ent)`.
    /// Unlike set_description (update-only), this creates the node if needed.
    pub fn add_node(
        &mut self,
        name: &str,
        entity_type: EntityType,
        description: impl Into<String>,
        source_id: impl Into<String>,
    ) {
        let node = self
            .nodes
            .entry(name.to_string())
            .or_insert_with(|| GraphNode {
                name: name.to_string(),
                entity_type,
                description: String::new(),
                source_id: Vec::new(),
                connections: Vec::new(),
            });
        if !node.description.is_empty() {
            node.description.push_str("<SEP>");
        }
        node.description.push_str(&description.into());
        let sid = source_id.into();
        if !node.source_id.iter().any(|s| s == &sid) {
            node.source_id.push(sid);
        }
    }

    /// Set a node's source_id (which chunks/documents it came from).
    pub fn set_source_id(&mut self, name: &str, source_id: impl Into<String>) {
        if let Some(node) = self.nodes.get_mut(name) {
            node.source_id = vec![source_id.into()];
        }
    }

    /// Append to a node's source_id list (used by graph_merge).
    pub fn append_source_id(&mut self, name: &str, source_id: &str) {
        if let Some(node) = self.nodes.get_mut(name)
            && !node.source_id.iter().any(|s| s == source_id) {
                node.source_id.push(source_id.to_string());
            }
    }

    /// Source ids of a node, if any.
    pub fn source_id_of(&self, name: &str) -> Vec<String> {
        self.nodes
            .get(name)
            .map(|n| n.source_id.clone())
            .unwrap_or_default()
    }

    /// Remove a node and all its edges (mirrors nx.Graph.remove_node).
    pub fn remove_node(&mut self, name: &str) {
        self.nodes.remove(name);
        for node in self.nodes.values_mut() {
            node.connections.retain(|(t, _)| t != name);
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
        self.nodes
            .get(name)
            .map(|n| n.connections.len())
            .unwrap_or(0)
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

    /// Edge weight between two nodes (either direction), if any.
    pub fn edge_weight(&self, a: &str, b: &str) -> Option<f64> {
        self.nodes.get(a).and_then(|n| {
            n.connections
                .iter()
                .find(|(t, _)| t == b)
                .map(|(_, w)| *w as f64)
        })
    }

    /// Overwrite (or create) the weight of an undirected edge.
    pub fn set_edge_weight(&mut self, a: &str, b: &str, weight: f64) {
        let set = |node: &mut GraphNode| {
            if let Some((_, w)) = node.connections.iter_mut().find(|(t, _)| t == b) {
                *w = weight as f32;
            }
        };
        if let Some(node) = self.nodes.get_mut(a) {
            set(node);
        }
        if let Some(node) = self.nodes.get_mut(b)
            && let Some((_, w)) = node.connections.iter_mut().find(|(t, _)| t == a) {
                *w = weight as f32;
            }
    }

    /// Set a node's entity type (re-typing after extraction).
    pub fn set_node_type(&mut self, name: &str, entity_type: EntityType) {
        if let Some(node) = self.nodes.get_mut(name) {
            node.entity_type = entity_type;
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
                source_id: node.source_id.clone(),
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

    /// Entities of a given type (mirrors ES `entity_type_kwd` filter).
    pub fn entities_by_type(&self, entity_type: &EntityType) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|(_, n)| n.entity_type == *entity_type)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Per-type sample counts for the answer-type pool prompt — mirrors
    /// `get_entity_type2samples` (upstream pulls entity_type→samples from
    /// the index; here we count in-graph types).
    pub fn entity_type_pool(&self) -> serde_json::Value {
        let mut counts: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (name, node) in &self.nodes {
            let label = format!("{:?}", node.entity_type);
            let samples = counts.entry(label).or_default();
            if samples.len() < 3 {
                samples.push(name.clone());
            }
        }
        let mut obj = serde_json::Map::new();
        for (k, v) in counts {
            obj.insert(
                k,
                serde_json::Value::Array(v.into_iter().map(serde_json::Value::String).collect()),
            );
        }
        serde_json::Value::Object(obj)
    }

    /// N-hop paths from an entity with edge weights — mirrors the
    /// `n_hop_with_weight` field consumed by KGSearch (each path is a node
    /// sequence plus per-edge weights; downstream weights deep hops less).
    pub fn n_hop_paths(
        &self,
        entity_name: &str,
        max_depth: usize,
        max_paths: usize,
    ) -> Vec<(Vec<String>, Vec<f64>)> {
        let mut results = Vec::new();
        let mut visited = HashSet::new();
        self.dfs_paths(
            entity_name,
            0,
            max_depth,
            max_paths,
            &mut visited,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut results,
        );
        results
    }

    #[allow(clippy::too_many_arguments)]
    fn dfs_paths(
        &self,
        name: &str,
        depth: usize,
        max_depth: usize,
        max_paths: usize,
        visited: &mut HashSet<String>,
        path: &mut Vec<String>,
        weights: &mut Vec<f64>,
        results: &mut Vec<(Vec<String>, Vec<f64>)>,
    ) {
        if results.len() >= max_paths || depth > max_depth || visited.contains(name) {
            return;
        }
        visited.insert(name.to_string());
        path.push(name.to_string());
        if depth > 0 {
            results.push((path.clone(), weights.clone()));
        }
        if let Some(node) = self.nodes.get(name) {
            for (target, weight) in &node.connections {
                weights.push(*weight as f64);
                self.dfs_paths(
                    target,
                    depth + 1,
                    max_depth,
                    max_paths,
                    visited,
                    path,
                    weights,
                    results,
                );
                weights.pop();
            }
        }
        path.pop();
        visited.remove(name);
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
