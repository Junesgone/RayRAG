//! Mind map extractor — RAGFlow v0.27.2
//! `rag/advanced_rag/knowlege_compile/mind_map_extractor.py`.
//!
//! Extracts a unipartite mind map from text sections: the model replies in
//! markdown, which is converted into a nested `{id, children}` tree and merged
//! across section groups. The Python module converts markdown with the
//! third-party `markdown_to_json` package; this port implements the subset of
//! `dictify` the extractor relies on (ATX headings, ordered/unordered lists
//! with 2-space indentation, paragraphs) plus the `_todict` / `_list_to_kv`
//! post-processing, and documents that divergence.

use regex::Regex;
use serde_json::{Map, Value};

use crate::harness::HarnessChat;

/// `MIND_MAP_EXTRACTION_PROMPT` (verbatim `graphrag/general/mind_map_prompt.py`).
pub const MIND_MAP_EXTRACTION_PROMPT: &str = "\n- Role: You're a talent text processor to summarize a piece of text into a mind map.\n\n- Step of task:\n  1. Generate a title for user's 'TEXT'。\n  2. Classify the 'TEXT' into sections of a mind map.\n  3. If the subject matter is really complex, split them into sub-sections and sub-subsections.\n  4. Add a shot content summary of the bottom level section.\n\n- Output requirement:\n  - Generate at least 4 levels.\n  - Always try to maximize the number of sub-sections.\n  - In language of 'Text'\n  - MUST IN FORMAT OF MARKDOWN\n\n-TEXT-\n{input_text}\n\n";

/// `MindMapResult`.
#[derive(Debug, Clone, PartialEq)]
pub struct MindMapResult {
    pub output: Value,
}

/// `MindMapExtractor` (the chat surface is injected; the Python class also
/// carries an error handler that is never invoked in this path).
pub struct MindMapExtractor<'a> {
    chat: &'a dyn HarnessChat,
    prompt: String,
    input_text_key: String,
}

impl<'a> MindMapExtractor<'a> {
    /// `MindMapExtractor(llm_invoker, prompt=None, input_text_key=None, ...)`.
    pub fn new(chat: &'a dyn HarnessChat) -> Self {
        Self {
            chat,
            prompt: MIND_MAP_EXTRACTION_PROMPT.to_string(),
            input_text_key: "input_text".to_string(),
        }
    }

    /// Custom prompt / input key (upstream keyword arguments).
    pub fn with_prompt(mut self, prompt: &str, input_text_key: &str) -> Self {
        self.prompt = prompt.to_string();
        self.input_text_key = input_text_key.to_string();
        self
    }

    /// `__call__`: group sections by the context budget, ask the model for a
    /// markdown mind map per group, merge the parsed trees.
    pub async fn call(
        &self,
        sections: &[String],
        prompt_variables: &Map<String, Value>,
    ) -> MindMapResult {
        let max_length = self.chat.max_length() as f64;
        let token_count = (max_length * 0.8).max(max_length - 512.0);
        let mut texts: Vec<String> = Vec::new();
        let mut cnt = 0.0f64;
        let mut res: Vec<Value> = Vec::new();
        for section in sections {
            let section_cnt = num_tokens(section);
            if cnt + section_cnt >= token_count && !texts.is_empty() {
                res.push(
                    self.process_document(&texts.join(""), prompt_variables)
                        .await,
                );
                texts.clear();
                cnt = 0.0;
            }
            texts.push(section.clone());
            cnt += section_cnt;
        }
        if !texts.is_empty() {
            res.push(
                self.process_document(&texts.join(""), prompt_variables)
                    .await,
            );
        }
        if res.is_empty() {
            return MindMapResult {
                output: serde_json::json!({"id": "root", "children": []}),
            };
        }
        let mut merged = res.remove(0);
        for item in res {
            merge_json(&item, &mut merged);
        }
        let output = if merged.as_object().map(|o| o.len()).unwrap_or(0) > 1 {
            let mut keyset: Vec<String> = Vec::new();
            for (key, value) in merged.as_object().cloned().unwrap_or_default() {
                if value.is_object() {
                    let stripped = strip_asterisks(&key);
                    if !stripped.is_empty() && !keyset.contains(&stripped) {
                        keyset.push(stripped);
                    }
                }
            }
            let mut children: Vec<Value> = Vec::new();
            for (key, value) in merged.as_object().cloned().unwrap_or_default() {
                let stripped = strip_asterisks(&key);
                if value.is_object() && !stripped.is_empty() {
                    children.push(serde_json::json!({
                        "id": stripped,
                        "children": be_children(&value, &mut keyset.clone()),
                    }));
                }
            }
            serde_json::json!({"id": "root", "children": children})
        } else {
            let (first_key, first_value) = merged
                .as_object()
                .and_then(|o| o.iter().next().map(|(k, v)| (k.clone(), v.clone())))
                .unwrap_or_else(|| ("root".to_string(), serde_json::json!({})));
            let key = strip_asterisks(&first_key);
            let mut keyset = vec![key.clone()];
            serde_json::json!({
                "id": key,
                "children": be_children(&first_value, &mut keyset),
            })
        };
        MindMapResult { output }
    }

    /// `_process_document`: one markdown extraction call.
    async fn process_document(&self, text: &str, prompt_variables: &Map<String, Value>) -> Value {
        let mut replaced = self.prompt.clone();
        for (key, value) in prompt_variables {
            let needle = format!("{{{key}}}");
            let text_value = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            replaced = replaced.replace(&needle, &text_value);
        }
        replaced = replaced.replace(&format!("{{{}}}", self.input_text_key), text);
        let history = vec![serde_json::json!({"role": "user", "content": "Output:"})];
        let response = self
            .chat
            .chat(&replaced, &history, &serde_json::json!({}))
            .await
            .unwrap_or_default();
        let fence = Regex::new(r"```[^\n]*").expect("fence regex");
        let cleaned = fence.replace_all(&response, "").to_string();
        let mut parsed = dictify(&cleaned);
        to_dict(&mut parsed);
        parsed
    }
}

fn num_tokens(text: &str) -> f64 {
    crate::chunk::tokenizer::token_count(text) as f64
}

/// `_key`: strip asterisks (emphasis markers) from a key.
fn strip_asterisks(key: &str) -> String {
    let re = Regex::new(r"\*+").expect("asterisk regex");
    re.replace_all(key, "").to_string()
}

/// `_be_children`: `{id, children}` nodes from a dict/list/string subtree.
pub fn be_children(obj: &Value, keyset: &mut Vec<String>) -> Vec<Value> {
    match obj {
        Value::String(text) => {
            if !keyset.iter().any(|existing| existing == text) {
                keyset.push(text.clone());
            }
            let stripped = strip_asterisks(text);
            if stripped.is_empty() {
                Vec::new()
            } else {
                vec![serde_json::json!({"id": stripped, "children": []})]
            }
        }
        Value::Array(items) => {
            let mut out: Vec<Value> = Vec::new();
            for item in items {
                let id = match item {
                    Value::String(text) => strip_asterisks(text),
                    other => strip_asterisks(&other.to_string()),
                };
                if id.is_empty() {
                    continue;
                }
                if !keyset.iter().any(|existing| existing == &id) {
                    keyset.push(id.clone());
                }
                out.push(serde_json::json!({"id": id, "children": []}));
            }
            out
        }
        Value::Object(map) => {
            let mut out: Vec<Value> = Vec::new();
            for (key, value) in map {
                // The `_` container holds this node's list items: splice them
                // in as direct children instead of emitting an `_` node.
                if key == "_" {
                    out.extend(be_children(value, keyset));
                    continue;
                }
                let key = strip_asterisks(key);
                if key.is_empty() || keyset.iter().any(|existing| existing == &key) {
                    continue;
                }
                keyset.push(key.clone());
                out.push(serde_json::json!({
                    "id": key,
                    "children": be_children(value, keyset),
                }));
            }
            out
        }
        _ => Vec::new(),
    }
}

/// `_merge(d1, d2)`: fold `a` into `b` (dict recursion, list extend, else a wins).
pub fn merge_json(a: &Value, b: &mut Value) {
    let Some(d1) = a.as_object() else {
        return;
    };
    if !b.is_object() {
        *b = serde_json::json!({});
    }
    let Some(d2) = b.as_object_mut() else {
        return;
    };
    for (key, value) in d1 {
        match d2.get_mut(key) {
            Some(existing) => {
                if existing.is_object() && value.is_object() {
                    merge_json(value, existing);
                } else if let (Value::Array(target), Value::Array(source)) = (&mut *existing, value)
                {
                    target.extend(source.iter().cloned());
                } else {
                    *existing = value.clone();
                }
            }
            None => {
                d2.insert(key.clone(), value.clone());
            }
        }
    }
}

/// `_todict` recursion: convert ordered-dict-like structures; in Rust the
/// value is already a plain object, so only `_list_to_kv` remains.
fn to_dict(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for child in map.values_mut() {
                to_dict(child);
            }
            list_to_kv(value);
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                to_dict(item);
            }
        }
        _ => {}
    }
}

/// `_list_to_kv`: `[[k, v], ...]` pairs become `{k: v}`; other lists stay.
fn list_to_kv(value: &mut Value) {
    let Value::Object(map) = value else {
        return;
    };
    let keys: Vec<String> = map.keys().cloned().collect();
    for key in keys {
        let Some(child) = map.get_mut(&key) else {
            continue;
        };
        match child {
            Value::Object(_) => list_to_kv(child),
            Value::Array(items) => {
                let mut converted = Map::new();
                let mut saw_pair = false;
                for item in items.iter() {
                    if let Some(pair) = item.as_array()
                        && pair.len() >= 2
                    {
                        let pair_key = match &pair[0] {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        };
                        converted.insert(pair_key, pair[1].clone());
                        saw_pair = true;
                    }
                }
                if saw_pair {
                    *child = Value::Object(converted);
                } else {
                    for item in items.iter_mut() {
                        list_to_kv(item);
                    }
                }
            }
            _ => {}
        }
    }
}

/// `markdown_to_json.dictify` (documented subset): ATX headings create nested
/// keys; unordered (`-`, `*`, `+`) and ordered (`1.`) list items become list
/// entries, indentation (2 spaces per level) nests them; other non-empty lines
/// append paragraph text to the current container.
pub fn dictify(markdown: &str) -> Value {
    let mut root = Value::Object(Map::new());
    let mut heading_keys: Vec<String> = Vec::new();
    for line in markdown.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            continue;
        }
        let indent = trimmed.len() - trimmed.trim_start().len();
        let content = trimmed.trim_start();
        if let Some(rest) = content.strip_prefix('#') {
            let level = 1 + rest.chars().take_while(|c| *c == '#').count();
            let title = rest.trim_start_matches('#').trim().to_string();
            if title.is_empty() {
                continue;
            }
            heading_keys.truncate(level - 1);
            heading_keys.push(title);
            ensure_path(&mut root, &heading_keys);
            continue;
        }
        let (is_list, text) = if let Some(stripped) = content
            .strip_prefix("- ")
            .or_else(|| content.strip_prefix("* "))
            .or_else(|| content.strip_prefix("+ "))
        {
            (true, stripped.trim().to_string())
        } else {
            let ordered = Regex::new(r"^\d+\.\s+").expect("ol regex");
            match ordered.find(content) {
                Some(m) => (true, content[m.end()..].trim().to_string()),
                None => (false, content.to_string()),
            }
        };
        if text.is_empty() {
            continue;
        }
        let value = current_container(&mut root, &heading_keys);
        if is_list {
            let level = indent / 2;
            push_list_item(value, level, text);
        } else if let Some(items) = list_target(value) {
            items.push(Value::String(text));
        }
    }
    root
}

fn ensure_path(root: &mut Value, keys: &[String]) {
    let mut cursor = root;
    for key in keys {
        if !cursor.is_object() {
            *cursor = Value::Object(Map::new());
        }
        let map = cursor.as_object_mut().expect("object");
        cursor = map
            .entry(key.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
}

fn current_container<'a>(root: &'a mut Value, keys: &[String]) -> &'a mut Value {
    let mut cursor = root;
    for key in keys {
        let map = cursor.as_object_mut().expect("object");
        cursor = map
            .entry(key.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    cursor
}

fn list_target(value: &mut Value) -> Option<&mut Vec<Value>> {
    if value.is_object() {
        let map = value.as_object_mut()?;
        map.entry("_".to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
    } else {
        None
    }
}

fn push_list_item(value: &mut Value, level: usize, text: String) {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    let map = value.as_object_mut().expect("object");
    let list = map
        .entry("_".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(items) = list.as_array_mut() else {
        return;
    };
    if level == 0 || items.is_empty() {
        items.push(Value::String(text));
        return;
    }
    let last_index = items.len() - 1;
    if !items[last_index].is_object() {
        items[last_index] = Value::Object(Map::new());
    }
    push_list_item(&mut items[last_index], level - 1, text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct MockChat {
        reply: String,
    }

    #[async_trait::async_trait]
    impl HarnessChat for MockChat {
        async fn chat(
            &self,
            _system: &str,
            _history: &[Value],
            _gen_conf: &Value,
        ) -> Result<String, String> {
            Ok(self.reply.clone())
        }
        fn max_length(&self) -> usize {
            4096
        }
    }

    #[test]
    fn dictify_builds_nested_object_from_markdown() {
        let value = dictify("# Root\n## A\n- x\n- y\n## B\n- z\n");
        assert_eq!(value["Root"]["A"]["_"], json!(["x", "y"]));
        assert_eq!(value["Root"]["B"]["_"], json!(["z"]));
    }

    #[test]
    fn merge_recurses_lists_and_dicts() {
        let mut target = json!({"A": {"_": ["x"]}, "B": {"_": ["p"]}});
        let source = json!({"A": {"_": ["y"]}, "B": {"_": ["q"]}, "C": {"_": ["r"]}});
        merge_json(&source, &mut target);
        assert_eq!(target["A"]["_"], json!(["x", "y"]));
        assert_eq!(target["B"]["_"], json!(["p", "q"]));
        assert_eq!(target["C"]["_"], json!(["r"]));
    }

    #[test]
    fn be_children_strips_asterisks_and_dedupes() {
        let subtree = json!({"*A*": {"B": ["x"]}, "A": {"C": ["y"]}});
        let mut keyset: Vec<String> = Vec::new();
        let children = be_children(&subtree, &mut keyset);
        assert_eq!(children.len(), 1, "the second A is a duplicate key");
        assert_eq!(children[0]["id"], json!("A"));
        let list = be_children(&json!(["a", "", "b"]), &mut keyset);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["id"], json!("a"));
    }

    #[test]
    fn list_to_kv_converts_pair_lists() {
        let mut value = json!({"k": [["a", 1], ["b", 2]]});
        to_dict(&mut value);
        assert_eq!(value["k"]["a"], json!(1));
        assert_eq!(value["k"]["b"], json!(2));
    }

    #[tokio::test]
    async fn extractor_single_root_and_multi_root_shapes() {
        let chat = MockChat {
            reply: "# Topic\n- alpha\n- beta\n".to_string(),
        };
        let extractor = MindMapExtractor::new(&chat);
        let result = extractor.call(&["one".to_string()], &Map::new()).await;
        assert_eq!(result.output["id"], json!("Topic"));
        assert_eq!(result.output["children"][0]["id"], json!("alpha"));

        let multi = MockChat {
            reply: "# A\n- a1\n# B\n- b1\n".to_string(),
        };
        let extractor = MindMapExtractor::new(&multi);
        let result = extractor.call(&["one".to_string()], &Map::new()).await;
        assert_eq!(result.output["id"], json!("root"));
        let ids: Vec<String> = result.output["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|child| child["id"].as_str().unwrap().to_string())
            .collect();
        assert!(ids.contains(&"A".to_string()));
        assert!(ids.contains(&"B".to_string()));
    }

    #[tokio::test]
    async fn extractor_empty_sections_yield_empty_root() {
        let chat = MockChat {
            reply: String::new(),
        };
        let extractor = MindMapExtractor::new(&chat);
        let result = extractor.call(&[], &Map::new()).await;
        assert_eq!(result.output, json!({"id": "root", "children": []}));
    }
}
