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

use crate::Result;
use crate::graphrag_enhanced::{EntityGraph, EntityType};
use crate::llm::LlmClient;
use serde_json::{Value, json};
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
    let mut parent: HashMap<String, String> =
        names.iter().cloned().map(|n| (n.clone(), n)).collect();
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
        let neighbors: Vec<String> = graph.neighbors(name).into_iter().collect();
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
    let max_weight = communities
        .values()
        .map(|c| c.weight)
        .fold(0.0_f64, f64::max);
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
        for level_comms in communities.values() {
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

/// `_get_text_output` — mirrors community_reports_extractor.py: Markdown
/// `# title\n\nsummary\n\n## finding summary\n\nfinding explanation…`.
fn report_text_output(parsed: &Value, max_length: usize) -> String {
    let title = parsed
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Report");
    let summary = parsed.get("summary").and_then(Value::as_str).unwrap_or("");
    let mut out = format!("# {title}\n\n{summary}");
    if let Some(findings) = parsed.get("findings").and_then(Value::as_array) {
        for finding in findings {
            let fs = match finding {
                Value::String(s) => s.clone(),
                _ => finding
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            };
            let fe = match finding {
                Value::String(_) => String::new(),
                _ => finding
                    .get("explanation")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
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

// ── KG Search ────────────────────────────────────────────────────
// Mirrors rag/graphrag/search.py (343 lines) + query_analyze_prompt.py's
// minirag_query2kwd prompt. The upstream class subclasses the full-text
// Dealer and reads entities/relations/community reports from the vector
// store; RayRAG substitutes in-memory EntityGraph retrieval (keyword match
// on entity names, type filter, n-hop path aggregation) while keeping the
// fusion/ranking/output contract: P(E|Q) ≈ pagerank × sim, type hits ×2,
// relation sim boosted by n-hop paths and type endpoints, topn + max_token
// budget, and CSV-formatted Entities/Relations/Community Report sections.

/// minirag_query2kwd — verbatim from query_analyze_prompt.py (MiniRAG, MIT).
pub const MINIRAG_QUERY2KWD_PROMPT: &str = r#"---Role---

You are a helpful assistant tasked with identifying both answer-type and low-level keywords in the user's query.

---Goal---

Given the query, list both answer-type and low-level keywords.
answer_type_keywords focus on the type of the answer to the certain query, while low-level keywords focus on specific entities, details, or concrete terms.
The answer_type_keywords must be selected from Answer type pool. 
This pool is in the form of a dictionary, where the key represents the Type you should choose from and the value represents the example samples.

---Instructions---

- Output the keywords in JSON format.
- The JSON should have three keys:
  - "answer_type_keywords" for the types of the answer. In this list, the types with the highest likelihood should be placed at the forefront. No more than 3.
  - "entities_from_query" for specific entities or details. It must be extracted from the query.
######################
-Examples-
######################
Example 1:

Query: "How does international trade influence global economic stability?"
Answer type pool: {{
 'PERSONAL LIFE': ['FAMILY TIME', 'HOME MAINTENANCE'],
 'STRATEGY': ['MARKETING PLAN', 'BUSINESS EXPANSION'],
 'SERVICE FACILITATION': ['ONLINE SUPPORT', 'CUSTOMER SERVICE TRAINING'],
 'PERSON': ['JANE DOE', 'JOHN SMITH'],
 'FOOD': ['PASTA', 'SUSHI'],
 'EMOTION': ['HAPPINESS', 'ANGER'],
 'PERSONAL EXPERIENCE': ['TRAVEL ABROAD', 'STUDYING ABROAD'],
 'INTERACTION': ['TEAM MEETING', 'NETWORKING EVENT'],
 'BEVERAGE': ['COFFEE', 'TEA'],
 'PLAN': ['ANNUAL BUDGET', 'PROJECT TIMELINE'],
 'GEO': ['NEW YORK CITY', 'SOUTH AFRICA'],
 'GEAR': ['CAMPING TENT', 'CYCLING HELMET'],
 'EMOJI': ['🎉', '🚀'],
 'BEHAVIOR': ['POSITIVE FEEDBACK', 'NEGATIVE CRITICISM'],
 'TONE': ['FORMAL', 'INFORMAL'],
 'LOCATION': ['DOWNTOWN', 'SUBURBS']
}}
################
Output:
{{
  "answer_type_keywords": ["STRATEGY","PERSONAL LIFE"],
  "entities_from_query": ["Trade agreements", "Tariffs", "Currency exchange", "Imports", "Exports"]
}}
#############################
Example 2:

Query: "When was SpaceX's first rocket launch?"
Answer type pool: {{
 'DATE AND TIME': ['2023-10-10 10:00', 'THIS AFTERNOON'],
 'ORGANIZATION': ['GLOBAL INITIATIVES CORPORATION', 'LOCAL COMMUNITY CENTER'],
 'PERSONAL LIFE': ['DAILY EXERCISE ROUTINE', 'FAMILY VACATION PLANNING'],
 'STRATEGY': ['NEW PRODUCT LAUNCH', 'YEAR-END SALES BOOST'],
 'SERVICE FACILITATION': ['REMOTE IT SUPPORT', 'ON-SITE TRAINING SESSIONS'],
 'PERSON': ['ALEXANDER HAMILTON', 'MARIA CURIE'],
 'FOOD': ['GRILLED SALMON', 'VEGETARIAN BURRITO'],
 'EMOTION': ['EXCITEMENT', 'DISAPPOINTMENT'],
 'PERSONAL EXPERIENCE': ['BIRTHDAY CELEBRATION', 'FIRST MARATHON'],
 'INTERACTION': ['OFFICE WATER COOLER CHAT', 'ONLINE FORUM DEBATE'],
 'BEVERAGE': ['ICED COFFEE', 'GREEN SMOOTHIE'],
 'PLAN': ['WEEKLY MEETING SCHEDULE', 'MONTHLY BUDGET OVERVIEW'],
 'GEO': ['MOUNT EVEREST BASE CAMP', 'THE GREAT BARRIER REEF'],
 'GEAR': ['PROFESSIONAL CAMERA EQUIPMENT', 'OUTDOOR HIKING GEAR'],
 'EMOJI': ['📅', '⏰'],
 'BEHAVIOR': ['PUNCTUALITY', 'HONESTY'],
 'TONE': ['CONFIDENTIAL', 'SATIRICAL'],
 'LOCATION': ['CENTRAL PARK', 'DOWNTOWN LIBRARY']
}}

################
Output:
{{
  "answer_type_keywords": ["DATE AND TIME", "ORGANIZATION", "PLAN"],
  "entities_from_query": ["SpaceX", "Rocket launch", "Aerospace", "Power Recovery"]

}}
#############################
Example 3:

Query: "What is the role of education in reducing poverty?"
Answer type pool: {{
 'PERSONAL LIFE': ['MANAGING WORK-LIFE BALANCE', 'HOME IMPROVEMENT PROJECTS'],
 'STRATEGY': ['MARKETING STRATEGIES FOR Q4', 'EXPANDING INTO NEW MARKETS'],
 'SERVICE FACILITATION': ['CUSTOMER SATISFACTION SURVEYS', 'STAFF RETENTION PROGRAMS'],
 'PERSON': ['ALBERT EINSTEIN', 'MARIA CALLAS'],
 'FOOD': ['PAN-FRIED STEAK', 'POACHED EGGS'],
 'EMOTION': ['OVERWHELM', 'CONTENTMENT'],
 'PERSONAL EXPERIENCE': ['LIVING ABROAD', 'STARTING A NEW JOB'],
 'INTERACTION': ['SOCIAL MEDIA ENGAGEMENT', 'PUBLIC SPEAKING'],
 'BEVERAGE': ['CAPPUCCINO', 'MATCHA LATTE'],
 'PLAN': ['ANNUAL FITNESS GOALS', 'QUARTERLY BUSINESS REVIEW'],
 'GEO': ['THE AMAZON RAINFOREST', 'THE GRAND CANYON'],
 'GEAR': ['SURFING ESSENTIALS', 'CYCLING ACCESSORIES'],
 'EMOJI': ['💻', '📱'],
 'BEHAVIOR': ['TEAMWORK', 'LEADERSHIP'],
 'TONE': ['FORMAL MEETING', 'CASUAL CONVERSATION'],
 'LOCATION': ['URBAN CITY CENTER', 'RURAL COUNTRYSIDE']
}}

################
Output:
{{
  "answer_type_keywords": ["STRATEGY", "PERSON"],
  "entities_from_query": ["School access", "Literacy rates", "Job training", "Income inequality"]
}}
#############################
Example 4:

Query: "Where is the capital of the United States?"
Answer type pool: {{
 'ORGANIZATION': ['GREENPEACE', 'RED CROSS'],
 'PERSONAL LIFE': ['DAILY WORKOUT', 'HOME COOKING'],
 'STRATEGY': ['FINANCIAL INVESTMENT', 'BUSINESS EXPANSION'],
 'SERVICE FACILITATION': ['ONLINE SUPPORT', 'CUSTOMER SERVICE TRAINING'],
 'PERSON': ['ALBERTA SMITH', 'BENJAMIN JONES'],
 'FOOD': ['PASTA CARBONARA', 'SUSHI PLATTER'],
 'EMOTION': ['HAPPINESS', 'SADNESS'],
 'PERSONAL EXPERIENCE': ['TRAVEL ADVENTURE', 'BOOK CLUB'],
 'INTERACTION': ['TEAM BUILDING', 'NETWORKING MEETUP'],
 'BEVERAGE': ['LATTE', 'GREEN TEA'],
 'PLAN': ['WEIGHT LOSS', 'CAREER DEVELOPMENT'],
 'GEO': ['PARIS', 'NEW YORK'],
 'GEAR': ['CAMERA', 'HEADPHONES'],
 'EMOJI': ['🏢', '🌍'],
 'BEHAVIOR': ['POSITIVE THINKING', 'STRESS MANAGEMENT'],
 'TONE': ['FRIENDLY', 'PROFESSIONAL'],
 'LOCATION': ['DOWNTOWN', 'SUBURBS']
}}
################
Output:
{{
  "answer_type_keywords": ["LOCATION"],
  "entities_from_query": ["capital of the United States", "Washington", "New York"]
}}
#############################

-Real Data-
######################
Query: {query}
Answer type pool:{TYPE_POOL}
######################
Output:

"#;

/// keywords_extraction — verbatim from query_analyze_prompt.py (used by the
/// graph extraction flow to split a chunk into high/low-level keywords).
pub const KEYWORDS_EXTRACTION_PROMPT: &str = r#"---Role---

You are a helpful assistant tasked with identifying both high-level and low-level keywords in the user's query.

---Goal---

Given the query, list both high-level and low-level keywords. High-level keywords focus on overarching concepts or themes, while low-level keywords focus on specific entities, details, or concrete terms.

---Instructions---

- Output the keywords in JSON format.
- The JSON should have two keys:
  - "high_level_keywords" for overarching concepts or themes.
  - "low_level_keywords" for specific entities or details.

######################
-Examples-
######################
{examples}

#############################
-Real Data-
######################
Query: {query}
######################
The `Output` should be human text, not unicode characters. Keep the same language as `Query`.
Output:

"#;

/// Default examples for KEYWORDS_EXTRACTION_PROMPT.
pub const KEYWORDS_EXTRACTION_EXAMPLES: [&str; 3] = [
    r#"Example 1:

Query: "How does international trade influence global economic stability?"
################
Output:
{
  "high_level_keywords": ["International trade", "Global economic stability", "Economic impact"],
  "low_level_keywords": ["Trade agreements", "Tariffs", "Currency exchange", "Imports", "Exports"]
}
#############################"#,
    r#"Example 2:

Query: "What are the environmental consequences of deforestation on biodiversity?"
################
Output:
{
  "high_level_keywords": ["Environmental consequences", "Deforestation", "Biodiversity loss"],
  "low_level_keywords": ["Species extinction", "Habitat destruction", "Carbon emissions", "Rainforest", "Ecosystem"]
}
#############################"#,
    r#"Example 3:

Query: "What is the role of education in reducing poverty?"
################
Output:
{
  "high_level_keywords": ["Education", "Poverty reduction", "Socioeconomic development"],
  "low_level_keywords": ["School access", "Literacy rates", "Job training", "Income inequality"]
}
#############################"#,
];

/// An entity hit: similarity, pagerank proxy (degree), n-hop neighbors.
#[derive(Debug, Clone, Default)]
pub struct KgEntity {
    pub sim: f64,
    pub pagerank: f64,
    pub description: String,
    pub n_hop_ents: Vec<String>,
}

/// A relation hit between two entities.
#[derive(Debug, Clone, Default)]
pub struct KgRelation {
    pub sim: f64,
    pub pagerank: f64,
    pub description: String,
}

/// Knowledge-graph retrieval over an in-memory EntityGraph.
///
/// Mirrors `KGSearch` (rag/graphrag/search.py): query rewrite via LLM,
/// entity/relation retrieval, n-hop path aggregation, fusion scoring
/// (`sim × pagerank`, type hits boost, relation n-hop boost), topn and
/// max-token budget, and CSV-style output sections.
pub struct KgSearch;

impl KgSearch {
    /// Parse an LLM JSON reply into (answer_type_keywords, entities_from_query).
    /// Mirrors `query_rewrite`'s json_repair fallback: strip to first `{`…
    /// last `}`, then try to extract the two keys (missing keys → empty).
    pub fn parse_rewrite_json(response: &str) -> (Vec<String>, Vec<String>) {
        let Some(start) = response.find('{') else {
            return (Vec::new(), Vec::new());
        };
        let Some(end) = response.rfind('}') else {
            return (Vec::new(), Vec::new());
        };
        if end <= start {
            return (Vec::new(), Vec::new());
        }
        let body = &response[start..=end];
        let parsed = serde_json::from_str::<Value>(body).ok();
        let types = parsed
            .as_ref()
            .and_then(|v| v.get("answer_type_keywords"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let ents = parsed
            .as_ref()
            .and_then(|v| v.get("entities_from_query"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .take(5)
                    .collect()
            })
            .unwrap_or_default();
        (types, ents)
    }

    /// Render the minirag_query2kwd prompt for a question + type pool.
    pub fn render_query2kwd(query: &str, type_pool: &Value) -> String {
        perform_variable_replacements(
            MINIRAG_QUERY2KWD_PROMPT,
            &HashMap::from([
                ("query".to_string(), query.to_string()),
                (
                    "TYPE_POOL".to_string(),
                    serde_json::to_string_pretty(type_pool).unwrap_or_default(),
                ),
            ]),
        )
    }

    /// LLM query rewrite — returns (type_keywords, entities_from_query).
    pub async fn query_rewrite(
        llm: &LlmClient,
        question: &str,
        type_pool: &Value,
    ) -> Result<(Vec<String>, Vec<String>)> {
        let prompt = Self::render_query2kwd(question, type_pool);
        let messages = [
            crate::llm::ChatMessage::new("system", prompt),
            crate::llm::ChatMessage::new("user", "Output:"),
        ];
        let response = llm.chat(&messages).await?;
        Ok(Self::parse_rewrite_json(&response))
    }

    /// Keyword → in-graph entity hits. Mirrors
    /// `get_relevant_ents_by_keywords` (vector search replaced by token
    /// overlap; sim = Jaccard-ish score, pagerank = degree).
    pub fn get_relevant_ents_by_keywords(
        graph: &EntityGraph,
        keywords: &[String],
        sim_thr: f64,
        n: usize,
    ) -> HashMap<String, KgEntity> {
        let mut res = HashMap::new();
        if keywords.is_empty() {
            return res;
        }
        let kw_joined = keywords.join(" ").to_lowercase();
        for name in graph.node_names() {
            let name_l = name.to_lowercase();
            let overlap = keywords
                .iter()
                .filter(|k| name_l.contains(&k.to_lowercase()))
                .count();
            if overlap == 0 && !kw_joined.contains(&name_l) {
                continue;
            }
            let sim = if overlap > 0 {
                (overlap as f64) / (keywords.len() as f64)
            } else {
                // name is a substring of the query text
                0.5
            };
            if sim < sim_thr {
                continue;
            }
            let mut e = KgEntity {
                sim,
                pagerank: graph.degree(&name) as f64,
                description: graph.description_of(&name).unwrap_or_default().to_string(),
                n_hop_ents: graph.search_connected(&name, 2),
            };
            e.n_hop_ents.retain(|x| x != &name);
            res.insert(name, e);
            if res.len() >= n {
                break;
            }
        }
        res
    }

    /// Type → in-graph entity hits (mirrors get_relevant_ents_by_types; the
    /// upstream orders by rank desc, we order by degree desc).
    pub fn get_relevant_ents_by_types(
        graph: &EntityGraph,
        types: &[String],
        n: usize,
    ) -> HashMap<String, KgEntity> {
        let mut res = HashMap::new();
        for ty in types {
            let etype = match ty.to_uppercase().as_str() {
                "PERSON" | "人" => EntityType::Person,
                "ORGANIZATION" | "ORG" | "组织" => EntityType::Organization,
                "LOCATION" | "GEO" | "地点" => EntityType::Location,
                "DATE" | "时间" => EntityType::Date,
                "TECHNOLOGY" | "技术" => EntityType::Technology,
                "PRODUCT" | "产品" => EntityType::Product,
                "EVENT" | "事件" => EntityType::Event,
                "CONCEPT" | "概念" => EntityType::Concept,
                _ => EntityType::Unknown,
            };
            let mut names = graph.entities_by_type(&etype);
            names.sort_by(|a, b| graph.degree(b).cmp(&graph.degree(a)));
            for name in names {
                res.entry(name.clone()).or_insert_with(|| KgEntity {
                    sim: 1.0,
                    pagerank: graph.degree(&name) as f64,
                    description: graph.description_of(&name).unwrap_or_default().to_string(),
                    n_hop_ents: Vec::new(),
                });
                if res.len() >= n {
                    return res;
                }
            }
        }
        res
    }

    /// Text → in-graph relation hits (mirrors get_relevant_relations_by_txt;
    /// relation sim boosted when both endpoints are retrieved).
    pub fn get_relevant_relations_by_txt(
        graph: &EntityGraph,
        txt: &str,
        sim_thr: f64,
        n: usize,
    ) -> HashMap<(String, String), KgRelation> {
        let mut res = HashMap::new();
        let txt = txt.to_lowercase();
        for name in graph.node_names() {
            for nbr in graph.neighbors(&name) {
                let key = if name < nbr {
                    (name.clone(), nbr)
                } else {
                    (nbr, name.clone())
                };
                let hit = txt.contains(&name.to_lowercase())
                    || txt.contains(&key.0.to_lowercase())
                    || txt.contains(&key.1.to_lowercase());
                if !hit {
                    continue;
                }
                let sim =
                    if txt.contains(&key.0.to_lowercase()) && txt.contains(&key.1.to_lowercase()) {
                        1.0
                    } else {
                        0.5
                    };
                if sim < sim_thr {
                    continue;
                }
                let rel = KgRelation {
                    sim,
                    pagerank: graph.edge_weight(&key.0, &key.1).unwrap_or(0.0),
                    description: graph
                        .edge_description(&key.0, &key.1)
                        .unwrap_or_default()
                        .to_string(),
                };
                res.insert(key, rel);
                if res.len() >= n {
                    return res;
                }
            }
        }
        res
    }

    /// Community report retrieval — mirrors `_community_retrieval_`. RayRAG
    /// keeps reports in a caller-supplied map (title → report text); the
    /// section format matches upstream (`# N. title\n## Content\n…`).
    pub fn community_retrieval(
        community_reports: &HashMap<String, String>,
        entities: &[String],
        topn: usize,
    ) -> String {
        // Simple relevance: a report matches when any of its entities appear
        // in the requested entity list. Reports are stored as
        // "entities|title|report" to make this feasible in-memory.
        let mut matched: Vec<(&String, &String)> = community_reports
            .iter()
            .filter(|(key, _)| {
                key.split('|')
                    .any(|e| entities.iter().any(|q| q.eq_ignore_ascii_case(e)))
            })
            .take(topn)
            .collect();
        matched.sort_by_key(|(_, report)| report.len());
        let mut txts = Vec::new();
        for (i, (key, report)) in matched.iter().enumerate() {
            let title = key.split('|').nth(1).unwrap_or("Community");
            txts.push(format!("# {}. {}\n## Content\n{}\n", i + 1, title, report));
        }
        if txts.is_empty() {
            return String::new();
        }
        "\n---- Community Report ----\n".to_string() + &txts.join("\n")
    }

    /// Full retrieval — mirrors `KGSearch.retrieval`. Returns the
    /// content_with_weight payload (Entities/Relations/Community sections).
    #[allow(clippy::too_many_arguments)]
    pub async fn retrieval(
        graph: &EntityGraph,
        llm: &LlmClient,
        community_reports: &HashMap<String, String>,
        question: &str,
        ent_topn: usize,
        rel_topn: usize,
        comm_topn: usize,
        sim_threshold: f64,
    ) -> Result<String> {
        let type_pool = graph.entity_type_pool();
        let (ty_kwds, ents) = match Self::query_rewrite(llm, question, &type_pool).await {
            Ok(v) => v,
            Err(_) => (Vec::new(), vec![question.to_string()]),
        };

        let mut ents_from_query =
            Self::get_relevant_ents_by_keywords(graph, &ents, sim_threshold, 56);
        let ents_from_types = Self::get_relevant_ents_by_types(graph, &ty_kwds, 10000);
        let mut rels_from_txt =
            Self::get_relevant_relations_by_txt(graph, question, sim_threshold, 56);

        // n-hop path aggregation: deep hops contribute less
        let mut nhop_pathes: HashMap<(String, String), (f64, f64)> = HashMap::new();
        for (ent_name, ent) in &ents_from_query {
            let paths = graph.n_hop_paths(ent_name, 2, 64);
            for (path, wts) in paths {
                for i in 0..path.len().saturating_sub(1) {
                    let (f, t) = if path[i] < path[i + 1] {
                        (path[i].clone(), path[i + 1].clone())
                    } else {
                        (path[i + 1].clone(), path[i].clone())
                    };
                    let entry = nhop_pathes.entry((f, t)).or_insert((0.0, 0.0));
                    entry.0 += ent.sim / (2.0 + i as f64);
                    entry.1 = wts.get(i).copied().unwrap_or(0.0);
                }
            }
        }

        // type hits boost entity sim ×2
        for ent in ents_from_types.keys() {
            if let Some(e) = ents_from_query.get_mut(ent) {
                e.sim *= 2.0;
            }
        }

        // relation sim boost: n-hop paths + type endpoints
        for (pair, rel) in rels_from_txt.iter_mut() {
            let mut s = 0.0;
            if let Some((nsim, _)) = nhop_pathes.remove(pair) {
                s += nsim;
            }
            if ents_from_types.contains_key(&pair.0) {
                s += 1.0;
            }
            if ents_from_types.contains_key(&pair.1) {
                s += 1.0;
            }
            rel.sim *= s + 1.0;
        }
        // relations discovered only via n-hop paths
        for ((f, t), (nsim, pr)) in nhop_pathes {
            let mut s = 0.0;
            if ents_from_types.contains_key(&f) {
                s += 1.0;
            }
            if ents_from_types.contains_key(&t) {
                s += 1.0;
            }
            rels_from_txt.insert(
                (f.clone(), t.clone()),
                KgRelation {
                    sim: nsim * (s + 1.0),
                    pagerank: pr,
                    description: graph
                        .edge_description(&f, &t)
                        .unwrap_or_default()
                        .to_string(),
                },
            );
        }

        // sort by sim × pagerank, take topn
        let mut ents_sorted: Vec<(String, KgEntity)> = ents_from_query.into_iter().collect();
        ents_sorted.sort_by(|a, b| {
            (b.1.sim * b.1.pagerank)
                .partial_cmp(&(a.1.sim * a.1.pagerank))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ents_sorted.truncate(ent_topn);

        let mut rels_sorted: Vec<((String, String), KgRelation)> =
            rels_from_txt.into_iter().collect();
        rels_sorted.sort_by(|a, b| {
            (b.1.sim * b.1.pagerank)
                .partial_cmp(&(a.1.sim * a.1.pagerank))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        rels_sorted.truncate(rel_topn);

        let mut ents_csv = String::new();
        if !ents_sorted.is_empty() {
            ents_csv.push_str("\n---- Entities ----\n");
            ents_csv.push_str("Entity,Score,Description\n");
            for (name, ent) in &ents_sorted {
                let desc = ent.description.replace([',', '\n'], " ");
                ents_csv.push_str(&format!(
                    "{},{:.2},{}\n",
                    name,
                    ent.sim * ent.pagerank,
                    desc
                ));
            }
        }
        let mut rels_csv = String::new();
        if !rels_sorted.is_empty() {
            rels_csv.push_str("\n---- Relations ----\n");
            rels_csv.push_str("From Entity,To Entity,Score,Description\n");
            for ((f, t), rel) in &rels_sorted {
                let desc = rel.description.replace([',', '\n'], " ");
                rels_csv.push_str(&format!("{f},{t},{:.2},{desc}\n", rel.sim * rel.pagerank));
            }
        }

        let comm = Self::community_retrieval(
            community_reports,
            &ents_sorted
                .iter()
                .map(|(n, _)| n.clone())
                .collect::<Vec<_>>(),
            comm_topn,
        );

        Ok(ents_csv + &rels_csv + &comm)
    }
}

// ── Graph Extraction ─────────────────────────────────────────────
// Mirrors rag/graphrag/general/graph_extractor.py (150 lines) +
// graph_prompt.py (123 lines) + utils.py's handle_single_entity_extraction /
// handle_single_relationship_extraction / split_string_by_multi_markers /
// is_float_regex. The upstream class runs one LLM pass per chunk with a
// "gleaning" loop (CONTINUE_PROMPT to append missed entities, LOOP_PROMPT
// to decide whether to keep going), then splits records and parses
// entities/relationships.

pub const DEFAULT_TUPLE_DELIMITER: &str = "<|>";
pub const DEFAULT_RECORD_DELIMITER: &str = "##";
pub const DEFAULT_COMPLETION_DELIMITER: &str = "<|COMPLETE|>";

/// GRAPH_EXTRACTION_PROMPT — verbatim from graph_prompt.py (GraphRAG, MIT).
pub const GRAPH_EXTRACTION_PROMPT: &str = r#"-Goal-
Given a text document that is potentially relevant to this activity and a list of entity types, identify all entities of those types from the text and all relationships among the identified entities.

-Steps-
1. Identify all entities. For each identified entity, extract the following information:
- entity_name: Name of the entity, capitalized, in language of 'Text'
- entity_type: One of the following types: [{entity_types}]
- entity_description: Comprehensive description of the entity's attributes and activities in language of 'Text'
Format each entity as ("entity"{tuple_delimiter}<entity_name>{tuple_delimiter}<entity_type>{tuple_delimiter}<entity_description>

2. From the entities identified in step 1, identify all pairs of (source_entity, target_entity) that are *clearly related* to each other.
For each pair of related entities, extract the following information:
- source_entity: name of the source entity, as identified in step 1
- target_entity: name of the target entity, as identified in step 1
- relationship_description: explanation as to why you think the source entity and the target entity are related to each other in language of 'Text'
- relationship_strength: a numeric score indicating strength of the relationship between the source entity and target entity
 Format each relationship as ("relationship"{tuple_delimiter}<source_entity>{tuple_delimiter}<target_entity>{tuple_delimiter}<relationship_description>{tuple_delimiter}<relationship_strength>)

3. Return output as a single list of all the entities and relationships identified in steps 1 and 2. Use **{record_delimiter}** as the list delimiter.

4. When finished, output {completion_delimiter}

######################
-Examples-
######################
Example 1:

Entity_types: [person, technology, mission, organization, location]
Text:
while Alex clenched his jaw, the buzz of frustration dull against the backdrop of Taylor's authoritarian certainty. It was this competitive undercurrent that kept him alert, the sense that his and Jordan's shared commitment to discovery was an unspoken rebellion against Cruz's narrowing vision of control and order.

Then Taylor did something unexpected. They paused beside Jordan and, for a moment, observed the device with something akin to reverence. "If this tech can be understood..." Taylor said, their voice quieter, "It could change the game for us. For all of us."

The underlying dismissal earlier seemed to falter, replaced by a glimpse of reluctant respect for the gravity of what lay in their hands. Jordan looked up, and for a fleeting heartbeat, their eyes locked with Taylor's, a wordless clash of wills softening into an uneasy truce.

It was a small transformation, barely perceptible, but one that Alex noted with an inward nod. They had all been brought here by different paths
################
Output:
("entity"{tuple_delimiter}"Alex"{tuple_delimiter}"person"{tuple_delimiter}"Alex is a character who experiences frustration and is observant of the dynamics among other characters."){record_delimiter}
("entity"{tuple_delimiter}"Taylor"{tuple_delimiter}"person"{tuple_delimiter}"Taylor is portrayed with authoritarian certainty and shows a moment of reverence towards a device, indicating a change in perspective."){record_delimiter}
("entity"{tuple_delimiter}"Jordan"{tuple_delimiter}"person"{tuple_delimiter}"Jordan shares a commitment to discovery and has a significant interaction with Taylor regarding a device."){record_delimiter}
("entity"{tuple_delimiter}"Cruz"{tuple_delimiter}"person"{tuple_delimiter}"Cruz is associated with a vision of control and order, influencing the dynamics among other characters."){record_delimiter}
("entity"{tuple_delimiter}"The Device"{tuple_delimiter}"technology"{tuple_delimiter}"The Device is central to the story, with potential game-changing implications, and is revered by Taylor."){record_delimiter}
("relationship"{tuple_delimiter}"Alex"{tuple_delimiter}"Taylor"{tuple_delimiter}"Alex is affected by Taylor's authoritarian certainty and observes changes in Taylor's attitude towards the device."{tuple_delimiter}7){record_delimiter}
("relationship"{tuple_delimiter}"Alex"{tuple_delimiter}"Jordan"{tuple_delimiter}"Alex and Jordan share a commitment to discovery, which contrasts with Cruz's vision."{tuple_delimiter}6){record_delimiter}
("relationship"{tuple_delimiter}"Taylor"{tuple_delimiter}"Jordan"{tuple_delimiter}"Taylor and Jordan interact directly regarding the device, leading to a moment of mutual respect and an uneasy truce."{tuple_delimiter}8){record_delimiter}
("relationship"{tuple_delimiter}"Jordan"{tuple_delimiter}"Cruz"{tuple_delimiter}"Jordan's commitment to discovery is in rebellion against Cruz's vision of control and order."{tuple_delimiter}5){record_delimiter}
("relationship"{tuple_delimiter}"Taylor"{tuple_delimiter}"The Device"{tuple_delimiter}"Taylor shows reverence towards the device, indicating its importance and potential impact."{tuple_delimiter}9){completion_delimiter}
#############################
Example 2:

Entity_types: [person, technology, mission, organization, location]
Text:
They were no longer mere operatives; they had become guardians of a threshold, keepers of a message from a realm beyond stars and stripes. This elevation in their mission could not be shackled by regulations and established protocols—it demanded a new perspective, a new resolve.

Tension threaded through the dialogue of beeps and static as communications with Washington buzzed in the background. The team stood, a portentous air enveloping them. It was clear that the decisions they made in the ensuing hours could redefine humanity's place in the cosmos or condemn them to ignorance and potential peril.

Their connection to the stars solidified, the group moved to address the crystallizing warning, shifting from passive recipients to active participants. Mercer's latter instincts gained precedence— the team's mandate had evolved, no longer solely to observe and report but to interact and prepare. A metamorphosis had begun, and Operation: Dulce hummed with the newfound frequency of their daring, a tone set not by the earthly
#############
Output:
("entity"{tuple_delimiter}"Washington"{tuple_delimiter}"location"{tuple_delimiter}"Washington is a location where communications are being received, indicating its importance in the decision-making process."){record_delimiter}
("entity"{tuple_delimiter}"Operation: Dulce"{tuple_delimiter}"mission"{tuple_delimiter}"Operation: Dulce is described as a mission that has evolved to interact and prepare, indicating a significant shift in objectives and activities."){record_delimiter}
("entity"{tuple_delimiter}"The team"{tuple_delimiter}"organization"{tuple_delimiter}"The team is portrayed as a group of individuals who have transitioned from passive observers to active participants in a mission, showing a dynamic change in their role."){record_delimiter}
("relationship"{tuple_delimiter}"The team"{tuple_delimiter}"Washington"{tuple_delimiter}"The team receives communications from Washington, which influences their decision-making process."{tuple_delimiter}7){record_delimiter}
("relationship"{tuple_delimiter}"The team"{tuple_delimiter}"Operation: Dulce"{tuple_delimiter}"The team is directly involved in Operation: Dulce, executing its evolved objectives and activities."{tuple_delimiter}9){completion_delimiter}
#############################
Example 3:

Entity_types: [person, role, technology, organization, event, location, concept]
Text:
their voice slicing through the buzz of activity. "Control may be an illusion when facing an intelligence that literally writes its own rules," they stated stoically, casting a watchful eye over the flurry of data.


"It's like it's learning to communicate," offered Sam Rivera from a nearby interface, their youthful energy boding a mix of awe and anxiety. "This gives talking to strangers' a whole new meaning."

Alex surveyed his team—each face a study in concentration, determination, and not a small measure of trepidation. "This might well be our first contact," he acknowledged, "And we need to be ready for whatever answers back."

Together, they stood on the edge of the unknown, forging humanity's response to a message from the heavens. The ensuing silence was palpable—a collective introspection about their role in this grand cosmic play, one that could rewrite human history.

The encrypted dialogue continued to unfold, its intricate patterns showing an almost uncanny anticipation
#############
Output:
("entity"{tuple_delimiter}"Sam Rivera"{tuple_delimiter}"person"{tuple_delimiter}"Sam Rivera is a member of a team working on communicating with an unknown intelligence, showing a mix of awe and anxiety."){record_delimiter}
("entity"{tuple_delimiter}"Alex"{tuple_delimiter}"person"{tuple_delimiter}"Alex is the leader of a team attempting first contact with an unknown intelligence, acknowledging the significance of their task."){record_delimiter}
("entity"{tuple_delimiter}"Control"{tuple_delimiter}"concept"{tuple_delimiter}"Control refers to the ability to manage or govern, which is challenged by an intelligence that writes its own rules."){record_delimiter}
("entity"{tuple_delimiter}"Intelligence"{tuple_delimiter}"concept"{tuple_delimiter}"Intelligence here refers to an unknown entity capable of writing its own rules and learning to communicate."){record_delimiter}
("entity"{tuple_delimiter}"First Contact"{tuple_delimiter}"event"{tuple_delimiter}"First Contact is the potential initial communication between humanity and an unknown intelligence."){record_delimiter}
("entity"{tuple_delimiter}"Humanity's Response"{tuple_delimiter}"event"{tuple_delimiter}"Humanity's Response is the collective action taken by Alex's team in response to a message from an unknown intelligence."){record_delimiter}
("relationship"{tuple_delimiter}"Sam Rivera"{tuple_delimiter}"Intelligence"{tuple_delimiter}"Sam Rivera is directly involved in the process of learning to communicate with the unknown intelligence."{tuple_delimiter}9){record_delimiter}
("relationship"{tuple_delimiter}"Alex"{tuple_delimiter}"First Contact"{tuple_delimiter}"Alex leads the team that might be making the First Contact with the unknown intelligence."{tuple_delimiter}10){record_delimiter}
("relationship"{tuple_delimiter}"Alex"{tuple_delimiter}"Humanity's Response"{tuple_delimiter}"Alex and his team are the key figures in Humanity's Response to the unknown intelligence."{tuple_delimiter}8){record_delimiter}
("relationship"{tuple_delimiter}"Control"{tuple_delimiter}"Intelligence"{tuple_delimiter}"The concept of Control is challenged by the Intelligence that writes its own rules."{tuple_delimiter}7){completion_delimiter}
#############################
-Real Data-
######################
Entity_types: {entity_types}
Text: {input_text}
######################
Output:"#;

/// CONTINUE_PROMPT — append missed entities after the first pass.
pub const CONTINUE_PROMPT: &str =
    "MANY entities were missed in the last extraction.  Add them below using the same format:\n";

/// LOOP_PROMPT — ask whether more entities remain (Y/N).
pub const LOOP_PROMPT: &str = "It appears some entities may have still been missed. Answer Y if there are still entities that need to be added, or N if there are none. Please answer with a single letter Y or N.\n";

/// SUMMARIZE_DESCRIPTIONS_PROMPT — merge per-chunk descriptions of the same
/// entity into one comprehensive description.
pub const SUMMARIZE_DESCRIPTIONS_PROMPT: &str = r#"
You are a helpful assistant responsible for generating a comprehensive summary of the data provided below.
Given one or two entities, and a list of descriptions, all related to the same entity or group of entities.
Please concatenate all of these into a single, comprehensive description. Make sure to include information collected from all the descriptions.
If the provided descriptions are contradictory, please resolve the contradictions and provide a single, coherent summary.
Make sure it is written in third person, and include the entity names so we the have full context.
Use {language} as output language.

#######
-Data-
Entities: {entity_name}
Description List: {description_list}
#######
"#;

/// An extracted entity record — mirrors handle_single_entity_extraction.
#[derive(Debug, Clone)]
pub struct KgExtractedNode {
    pub entity_name: String,
    pub entity_type: String,
    pub description: String,
    pub source_id: String,
}

/// An extracted relationship record — mirrors
/// handle_single_relationship_extraction.
#[derive(Debug, Clone)]
pub struct KgExtractedEdge {
    pub src_id: String,
    pub tgt_id: String,
    pub weight: f64,
    pub description: String,
    pub keywords: String,
    pub source_id: String,
}

/// split_string_by_multi_markers — mirrors rag/graphrag/utils.py.
pub fn split_string_by_multi_markers(content: &str, markers: &[&str]) -> Vec<String> {
    if markers.is_empty() {
        return if content.trim().is_empty() {
            Vec::new()
        } else {
            vec![content.trim().to_string()]
        };
    }
    let mut results = vec![content.to_string()];
    for marker in markers {
        let mut next = Vec::new();
        for part in results {
            next.extend(part.split(marker).map(|s| s.trim().to_string()));
        }
        results = next;
    }
    results.retain(|s| !s.is_empty());
    results
}

/// is_float_regex — mirrors rag/graphrag/utils.py.
pub fn is_float_regex(value: &str) -> bool {
    let v = value.trim();
    !v.is_empty()
        && v.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.'))
        && v.chars().any(|c| c.is_ascii_digit())
}

/// handle_single_entity_extraction — mirrors rag/graphrag/utils.py: a record
/// is an entity iff it has ≥4 attributes and the first is `"entity"`.
pub fn handle_single_entity_extraction(
    record_attributes: &[String],
    chunk_key: &str,
) -> Option<KgExtractedNode> {
    if record_attributes.len() < 4 || record_attributes[0] != "\"entity\"" {
        return None;
    }
    let entity_name = clean_str(&record_attributes[1].to_uppercase());
    if entity_name.trim().is_empty() {
        return None;
    }
    let entity_type = clean_str(&record_attributes[2].to_uppercase());
    let description = clean_str(&record_attributes[3]);
    Some(KgExtractedNode {
        entity_name: entity_name.to_uppercase(),
        entity_type: entity_type.to_uppercase(),
        description,
        source_id: chunk_key.to_string(),
    })
}

/// handle_single_relationship_extraction — mirrors rag/graphrag/utils.py.
pub fn handle_single_relationship_extraction(
    record_attributes: &[String],
    chunk_key: &str,
) -> Option<KgExtractedEdge> {
    if record_attributes.len() < 5 || record_attributes[0] != "\"relationship\"" {
        return None;
    }
    let source = clean_str(&record_attributes[1].to_uppercase());
    let target = clean_str(&record_attributes[2].to_uppercase());
    let description = clean_str(&record_attributes[3]);
    let keywords = clean_str(&record_attributes[4]);
    let weight = if is_float_regex(record_attributes.last()?) {
        record_attributes
            .last()?
            .trim()
            .parse::<f64>()
            .unwrap_or(1.0)
    } else {
        1.0
    };
    let (src_id, tgt_id) = if source <= target {
        (source, target)
    } else {
        (target, source)
    };
    Some(KgExtractedEdge {
        src_id: src_id.to_uppercase(),
        tgt_id: tgt_id.to_uppercase(),
        weight,
        description,
        keywords,
        source_id: chunk_key.to_string(),
    })
}

/// Parse raw LLM output into (entities, relationships) — mirrors
/// `GraphExtractor._process_single_content`'s record split + extractor.py's
/// `_entities_and_relations`. `entity_types` filters nodes (lowercased);
/// relations pass through.
pub fn parse_extraction_records(
    results: &str,
    entity_types: &[String],
    chunk_key: &str,
) -> (Vec<KgExtractedNode>, Vec<KgExtractedEdge>) {
    let records = split_string_by_multi_markers(
        results,
        &[DEFAULT_RECORD_DELIMITER, DEFAULT_COMPLETION_DELIMITER],
    );
    let mut rcds = Vec::new();
    for record in records {
        // extract the parenthesized body: `("entity"<|>... )`
        let Some(open) = record.find('(') else {
            continue;
        };
        let Some(close) = record.rfind(')') else {
            continue;
        };
        if close <= open {
            continue;
        }
        rcds.push(record[open + 1..close].to_string());
    }
    let ent_types: Vec<String> = entity_types.iter().map(|t| t.to_lowercase()).collect();
    let mut nodes: Vec<KgExtractedNode> = Vec::new();
    let mut edges: Vec<KgExtractedEdge> = Vec::new();
    for record in rcds {
        let attrs = split_string_by_multi_markers(&record, &[DEFAULT_TUPLE_DELIMITER]);
        if let Some(node) = handle_single_entity_extraction(&attrs, chunk_key) {
            if ent_types.is_empty() || ent_types.contains(&node.entity_type.to_lowercase()) {
                nodes.push(node);
            }
            continue;
        }
        if let Some(edge) = handle_single_relationship_extraction(&attrs, chunk_key) {
            edges.push(edge);
        }
    }
    (nodes, edges)
}

/// A chunk's extraction result.
#[derive(Debug, Default)]
pub struct GraphExtractionResult {
    pub nodes: Vec<KgExtractedNode>,
    pub edges: Vec<KgExtractedEdge>,
    pub token_count: usize,
}

/// GraphExtractor — mirrors GraphExtractor._process_single_content: one LLM
/// pass per chunk + gleaning loop (CONTINUE/LOOP prompts), then record
/// parsing. `max_gleanings` defaults to 2 (ENTITY_EXTRACTION_MAX_GLEANINGS).
#[derive(Debug, Clone)]
pub struct GraphExtractor {
    pub max_gleanings: usize,
}

impl Default for GraphExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphExtractor {
    pub fn new() -> Self {
        Self { max_gleanings: 2 }
    }

    /// Render the extraction prompt for one chunk.
    pub fn render_prompt(&self, content: &str, entity_types: &[String], language: &str) -> String {
        let mut vars = HashMap::new();
        vars.insert(
            "tuple_delimiter".to_string(),
            DEFAULT_TUPLE_DELIMITER.to_string(),
        );
        vars.insert(
            "record_delimiter".to_string(),
            DEFAULT_RECORD_DELIMITER.to_string(),
        );
        vars.insert(
            "completion_delimiter".to_string(),
            DEFAULT_COMPLETION_DELIMITER.to_string(),
        );
        vars.insert("entity_types".to_string(), entity_types.join(","));
        vars.insert("input_text".to_string(), content.to_string());
        vars.insert("language".to_string(), language.to_string());
        perform_variable_replacements(GRAPH_EXTRACTION_PROMPT, &vars)
    }

    /// Process a single chunk: initial LLM pass + gleanings.
    pub async fn process_single_content(
        &self,
        chunk_key: &str,
        content: &str,
        entity_types: &[String],
        llm: &LlmClient,
        language: &str,
    ) -> Result<GraphExtractionResult> {
        let hint_prompt = self.render_prompt(content, entity_types, language);
        let system = crate::llm::ChatMessage::new("system", hint_prompt.clone());
        let mut history = vec![system, crate::llm::ChatMessage::new("user", "Output:")];
        let response = llm.chat(&history).await?;
        let mut token_count = hint_prompt.chars().count() + response.chars().count();
        let mut results = response.clone();

        // Gleaning loop: keep asking for missed entities, stop on N or cap.
        for i in 0..self.max_gleanings {
            history.push(crate::llm::ChatMessage::new("user", CONTINUE_PROMPT));
            let response = llm.chat(&history).await?;
            token_count += history
                .iter()
                .map(|m| m.content.chars().count())
                .sum::<usize>()
                + response.chars().count();
            results.push_str(&response);

            if i >= self.max_gleanings - 1 {
                break;
            }
            history.push(crate::llm::ChatMessage::new("assistant", response));
            history.push(crate::llm::ChatMessage::new("user", LOOP_PROMPT));
            let continuation = llm.chat(&history).await?;
            if !continuation.trim().eq_ignore_ascii_case("y") {
                break;
            }
            history.push(crate::llm::ChatMessage::new("assistant", "Y"));
        }

        let (nodes, edges) = parse_extraction_records(&results, entity_types, chunk_key);
        Ok(GraphExtractionResult {
            nodes,
            edges,
            token_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphrag_enhanced::{EntityGraph, EntityType, NerEntity};

    // ── community report helpers ──────────────────────────────────

    fn graph_with_entities() -> EntityGraph {
        let mut g = EntityGraph::new();
        g.add_entities(&[
            NerEntity {
                name: "中山市百鲤居".into(),
                entity_type: EntityType::Organization,
                start: 0,
                end: 0,
                confidence: 1.0,
            },
            NerEntity {
                name: "李锦澎".into(),
                entity_type: EntityType::Person,
                start: 0,
                end: 0,
                confidence: 1.0,
            },
            NerEntity {
                name: "水产养殖基地".into(),
                entity_type: EntityType::Location,
                start: 0,
                end: 0,
                confidence: 1.0,
            },
            NerEntity {
                name: "孤岛节点".into(),
                entity_type: EntityType::Unknown,
                start: 0,
                end: 0,
                confidence: 1.0,
            },
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
        assert_eq!(
            html_unescape("a&amp;b&lt;c&gt;&quot;d&quot;&#39;e&#39;"),
            "a&b<c>\"d\"'e'"
        );
        // unknown entity preserved verbatim
        assert_eq!(html_unescape("a&unknown;b"), "a&unknown;b");
    }

    // ── KG search ─────────────────────────────────────────────────

    fn kg_graph() -> EntityGraph {
        let mut g = EntityGraph::new();
        g.add_entities(&[
            NerEntity {
                name: "中山市百鲤居".into(),
                entity_type: EntityType::Organization,
                start: 0,
                end: 0,
                confidence: 1.0,
            },
            NerEntity {
                name: "李锦澎".into(),
                entity_type: EntityType::Person,
                start: 0,
                end: 0,
                confidence: 1.0,
            },
            NerEntity {
                name: "水产养殖基地".into(),
                entity_type: EntityType::Location,
                start: 0,
                end: 0,
                confidence: 1.0,
            },
            NerEntity {
                name: "罗氏沼虾".into(),
                entity_type: EntityType::Product,
                start: 0,
                end: 0,
                confidence: 1.0,
            },
        ]);
        g.add_relation("中山市百鲤居", "李锦澎", 1.0);
        g.add_relation("中山市百鲤居", "水产养殖基地", 0.8);
        g.add_relation("水产养殖基地", "罗氏沼虾", 0.6);
        g.set_description("罗氏沼虾", "淡水养殖虾类");
        g
    }

    #[test]
    fn kg_entities_by_keywords_matches_names() {
        let g = kg_graph();
        let hits = KgSearch::get_relevant_ents_by_keywords(&g, &["百鲤居".to_string()], 0.3, 10);
        assert!(hits.contains_key("中山市百鲤居"));
        let e = &hits["中山市百鲤居"];
        assert!(e.pagerank >= 2.0, "hub node should have degree 2");
    }

    #[test]
    fn kg_entities_by_types_filters_and_orders() {
        let g = kg_graph();
        let hits = KgSearch::get_relevant_ents_by_types(&g, &["PERSON".to_string()], 10);
        assert!(hits.contains_key("李锦澎"));
        assert!(!hits.contains_key("中山市百鲤居"));
        let prods = KgSearch::get_relevant_ents_by_types(&g, &["产品".to_string()], 10);
        assert!(prods.contains_key("罗氏沼虾"));
    }

    #[test]
    fn kg_relations_by_txt_finds_edges() {
        let g = kg_graph();
        let rels = KgSearch::get_relevant_relations_by_txt(&g, "百鲤居 水产养殖基地", 0.3, 10);
        assert!(rels.contains_key(&("中山市百鲤居".to_string(), "水产养殖基地".to_string())));
    }

    #[test]
    fn kg_parse_rewrite_json_handles_fences() {
        let raw = "Here you go:\n```json\n{\"answer_type_keywords\": [\"LOCATION\"], \"entities_from_query\": [\"中山\", \"虾\"]}\n```";
        let (types, ents) = KgSearch::parse_rewrite_json(raw);
        assert_eq!(types, vec!["LOCATION"]);
        assert_eq!(ents.len(), 2);
        // Truncation to 5 entities
        let big = "{\"answer_type_keywords\": [], \"entities_from_query\": [\"1\",\"2\",\"3\",\"4\",\"5\",\"6\"]}";
        let (_, ents2) = KgSearch::parse_rewrite_json(big);
        assert_eq!(ents2.len(), 5);
    }

    #[test]
    fn kg_render_query2kwd_substitutes_both_vars() {
        let pool = json!({"LOCATION": ["中山市"]});
        let rendered = KgSearch::render_query2kwd("Where is the farm?", &pool);
        assert!(rendered.contains("Where is the farm?"));
        assert!(rendered.contains("\"LOCATION\""));
        assert!(!rendered.contains("{query}"));
        assert!(!rendered.contains("{TYPE_POOL}"));
    }

    #[test]
    fn kg_community_retrieval_matches_entities_and_formats() {
        let mut reports = HashMap::new();
        reports.insert(
            "罗氏沼虾|罗氏沼虾社区|报告内容".to_string(),
            "report body".to_string(),
        );
        reports.insert(
            "无关实体|其他社区|其他内容".to_string(),
            "unrelated".to_string(),
        );
        let out = KgSearch::community_retrieval(&reports, &["罗氏沼虾".to_string()], 5);
        assert!(out.contains("---- Community Report ----"));
        assert!(out.contains("# 1. 罗氏沼虾社区"));
        assert!(out.contains("## Content"));
        assert!(!out.contains("其他社区"));
    }

    #[test]
    fn kg_n_hop_paths_return_weights() {
        let g = kg_graph();
        let paths = g.n_hop_paths("中山市百鲤居", 2, 64);
        assert!(!paths.is_empty());
        // every path starts at the query entity and has matching weights
        for (path, wts) in &paths {
            assert_eq!(path.first().map(String::as_str), Some("中山市百鲤居"));
            assert_eq!(path.len() - 1, wts.len());
        }
    }

    #[test]
    fn kg_entity_type_pool_counts_samples() {
        let g = kg_graph();
        let pool = g.entity_type_pool();
        let obj = pool.as_object().expect("object pool");
        assert!(obj.contains_key("Person"));
        assert!(obj.contains_key("Organization"));
    }

    // ── graph extraction ──────────────────────────────────────────

    const SAMPLE_OUTPUT: &str = r#"("entity"<|>"Alex"<|>"person"<|>"Alex is a character.")##("entity"<|>"The Device"<|>"technology"<|>"Central device.")##("relationship"<|>"Alex"<|>"The Device"<|>"Alex interacts with the device."<|>7)<|COMPLETE|>"#;

    #[test]
    fn parse_extraction_records_extracts_entities_and_edges() {
        let types = vec!["person".to_string(), "technology".to_string()];
        let (nodes, edges) = parse_extraction_records(SAMPLE_OUTPUT, &types, "chunk-1");
        assert_eq!(nodes.len(), 2, "2 entities");
        assert_eq!(edges.len(), 1, "1 relationship");
        let alex = nodes.iter().find(|n| n.entity_name == "ALEX").unwrap();
        assert_eq!(alex.entity_type, "PERSON");
        assert_eq!(alex.source_id, "chunk-1");
        let edge = &edges[0];
        // sorted pair: Alex < The Device
        assert_eq!(edge.src_id, "ALEX");
        assert_eq!(edge.tgt_id, "THE DEVICE");
        assert_eq!(edge.weight, 7.0);
    }

    #[test]
    fn parse_extraction_records_filters_by_type() {
        let types = vec!["person".to_string()]; // technology excluded
        let (nodes, _) = parse_extraction_records(SAMPLE_OUTPUT, &types, "c");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].entity_name, "ALEX");
    }

    #[test]
    fn parse_extraction_records_skips_garbage() {
        let out =
            "no parens here\n##(\"entity\"<|>\"\"<|>\"person\"<|>\"empty name\")##(\"entity\"";
        let (nodes, edges) = parse_extraction_records(out, &["person".to_string()], "c");
        assert!(nodes.is_empty(), "empty name entity skipped");
        assert!(edges.is_empty());
    }

    #[test]
    fn split_markers_handles_empty_and_multiple() {
        assert!(split_string_by_multi_markers("", &["##"]).is_empty());
        assert!(split_string_by_multi_markers("   ", &[]).is_empty());
        let parts = split_string_by_multi_markers("a##b<|COMPLETE|>c", &["##", "<|COMPLETE|>"]);
        assert_eq!(parts, vec!["a", "b", "c"]);
    }

    #[test]
    fn is_float_regex_matches_numbers() {
        assert!(is_float_regex("7"));
        assert!(is_float_regex("-3.5"));
        assert!(is_float_regex("+0.25"));
        assert!(is_float_regex("10."));
        assert!(!is_float_regex("abc"));
        assert!(!is_float_regex(""));
        assert!(!is_float_regex("1e5"));
    }

    #[test]
    fn handle_entity_and_relationship_parsers() {
        let ent = handle_single_entity_extraction(
            &[
                "\"entity\"".into(),
                "  name  ".into(),
                "person".into(),
                "desc".into(),
            ],
            "k",
        )
        .unwrap();
        assert_eq!(ent.entity_name, "NAME");
        assert_eq!(ent.entity_type, "PERSON");
        // wrong first attr → None
        assert!(
            handle_single_entity_extraction(
                &[
                    "\"relationship\"".into(),
                    "a".into(),
                    "b".into(),
                    "c".into()
                ],
                "k",
            )
            .is_none()
        );
        // relationship parser
        let rel = handle_single_relationship_extraction(
            &[
                "\"relationship\"".into(),
                "Zeta".into(),
                "Alpha".into(),
                "desc".into(),
                "kw".into(),
                "4.5".into(),
            ],
            "k",
        )
        .unwrap();
        assert_eq!(rel.src_id, "ALPHA");
        assert_eq!(rel.tgt_id, "ZETA");
        assert_eq!(rel.weight, 4.5);
    }

    #[test]
    fn render_graph_extraction_prompt_substitutes_vars() {
        let ex = GraphExtractor::new();
        let rendered = ex.render_prompt(
            "Some text about 中山市.",
            &["person".to_string(), "organization".to_string()],
            "Chinese",
        );
        assert!(rendered.contains("Some text about 中山市."));
        assert!(rendered.contains("person,organization"));
        assert!(rendered.contains("<|>"));
        assert!(!rendered.contains("{input_text}"));
        assert!(!rendered.contains("{entity_types}"));
    }
}
