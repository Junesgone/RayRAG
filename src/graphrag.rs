//! GraphRAG community reports — mirrors RAGFlow
//! `rag/graphrag/general/community_reports_extractor.py` +
//! `community_report_prompt.py` + the helper functions in `rag/graphrag/utils.py`
//! (`perform_variable_replacements`, `clean_str`, `dict_has_keys_with_types`).
//!
//! Pipeline (mirrors upstream `CommunityReportsExtractor.__call__`):
//!   1. rank every node by degree (graph.degree → int rank)
//!   2. partition the graph into communities (upstream uses graspologic
//!      hierarchical_leiden — we substitute a deterministic connected-
//!      component partition, see [`partition_communities`])
//!   3. per community with ≥ 2 entities: render entity/relation CSV into the
//!      COMMUNITY_REPORT_PROMPT, call the LLM, strip stray braces, parse JSON,
//!      validate keys/types, emit both text and structured output, and tag
//!      nodes with their community titles.

use crate::graphrag_enhanced::EntityGraph;
use crate::llm::LlmClient;
use crate::Result;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

/// COMMUNITY_REPORT_PROMPT — verbatim from
/// rag/graphrag/general/community_report_prompt.py (Microsoft GraphRAG,
/// MIT). `{entity_df}` / `{relation_df}` are filled at call time.
pub const COMMUNITY_REPORT_PROMPT: &str = r#"You are an AI assistant that helps a human analyst to perform general information discovery. Information discovery is the process of identifying and assessing relevant information associated with certain entities (e.g., organizations and individuals) within a network.

# Goal
Write a comprehensive report of a community, given a list of entities that belong to the community as well as their relationships and optional associated claims. The report will be used to inform decision-makers about information associated with the community and their potential impact. The content of this report includes an overview of the community's key entities, their legal compliance, technical capabilities, reputation, and noteworthy claims.

# Report Structure

The report should include the following sections:

- TITLE: community's name that represents its key entities - title should be short but specific. When possible, include representative named entities in the title.
- SUMMARY: An executive summary of the community's overall structure, how its entities are related to each other, and significant information associated with its entities.
- IMPACT SEVERITY RATING: a float score between 0-10 that represents the severity of IMPACT posed by entities within the community.  IMPACT is the scored importance of a community.
- RATING EXPLANATION: Give a single sentence explanation of the IMPACT severity rating.
- DETAILED FINDINGS: A list of 5-10 key insights about the community. Each insight should have a short summary followed by multiple paragraphs of explanatory text grounded according to the grounding rules below. Be comprehensive.

Return output as a well-formed JSON-formatted string with the following format(in language of 'Text' content):
    {{
        "title": <report_title>,
        "summary": <executive_summary>,
        "rating": <impact_severity_rating>,
        "rating_explanation": <rating_explanation>,
        "findings": [
            {{
                "summary":<insight_1_summary>,
                "explanation": <insight_1_explanation>
            }},
            {{
                "summary":<insight_2_summary>,
                "explanation": <insight_2_explanation>
            }}
        ]
    }}

# Grounding Rules

Points supported by data should list their data references as follows:

"This is an example sentence supported by multiple data references [Data: <dataset name> (record ids); <dataset name> (record ids)]."

Do not list more than 5 record ids in a single reference. Instead, list the top 5 most relevant record ids and add "+more" to indicate that there are more.

For example:
"Person X is the owner of Company Y and subject to many allegations of wrongdoing [Data: Reports (1), Entities (5, 7); Relationships (23); Claims (7, 2, 34, 64, 46, +more)]."

where 1, 5, 7, 23, 2, 34, 46, and 64 represent the id (not the index) of the relevant data record.

Do not include information where the supporting evidence for it is not provided.


# Example Input
-----------
Text:

-Entities-

id,entity,description
5,VERDANT OASIS PLAZA,Verdant Oasis Plaza is the location of the Unity March
6,HARMONY ASSEMBLY,Harmony Assembly is an organization that is holding a march at Verdant Oasis Plaza

-Relationships-

id,source,target,description
37,VERDANT OASIS PLAZA,UNITY MARCH,Verdant Oasis Plaza is the location of the Unity March
38,VERDANT OASIS PLAZA,HARMONY ASSEMBLY,Harmony Assembly is holding a march at Verdant Oasis Plaza
39,VERDANT OASIS PLAZA,UNITY MARCH,The Unity March is taking place at Verdant Oasis Plaza
40,VERDANT OASIS PLAZA,TRIBUNE SPOTLIGHT,Tribune Spotlight is reporting on the Unity march taking place at Verdant Oasis Plaza
41,VERDANT OASIS PLAZA,BAILEY ASADI,Bailey Asadi is speaking at Verdant Oasis Plaza about the march
43,HARMONY ASSEMBLY,UNITY MARCH,Harmony Assembly is organizing the Unity March

Output:
{{
    "title": "Verdant Oasis Plaza and Unity March",
    "summary": "The community revolves around the Verdant Oasis Plaza, which is the location of the Unity March. The plaza has relationships with the Harmony Assembly, Unity March, and Tribune Spotlight, all of which are associated with the march event.",
    "rating": 5.0,
    "rating_explanation": "The impact severity rating is moderate due to the potential for unrest or conflict during the Unity March.",
    "findings": [
        {{
            "summary": "Verdant Oasis Plaza as the central location",
            "explanation": "Verdant Oasis Plaza is the central entity in this community, serving as the location for the Unity March. This plaza is the common link between all other entities, suggesting its significance in the community. The plaza's association with the march could potentially lead to issues such as public disorder or conflict, depending on the nature of the march and the reactions it provokes. [Data: Entities (5), Relationships (37, 38, 39, 40, 41,+more)]"
        }},
        {{
            "summary": "Harmony Assembly's role in the community",
            "explanation": "Harmony Assembly is another key entity in this community, being the organizer of the march at Verdant Oasis Plaza. The nature of Harmony Assembly and its march could be a potential source of threat, depending on their objectives and the reactions they provoke. The relationship between Harmony Assembly and the plaza is crucial in understanding the dynamics of this community. [Data: Entities(6), Relationships (38, 43)]"
        }},
        {{
            "summary": "Unity March as a significant event",
            "explanation": "The Unity March is a significant event taking place at Verdant Oasis Plaza. This event is a key factor in the community's dynamics and could be a potential source of threat, depending on the nature of the march and the reactions it provokes. The relationship between the march and the plaza is crucial in understanding the dynamics of this community. [Data: Relationships (39)]"
        }},
        {{
            "summary": "Role of Tribune Spotlight",
            "explanation": "Tribune Spotlight is reporting on the Unity March taking place in Verdant Oasis Plaza. This suggests that the event has attracted media attention, which could amplify its impact on the community. The role of Tribune Spotlight could be significant in shaping public perception of the event and the entities involved. [Data: Relationships (40)]"
        }}
    ]
}}


# Real Data

Use the following text for your answer. Do not make anything up in your answer.

Text:

-Entities-
{entity_df}

-Relationships-
{relation_df}

The report should include the following sections:

- TITLE: community's name that represents its key entities - title should be short but specific. When possible, include representative named entities in the title.
- SUMMARY: An executive summary of the community's overall structure, how its entities are related to each other, and significant information associated with its entities.
- IMPACT SEVERITY RATING: a float score between 0-10 that represents the severity of IMPACT posed by entities within the community.  IMPACT is the scored importance of a community.
- RATING EXPLANATION: Give a single sentence explanation of the IMPACT severity rating.
- DETAILED FINDINGS: A list of 5-10 key insights about the community. Each insight should have a short summary followed by multiple paragraphs of explanatory text grounded according to the grounding rules below. Be comprehensive.

Return output as a well-formed JSON-formatted string with the following format(in language of 'Text' content):
    {{
        "title": <report_title>,
        "summary": <executive_summary>,
        "rating": <impact_severity_rating>,
        "rating_explanation": <rating_explanation>,
        "findings": [
            {{
                "summary":<insight_1_summary>,
                "explanation": <insight_1_explanation>
            }},
            {{
                "summary":<insight_2_summary>,
                "explanation": <insight_2_explanation>
            }}
        ]
    }}

# Grounding Rules

Points supported by data should list their data references as follows:

"This is an example sentence supported by multiple data references [Data: <dataset name> (record ids); <dataset name> (record ids)]."

Do not list more than 5 record ids in a single reference. Instead, list the top 5 most relevant record ids and add "+more" to indicate that there are more.

For example:
"Person X is the owner of Company Y and subject to many allegations of wrongdoing [Data: Reports (1), Entities (5, 7); Relationships (23); Claims (7, 2, 34, 64, 46, +more)]."

where 1, 5, 7, 23, 2, 34, 46, and 64 represent the id (not the index) of the relevant data record.

Do not include information where the supporting evidence for it is not provided."#;

/// perform_variable_replacements — mirrors rag/graphrag/utils.py: replace
/// every `{key}` in the input with its string value.
pub fn perform_variable_replacements(input: &str, variables: &HashMap<String, String>) -> String {
    let mut result = input.to_string();
    for (k, v) in variables {
        result = result.replace(&format!("{{{k}}}"), v);
    }
    result
}

/// clean_str — mirrors rag/graphrag/utils.py: HTML-unescape, strip, drop
/// control characters (0x00-0x1f, 0x7f-0x9f) and stray double quotes.
pub fn clean_str(input: &str) -> String {
    let unescaped = html_unescape(input.trim());
    unescaped
        .chars()
        .filter(|c| {
            let cp = *c as u32;
            !(cp <= 0x1f || (0x7f..=0x9f).contains(&cp)) && *c != '"'
        })
        .collect()
}

/// Minimal HTML entity unescape for the entities the prompt path produces
/// (&amp; &lt; &gt; &quot; &#39; &nbsp;) — mirrors Python html.unescape.
fn html_unescape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        let semicolon = rest.find(';');
        let (entity, tail) = match semicolon {
            Some(end) => (&rest[..=end], &rest[end + 1..]),
            None => {
                out.push_str(rest);
                return out;
            }
        };
        let replacement = match entity {
            "&amp;" => "&",
            "&lt;" => "<",
            "&gt;" => ">",
            "&quot;" => "\"",
            "&#39;" => "'",
            "&nbsp;" => " ",
            _ => {
                out.push_str(entity);
                rest = tail;
                continue;
            }
        };
        out.push_str(replacement);
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// dict_has_keys_with_types — mirrors rag/graphrag/utils.py: every expected
/// field must be present with a matching JSON type.
pub fn dict_has_keys_with_types(data: &Value, expected: &[(&str, JsonType)]) -> bool {
    for (field, ty) in expected {
        let Some(value) = data.get(*field) else {
            return false;
        };
        if !json_type_matches(value, *ty) {
            return false;
        }
    }
    true
}

/// JSON type used by the key/type validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonType {
    Str,
    Float,
    List,
}

fn json_type_matches(value: &Value, ty: JsonType) -> bool {
    match ty {
        JsonType::Str => value.is_string(),
        JsonType::Float => value.is_number(),
        JsonType::List => value.is_array(),
    }
}

/// A single community: member nodes plus a weight (sum of rank*weight over
/// nodes, normalized by the max community weight — mirrors leiden.run).
#[derive(Debug, Clone, Default)]
pub struct Community {
    pub weight: f64,
    pub nodes: Vec<String>,
}

/// Partition the graph into communities. Upstream uses graspologic
/// `hierarchical_leiden`; here we use a deterministic connected-component
/// partition (union-find), which preserves the extractor contract — groups
/// of densely connected entities — without the C++ dependency.
pub fn partition_communities(graph: &EntityGraph) -> BTreeMap<usize, HashMap<String, Community>> {
    // Single level (0), like hierarchical_leiden's root level.
    let names = graph.node_names();
    let mut parent: HashMap<String, String> = names.iter().cloned().map(|n| (n.clone(), n)).collect();
    fn find(parent: &mut HashMap<String, String>, x: &str) -> String {
        let p = parent.get(x).cloned().unwrap_or_else(|| x.to_string());
        if p != x {
            let root = find(parent, &p);
            parent.insert(x.to_string(), root.clone());
            root
        } else {
            p
        }
    }
    for name in &names {
        // connections are undirected (add_relation pushes both ways)
        let neighbors: Vec<String> = graph
            .neighbors(name)
            .into_iter()
            .collect();
        for n in neighbors {
            let (ra, rb) = (find(&mut parent, name), find(&mut parent, &n));
            if ra != rb {
                let copy = ra.clone();
                parent.insert(copy, rb);
            }
        }
    }
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for name in &names {
        let root = find(&mut parent, name);
        groups.entry(root).or_default().push(name.clone());
    }
    let mut communities = HashMap::new();
    for members in groups.into_values() {
        let mut comm = Community::default();
        for m in &members {
            comm.nodes.push(m.clone());
            let rank = graph.degree(m) as f64;
            comm.weight += rank; // node.weight defaults to 1
        }
        communities.insert(comm_key(&members), comm);
    }
    // Normalize weights by the max (mirrors leiden.run)
    let max_weight = communities.values().map(|c| c.weight).fold(0.0_f64, f64::max);
    if max_weight > 0.0 {
        for c in communities.values_mut() {
            c.weight /= max_weight;
        }
    }
    let mut out = BTreeMap::new();
    out.insert(0, communities);
    out
}

fn comm_key(members: &[String]) -> String {
    // stable key: sorted joined
    let mut sorted = members.to_vec();
    sorted.sort();
    sorted.join("|")
}

/// Community reports result — mirrors CommunityReportsResult.
#[derive(Debug, Default)]
pub struct CommunityReportsResult {
    pub output: Vec<String>,
    pub structured_output: Vec<Value>,
}

/// Community reports extractor — mirrors CommunityReportsExtractor.
pub struct CommunityReportsExtractor {
    extraction_prompt: String,
    max_report_length: usize,
}

impl Default for CommunityReportsExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl CommunityReportsExtractor {
    pub fn new() -> Self {
        Self {
            extraction_prompt: COMMUNITY_REPORT_PROMPT.to_string(),
            max_report_length: 1500,
        }
    }

    pub fn with_max_report_length(mut self, max: usize) -> Self {
        self.max_report_length = max;
        self
    }

    /// Run community report extraction over a graph using the LLM.
    /// Mirrors `__call__`: degree ranks → partition → per-community
    /// prompt + LLM → JSON validation → text/structured output.
    pub async fn run(
        &self,
        graph: &EntityGraph,
        llm: &LlmClient,
    ) -> Result<CommunityReportsResult> {
        let communities = partition_communities(graph);
        let mut res_str = Vec::new();
        let mut res_dict = Vec::new();
        for (_level, level_comms) in &communities {
            for comm in level_comms.values() {
                if comm.nodes.len() < 2 {
                    continue; // mirrors `if len(ents) < 2: return`
                }
                // entity CSV
                let mut ent_csv = String::from("id,entity,description\n");
                for (i, ent) in comm.nodes.iter().enumerate() {
                    let desc = graph
                        .description_of(ent)
                        .unwrap_or_default()
                        .replace(',', " ");
                    ent_csv.push_str(&format!("{i},{ent},{desc}\n"));
                }
                // relation CSV (pairwise within community, capped at 10000)
                let mut rela_csv = String::from("id,source,target,description\n");
                let mut k = 0usize;
                'outer: for i in 0..comm.nodes.len() {
                    for j in (i + 1)..comm.nodes.len() {
                        if k >= 10000 {
                            break 'outer;
                        }
                        let (a, b) = (&comm.nodes[i], &comm.nodes[j]);
                        let Some(desc) = graph.edge_description(a, b) else {
                            continue;
                        };
                        let desc = desc.replace(',', " ");
                        rela_csv.push_str(&format!("{k},{a},{b},{desc}\n"));
                        k += 1;
                    }
                }
                let mut variables = HashMap::new();
                variables.insert("entity_df".to_string(), ent_csv);
                variables.insert("relation_df".to_string(), rela_csv);
                let text = perform_variable_replacements(&self.extraction_prompt, &variables);
                // Upstream passes the rendered prompt, then a user "Output:".
                let system_msg = crate::llm::ChatMessage::new("system", text);
                let response = match llm
                    .chat(&[system_msg, crate::llm::ChatMessage::new("user", "Output:")])
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!("community report chat failed: {e}");
                        continue;
                    }
                };
                let cleaned = clean_str(&response);
                let parsed = extract_report_json(&cleaned);
                let Some(parsed) = parsed else {
                    continue;
                };
                if !dict_has_keys_with_types(
                    &parsed,
                    &[
                        ("title", JsonType::Str),
                        ("summary", JsonType::Str),
                        ("findings", JsonType::List),
                        ("rating", JsonType::Float),
                        ("rating_explanation", JsonType::Str),
                    ],
                ) {
                    continue;
                }
                let mut parsed = parsed;
                parsed["weight"] = json!(comm.weight);
                parsed["entities"] = json!(comm.nodes);
                res_str.push(report_text_output(&parsed, self.max_report_length));
                res_dict.push(parsed);
            }
        }
        Ok(CommunityReportsResult {
            output: res_str,
            structured_output: res_dict,
        })
    }
}

/// Extract the JSON object from an LLM reply: strip everything before the
/// first `{` and after the last `}`, then un-double the braces (upstream
/// re.subs `{{`→`{` / `}}`→`}` after slicing).
fn extract_report_json(response: &str) -> Option<Value> {
    let start = response.find('{')?;
    let end = response.rfind('}')?;
    if end <= start {
        return None;
    }
    let mut body = response[start..=end].to_string();
    body = body.replace("{{", "{").replace("}}", "}");
    serde_json::from_str(&body).ok()
}

/// _get_text_output — mirrors community_reports_extractor.py: Markdown
/// `# title\n\nsummary\n\n## finding summary\n\nfinding explanation…`.
fn report_text_output(parsed: &Value, max_length: usize) -> String {
    let title = parsed.get("title").and_then(Value::as_str).unwrap_or("Report");
    let summary = parsed.get("summary").and_then(Value::as_str).unwrap_or("");
    let mut out = format!("# {title}\n\n{summary}");
    if let Some(findings) = parsed.get("findings").and_then(Value::as_array) {
        for finding in findings {
            let fs = match finding {
                Value::String(s) => s.clone(),
                _ => finding.get("summary").and_then(Value::as_str).unwrap_or("").to_string(),
            };
            let fe = match finding {
                Value::String(_) => String::new(),
                _ => finding.get("explanation").and_then(Value::as_str).unwrap_or("").to_string(),
            };
            if fs.is_empty() && fe.is_empty() {
                continue;
            }
            out.push_str(&format!("\n\n## {fs}\n\n{fe}"));
        }
    }
    let mut chars: Vec<char> = out.chars().collect();
    if chars.len() > max_length {
        chars.truncate(max_length);
    }
    chars.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphrag_enhanced::{EntityGraph, NerEntity, EntityType};

    fn graph_with_entities() -> EntityGraph {
        let mut g = EntityGraph::new();
        g.add_entities(&[
            NerEntity { name: "中山市百鲤居".into(), entity_type: EntityType::Organization, start: 0, end: 0, confidence: 1.0 },
            NerEntity { name: "李锦澎".into(), entity_type: EntityType::Person, start: 0, end: 0, confidence: 1.0 },
            NerEntity { name: "水产养殖基地".into(), entity_type: EntityType::Location, start: 0, end: 0, confidence: 1.0 },
            NerEntity { name: "孤岛节点".into(), entity_type: EntityType::Unknown, start: 0, end: 0, confidence: 1.0 },
        ]);
        g.add_relation("中山市百鲤居", "李锦澎", 1.0);
        g.add_relation("中山市百鲤居", "水产养殖基地", 1.0);
        g
    }

    #[test]
    fn partition_finds_connected_components() {
        let g = graph_with_entities();
        let communities = partition_communities(&g);
        let level0 = &communities[&0];
        // 3 connected nodes form one community; the isolated node another
        assert_eq!(level0.len(), 2, "expected 2 components");
        let big = level0
            .values()
            .find(|c| c.nodes.len() == 3)
            .expect("3-node community");
        assert!(big.nodes.contains(&"中山市百鲤居".to_string()));
        let single = level0
            .values()
            .find(|c| c.nodes.len() == 1)
            .expect("single-node community");
        assert_eq!(single.nodes[0], "孤岛节点");
    }

    #[test]
    fn perform_variable_replacements_substitutes_braces() {
        let mut vars = HashMap::new();
        vars.insert("entity_df".to_string(), "id,entity\n0,A\n".to_string());
        let out = perform_variable_replacements("Text:\n-Entities-\n{entity_df}", &vars);
        assert!(out.contains("id,entity\n0,A"));
        assert!(!out.contains("{entity_df}"));
    }

    #[test]
    fn clean_str_removes_control_chars_and_unescapes() {
        let cleaned = clean_str("  A&amp;B\u{0} C\u{7f}D  ");
        assert_eq!(cleaned, "A&B CD");
        assert_eq!(clean_str("&lt;x&gt;"), "<x>");
    }

    #[test]
    fn dict_keys_type_validation() {
        let ok = json!({"title": "t", "summary": "s", "rating": 5.0, "findings": [], "rating_explanation": "e"});
        assert!(dict_has_keys_with_types(
            &ok,
            &[
                ("title", JsonType::Str),
                ("summary", JsonType::Str),
                ("findings", JsonType::List),
                ("rating", JsonType::Float),
                ("rating_explanation", JsonType::Str),
            ]
        ));
        let bad = json!({"title": "t", "summary": 42, "rating": 5.0, "findings": [], "rating_explanation": "e"});
        assert!(!dict_has_keys_with_types(
            &bad,
            &[
                ("title", JsonType::Str),
                ("summary", JsonType::Str),
                ("findings", JsonType::List),
                ("rating", JsonType::Float),
                ("rating_explanation", JsonType::Str),
            ]
        ));
        let missing = json!({"title": "t"});
        assert!(!dict_has_keys_with_types(
            &missing,
            &[("title", JsonType::Str), ("rating", JsonType::Float)]
        ));
    }

    #[test]
    fn extract_report_json_handles_fences_and_prose() {
        let raw = "Sure!\n```json\n{{\"title\": \"A\", \"summary\": \"B\", \"rating\": 5.0, \"findings\": [], \"rating_explanation\": \"E\"}}\n```\nDone.";
        let parsed = extract_report_json(raw).expect("should parse");
        assert_eq!(parsed["title"], "A");
        assert_eq!(parsed["rating"], 5.0);
    }

    #[test]
    fn report_text_output_formats_markdown() {
        let parsed = json!({
            "title": "社区报告",
            "summary": "概览",
            "findings": [
                {"summary": "洞察一", "explanation": "解释一"},
                "字符串洞察"
            ]
        });
        let out = report_text_output(&parsed, 1500);
        assert!(out.starts_with("# 社区报告"));
        assert!(out.contains("## 洞察一"));
        assert!(out.contains("解释一"));
        assert!(out.contains("## 字符串洞察"));
    }

    #[test]
    fn report_text_output_truncates_to_max_length() {
        let parsed = json!({
            "title": "长标题",
            "summary": "x".repeat(300),
            "findings": []
        });
        let out = report_text_output(&parsed, 100);
        assert!(out.chars().count() <= 100);
    }

    #[test]
    fn html_unescape_known_entities() {
        assert_eq!(html_unescape("a&amp;b&lt;c&gt;&quot;d&quot;&#39;e&#39;"), "a&b<c>\"d\"'e'");
        // unknown entity preserved verbatim
        assert_eq!(html_unescape("a&unknown;b"), "a&unknown;b");
    }
}
