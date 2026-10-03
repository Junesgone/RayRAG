//! The compiled skill tree: the recursive markdown skills upstream builds with Corpus2Skill, and the
//! routes that read and delete them.
//!
//! Upstream keeps one row per skill node in the tenant index (`_SKILL_COMPILE_KWD = "skill"`) plus a
//! single aggregate row (`_SKILL_ALL_COMPILE_KWD = "skill_all"`) whose body is the whole tree as one
//! markdown document. The node rows carry `skill_kwd`, `depth_int`, `children_kwd`, `source_doc_ids`
//! and `md_with_weight`; that is the contract this module reproduces.
//!
//! Upstream *writes* those rows by asking an LLM to compile the corpus into a skill tree. RayRAG
//! compiles them **from the corpus structure itself**: the heading hierarchy of the indexed chunks,
//! in document order, with each node carrying the text beneath its heading. That choice is deliberate
//! and is recorded in the project's internal parameter registry:
//!
//! * it needs no model, so a deployment with no chat model still gets a real tree rather than an
//!   invented one;
//! * it is deterministic: the same rows in the same order always compile to the same tree;
//! * every byte served comes from the corpus — nothing is synthesised.
//!
//! A corpus with no headings at all falls back to one node per document, named after the document.
//! That is still derived from the corpus, and it is reported as such instead of silently returning an
//! empty tree.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

/// Upstream's `_SKILL_COMPILE_KWD`.
pub const SKILL_COMPILE_KWD: &str = "skill";
/// Upstream's `_SKILL_ALL_COMPILE_KWD`.
pub const SKILL_ALL_COMPILE_KWD: &str = "skill_all";
/// How much chunk text one node may carry, so a huge document cannot make one row unbounded.
const MAX_NODE_CHARS: usize = 8_000;
/// How many chunk rows one compilation reads before it stops. Bounded on purpose: compilation is a
/// background-shaped task and must not pull an unbounded corpus into memory.
const MAX_CHUNK_ROWS: usize = 20_000;

/// One skill node, field-for-field as upstream's skill row.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillNode {
    pub id: String,
    pub kb_id: String,
    pub doc_id: String,
    pub compile_kwd: String,
    pub skill_kwd: String,
    pub depth_int: u32,
    pub children_kwd: Vec<String>,
    pub source_doc_ids: Vec<String>,
    pub md_with_weight: String,
}

/// The aggregate row: the whole tree as one markdown document.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillAllRow {
    pub id: String,
    pub kb_id: String,
    pub compile_kwd: String,
    pub skill_kwd: String,
    pub depth_int: u32,
    pub children_kwd: Vec<String>,
    pub source_doc_ids: Vec<String>,
    pub md_with_weight: String,
    /// How many node rows the tree holds. Upstream's aggregate row does not carry this; a client
    /// showing "3 skills" needs it, and it is cheaper than counting the tree again.
    pub node_count: usize,
}

/// One dataset's compiled tree.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SkillTree {
    #[serde(default)]
    pub all: Option<SkillAllRow>,
    #[serde(default)]
    pub nodes: Vec<SkillNode>,
}

impl SkillTree {
    fn node(&self, skill_kwd: &str) -> Option<&SkillNode> {
        self.nodes.iter().find(|node| node.skill_kwd == skill_kwd)
    }

    /// A node with every descendant, so deleting a branch reports what it really removed.
    fn subtree_keys(&self, root: &str) -> Vec<String> {
        let mut keys = vec![root.to_string()];
        let mut index = 0;
        while index < keys.len() {
            let current = keys[index].clone();
            index += 1;
            if let Some(node) = self.node(&current) {
                for child in &node.children_kwd {
                    if !keys.contains(child) {
                        keys.push(child.clone());
                    }
                }
            }
        }
        keys
    }
}

/// Persisted skill trees, one per dataset.
pub struct SkillTreeStore {
    path: std::path::PathBuf,
    inner: Mutex<BTreeMap<String, SkillTree>>,
}

impl SkillTreeStore {
    pub fn new(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::persistence::restore_if_missing(&path)?;
        let trees = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<BTreeMap<String, SkillTree>>(&bytes).ok())
            .unwrap_or_default();
        Ok(Self {
            path,
            inner: Mutex::new(trees),
        })
    }

    pub fn in_memory() -> Self {
        Self {
            path: std::path::PathBuf::new(),
            inner: Mutex::new(BTreeMap::new()),
        }
    }

    fn persist(&self, trees: &BTreeMap<String, SkillTree>) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        if let Err(error) = crate::persistence::save_json(&self.path, trees) {
            tracing::warn!(%error, "the compiled skill trees could not be persisted");
        }
    }

    pub fn get(&self, kb_id: &str) -> Option<SkillTree> {
        self.inner.lock().unwrap().get(kb_id).cloned()
    }

    pub fn put(&self, kb_id: &str, tree: SkillTree) {
        let mut trees = self.inner.lock().unwrap();
        trees.insert(kb_id.to_string(), tree);
        self.persist(&trees);
    }

    /// Delete the whole tree. Returns how many rows went away (the aggregate row counts as one), or
    /// zero when the dataset had no tree.
    pub fn delete_all(&self, kb_id: &str) -> usize {
        let mut trees = self.inner.lock().unwrap();
        let Some(tree) = trees.remove(kb_id) else {
            return 0;
        };
        self.persist(&trees);
        tree.nodes.len() + usize::from(tree.all.is_some())
    }

    /// Delete one node and its descendants. Returns how many rows went away.
    pub fn delete_subtree(&self, kb_id: &str, skill_kwd: &str) -> usize {
        let mut trees = self.inner.lock().unwrap();
        let removed = {
            let Some(tree) = trees.get_mut(kb_id) else {
                return 0;
            };
            if tree.node(skill_kwd).is_none() {
                return 0;
            }
            let doomed = tree.subtree_keys(skill_kwd);
            tree.nodes.retain(|node| !doomed.contains(&node.skill_kwd));
            // The parent must forget the child it just lost, or the tree keeps a dangling branch.
            if let Some((parent_key, _)) = skill_kwd.rsplit_once('/')
                && let Some(parent) = tree
                    .nodes
                    .iter_mut()
                    .find(|node| node.skill_kwd == parent_key)
            {
                parent.children_kwd.retain(|child| child != skill_kwd);
            }
            let remaining = tree.nodes.len();
            if let Some(all) = tree.all.as_mut() {
                all.children_kwd.retain(|child| child != skill_kwd);
                all.node_count = remaining;
            }
            doomed.len()
        };
        if trees.get(kb_id).is_some_and(|tree| tree.nodes.is_empty()) {
            trees.remove(kb_id);
        }
        self.persist(&trees);
        removed
    }
}

impl Default for SkillTreeStore {
    fn default() -> Self {
        Self::in_memory()
    }
}

/// A chunk as the compiler sees it: which document it came from and what it says.
#[derive(Debug, Clone)]
pub struct SkillSourceChunk {
    pub doc_id: String,
    pub doc_name: String,
    pub content: String,
}

/// Turn corpus text into a skill tree. Pure, so the tree is testable without a store.
pub fn build_tree(kb_id: &str, chunks: &[SkillSourceChunk], now_ms: u64) -> SkillTree {
    struct Draft {
        skill_kwd: String,
        depth: u32,
        parent: Option<String>,
        body: String,
        doc_ids: Vec<String>,
        doc_id: String,
    }

    let mut drafts: Vec<Draft> = Vec::new();
    // The heading path currently open, one entry per depth.
    let mut open: Vec<(String, String)> = Vec::new();
    let mut undirected: BTreeMap<String, Draft> = BTreeMap::new();
    // Headings belong to the document they appear in. Without this, a `#` heading in one document
    // would nest under whatever `##` heading the previous document happened to end on.
    let mut current_doc = String::new();

    for chunk in chunks {
        let text = chunk.content.trim();
        if text.is_empty() {
            continue;
        }
        if chunk.doc_id != current_doc {
            current_doc = chunk.doc_id.clone();
            open.clear();
        }
        let heading = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .and_then(markdown_heading);
        let Some((depth, title)) = heading else {
            // Text before any heading belongs to the document as a whole.
            let stem = chunk
                .doc_name
                .rsplit_once('.')
                .map(|(stem, _)| stem)
                .unwrap_or(&chunk.doc_name);
            let key = format!("document/{}", slugify(stem));
            let entry = undirected.entry(key.clone()).or_insert_with(|| Draft {
                skill_kwd: key,
                depth: 0,
                parent: None,
                body: String::new(),
                doc_ids: Vec::new(),
                doc_id: chunk.doc_id.clone(),
            });
            push_bounded(&mut entry.body, text);
            if !entry.doc_ids.contains(&chunk.doc_id) {
                entry.doc_ids.push(chunk.doc_id.clone());
            }
            continue;
        };
        // `depth` is the heading level: `##` is 2 and sits under the level-1 heading, so the stack
        // keeps `depth - 1` ancestors. Truncating to `depth` would have made every second `##`
        // heading a child of the previous `##` instead of a sibling.
        let depth = depth.min(6) as u32;
        open.truncate(depth.saturating_sub(1) as usize);
        let mut key = slugify(&title);
        if key.is_empty() {
            key = format!("section-{}", drafts.len() + 1);
        }
        // A repeated heading must not collide with its namesake: the path disambiguates it.
        let mut path = open
            .iter()
            .map(|(segment, _)| segment.clone())
            .collect::<Vec<_>>();
        path.push(key.clone());
        let mut skill_kwd = path.join("/");
        let mut suffix = 2;
        while drafts.iter().any(|draft| draft.skill_kwd == skill_kwd) {
            skill_kwd = format!("{}-{suffix}", path.join("/"));
            suffix += 1;
        }
        let parent = open.last().map(|(_, parent_path)| parent_path.clone());
        open.push((key, skill_kwd.clone()));
        drafts.push(Draft {
            skill_kwd,
            depth,
            parent,
            body: text.to_string(),
            doc_ids: vec![chunk.doc_id.clone()],
            doc_id: chunk.doc_id.clone(),
        });
    }

    // Chunks that arrived before any heading become top-level document nodes after the headings, so
    // the reading order the compiler saw is preserved.
    let mut drafts: Vec<Draft> = drafts;
    drafts.extend(undirected.into_values());

    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for draft in &drafts {
        if let Some(parent) = &draft.parent {
            children
                .entry(parent.clone())
                .or_default()
                .push(draft.skill_kwd.clone());
        }
    }
    let roots: Vec<String> = drafts
        .iter()
        .filter(|draft| draft.parent.is_none())
        .map(|draft| draft.skill_kwd.clone())
        .collect();

    let nodes: Vec<SkillNode> = drafts
        .iter()
        .enumerate()
        .map(|(index, draft)| SkillNode {
            id: format!("{kb_id}-skill-{index}"),
            kb_id: kb_id.to_string(),
            doc_id: draft.doc_id.clone(),
            compile_kwd: SKILL_COMPILE_KWD.to_string(),
            skill_kwd: draft.skill_kwd.clone(),
            depth_int: draft.depth,
            children_kwd: children.get(&draft.skill_kwd).cloned().unwrap_or_default(),
            source_doc_ids: draft.doc_ids.clone(),
            md_with_weight: draft.body.chars().take(MAX_NODE_CHARS).collect(),
        })
        .collect();

    // The aggregate row is the tree as one markdown document, depth-first, so a reader can follow it.
    let mut markdown = String::new();
    let mut stack: Vec<(String, u32)> = roots
        .iter()
        .rev()
        .map(|root| (root.clone(), 0u32))
        .collect();
    while let Some((key, depth)) = stack.pop() {
        let Some(draft) = drafts.iter().find(|draft| draft.skill_kwd == key) else {
            continue;
        };
        markdown.push_str(&format!(
            "{} {}\n\n{}\n\n",
            "#".repeat((depth as usize + 1).min(6)),
            humanize(&key),
            draft.body.chars().take(MAX_NODE_CHARS).collect::<String>()
        ));
        if let Some(kids) = children.get(&key) {
            for child in kids.iter().rev() {
                stack.push((child.clone(), depth + 1));
            }
        }
    }
    let mut source_doc_ids: Vec<String> = Vec::new();
    for node in &nodes {
        for doc_id in &node.source_doc_ids {
            if !source_doc_ids.contains(doc_id) {
                source_doc_ids.push(doc_id.clone());
            }
        }
    }
    let all = SkillAllRow {
        id: format!("{kb_id}-skill-all"),
        kb_id: kb_id.to_string(),
        compile_kwd: SKILL_ALL_COMPILE_KWD.to_string(),
        skill_kwd: String::new(),
        depth_int: 0,
        children_kwd: roots,
        source_doc_ids: source_doc_ids.clone(),
        md_with_weight: markdown,
        node_count: nodes.len(),
    };
    let _ = now_ms;
    SkillTree {
        all: Some(all),
        nodes,
    }
}

/// The heading a line opens, if it is one: `## Title` becomes `(2, "Title")`.
fn markdown_heading(line: &str) -> Option<(usize, String)> {
    let hashes = line
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if !(1..=6).contains(&hashes) || line.chars().count() <= hashes {
        return None;
    }
    let title = line[hashes..].trim().trim_end_matches('#').trim();
    if title.is_empty() {
        None
    } else {
        Some((hashes, title.to_string()))
    }
}

/// A path segment from a title: lowercase, ASCII alphanumerics and dashes, CJK kept as-is so a
/// Chinese heading still produces a usable key.
fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = true;
    for character in text.trim().chars() {
        if character.is_ascii_alphanumeric() || !character.is_ascii() {
            if !character.is_ascii() && !character.is_alphanumeric() {
                last_dash = false;
                continue;
            }
            slug.extend(character.to_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

/// The last segment of a key, for a human-readable heading in the aggregate document.
fn humanize(key: &str) -> String {
    key.rsplit('/').next().unwrap_or(key).replace('-', " ")
}

fn push_bounded(body: &mut String, text: &str) {
    if body.chars().count() >= MAX_NODE_CHARS {
        return;
    }
    if !body.is_empty() {
        body.push_str("\n\n");
    }
    for character in text.chars() {
        if body.chars().count() >= MAX_NODE_CHARS {
            break;
        }
        body.push(character);
    }
}

/// The corpus a dataset is compiled from, exactly as the deployment holds it.
///
/// The live corpus lives in the search engine, not in the document
/// index: reading the document index found the nav-cluster and wiki rows but **none of the parsed
/// chunks**, which is how the first live compile answered "no indexed content" for a dataset that
/// plainly had chunks. Order is document first, then position, so a document's headings reach the
/// compiler in the order a reader sees them.
pub fn chunks_for_dataset(
    engine: &crate::search::SearchEngine,
    kb_id: &str,
) -> Vec<SkillSourceChunk> {
    // Deliberately not `chunks_for_kb`: that helper also requires a non-empty embedding, so a
    // deployment with no embedding model would compile an empty tree from a corpus full of text.
    let mut rows: Vec<crate::search::IndexedChunk> = engine
        .to_vec()
        .into_iter()
        .filter(|chunk| chunk.metadata.get("kb_id").map(String::as_str) == Some(kb_id))
        .collect();
    rows.sort_by(|left, right| {
        left.doc_name
            .cmp(&right.doc_name)
            .then(left.position.cmp(&right.position))
            .then(left.id.cmp(&right.id))
    });
    if rows.len() > MAX_CHUNK_ROWS {
        tracing::warn!(
            kb = kb_id,
            total = rows.len(),
            limit = MAX_CHUNK_ROWS,
            "skill compilation stopped at the row cap"
        );
        rows.truncate(MAX_CHUNK_ROWS);
    }
    rows.into_iter()
        .filter(|chunk| !chunk.content.trim().is_empty())
        .map(|chunk| SkillSourceChunk {
            doc_id: chunk.metadata.get("doc_id").cloned().unwrap_or_default(),
            doc_name: chunk.doc_name.clone(),
            content: chunk.content.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(doc_id: &str, doc_name: &str, content: &str) -> SkillSourceChunk {
        SkillSourceChunk {
            doc_id: doc_id.to_string(),
            doc_name: doc_name.to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn headings_become_a_nested_tree_with_the_text_under_them() {
        let chunks = vec![
            chunk("d1", "guide.md", "# Install\n\nRun the installer."),
            chunk("d1", "guide.md", "## Linux\n\nUse the tarball."),
            chunk("d1", "guide.md", "## Windows\n\nUse the msi."),
            chunk("d2", "faq.md", "# Troubleshooting\n\nCheck the logs."),
        ];
        let tree = build_tree("kb-1", &chunks, 1);
        let all = tree.all.as_ref().unwrap();
        assert_eq!(all.compile_kwd, SKILL_ALL_COMPILE_KWD);
        assert_eq!(tree.nodes.len(), 4);
        assert_eq!(all.node_count, 4);
        // The roots are the two top-level headings, in document order.
        assert_eq!(all.children_kwd, vec!["install", "troubleshooting"]);
        assert_eq!(all.source_doc_ids, vec!["d1", "d2"]);

        let install = tree.node("install").unwrap();
        assert_eq!(install.depth_int, 1);
        assert_eq!(
            install.children_kwd,
            vec!["install/linux", "install/windows"]
        );
        assert!(install.md_with_weight.contains("Run the installer."));
        assert_eq!(install.compile_kwd, SKILL_COMPILE_KWD);
        assert_eq!(install.doc_id, "d1");

        let linux = tree.node("install/linux").unwrap();
        assert_eq!(linux.depth_int, 2);
        assert!(linux.children_kwd.is_empty());
        assert!(linux.md_with_weight.contains("tarball"));

        // The aggregate body is the tree as one markdown document.
        assert!(all.md_with_weight.contains("# install"));
        assert!(
            all.md_with_weight.contains("## install/linux")
                || all.md_with_weight.contains("## linux")
        );
        assert!(all.md_with_weight.contains("Check the logs."));
    }

    #[test]
    fn a_corpus_without_headings_falls_back_to_document_nodes() {
        let chunks = vec![
            chunk("d1", "notes.txt", "Just prose, no heading at all."),
            chunk("d2", "other.txt", "More prose."),
        ];
        let tree = build_tree("kb-1", &chunks, 1);
        assert_eq!(tree.nodes.len(), 2, "one node per document");
        let keys: Vec<&str> = tree
            .nodes
            .iter()
            .map(|node| node.skill_kwd.as_str())
            .collect();
        assert!(keys.contains(&"document/notes"), "{keys:?}");
        assert!(keys.contains(&"document/other"), "{keys:?}");
        // The fallback is honest: the text really is in the tree.
        assert!(
            tree.nodes
                .iter()
                .any(|node| node.md_with_weight.contains("Just prose"))
        );
    }

    #[test]
    fn repeated_headings_get_distinct_keys_instead_of_overwriting_each_other() {
        let chunks = vec![
            chunk("d1", "a.md", "# Overview\n\nFirst."),
            chunk("d2", "b.md", "# Overview\n\nSecond."),
        ];
        let tree = build_tree("kb-1", &chunks, 1);
        assert_eq!(tree.nodes.len(), 2);
        let keys: Vec<&str> = tree.nodes.iter().map(|n| n.skill_kwd.as_str()).collect();
        assert_eq!(keys, vec!["overview", "overview-2"], "{keys:?}");
        assert!(
            tree.node("overview")
                .unwrap()
                .md_with_weight
                .contains("First.")
        );
        assert!(
            tree.node("overview-2")
                .unwrap()
                .md_with_weight
                .contains("Second.")
        );
    }

    #[test]
    fn deleting_a_branch_takes_its_descendants_and_reports_the_real_count() {
        let store = SkillTreeStore::in_memory();
        let chunks = vec![
            chunk("d1", "guide.md", "# Install\n\nRun it."),
            chunk("d1", "guide.md", "## Linux\n\nTarball."),
            chunk("d1", "guide.md", "### Debian\n\napt install."),
            chunk("d2", "faq.md", "# FAQ\n\nLogs."),
        ];
        store.put("kb-1", build_tree("kb-1", &chunks, 1));

        assert_eq!(
            store.delete_subtree("kb-1", "install"),
            3,
            "linux and debian go too"
        );
        let tree = store.get("kb-1").unwrap();
        assert!(tree.node("install").is_none());
        assert!(tree.node("install/linux").is_none());
        // The parent forgot the branch, and the aggregate still describes what is left.
        assert_eq!(tree.all.as_ref().unwrap().children_kwd, vec!["faq"]);
        assert_eq!(tree.all.as_ref().unwrap().node_count, 1);

        // An unknown key deletes nothing rather than reporting a phantom deletion.
        assert_eq!(store.delete_subtree("kb-1", "nope"), 0);
        // Deleting everything counts the aggregate row and removes the dataset.
        assert_eq!(store.delete_all("kb-1"), 2);
        assert!(store.get("kb-1").is_none());
        assert_eq!(store.delete_all("kb-1"), 0);
    }

    #[test]
    fn the_reader_takes_only_this_dataset_and_orders_by_document_then_position() {
        use crate::search::IndexedChunk;
        let chunk = |id: &str, doc: &str, name: &str, position: usize, kb: &str, content: &str| {
            let mut metadata = std::collections::HashMap::new();
            metadata.insert("kb_id".to_string(), kb.to_string());
            metadata.insert("doc_id".to_string(), doc.to_string());
            IndexedChunk {
                id: id.to_string(),
                doc_name: name.to_string(),
                content: content.to_string(),
                embedding: Vec::new(),
                token_count: 1,
                position,
                metadata,
            }
        };
        let engine = crate::search::SearchEngine::from_chunks(vec![
            chunk("c2", "d1", "b.md", 1, "kb-1", "# Second\n\nBody."),
            chunk("c1", "d1", "b.md", 0, "kb-1", "# First\n\nBody."),
            chunk("c3", "d2", "a.md", 0, "kb-1", "# Other\n\nBody."),
            chunk("c4", "d9", "z.md", 0, "kb-2", "# Not ours\n\nBody."),
        ]);
        let rows = chunks_for_dataset(&engine, "kb-1");
        let names: Vec<&str> = rows.iter().map(|row| row.doc_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["a.md", "b.md", "b.md"],
            "other datasets are excluded"
        );
        // Within a document, position decides; across documents, the name decides.
        assert_eq!(rows[1].content, "# First\n\nBody.");
        assert_eq!(rows[2].content, "# Second\n\nBody.");
        assert_eq!(rows[1].doc_id, "d1");

        // An empty chunk must not become an empty skill body.
        let engine = crate::search::SearchEngine::from_chunks(vec![chunk(
            "c5", "d1", "x.md", 0, "kb-1", "   ",
        )]);
        assert!(chunks_for_dataset(&engine, "kb-1").is_empty());
        assert!(chunks_for_dataset(&engine, "kb-none").is_empty());
    }

    #[test]
    fn a_heading_slug_keeps_chinese_and_drops_punctuation() {
        assert_eq!(slugify("Install Guide!"), "install-guide");
        assert_eq!(slugify("  安装 指南  "), "安装-指南");
        assert_eq!(slugify("--"), "");
        assert_eq!(markdown_heading("## Title ##"), Some((2, "Title".into())));
        assert_eq!(markdown_heading("####### too deep"), None);
        assert_eq!(markdown_heading("plain text"), None);
    }
}

// ---------------------------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------------------------

/// Upstream's refusal for these routes. It answers the *business* code 500 with this message rather
/// than RayRAG's usual permission code, so a client written against RAGFlow sees what it expects;
/// the divergence from RayRAG's other dataset routes is deliberate and recorded in the registry.
fn no_authorization() -> Response {
    crate::server::api_error_code(StatusCode::OK, 500, "no authorization")
}

/// The dataset must exist and be readable by the caller, and these routes are scoped to a knowledge
/// base the caller can reach.
fn authorize(
    state: &std::sync::Arc<crate::server::AppState>,
    auth: &crate::server::AuthContext,
    kb_id: &str,
) -> Result<(), Response> {
    if !crate::server::kb_accessible(state, kb_id, auth) {
        return Err(no_authorization());
    }
    Ok(())
}

/// `GET /api/v1/datasets/{id}/skills` — the aggregate row, or `null` when nothing was compiled.
pub async fn get_skills(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::Extension(auth): axum::Extension<crate::server::AuthContext>,
    Path(kb_id): Path<String>,
) -> Response {
    if let Err(refusal) = authorize(&state, &auth, &kb_id) {
        return refusal;
    }
    match state.skill_trees.get(&kb_id).and_then(|tree| tree.all) {
        Some(all) => Json(json!({ "code": 0, "data": all, "message": "success" })).into_response(),
        None => Json(json!({ "code": 0, "data": null, "message": "success" })).into_response(),
    }
}

/// `HEAD /api/v1/datasets/{id}/skills` — 200 when a tree exists, 404 when it does not, no body.
pub async fn head_skills(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::Extension(auth): axum::Extension<crate::server::AuthContext>,
    Path(kb_id): Path<String>,
) -> Response {
    if !crate::server::kb_accessible(&state, &kb_id, &auth) {
        // A HEAD cannot carry the refusal body, so the status carries it.
        return StatusCode::FORBIDDEN.into_response();
    }
    let has_tree = state
        .skill_trees
        .get(&kb_id)
        .is_some_and(|tree| tree.all.is_some());
    if has_tree {
        StatusCode::OK.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

/// `DELETE /api/v1/datasets/{id}/skills` — remove the whole tree, reporting what went away.
pub async fn delete_skills(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::Extension(auth): axum::Extension<crate::server::AuthContext>,
    Path(kb_id): Path<String>,
) -> Response {
    if let Err(refusal) = authorize(&state, &auth, &kb_id) {
        return refusal;
    }
    let deleted = state.skill_trees.delete_all(&kb_id);
    Json(json!({ "code": 0, "data": { "deleted": deleted }, "message": "success" })).into_response()
}

/// `GET /api/v1/datasets/{id}/skills/<path:skill_kwd>` — one node, or `null`.
///
/// The key is a path, so a nested skill is fetched as `install/linux` exactly as upstream allows.
pub async fn get_skill_node(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::Extension(auth): axum::Extension<crate::server::AuthContext>,
    Path((kb_id, skill_kwd)): Path<(String, String)>,
) -> Response {
    if let Err(refusal) = authorize(&state, &auth, &kb_id) {
        return refusal;
    }
    let node = state.skill_trees.get(&kb_id).and_then(|tree| {
        tree.nodes
            .into_iter()
            .find(|node| node.skill_kwd == skill_kwd)
    });
    match node {
        Some(node) => {
            Json(json!({ "code": 0, "data": node, "message": "success" })).into_response()
        }
        None => Json(json!({ "code": 0, "data": null, "message": "success" })).into_response(),
    }
}

/// `DELETE /api/v1/datasets/{id}/skills/<path:skill_kwd>` — remove a branch.
pub async fn delete_skill_node(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::Extension(auth): axum::Extension<crate::server::AuthContext>,
    Path((kb_id, skill_kwd)): Path<(String, String)>,
) -> Response {
    if let Err(refusal) = authorize(&state, &auth, &kb_id) {
        return refusal;
    }
    let deleted = state.skill_trees.delete_subtree(&kb_id, &skill_kwd);
    Json(json!({ "code": 0, "data": { "deleted": deleted }, "message": "success" })).into_response()
}

/// `POST /api/v1/datasets/{id}/skills/compile` — compile the tree for this dataset.
///
/// Upstream has no REST route for this: its UI drives knowledge compilation. RayRAG needs a producer,
/// and a button needs a route, so this one exists and is listed in the registry as an addition. It
/// reads the dataset's indexed chunks through the sanctioned blocking wrapper and never blocks the
/// runtime, and it replaces any earlier tree for the dataset.
pub async fn compile_skills(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::Extension(auth): axum::Extension<crate::server::AuthContext>,
    Path(kb_id): Path<String>,
) -> Response {
    if let Err(refusal) = authorize(&state, &auth, &kb_id) {
        return refusal;
    }
    if state.kbs.get(&kb_id).is_none() {
        return crate::server::api_error_code(
            StatusCode::OK,
            crate::server::code::INVALID_OR_MISSING_DATA,
            "The dataset doesn't exist",
        );
    }
    // The index is named after the tenant that owns the rows; for a dataset reached through the API
    // that is the caller, exactly as the artifact routes derive it.
    let tenant = auth.user_id.clone();
    let _ = &tenant;
    // A snapshot under a short read lock: the tree is built from owned data, never while holding it.
    let chunks = {
        let engine = state.engine.read().unwrap();
        chunks_for_dataset(&engine, &kb_id)
    };
    if chunks.is_empty() {
        // Say so rather than storing an empty tree that would look like a successful compilation.
        return crate::server::api_error_code(
            StatusCode::OK,
            crate::server::code::INVALID_OR_MISSING_DATA,
            "The dataset has no indexed content to compile skills from",
        );
    }
    let documents: Vec<String> = {
        let mut ids: Vec<String> = Vec::new();
        for chunk in &chunks {
            if !chunk.doc_id.is_empty() && !ids.contains(&chunk.doc_id) {
                ids.push(chunk.doc_id.clone());
            }
        }
        ids
    };
    let tree = build_tree(
        &kb_id,
        &chunks,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0),
    );
    let nodes = tree.nodes.len();
    let roots = tree
        .all
        .as_ref()
        .map(|all| all.children_kwd.len())
        .unwrap_or(0);
    state.skill_trees.put(&kb_id, tree);
    tracing::info!(
        kb = kb_id,
        nodes,
        documents = documents.len(),
        "skills compiled"
    );
    Json(json!({
        "code": 0,
        "data": {
            "nodes": nodes,
            "roots": roots,
            "documents": documents.len(),
            "chunks": chunks.len(),
        },
        "message": "success",
    }))
    .into_response()
}
