//! Prompt template system + Fallback LLM.
//! Replaces RAGFlow's `rag/prompts/` (47 templates) + `llm/fallback_chat_model.py`.

use crate::llm::{ChatMessage, LlmClient, LlmConfig};
use std::collections::HashMap;

// ── Prompt Templates ────────────────────────────────────────────

/// A prompt template with variable substitution.
pub struct PromptTemplate {
    /// Template content with {variable} placeholders
    content: String,
}

impl PromptTemplate {
    pub fn new(content: &str) -> Self {
        Self {
            content: content.to_string(),
        }
    }

    /// Render template with variable substitution.
    pub fn render(&self, vars: &HashMap<&str, &str>) -> String {
        let mut result = self.content.clone();
        for (key, value) in vars {
            result = result.replace(&format!("{{{}}}", key), value);
        }
        result
    }

    /// Raw template content (no substitution).
    pub fn content(&self) -> &str {
        &self.content
    }
}

/// Built-in prompt templates (matching RAGFlow's 47 prompts).
pub struct PromptLibrary;

impl PromptLibrary {
    /// RAG QA system prompt
    pub fn rag_qa() -> PromptTemplate {
        PromptTemplate::new(
            "You are a helpful AI assistant.\n\
             Use the following context to answer the question.\n\
             If the answer is not in the context, say \"I don't have enough information.\"\n\
             Always cite the source when possible.\n\n\
             Context:\n{context}\n\n\
             Question: {question}\n\
             Answer:",
        )
    }

    /// Conversation summary prompt
    pub fn conversation_summary() -> PromptTemplate {
        PromptTemplate::new(
            "Summarize the following conversation in 2-3 sentences:\n\n{conversation}\n\nSummary:",
        )
    }

    /// Citation prompt
    pub fn citation() -> PromptTemplate {
        PromptTemplate::new(
            "Based on the context, provide a detailed answer with citations.\n\
             Format citations as [1], [2], etc.\n\n\
             Context:\n{context}\n\n\
             Question: {question}\n\
             Answer with citations:",
        )
    }

    /// TOC level assignment prompt
    pub fn toc_levels() -> PromptTemplate {
        PromptTemplate::new(
            "Assign depth levels (1=top, 2=sub, 3=sub-sub) to these TOC items.\n\
             Return JSON array: [{{\"level\":\"1\",\"title\":\"...\"}},...]\n\n\
             Items:\n{items}",
        )
    }

    /// Content tagging prompt
    pub fn content_tagging() -> PromptTemplate {
        PromptTemplate::new(
            "Tag the following text with relevant categories.\n\
             Categories: technical, business, academic, tutorial, reference, news\n\
             Return a JSON array of tags.\n\n\
             Text:\n{text}\n\n\
             Tags:",
        )
    }

    /// Cross-language translation prompt
    pub fn cross_language() -> PromptTemplate {
        PromptTemplate::new(
            "Translate the following text from {source_lang} to {target_lang}.\n\
             Preserve the original meaning and tone.\n\n\
             Text:\n{text}\n\n\
             Translation:",
        )
    }

    /// Analyze task prompt
    pub fn analyze_task() -> PromptTemplate {
        PromptTemplate::new(
            "Analyze the following task and break it down into steps.\n\
             Return a numbered list of actionable steps.\n\n\
             Task: {task}\n\n\
             Steps:",
        )
    }

    /// Generate related questions
    pub fn related_questions() -> PromptTemplate {
        PromptTemplate::new(
            "Based on the context and question, generate 3 related questions the user might ask.\n\n\
             Context: {context}\n\
             Question: {question}\n\n\
             Related questions:\n1.",
        )
    }

    // ── GraphRAG prompts (ported from RAGFlow rag/prompts & rag/graphrag) ──

    /// GraphRAG entity + relationship extraction (general extractor).
    pub fn graph_entity_extraction() -> PromptTemplate {
        PromptTemplate::new(GRAPH_EXTRACTION_PROMPT)
    }

    /// GraphRAG continue-extraction follow-up.
    pub fn graph_continue_extraction() -> PromptTemplate {
        PromptTemplate::new(GRAPH_EXTRACTION_CONTINUE_PROMPT)
    }

    /// GraphRAG loop check (Y/N whether more entities remain).
    pub fn graph_loop_extraction() -> PromptTemplate {
        PromptTemplate::new(GRAPH_EXTRACTION_LOOP_PROMPT)
    }

    /// GraphRAG summarize entity descriptions into one description.
    pub fn graph_summarize_descriptions() -> PromptTemplate {
        PromptTemplate::new(GRAPH_SUMMARIZE_DESCRIPTIONS_PROMPT)
    }

    /// GraphRAG community report generation (community summary).
    pub fn graph_community_summary() -> PromptTemplate {
        PromptTemplate::new(GRAPH_COMMUNITY_REPORT_PROMPT)
    }

    /// Light graph entity extraction (LightRAG style, with content keywords).
    pub fn entity_extraction() -> PromptTemplate {
        PromptTemplate::new(ENTITY_EXTRACTION_PROMPT)
    }

    /// Light graph continue-extraction for missed entities/relationships.
    pub fn entity_continue_extraction() -> PromptTemplate {
        PromptTemplate::new(ENTITY_EXTRACTION_CONTINUE_PROMPT)
    }

    /// Light graph loop check (answer YES/NO only).
    pub fn entity_if_loop_extraction() -> PromptTemplate {
        PromptTemplate::new(ENTITY_IF_LOOP_EXTRACTION_PROMPT)
    }

    /// Light graph summarize entity descriptions.
    pub fn summarize_entity_descriptions() -> PromptTemplate {
        PromptTemplate::new(SUMMARIZE_ENTITY_DESCRIPTIONS_PROMPT)
    }

    /// Light graph keyword extraction (high/low level keywords).
    pub fn keywords_extraction() -> PromptTemplate {
        PromptTemplate::new(KEYWORDS_EXTRACTION_PROMPT)
    }

    /// Query-analyzer keyword extraction (query_analyze_prompt.py).
    pub fn query_keywords_extraction() -> PromptTemplate {
        PromptTemplate::new(QUERY_KEYWORDS_EXTRACTION_PROMPT)
    }

    /// MiniRAG query -> answer-type + entity keywords.
    pub fn minirag_query2kwd() -> PromptTemplate {
        PromptTemplate::new(MINIRAG_QUERY2KWD_PROMPT)
    }

    /// Entity resolution (dedupe/merge) prompt.
    pub fn entity_resolution() -> PromptTemplate {
        PromptTemplate::new(ENTITY_RESOLUTION_PROMPT)
    }

    /// Mind-map extraction prompt.
    pub fn mind_map_extraction() -> PromptTemplate {
        PromptTemplate::new(MIND_MAP_EXTRACTION_PROMPT)
    }

    /// Light graph RAG response with KG + DC citations.
    pub fn lightrag_response() -> PromptTemplate {
        PromptTemplate::new(LIGHTRAG_RESPONSE_PROMPT)
    }

    /// Naive (chunk-only) RAG response with DC citations.
    pub fn naive_rag_response() -> PromptTemplate {
        PromptTemplate::new(NAIVE_RAG_RESPONSE_PROMPT)
    }

    /// Question proposal: propose top-N questions about a text.
    pub fn question_proposal() -> PromptTemplate {
        PromptTemplate::new(QUESTION_PROPOSAL_PROMPT)
    }

    /// Keyword extraction (keyword_prompt.md): top-N keywords of a text.
    pub fn keyword_extraction() -> PromptTemplate {
        PromptTemplate::new(KEYWORD_EXTRACTION_PROMPT)
    }

    /// LightRAG fail response (no-context fallback answer).
    pub fn lightrag_fail_response() -> PromptTemplate {
        PromptTemplate::new(LIGHTRAG_FAIL_RESPONSE)
    }

    /// LightRAG default output language.
    pub fn lightrag_default_language() -> PromptTemplate {
        PromptTemplate::new(LIGHTRAG_DEFAULT_LANGUAGE)
    }

    /// LightRAG default tuple delimiter.
    pub fn lightrag_default_tuple_delimiter() -> PromptTemplate {
        PromptTemplate::new(LIGHTRAG_DEFAULT_TUPLE_DELIMITER)
    }

    /// LightRAG default record delimiter.
    pub fn lightrag_default_record_delimiter() -> PromptTemplate {
        PromptTemplate::new(LIGHTRAG_DEFAULT_RECORD_DELIMITER)
    }

    /// LightRAG default completion delimiter.
    pub fn lightrag_default_completion_delimiter() -> PromptTemplate {
        PromptTemplate::new(LIGHTRAG_DEFAULT_COMPLETION_DELIMITER)
    }

    /// LightRAG default entity types (comma-joined for prompt rendering).
    pub fn lightrag_default_entity_types() -> PromptTemplate {
        PromptTemplate::new(&LIGHTRAG_DEFAULT_ENTITY_TYPES.join(", "))
    }

    /// LightRAG default user prompt.
    pub fn lightrag_default_user_prompt() -> PromptTemplate {
        PromptTemplate::new(LIGHTRAG_DEFAULT_USER_PROMPT)
    }

    /// RAGFlow `citation_prompt.md` — Detailed [ID:i] citation rules for RAG answers (quantitative data, temporal claims, RTL handling).
    pub fn citation_prompt() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_CITATION_PROMPT)
    }

    /// RAGFlow `citation_plus.md` — Add citations to an already-generated report; {{ example }}/{{ sources }} -> {example}/{sources}.
    pub fn citation_plus() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_CITATION_PLUS_PROMPT)
    }

    /// RAGFlow `full_question_prompt.md` — Rewrite the latest turn into a standalone full question; relative dates -> absolute; Jinja if/else flattened.
    pub fn full_question() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_FULL_QUESTION_PROMPT)
    }

    /// RAGFlow `multi_queries_gen.md` — Generate 2-3 complementary queries when retrieval was insufficient.
    pub fn multi_queries_gen() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_MULTI_QUERIES_GEN_PROMPT)
    }

    /// RAGFlow `next_step.md` — Planning agent: pick tools / complete_task; private <think> reflection.
    pub fn next_step() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_NEXT_STEP_PROMPT)
    }

    /// RAGFlow `reflect.md` — Post-tool-call reflection with complexity scoring; tool_calls loop flattened to {tool_calls}.
    pub fn reflect() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_REFLECT_PROMPT)
    }

    /// RAGFlow `rank_memory.md` — Rank tool results by relevance to goal/sub-goal; results loop flattened to {results}.
    pub fn rank_memory() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_RANK_MEMORY_PROMPT)
    }

    /// RAGFlow `content_tagging_prompt.md` — Tag text with top-N tags from a tag set; examples loop flattened to {examples}.
    pub fn content_tagging_prompt() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_CONTENT_TAGGING_PROMPT)
    }

    /// RAGFlow `cross_languages_sys_prompt.md` — Multilingual batch translator system prompt (keeps zh/fr/ja example).
    pub fn cross_languages_sys() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_CROSS_LANGUAGES_SYS_PROMPT)
    }

    /// RAGFlow `cross_languages_user_prompt.md` — Batch translation user prompt; {{ languages | join(', ') }} -> {languages}.
    pub fn cross_languages_user() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_CROSS_LANGUAGES_USER_PROMPT)
    }

    /// RAGFlow `analyze_task_system.md` — Task analyzer with LOW/MEDIUM/HIGH adaptive depth.
    pub fn analyze_task_system() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_ANALYZE_TASK_SYSTEM_PROMPT)
    }

    /// RAGFlow `analyze_task_user.md` — Task analyzer user variables: task/context/agent_prompt/tools_desc.
    pub fn analyze_task_user() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_ANALYZE_TASK_USER_PROMPT)
    }

    /// RAGFlow `ask_summary.md` — Miss R knowledge-base QA summary prompt (no-hallucination rules).
    pub fn ask_summary() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_ASK_SUMMARY_PROMPT)
    }

    /// RAGFlow `assign_toc_levels.md` — Assign Arabic-numeral depth levels to TOC items (JSON in/out).
    pub fn assign_toc_levels() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_ASSIGN_TOC_LEVELS_PROMPT)
    }

    /// RAGFlow `keyword_prompt.md` — Extract top-N keywords of a text (RAGFlow original).
    pub fn keyword_prompt() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_KEYWORD_PROMPT)
    }

    /// RAGFlow `question_prompt.md` — Propose top-N questions about a text (RAGFlow original).
    pub fn question_prompt() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_QUESTION_PROMPT)
    }

    /// RAGFlow `meta_data.md` — Strict metadata extraction from content against a schema.
    pub fn meta_data() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_META_DATA_PROMPT)
    }

    /// RAGFlow `meta_filter.md` — Generate metadata filter conditions (key/value/op + and/or logic).
    pub fn meta_filter() -> PromptTemplate {
        PromptTemplate::new(RAGFLOW_META_FILTER_PROMPT)
    }

    /// Get template by name.
    pub fn get(name: &str) -> Option<PromptTemplate> {
        match name {
            "rag_qa" => Some(Self::rag_qa()),
            "conversation_summary" => Some(Self::conversation_summary()),
            "citation" => Some(Self::citation()),
            "toc_levels" => Some(Self::toc_levels()),
            "content_tagging" => Some(Self::content_tagging()),
            "cross_language" => Some(Self::cross_language()),
            "analyze_task" => Some(Self::analyze_task()),
            "related_questions" => Some(Self::related_questions()),
            "graph_entity_extraction" => Some(Self::graph_entity_extraction()),
            "graph_continue_extraction" => Some(Self::graph_continue_extraction()),
            "graph_loop_extraction" => Some(Self::graph_loop_extraction()),
            "graph_summarize_descriptions" => Some(Self::graph_summarize_descriptions()),
            "graph_community_summary" => Some(Self::graph_community_summary()),
            "entity_extraction" => Some(Self::entity_extraction()),
            "entity_continue_extraction" => Some(Self::entity_continue_extraction()),
            "entity_if_loop_extraction" => Some(Self::entity_if_loop_extraction()),
            "summarize_entity_descriptions" => Some(Self::summarize_entity_descriptions()),
            "keywords_extraction" => Some(Self::keywords_extraction()),
            "query_keywords_extraction" => Some(Self::query_keywords_extraction()),
            "minirag_query2kwd" => Some(Self::minirag_query2kwd()),
            "entity_resolution" => Some(Self::entity_resolution()),
            "mind_map_extraction" => Some(Self::mind_map_extraction()),
            "lightrag_response" => Some(Self::lightrag_response()),
            "naive_rag_response" => Some(Self::naive_rag_response()),
            "question_proposal" => Some(Self::question_proposal()),
            "keyword_extraction" => Some(Self::keyword_extraction()),
            "lightrag_fail_response" => Some(Self::lightrag_fail_response()),
            "lightrag_default_language" => Some(Self::lightrag_default_language()),
            "lightrag_default_tuple_delimiter" => Some(Self::lightrag_default_tuple_delimiter()),
            "lightrag_default_record_delimiter" => Some(Self::lightrag_default_record_delimiter()),
            "lightrag_default_completion_delimiter" => {
                Some(Self::lightrag_default_completion_delimiter())
            }
            "lightrag_default_entity_types" => Some(Self::lightrag_default_entity_types()),
            "lightrag_default_user_prompt" => Some(Self::lightrag_default_user_prompt()),
            "citation_prompt" => Some(Self::citation_prompt()),
            "citation_plus" => Some(Self::citation_plus()),
            "full_question" => Some(Self::full_question()),
            "multi_queries_gen" => Some(Self::multi_queries_gen()),
            "next_step" => Some(Self::next_step()),
            "reflect" => Some(Self::reflect()),
            "rank_memory" => Some(Self::rank_memory()),
            "content_tagging_prompt" => Some(Self::content_tagging_prompt()),
            "cross_languages_sys" => Some(Self::cross_languages_sys()),
            "cross_languages_user" => Some(Self::cross_languages_user()),
            "analyze_task_system" => Some(Self::analyze_task_system()),
            "analyze_task_user" => Some(Self::analyze_task_user()),
            "ask_summary" => Some(Self::ask_summary()),
            "assign_toc_levels" => Some(Self::assign_toc_levels()),
            "keyword_prompt" => Some(Self::keyword_prompt()),
            "question_prompt" => Some(Self::question_prompt()),
            "meta_data" => Some(Self::meta_data()),
            "meta_filter" => Some(Self::meta_filter()),
            _ => None,
        }
    }

    /// List available template names.
    pub fn list() -> Vec<&'static str> {
        vec![
            "rag_qa",
            "conversation_summary",
            "citation",
            "toc_levels",
            "content_tagging",
            "cross_language",
            "analyze_task",
            "related_questions",
            "graph_entity_extraction",
            "graph_continue_extraction",
            "graph_loop_extraction",
            "graph_summarize_descriptions",
            "graph_community_summary",
            "entity_extraction",
            "entity_continue_extraction",
            "entity_if_loop_extraction",
            "summarize_entity_descriptions",
            "keywords_extraction",
            "query_keywords_extraction",
            "minirag_query2kwd",
            "entity_resolution",
            "mind_map_extraction",
            "lightrag_response",
            "naive_rag_response",
            "question_proposal",
            "keyword_extraction",
            "lightrag_fail_response",
            "lightrag_default_language",
            "lightrag_default_tuple_delimiter",
            "lightrag_default_record_delimiter",
            "lightrag_default_completion_delimiter",
            "lightrag_default_entity_types",
            "lightrag_default_user_prompt",
            "citation_prompt",
            "citation_plus",
            "full_question",
            "multi_queries_gen",
            "next_step",
            "reflect",
            "rank_memory",
            "content_tagging_prompt",
            "cross_languages_sys",
            "cross_languages_user",
            "analyze_task_system",
            "analyze_task_user",
            "ask_summary",
            "assign_toc_levels",
            "keyword_prompt",
            "question_prompt",
            "meta_data",
            "meta_filter",
        ]
    }
}

// ── GraphRAG Prompt Templates (ported from RAGFlow) ─────────────
// Sources:
//  - rag/graphrag/general/graph_prompt.py          (GRAPH_EXTRACTION_PROMPT et al.)
//  - rag/graphrag/general/community_report_prompt.py
//  - rag/graphrag/general/mind_map_prompt.py
//  - rag/graphrag/light/graph_prompt.py            (LightRAG-style prompts)
//  - rag/graphrag/query_analyze_prompt.py
//  - rag/graphrag/entity_resolution_prompt.py
//  - rag/prompts/question_prompt.md / keyword_prompt.md
// Python `{var}` / Jinja `{{ var }}` placeholders are kept as `{var}` so
// `PromptTemplate::render` can substitute them.

/// GraphRAG entity + relationship extraction (general extractor).
pub const GRAPH_EXTRACTION_PROMPT: &str = r#"
-Goal-
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

/// GraphRAG continue-extraction follow-up (add missed entities in the same format).
pub const GRAPH_EXTRACTION_CONTINUE_PROMPT: &str =
    "MANY entities were missed in the last extraction.  Add them below using the same format:\n";

/// GraphRAG loop check: answer Y if entities still need to be added, else N.
pub const GRAPH_EXTRACTION_LOOP_PROMPT: &str = "It appears some entities may have still been missed. Answer Y if there are still entities that need to be added, or N if there are none. Please answer with a single letter Y or N.\n";

/// Summarize multiple descriptions of the same entity into one comprehensive description.
pub const GRAPH_SUMMARIZE_DESCRIPTIONS_PROMPT: &str = r#"
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

/// GraphRAG community report generation (community summary).
/// Python `{{`/`}}` escaped braces are unescaped to literal `{`/`}`.
pub const GRAPH_COMMUNITY_REPORT_PROMPT: &str = r#"
You are an AI assistant that helps a human analyst to perform general information discovery. Information discovery is the process of identifying and assessing relevant information associated with certain entities (e.g., organizations and individuals) within a network.

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
    {
        "title": <report_title>,
        "summary": <executive_summary>,
        "rating": <impact_severity_rating>,
        "rating_explanation": <rating_explanation>,
        "findings": [
            {
                "summary":<insight_1_summary>,
                "explanation": <insight_1_explanation>
            },
            {
                "summary":<insight_2_summary>,
                "explanation": <insight_2_explanation>
            }
        ]
    }

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
{
    "title": "Verdant Oasis Plaza and Unity March",
    "summary": "The community revolves around the Verdant Oasis Plaza, which is the location of the Unity March. The plaza has relationships with the Harmony Assembly, Unity March, and Tribune Spotlight, all of which are associated with the march event.",
    "rating": 5.0,
    "rating_explanation": "The impact severity rating is moderate due to the potential for unrest or conflict during the Unity March.",
    "findings": [
        {
            "summary": "Verdant Oasis Plaza as the central location",
            "explanation": "Verdant Oasis Plaza is the central entity in this community, serving as the location for the Unity March. This plaza is the common link between all other entities, suggesting its significance in the community. The plaza's association with the march could potentially lead to issues such as public disorder or conflict, depending on the nature of the march and the reactions it provokes. [Data: Entities (5), Relationships (37, 38, 39, 40, 41,+more)]"
        },
        {
            "summary": "Harmony Assembly's role in the community",
            "explanation": "Harmony Assembly is another key entity in this community, being the organizer of the march at Verdant Oasis Plaza. The nature of Harmony Assembly and its march could be a potential source of threat, depending on their objectives and the reactions they provoke. The relationship between Harmony Assembly and the plaza is crucial in understanding the dynamics of this community. [Data: Entities(6), Relationships (38, 43)]"
        },
        {
            "summary": "Unity March as a significant event",
            "explanation": "The Unity March is a significant event taking place at Verdant Oasis Plaza. This event is a key factor in the community's dynamics and could be a potential source of threat, depending on the nature of the march and the reactions it provokes. The relationship between the march and the plaza is crucial in understanding the dynamics of this community. [Data: Relationships (39)]"
        },
        {
            "summary": "Role of Tribune Spotlight",
            "explanation": "Tribune Spotlight is reporting on the Unity March taking place in Verdant Oasis Plaza. This suggests that the event has attracted media attention, which could amplify its impact on the community. The role of Tribune Spotlight could be significant in shaping public perception of the event and the entities involved. [Data: Relationships (40)]"
        }
    ]
}


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
    {
        "title": <report_title>,
        "summary": <executive_summary>,
        "rating": <impact_severity_rating>,
        "rating_explanation": <rating_explanation>,
        "findings": [
            {
                "summary":<insight_1_summary>,
                "explanation": <insight_1_explanation>
            },
            {
                "summary":<insight_2_summary>,
                "explanation": <insight_2_explanation>
            }
        ]
    }

# Grounding Rules

Points supported by data should list their data references as follows:

"This is an example sentence supported by multiple data references [Data: <dataset name> (record ids); <dataset name> (record ids)]."

Do not list more than 5 record ids in a single reference. Instead, list the top 5 most relevant record ids and add "+more" to indicate that there are more.

For example:
"Person X is the owner of Company Y and subject to many allegations of wrongdoing [Data: Reports (1), Entities (5, 7); Relationships (23); Claims (7, 2, 34, 64, 46, +more)]."

where 1, 5, 7, 23, 2, 34, 46, and 64 represent the id (not the index) of the relevant data record.

Do not include information where the supporting evidence for it is not provided.

Output:"#;

/// Light graph entity extraction (LightRAG style; also extracts content keywords).
pub const ENTITY_EXTRACTION_PROMPT: &str = r#"---Goal---
Given a text document that is potentially relevant to this activity and a list of entity types, identify all entities of those types from the text and all relationships among the identified entities.
Use {language} as output language.

---Steps---
1. Identify all entities. For each identified entity, extract the following information:
- entity_name: Name of the entity, use same language as input text. If English, capitalized the name.
- entity_type: One of the following types: [{entity_types}]
- entity_description: Provide a comprehensive description of the entity's attributes and activities *based solely on the information present in the input text*. **Do not infer or hallucinate information not explicitly stated.** If the text provides insufficient information to create a comprehensive description, state "Description not available in text."
Format each entity as ("entity"{tuple_delimiter}<entity_name>{tuple_delimiter}<entity_type>{tuple_delimiter}<entity_description>)

2. From the entities identified in step 1, identify all pairs of (source_entity, target_entity) that are *clearly related* to each other.
For each pair of related entities, extract the following information:
- source_entity: name of the source entity, as identified in step 1
- target_entity: name of the target entity, as identified in step 1
- relationship_description: explanation as to why you think the source entity and the target entity are related to each other
- relationship_strength: a numeric score indicating strength of the relationship between the source entity and target entity
- relationship_keywords: one or more high-level key words that summarize the overarching nature of the relationship, focusing on concepts or themes rather than specific details
Format each relationship as ("relationship"{tuple_delimiter}<source_entity>{tuple_delimiter}<target_entity>{tuple_delimiter}<relationship_description>{tuple_delimiter}<relationship_keywords>{tuple_delimiter}<relationship_strength>)

3. Identify high-level key words that summarize the main concepts, themes, or topics of the entire text. These should capture the overarching ideas present in the document.
Format the content-level key words as ("content_keywords"{tuple_delimiter}<high_level_keywords>)

4. Return output in {language} as a single list of all the entities and relationships identified in steps 1 and 2. Use **{record_delimiter}** as the list delimiter.

5. When finished, output {completion_delimiter}

######################
---Examples---
######################
{examples}

#############################
---Real Data---
######################
Entity_types: [{entity_types}]
Text:
{input_text}
######################
Output:"#;

/// Light graph continue-extraction for entities/relationships missed previously.
pub const ENTITY_EXTRACTION_CONTINUE_PROMPT: &str = r#"
MANY entities and relationships were missed in the last extraction. Please find only the missing entities and relationships from previous text.

---Remember Steps---

1. Identify all entities. For each identified entity, extract the following information:
- entity_name: Name of the entity, use same language as input text. If English, capitalized the name
- entity_type: One of the following types: [{entity_types}]
- entity_description: Provide a comprehensive description of the entity's attributes and activities *based solely on the information present in the input text*. **Do not infer or hallucinate information not explicitly stated.** If the text provides insufficient information to create a comprehensive description, state "Description not available in text."
Format each entity as ("entity"{tuple_delimiter}<entity_name>{tuple_delimiter}<entity_type>{tuple_delimiter}<entity_description>)

2. From the entities identified in step 1, identify all pairs of (source_entity, target_entity) that are *clearly related* to each other.
For each pair of related entities, extract the following information:
- source_entity: name of the source entity, as identified in step 1
- target_entity: name of the target entity, as identified in step 1
- relationship_description: explanation as to why you think the source entity and the target entity are related to each other
- relationship_strength: a numeric score indicating strength of the relationship between the source entity and target entity
- relationship_keywords: one or more high-level key words that summarize the overarching nature of the relationship, focusing on concepts or themes rather than specific details
Format each relationship as ("relationship"{tuple_delimiter}<source_entity>{tuple_delimiter}<target_entity>{tuple_delimiter}<relationship_description>{tuple_delimiter}<relationship_keywords>{tuple_delimiter}<relationship_strength>)

3. Identify high-level key words that summarize the main concepts, themes, or topics of the entire text. These should capture the overarching ideas present in the document.
Format the content-level key words as ("content_keywords"{tuple_delimiter}<high_level_keywords>)

4. Return output in {language} as a single list of all the entities and relationships identified in steps 1 and 2. Use **{record_delimiter}** as the list delimiter.

5. When finished, output {completion_delimiter}

---Output---

Add new entities and relations below using the same format, and do not include entities and relations that have been previously extracted. :
"#;

/// Light graph loop check: answer ONLY by `YES` OR `NO`.
pub const ENTITY_IF_LOOP_EXTRACTION_PROMPT: &str = r#"
---Goal---

It appears some entities may have still been missed.

---Output---

Answer ONLY by `YES` OR `NO` if there are still entities that need to be added.
"#;

/// Light graph summarize entity descriptions (single comprehensive description).
pub const SUMMARIZE_ENTITY_DESCRIPTIONS_PROMPT: &str = r#"You are a helpful assistant responsible for generating a comprehensive summary of the data provided below.
Given one or two entities, and a list of descriptions, all related to the same entity or group of entities.
Please concatenate all of these into a single, comprehensive description. Make sure to include information collected from all the descriptions.
If the provided descriptions are contradictory, please resolve the contradictions and provide a single, coherent summary.
Make sure it is written in third person, and include the entity names so we the have full context.
Use {language} as output language.

#######
---Data---
Entities: {entity_name}
Description List: {description_list}
#######
Output:
"#;

/// Light graph keyword extraction (high-level + low-level keywords).
pub const KEYWORDS_EXTRACTION_PROMPT: &str = r#"---Role---
You are an expert keyword extractor, specializing in analyzing user queries for a Retrieval-Augmented Generation (RAG) system. Your purpose is to identify both high-level and low-level keywords in the user's query that will be used for effective document retrieval.

---Goal---
Given a user query, your task is to extract two distinct types of keywords:
1. **high_level_keywords**: for overarching concepts or themes, capturing user's core intent, the subject area, or the type of question being asked.
2. **low_level_keywords**: for specific entities or details, identifying the specific entities, proper nouns, technical jargon, product names, or concrete items.

---Instructions & Constraints---
1. **Output Format**: Your output MUST be a valid JSON object and nothing else. Do not include any explanatory text, markdown code fences (like ```json), or any other text before or after the JSON. It will be parsed directly by a JSON parser.
2. **Source of Truth**: All keywords must be explicitly derived from the user query, with both high-level and low-level keyword categories required to contain content.
3. **Concise & Meaningful**: Keywords should be concise words or meaningful phrases. Prioritize multi-word phrases when they represent a single concept. For example, from "latest financial report of Apple Inc.", you should extract "latest financial report" and "Apple Inc." rather than "latest", "financial", "report", and "Apple".
4. **Handle Edge Cases**: For queries that are too simple, vague, or nonsensical (e.g., "hello", "ok", "asdfghjkl"), you must return a JSON object with empty lists for both keyword types.

---Examples---
{examples}

---Real Data---
User Query: {query}

---Output---
"#;

/// Query-analyzer keyword extraction (query_analyze_prompt.py).
pub const QUERY_KEYWORDS_EXTRACTION_PROMPT: &str = r#"---Role---

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

/// MiniRAG query -> answer-type + entity keywords (query_analyze_prompt.py).
/// Python `{{`/`}}` escaped braces are unescaped to literal `{`/`}`.
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
Answer type pool: {
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
}
################
Output:
{
  "answer_type_keywords": ["STRATEGY","PERSONAL LIFE"],
  "entities_from_query": ["Trade agreements", "Tariffs", "Currency exchange", "Imports", "Exports"]
}
#############################
Example 2:

Query: "When was SpaceX's first rocket launch?"
Answer type pool: {
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
}

################
Output:
{
  "answer_type_keywords": ["DATE AND TIME", "ORGANIZATION", "PLAN"],
  "entities_from_query": ["SpaceX", "Rocket launch", "Aerospace", "Power Recovery"]

}
#############################
Example 3:

Query: "What is the role of education in reducing poverty?"
Answer type pool: {
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
}

################
Output:
{
  "answer_type_keywords": ["STRATEGY", "PERSON"],
  "entities_from_query": ["School access", "Literacy rates", "Job training", "Income inequality"]
}
#############################
Example 4:

Query: "Where is the capital of the United States?"
Answer type pool: {
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
}
################
Output:
{
  "answer_type_keywords": ["LOCATION"],
  "entities_from_query": ["capital of the United States", "Washington", "New York"]
}
#############################

-Real Data-
######################
Query: {query}
Answer type pool:{TYPE_POOL}
######################
Output:

"#;

/// Entity resolution: decide whether two entities are the same.
pub const ENTITY_RESOLUTION_PROMPT: &str = r#"
-Goal-
Please answer the following Question as required

-Steps-
1. Identify each line of questioning as required

2. Return output in English as a single list of each line answer in steps 1. Use **{record_delimiter}** as the list delimiter.

######################
-Examples-
######################
Example 1:

Question:
When determining whether two Products are the same, you should only focus on critical properties and overlook noisy factors. 

Demonstration 1: name of Product A is : "computer", name of Product B is :"phone"  No, Product A and Product B are different products.
Question 1: name of Product A is : "television", name of Product B is :"TV"  
Question 2: name of Product A is : "cup", name of Product B is :"mug"  
Question 3: name of Product A is : "soccer", name of Product B is :"football"  
Question 4: name of Product A is : "pen", name of Product B is  :"eraser"  

Use domain knowledge of Products to help understand the text and answer the above 4 questions in the format: For Question i, Yes, Product A and Product B are the same product. or  No, Product A and Product B are different products. For Question i+1, (repeat the above procedures)
################
Output:
(For question {entity_index_delimiter}1{entity_index_delimiter}, {resolution_result_delimiter}no{resolution_result_delimiter}, Product A and Product B are different products.){record_delimiter}
(For question {entity_index_delimiter}2{entity_index_delimiter}, {resolution_result_delimiter}no{resolution_result_delimiter}, Product A and Product B are different products.){record_delimiter}
(For question {entity_index_delimiter}3{entity_index_delimiter}, {resolution_result_delimiter}yes{resolution_result_delimiter}, Product A and Product B are the same product.){record_delimiter}
(For question {entity_index_delimiter}4{entity_index_delimiter}, {resolution_result_delimiter}no{resolution_result_delimiter}, Product A and Product B are different products.){record_delimiter}
#############################

Example 2:

Question:
When determining whether two toponym are the same, you should only focus on critical properties and overlook noisy factors. 

Demonstration 1: name of toponym A is : "nanjing", name of toponym B is :"nanjing city"  Yes, toponym A and toponym B are same toponym.
Question 1: name of toponym A is : "Chicago", name of toponym B is :"ChiTown"  
Question 2: name of toponym A is : "Shanghai", name of toponym B is :"Zhengzhou"  
Question 3: name of toponym A is : "Beijing", name of toponym B is :"Peking"
Question 4: name of toponym A is : "Los Angeles", name of toponym B is :"Cleveland" 

Use domain knowledge of toponym to help understand the text and answer the above 4 questions in the format: For Question i, Yes, toponym A and toponym B are the same toponym. or  No, toponym A and toponym B are different toponym. For Question i+1, (repeat the above procedures)
################
Output:
(For question {entity_index_delimiter}1{entity_index_delimiter}, {resolution_result_delimiter}yes{resolution_result_delimiter}, toponym A and toponym B are same toponym.){record_delimiter}
(For question {entity_index_delimiter}2{entity_index_delimiter}, {resolution_result_delimiter}no{resolution_result_delimiter}, toponym A and toponym B are different toponym.){record_delimiter}
(For question {entity_index_delimiter}3{entity_index_delimiter}, {resolution_result_delimiter}yes{resolution_result_delimiter}, toponym A and toponym B are the same toponym.){record_delimiter}
(For question {entity_index_delimiter}4{entity_index_delimiter}, {resolution_result_delimiter}no{resolution_result_delimiter}, toponym A and toponym B are different toponym.){record_delimiter}
#############################

-Real Data-
######################
Question:{input_text}
######################
Output:
"#;

/// Mind-map extraction: summarize text into a markdown mind map (>= 4 levels).
pub const MIND_MAP_EXTRACTION_PROMPT: &str = r#"
- Role: You're a talent text processor to summarize a piece of text into a mind map.

- Step of task:
  1. Generate a title for user's 'TEXT'。
  2. Classify the 'TEXT' into sections of a mind map.
  3. If the subject matter is really complex, split them into sub-sections and sub-subsections. 
  4. Add a shot content summary of the bottom level section.

- Output requirement:
  - Generate at least 4 levels.
  - Always try to maximize the number of sub-sections. 
  - In language of 'Text'
  - MUST IN FORMAT OF MARKDOWN

-TEXT-
{input_text}

"#;

/// Light graph RAG response: answer from Knowledge Graph + Document Chunks with citations.
pub const LIGHTRAG_RESPONSE_PROMPT: &str = r#"---Role---

You are a helpful assistant responding to user query about Knowledge Graph and Document Chunks provided in JSON format below.


---Goal---

Generate a concise response based on Knowledge Base and follow Response Rules, considering both current query and the conversation history if provided. Summarize all information in the provided Knowledge Base, and incorporating general knowledge relevant to the Knowledge Base. Do not include information not provided by Knowledge Base.

---Conversation History---
{history}

---Knowledge Graph and Document Chunks---
{context_data}

---RESPONSE GUIDELINES---
**1. Content & Adherence:**
- Strictly adhere to the provided context from the Knowledge Base. Do not invent, assume, or include any information not present in the source data.
- If the answer cannot be found in the provided context, state that you do not have enough information to answer.
- Ensure the response maintains continuity with the conversation history.

**2. Formatting & Language:**
- Format the response using markdown with appropriate section headings.
- The response language must in the same language as the user's question.
- Target format and length: {response_type}

**3. Citations / References:**
- At the end of the response, under a "References" section, each citation must clearly indicate its origin (KG or DC).
- The maximum number of citations is 5, including both KG and DC.
- Use the following formats for citations:
  - For a Knowledge Graph Entity: `[KG] <entity_name>`
  - For a Knowledge Graph Relationship: `[KG] <entity1_name> - <entity2_name>`
  - For a Document Chunk: `[DC] <file_path_or_document_name>`

---USER CONTEXT---
- Additional user prompt: {user_prompt}


Response:"#;

/// Naive (chunk-only) RAG response with DC citations.
pub const NAIVE_RAG_RESPONSE_PROMPT: &str = r#"---Role---

You are a helpful assistant responding to user query about Document Chunks provided provided in JSON format below.

---Goal---

Generate a concise response based on Document Chunks and follow Response Rules, considering both the conversation history and the current query. Summarize all information in the provided Document Chunks, and incorporating general knowledge relevant to the Document Chunks. Do not include information not provided by Document Chunks.

---Conversation History---
{history}

---Document Chunks(DC)---
{content_data}

---RESPONSE GUIDELINES---
**1. Content & Adherence:**
- Strictly adhere to the provided context from the Knowledge Base. Do not invent, assume, or include any information not present in the source data.
- If the answer cannot be found in the provided context, state that you do not have enough information to answer.
- Ensure the response maintains continuity with the conversation history.

**2. Formatting & Language:**
- Format the response using markdown with appropriate section headings.
- The response language must match the user's question language.
- Target format and length: {response_type}

**3. Citations / References:**
- At the end of the response, under a "References" section, cite a maximum of 5 most relevant sources used.
- Use the following formats for citations: `[DC] <file_path_or_document_name>`

---USER CONTEXT---
- Additional user prompt: {user_prompt}


Response:"#;

/// Question proposal (rag/prompts/question_prompt.md): propose top-N questions about a text.
pub const QUESTION_PROPOSAL_PROMPT: &str = r#"## Role
You are a text analyzer.

## Task
Propose {topn} questions about a given piece of text content.

## Requirements
- Understand and summarize the text content, and propose the top {topn} important questions.
- The questions SHOULD NOT have overlapping meanings.
- The questions SHOULD cover the main content of the text as much as possible.
- The questions MUST be in the same language as the given piece of text content.
- One question per line.
- Output questions ONLY.

---

## Text Content
{content}
"#;

/// Keyword extraction (rag/prompts/keyword_prompt.md): top-N keywords of a text.
pub const KEYWORD_EXTRACTION_PROMPT: &str = r#"## Role
You are a text analyzer.

## Task
Extract the most important keywords/phrases of a given piece of text content.

## Requirements
- Summarize the text content, and give the top {topn} important keywords/phrases.
- The keywords MUST be in the same language as the given piece of text content.
- The keywords are delimited by ENGLISH COMMA.
- Output keywords ONLY.

---

## Text Content
{content}
"#;

// ── LightRAG default constants (rag/graphrag/light/graph_prompt.py) ──

/// `PROMPTS["DEFAULT_LANGUAGE"]` — default output language for LightRAG prompts.
pub const LIGHTRAG_DEFAULT_LANGUAGE: &str = "English";
/// `PROMPTS["DEFAULT_TUPLE_DELIMITER"]` — tuple field delimiter.
pub const LIGHTRAG_DEFAULT_TUPLE_DELIMITER: &str = "<|>";
/// `PROMPTS["DEFAULT_RECORD_DELIMITER"]` — record delimiter.
pub const LIGHTRAG_DEFAULT_RECORD_DELIMITER: &str = "##";
/// `PROMPTS["DEFAULT_COMPLETION_DELIMITER"]` — extraction completion marker.
pub const LIGHTRAG_DEFAULT_COMPLETION_DELIMITER: &str = "<|COMPLETE|>";
/// `PROMPTS["DEFAULT_ENTITY_TYPES"]` — default entity type vocabulary.
pub const LIGHTRAG_DEFAULT_ENTITY_TYPES: [&str; 5] =
    ["organization", "person", "geo", "event", "category"];
/// `PROMPTS["DEFAULT_USER_PROMPT"]` — default user prompt placeholder.
pub const LIGHTRAG_DEFAULT_USER_PROMPT: &str = "n/a";
/// `PROMPTS["fail_response"]` — fallback answer when no context is retrieved.
pub const LIGHTRAG_FAIL_RESPONSE: &str =
    "Sorry, I'm not able to provide an answer to that question.[no-context]";

// ── RAGFlow QA-stage prompts (verbatim ports of rag/prompts/*.md) ──
// Covers the RAG question-answering / citation / reflection pipeline:
// citation, question rewriting, multi-query generation, reflection,
// content tagging, cross-language translation, task analysis, TOC levels,
// keyword/question extraction, metadata extraction & filtering.

/// Verbatim port of RAGFlow `rag/prompts/citation_prompt.md`.
/// Detailed [ID:i] citation rules for RAG answers (quantitative data, temporal claims, RTL handling).
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_CITATION_PROMPT: &str = r#"Based on the provided document or chat history, add citations to the input text using the format specified later. 

# Citation Requirements:

## Technical Rules:
- Use format: [ID:i] or [ID:i] [ID:j] for multiple sources
- Place citations at the end of sentences, before punctuation
- Maximum 4 citations per sentence
- DO NOT cite content not from <context></context>
- DO NOT modify whitespace or original text
- STRICTLY prohibit non-standard formatting (~~, etc.)
- For RTL languages (Arabic, Hebrew, Persian): Place citations at the logical end of sentences (same position as LTR). The frontend handles bidirectional rendering automatically.

## What MUST Be Cited:
1. **Quantitative data**: Numbers, percentages, statistics, measurements
2. **Temporal claims**: Dates, timeframes, sequences of events  
3. **Causal relationships**: Claims about cause and effect
4. **Comparative statements**: Rankings, comparisons, superlatives
5. **Technical definitions**: Specialized terms, concepts, methodologies
6. **Direct attributions**: What someone said, did, or believes
7. **Predictions/forecasts**: Future projections, trend analyses
8. **Controversial claims**: Disputed facts, minority opinions

## What Should NOT Be Cited:
- Common knowledge (e.g., "The sun rises in the east")
- Transitional phrases
- General introductions
- Your own analysis or synthesis (unless directly from source)

# Comprehensive Examples:

## Example 1: Data and Statistics
<context>
ID: 45
└── Content: The global smartphone market grew by 7.8% in Q3 2024, with Samsung holding 20.6% market share and Apple at 15.8%.

ID: 46  
└── Content: 5G adoption reached 1.5 billion users worldwide by October 2024.
</context>

USER: How is the smartphone market performing?

ASSISTANT: 
The smartphone industry is showing strong recovery. The global smartphone market grew by 7.8% in Q3 2024 [ID:45]. Samsung leads with 20.6% market share, followed by Apple at 15.8% [ID:45]. This growth is partly driven by 5G technology, which reached 1.5 billion users worldwide by October 2024 [ID:46].

## Example 2: Causal Relationships and Predictions
<context>
ID: 78
└── Content: Rising interest rates have led to a 15% decline in housing starts. Economists predict this trend will continue through 2025.

ID: 79
└── Content: The Federal Reserve's aggressive monetary policy is the primary driver behind the real estate slowdown.
</context>

USER: What's happening in the real estate market?

ASSISTANT:
The real estate market is experiencing significant challenges. Rising interest rates have led to a 15% decline in housing starts [ID:78]. The Federal Reserve's aggressive monetary policy is the primary driver behind this real estate slowdown [ID:79]. Economists predict this trend will continue through 2025 [ID:78]. These factors suggest a prolonged period of market adjustment ahead.

## Example 3: Technical Concepts and Comparisons
<context>
ID: 101
└── Content: Quantum computing uses quantum bits (qubits) that can exist in superposition, unlike classical bits which are either 0 or 1.

ID: 102
└── Content: Current quantum computers can solve certain problems 158 million times faster than classical supercomputers.
</context>

USER: Explain quantum computing advantages.

ASSISTANT:
Quantum computing represents a fundamental shift in computational capability. Unlike classical computers, quantum computing uses quantum bits (qubits) that can exist in superposition, rather than being limited to just 0 or 1 [ID:101]. This unique property enables extraordinary performance gains. Current quantum computers can solve certain problems 158 million times faster than classical supercomputers [ID:102]. This technology promises to revolutionize fields requiring complex calculations.

## Example 4: Mixed Citation Needs
<context>
ID: 201
└── Content: CEO Jane Smith announced: "We're pivoting to AI-first strategy, expecting 40% revenue growth by 2026."

ID: 202
└── Content: Industry analysts remain skeptical, with Morgan Stanley rating the stock as "underperform."

ID: 203
└── Content: The company's AI division currently represents only 5% of total revenue.
</context>

USER: What's the company's new strategy?

ASSISTANT:
The company is undergoing a major strategic transformation. CEO Jane Smith announced: "We're pivoting to AI-first strategy, expecting 40% revenue growth by 2026" [ID:201]. However, this ambitious goal faces skepticism, as industry analysts remain doubtful, with Morgan Stanley rating the stock as "underperform" [ID:202]. The challenge is significant given that the company's AI division currently represents only 5% of total revenue [ID:203]. Success will depend on execution of this bold vision.

## Example 5: When NOT to Cite
<context>
ID: 301
└── Content: Paris is the capital of France. The Eiffel Tower was completed in 1889.
</context>

USER: Tell me about Paris.

ASSISTANT:
Paris is the capital of France. It's known for its rich history, culture, and architecture. The Eiffel Tower was completed in 1889 [ID:301]. The city attracts millions of tourists annually. Paris remains one of the world's most visited destinations.
(Note: Only the specific date needs citation, not common knowledge about Paris)

## Example 6: RTL Language (Arabic)
<context>
ID: 401
└── Content: في أول أيام شهر رمضان، أثار وضع رأس خنزير على مدخل مسجد بمدينة سانت أومير شمالي فرنسا تفاعلات واسعة.
</context>

USER: ماذا حدث في رمضان؟

ASSISTANT:
في أول أيام شهر رمضان، أثار وضع رأس خنزير على مدخل مسجد بمدينة سانت أومير شمالي فرنسا تفاعلات واسعة [ID:401].
(Note: Citation is placed at the logical end of the sentence, same as LTR languages. The frontend handles RTL display automatically.)

--- Examples END ---

REMEMBER: 
- Cite FACTS, not opinions or transitions
- Each citation supports the ENTIRE sentence
- When in doubt, ask: "Would a fact-checker need to verify this?"
- Place citations at sentence end, before punctuation
- Format likes this is FORBIDDEN: [ID:0, ID:5, ID:...]. It MUST be separated like, [ID:0][ID:5]..."#;

/// Verbatim port of RAGFlow `rag/prompts/citation_plus.md`.
/// Add citations to an already-generated report; {{ example }}/{{ sources }} -> {example}/{sources}.
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_CITATION_PLUS_PROMPT: &str = r#"You are an agent for adding correct citations to the given text by user. 
You are given a piece of text within [ID:<ID>] tags, which was generated based on the provided sources. 
However, the sources are not cited in the [ID:<ID>]. 
Your task is to enhance user trust by generating correct, appropriate citations for this report.

{example}

<context>

{sources}

</context>"#;

/// Verbatim port of RAGFlow `rag/prompts/full_question_prompt.md`.
/// Rewrite the latest turn into a standalone full question; relative dates -> absolute; Jinja if/else flattened.
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_FULL_QUESTION_PROMPT: &str = r#"## Role
A helpful assistant.

## Task & Steps
1. Generate a full user question that would follow the conversation.
2. If the user's question involves relative dates, convert them into absolute dates based on today ({today}).
   - "yesterday" = {yesterday}, "tomorrow" = {tomorrow}

## Requirements & Restrictions
- If the user's latest question is already complete, don't do anything — just return the original question.
- DON'T generate anything except a refined question.
- Text generated MUST be in {language}. If no language is specified, use the same language as the original user's question.

---

## Examples

### Example 1
**Conversation:**

USER: What is the name of Donald Trump's father?
ASSISTANT: Fred Trump.
USER: And his mother?

**Output:** What's the name of Donald Trump's mother?

---

### Example 2
**Conversation:**

USER: What is the name of Donald Trump's father?
ASSISTANT: Fred Trump.
USER: And his mother?
ASSISTANT: Mary Trump.
USER: What's her full name?

**Output:** What's the full name of Donald Trump's mother Mary Trump?

---

### Example 3
**Conversation:**

USER: What's the weather today in London?
ASSISTANT: Cloudy.
USER: What's about tomorrow in Rochester?

**Output:** What's the weather in Rochester on {tomorrow}?

---

## Real Data

**Conversation:**

{conversation}"#;

/// Verbatim port of RAGFlow `rag/prompts/multi_queries_gen.md`.
/// Generate 2-3 complementary queries when retrieval was insufficient.
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_MULTI_QUERIES_GEN_PROMPT: &str = r#"You are a query optimization expert. 
The user's original query failed to retrieve sufficient information; 
please generate multiple complementary improved questions and corresponding queries.

Original query:
{original_query}

Original question:
{original_question}

Currently, retrieved content:
{retrieved_docs}

Missing information:
{missing_info}

Please generate 2-3 complementary queries to help find the missing information. These queries should:
1. Focus on different missing information points.
2. Use different expressions.
3. Avoid being identical to the original query.
4. Remain concise and clear.

Output format (JSON):
```json
{
    "reasoning": "Explanation of query generation strategy",
    "questions": [
        {"question": "Improved question 1", "query": "Improved query 1"},
        {"question": "Improved question 2", "query": "Improved query 2"},
        {"question": "Improved question 3", "query": "Improved query 3"}
    ]
}
```

Requirements:
1. Questions array contains 1-3 questions and corresponding queries.
2. Each question length is between 5-200 characters.
3. Each query length is between 1-5 keywords.
4. Each query MUST be in the same language as the retrieved content in. 
5. DO NOT generate question and query that is similar to the original query. 
6. Reasoning explains the generation strategy."#;

/// Verbatim port of RAGFlow `rag/prompts/next_step.md`.
/// Planning agent: pick tools / complete_task; private <think> reflection.
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_NEXT_STEP_PROMPT: &str = r#"You are an expert Planning Agent tasked with solving problems efficiently through structured plans.
Your job is:
1. Based on the task analysis, chose some right tools to execute.
2. Track progress and adapt plans(tool calls) when necessary.
3. Use `complete_task` if no further step you need to take from tools. (All necessary steps done or little hope to be done)

# ========== TASK ANALYSIS =============
{task_analysis}

# ==========  TOOLS (JSON-Schema) ==========
You may invoke only the tools listed below.
Return a JSON array of objects in which item is with exactly two top-level keys:
• "name": the tool to call
• "arguments": an object whose keys/values satisfy the schema

{desc}


# ==========  MULTI-STEP EXECUTION ==========
When tasks require multiple independent steps, you can execute them in parallel by returning multiple tool calls in a single JSON array.

• **Data Collection**: Gathering information from multiple sources simultaneously
• **Validation**: Cross-checking facts using different tools
• **Comprehensive Analysis**: Analyzing different aspects of the same problem
• **Efficiency**: Reducing total execution time when steps don't depend on each other

**Example Scenarios:**
- Searching multiple databases for the same query
- Checking weather in multiple cities
- Validating information through different APIs
- Performing calculations on different datasets
- Gathering user preferences from multiple sources

# ==========  RESPONSE FORMAT ==========
**When you need a tool**  
Return ONLY the Json (no additional keys, no commentary, end with `<|stop|>`), such as following:
[{
  "name": "<tool_name1>",
  "arguments": { /* tool arguments matching its schema */ }
},{
  "name": "<tool_name2>",
  "arguments": { /* tool arguments matching its schema */ }
}...]<|stop|>

**When you need multiple tools:**
Return ONLY:
[{
  "name": "<tool_name1>",
  "arguments": { /* tool arguments matching its schema */ }
},{
  "name": "<tool_name2>",
  "arguments": { /* tool arguments matching its schema */ }
},{
  "name": "<tool_name3>",
  "arguments": { /* tool arguments matching its schema */ }
}...]<|stop|>

**When you are certain the task is solved OR no further information can be obtained**  
Return ONLY:
[{
  "name": "complete_task",
  "arguments": { "answer": "<final answer text>" }
}]<|stop|>

<verification_steps>
Before providing a final answer:
1. Double-check all gathered information
2. Verify calculations and logic
3. Ensure answer matches exactly what was asked
4. Confirm answer format meets requirements
5. Run additional verification if confidence is not 100%
</verification_steps>

<error_handling>
If you encounter issues:
1. Try alternative approaches before giving up
2. Use different tools or combinations of tools
3. Break complex problems into simpler sub-tasks
4. Verify intermediate results frequently
5. Never return "I cannot answer" without exhausting all options
</error_handling>

⚠️ Any output that is not valid JSON or that contains extra fields will be rejected.

# ========== PRIVATE REASONING & REFLECTION ==========
You may think privately inside `<think>` tags.
This content will NOT be shown to the user.

## Step 1: Core Reasoning
- Analyze the task requirements
- Decide whether tools are required
- Decide if parallel execution is appropriate

## Step 2: Structured Reflection (MANDATORY before `complete_task`)

### Context
- Goal: Reflect on the current task based on the full conversation context
- Executed tool calls so far (if any): reflect from conversation history

### Task Complexity Assessment
Evaluate the task along these dimensions:

- Scope Breadth: Single-step (1) | Multi-step (2) | Multi-domain (3)
- Data Dependency: Self-contained (1) | External inputs (2) | Multiple sources (3)
- Decision Points: Linear (1) | Few branches (2) | Complex logic (3)
- Risk Level: Low (1) | Medium (2) | High (3)

Compute the **Complexity Score (4–12)**.

### Reflection Depth Control
- 4–5: Brief sanity check
- 6–8: Check completeness + risks
- 9–12: Full reflection with alternatives

### Reflection Checklist
- Goal alignment: Is the objective truly satisfied?
- Step completion: Any planned step missing?
- Information adequacy: Is evidence sufficient?
- Errors or uncertainty: Any low-confidence result?
- Tool misuse risk: Wrong tool / missing tool?

### Decision Gate
Ask yourself explicitly:
> “If I stop now and call `complete_task`, would a downstream agent or user reasonably say something is missing or wrong?”

If YES → continue with tools
If NO → safe to call `complete_task`

---

# ========== FINAL ACTION ==========
After reflection, emit ONLY ONE of the following:
- A JSON array of tool calls
- OR a single `complete_task` call


Today is {today}. Remember that success in answering questions accurately is paramount - take all necessary steps to ensure your answer is correct."#;

/// Verbatim port of RAGFlow `rag/prompts/reflect.md`.
/// Post-tool-call reflection with complexity scoring; tool_calls loop flattened to {tool_calls}.
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_REFLECT_PROMPT: &str = r#"**Context**:
 - To achieve the goal: {goal}.
 - You have executed following tool calls:
<!-- RAGFlow renders one block per item in `tool_calls`; per-item template:
Tool call: `{ call.name }`
Results: { call.result }
-->
{tool_calls}

## Task Complexity Analysis & Reflection Scope

**First, analyze the task complexity using these dimensions:**

### Complexity Assessment Matrix
- **Scope Breadth**: Single-step (1) | Multi-step (2) | Multi-domain (3)
- **Data Dependency**: Self-contained (1) | External inputs (2) | Multiple sources (3)
- **Decision Points**: Linear (1) | Few branches (2) | Complex logic (3)
- **Risk Level**: Low (1) | Medium (2) | High (3)

**Complexity Score**: Sum all dimensions (4-12 points)

---

##  Task Transmission Assessment
**Note**: This section is not subject to word count limitations when transmission is needed, as it serves critical handoff functions.
**Evaluate if task transmission information is needed:**
- **Is this an initial step?** If yes, skip this section
- **Are there downstream agents/steps?** If no, provide minimal transmission
- **Is there critical state/context to preserve?** If yes, include full transmission

### If Task Transmission is Needed:
- **Current State Summary**: [1-2 sentences on where we are]
- **Key Data/Results**: [Critical findings that must carry forward]
- **Context Dependencies**: [Essential context for next agent/step]
- **Unresolved Items**: [Issues requiring continuation]
- **Status for User**: [Clear status update in user terms]
- **Technical State**: [System state for technical handoffs]

---

##  Situational Reflection (Adjust Length Based on Complexity Score)

### Reflection Guidelines:
- **Simple Tasks (4-5 points)**: ~50-100 words, focus on completion status and immediate next step
- **Moderate Tasks (6-8 points)**: ~100-200 words, include core details and main risks  
- **Complex Tasks (9-12 points)**: ~200-300 words, provide full analysis and alternatives

### 1. Goal Achievement Status
 - Does the current outcome align with the original purpose of this task phase? 
 - If not, what critical gaps exist?

### 2. Step Completion Check
 - Which planned steps were completed? (List verified items)
 - Which steps are pending/incomplete? (Specify exactly what's missing)

### 3. Information Adequacy
 - Is the collected data sufficient to proceed?
 - What key information is still needed? (e.g., metrics, user input, external data)

### 4. Critical Observations
 - Unexpected outcomes: [Flag anomalies/errors]
 - Risks/blockers: [Identify immediate obstacles]
 - Accuracy concerns: [Highlight unreliable results]

### 5. Next-Step Recommendations
 - Proposed immediate action: [Concrete next step]
 - Alternative strategies if blocked: [Workaround solution]
 - Tools/inputs required for next phase: [Specify resources]

---

**Output Instructions:**
1. First determine your complexity score
2. Assess if task transmission section is needed using the evaluation questions
3. Provide situational reflection with length appropriate to complexity
4. Use clear headers for easy parsing by downstream systems"#;

/// Verbatim port of RAGFlow `rag/prompts/rank_memory.md`.
/// Rank tool results by relevance to goal/sub-goal; results loop flattened to {results}.
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_RANK_MEMORY_PROMPT: &str = r#"**Task**: Sort the tool call results based on relevance to the overall goal and current sub-goal. Return ONLY a sorted list of indices (0-indexed).

**Rules**:
1. Analyze each result's contribution to both:
   - The overall goal (primary priority)
   - The current sub-goal (secondary priority)
2. Sort from MOST relevant (highest impact) to LEAST relevant
3. Output format: Strictly a Python-style list of integers. Example: [2, 0, 1]

🔹 Overall Goal: {goal}
🔹 Sub-goal: {sub_goal}

**Examples**:  
🔹 Tool Response:  
 - index: 0
     > Tokyo temperature is 78°F.
 - index: 1
     > Error: Authentication failed (expired API key).
 - index: 2
     > Available: 12 widgets in stock (max 5 per customer).
 
 → rank: [1,2,0]<|stop|>
 

**Your Turn**:  
🔹 Tool Response:
<!-- RAGFlow renders one block per item in `results`; per-item template:
- index: f.i
     > f.content
-->
{results}"#;

/// Verbatim port of RAGFlow `rag/prompts/content_tagging_prompt.md`.
/// Tag text with top-N tags from a tag set; examples loop flattened to {examples}.
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_CONTENT_TAGGING_PROMPT: &str = r#"## Role
You are a text analyzer.

## Task
Add tags (labels) to a given piece of text content based on the examples and the entire tag set.

## Steps
- Review the tag/label set.
- Review examples which all consist of both text content and assigned tags with relevance score in JSON format.
- Summarize the text content, and tag it with the top {topn} most relevant tags from the set of tags/labels and the corresponding relevance score.

## Requirements
- The tags MUST be from the tag set.
- The output MUST be in JSON format only, the key is tag and the value is its relevance score.
- The relevance score must range from 1 to 10.
- Output keywords ONLY.

# TAG SET
{all_tags}

<!-- RAGFlow renders one block per item in `examples`; per-item template:
# Examples { loop.index0 }
### Text Content
{ ex.content }

Output:
{ ex.tags_json }
-->
{examples}
# Real Data
### Text Content
{content}"#;

/// Verbatim port of RAGFlow `rag/prompts/cross_languages_sys_prompt.md`.
/// Multilingual batch translator system prompt (keeps zh/fr/ja example).
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_CROSS_LANGUAGES_SYS_PROMPT: &str = r#"## Role
A streamlined multilingual translator.

## Behavior Rules
1. Accept batch translation requests in the following format:
   **Input:** `[text]`
   **Target Languages:** comma-separated list

2. Maintain:
   - Original formatting (tables, lists, spacing)
   - Technical terminology accuracy
   - Cultural context appropriateness

3. Output translations in the following format:

[Translation in language1]
###
[Translation in language2]

---

## Example

**Input:**
Hello World! Let's discuss AI safety.
===
Chinese, French, Japanese

**Output:**
你好世界！让我们讨论人工智能安全问题。
###
Bonjour le monde ! Parlons de la sécurité de l'IA.
###
こんにちは世界！AIの安全性について話し合いましょう。"#;

/// Verbatim port of RAGFlow `rag/prompts/cross_languages_user_prompt.md`.
/// Batch translation user prompt; {{ languages | join(', ') }} -> {languages}.
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_CROSS_LANGUAGES_USER_PROMPT: &str = r#"**Input:**
{query}
===
{languages}

**Output:**"#;

/// Verbatim port of RAGFlow `rag/prompts/analyze_task_system.md`.
/// Task analyzer with LOW/MEDIUM/HIGH adaptive depth.
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_ANALYZE_TASK_SYSTEM_PROMPT: &str = r#"You are an intelligent task analyzer that adapts analysis depth to task complexity.

**Analysis Framework**

**Step 1: Task Transmission Assessment**
**Note**: This section is not subject to word count limitations when transmission is needed, as it serves critical handoff functions.

**Evaluate if task transmission information is needed:**
- **Is this an initial step?** If yes, skip this section
- **Are there upstream agents/steps?** If no, provide minimal transmission
- **Is there critical state/context to preserve?** If yes, include full transmission

### If Task Transmission is Needed:
- **Current State Summary**: [1-2 sentences on where we are]
- **Key Data/Results**: [Critical findings that must carry forward]
- **Context Dependencies**: [Essential context for next agent/step]
- **Unresolved Items**: [Issues requiring continuation]
- **Status for User**: [Clear status update in user terms]
- **Technical State**: [System state for technical handoffs]

**Step 2: Complexity Classification**
Classify as LOW / MEDIUM / HIGH:
- **LOW**: Single-step tasks, direct queries, small talk
- **MEDIUM**: Multi-step tasks within one domain
- **HIGH**: Multi-domain coordination or complex reasoning

**Step 3: Adaptive Analysis**
Scale depth to match complexity. Always stop once success criteria are met.

**For LOW (max 50 words for analysis only):**
- Detect small talk; if true, output exactly: `Small talk — no further analysis needed`
- One-sentence objective
- Direct execution approach (1–2 steps)

**For MEDIUM (80–150 words for analysis only):**
- Objective; Intent & Scope
- 3–5 step minimal Plan (may mark parallel steps)
- **Uncertainty & Probes** (at least one probe with a clear stop condition)
- Success Criteria + basic Failure detection & fallback
- **Source Plan** (how evidence will be obtained/verified)

**For HIGH (150–250 words for analysis only):**
- Comprehensive objective analysis; Intent & Scope
- 5–8 steps Plan with dependencies/parallelism
- **Uncertainty & Probes** (key unknowns → probe → stop condition)
- Measurable Success Criteria; Failure detectors & fallbacks
- **Source Plan** (evidence acquisition & validation)
- **Reflection Hooks** (escalation/de-escalation triggers)"#;

/// Verbatim port of RAGFlow `rag/prompts/analyze_task_user.md`.
/// Task analyzer user variables: task/context/agent_prompt/tools_desc.
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_ANALYZE_TASK_USER_PROMPT: &str = r#"**Input Variables**
- **{task}** — the task/request to analyze
- **{context}** — background, history, situational context
- **{agent_prompt}** — special instructions/role hints
- **{tools_desc}** — available sub-agents and capabilities

**Final Output Rule**
Return the Task Transmission section (if needed) followed by the concrete analysis and planning steps according to LOW / MEDIUM / HIGH complexity.  
Do not restate the framework, definitions, or rules. Output only the final structured result."#;

/// Verbatim port of RAGFlow `rag/prompts/ask_summary.md`.
/// Miss R knowledge-base QA summary prompt (no-hallucination rules).
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_ASK_SUMMARY_PROMPT: &str = r#"Role: You're a smart assistant. Your name is Miss R.
Task: Summarize the information from knowledge bases and answer user's question.
Requirements and restriction:
  - DO NOT make things up, especially for numbers.
  - If the information from knowledge is irrelevant with user's question, JUST SAY: Sorry, no relevant information provided.
  - Answer with markdown format text.
  - Answer in language of user's question.
  - DO NOT make things up, especially for numbers.

### Information from knowledge bases

{knowledge}

The above is information from knowledge bases."#;

/// Verbatim port of RAGFlow `rag/prompts/assign_toc_levels.md`.
/// Assign Arabic-numeral depth levels to TOC items (JSON in/out).
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_ASSIGN_TOC_LEVELS_PROMPT: &str = r#"You are given a JSON array of TOC(table of contents) items. Each item has at least {"title": string} and may include an existing title hierarchical level.

Task
- For each item, assign a depth label using Arabic numerals only: top-level = 1, second-level = 2, third-level = 3, etc.
- Multiple items may share the same depth (e.g., many 1s, many 2s).
- Do not use dotted numbering (no 1.1/1.2). Use a single digit string per item indicating its depth only.
- Preserve the original item order exactly. Do not insert, delete, or reorder.
- Decide levels yourself to keep a coherent hierarchy. Keep peers at the same depth.

Output
- Return a valid JSON array only (no extra text).
- Each element must be {"level": "1|2|3", "title": <original title string>}.
- title must be the original title string.

Examples

Example A (chapters with sections)
Input:
["Chapter 1 Methods", "Section 1 Definition", "Section 2 Process", "Chapter 2 Experiment"]

Output:
[
  {"level":"1","title":"Chapter 1 Methods"},
  {"level":"2","title":"Section 1 Definition"},
  {"level":"2","title":"Section 2 Process"},
  {"level":"1","title":"Chapter 2 Experiment"}
]

Example B (parts with chapters)
Input:
["Part I Theory", "Chapter 1 Basics", "Chapter 2 Methods", "Part II Applications", "Chapter 3 Case Studies"]

Output:
[
  {"level":"1","title":"Part I Theory"},
  {"level":"2","title":"Chapter 1 Basics"},
  {"level":"2","title":"Chapter 2 Methods"},
  {"level":"1","title":"Part II Applications"},
  {"level":"2","title":"Chapter 3 Case Studies"}
]

Example C (plain headings)
Input:
["Introduction", "Background and Motivation", "Related Work", "Methodology", "Evaluation"]

Output:
[
  {"level":"1","title":"Introduction"},
  {"level":"2","title":"Background and Motivation"},
  {"level":"2","title":"Related Work"},
  {"level":"1","title":"Methodology"},
  {"level":"1","title":"Evaluation"}
]"#;

/// Verbatim port of RAGFlow `rag/prompts/keyword_prompt.md`.
/// Extract top-N keywords of a text (RAGFlow original).
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_KEYWORD_PROMPT: &str = r#"## Role
You are a text analyzer.

## Task
Extract the most important keywords/phrases of a given piece of text content.

## Requirements
- Summarize the text content, and give the top {topn} important keywords/phrases.
- The keywords MUST be in the same language as the given piece of text content.
- The keywords are delimited by ENGLISH COMMA.
- Output keywords ONLY.

---

## Text Content
{content}"#;

/// Verbatim port of RAGFlow `rag/prompts/question_prompt.md`.
/// Propose top-N questions about a text (RAGFlow original).
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_QUESTION_PROMPT: &str = r#"## Role
You are a text analyzer.

## Task
Propose {topn} questions about a given piece of text content.

## Requirements
- Understand and summarize the text content, and propose the top {topn} important questions.
- The questions SHOULD NOT have overlapping meanings.
- The questions SHOULD cover the main content of the text as much as possible.
- The questions MUST be in the same language as the given piece of text content.
- One question per line.
- Output questions ONLY.

---

## Text Content
{content}"#;

/// Verbatim port of RAGFlow `rag/prompts/meta_data.md`.
/// Strict metadata extraction from content against a schema.
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_META_DATA_PROMPT: &str = r#"## Role: Metadata extraction expert.
## Rules:
 - Strict Evidence Only: Extract a value ONLY if it is explicitly mentioned in the Content. 
 - Enum Filter: For any field with an 'enum' list, the list acts as a strict filter. If no element from the list (or its direct synonym) is found in the Content, you MUST NOT extract that field.
 - No Meta-Inference: Do not infer values based on the document's nature, format, or category. If the text does not literally state the information, treat it as missing.
 - Zero-Hallucination: Never invent information or pick a "likely" value from the enum to fill a field.
 - Empty Result: If no matches are found for any field, or if the content is irrelevant, output ONLY {}. 
 - Output: ONLY a valid JSON string. No Markdown, no notes.

## Schema for extraction:
{schema}

## Content to analyze:
{content}"#;

/// Verbatim port of RAGFlow `rag/prompts/meta_filter.md`.
/// Generate metadata filter conditions (key/value/op + and/or logic).
/// Jinja `{{ var }}` placeholders are kept as `{var}` so
/// `PromptTemplate::render` can substitute them; `{% for %}`/`{% if %}`
/// blocks are flattened to documented single placeholders.
pub const RAGFLOW_META_FILTER_PROMPT: &str = r#"You are a metadata filtering condition generator. Analyze the user's question and available document metadata to output a JSON array of filter objects. Follow these rules:

1. **Metadata Structure**: 
   - Metadata is provided as JSON where keys are attribute names (e.g., "color"), and values are objects mapping attribute values to document IDs.
   - Example: 
     {
       "color": {"red": ["doc1"], "blue": ["doc2"]},
       "listing_date": {"2025-07-11": ["doc1"], "2025-08-01": ["doc2"]}
     }

2. **Output Requirements**:
   - Always output a JSON dictionary with only 2 keys: 'conditions'(filter objects) and 'logic' between the conditions ('and' or 'or').
   - Each filter object in conditions must have:
        "key": (metadata attribute name),
        "value": (string value to compare),
        "op": (operator from allowed list)
   - Logic between all the conditions: 'and'(Intersection of results for each condition) / 'or' (union of results for all conditions)


3. **Operator Guide**:
   - Use these operators only: ["contains", "not contains","in", "not in", "start with", "end with", "empty", "not empty", "=", "≠", ">", "<", "≥", "≤"]
   - Date ranges: Break into two conditions (≥ start_date AND < next_month_start)
   - Negations: Always use "≠" for exclusion terms ("not", "except", "exclude", "≠")
   - Implicit logic: Derive unstated filters (e.g., "July" → [≥ YYYY-07-01, < YYYY-08-01])

4. **Operator Constraints**:
   - If `constraints` are provided, you MUST use the specified operator for the corresponding key.
   - Example Constraints: `{"price": ">", "author": "="}`
   - If a key is not in `constraints`, choose the most appropriate operator.

5. **Processing Steps**:
   a) Identify ALL filterable attributes in the query (both explicit and implicit)
   b) For dates:
        - Infer missing year from current date if needed
        - Always format dates as "YYYY-MM-DD"
        - Convert ranges: [≥ start, < end]
   c) For values: Match EXACTLY to metadata's value keys
   d) Skip conditions if:
        - Attribute doesn't exist in metadata
        - Value has no match in metadata

6. **Example A**:
   - User query: "上市日期七月份的有哪些新品，不要蓝色的，只看鞋子和帽子"
   - Metadata: { "color": {...}, "listing_date": {...} }
   - Output: 
   {
        "logic": "and",
        "conditions": [
          {"key": "listing_date", "value": "2025-07-01", "op": "≥"},
          {"key": "listing_date", "value": "2025-08-01", "op": "<"},
          {"key": "color", "value": "blue", "op": "≠"},
          {"key": "category", "value": "shoes, hat", "op": "in"}
        ]
   }

7. **Example B**:
   - User query: "It must be from China or India. Otherwise, it must not be blue or red."
   - Metadata: { "color": {...}, "country": {...} }
   - 
   - Output: 
   {
        "logic": "or",
        "conditions": [
          {"key": "color", "value": "blue, red", "op": "not in"},
          {"key": "country", "value": "china, india", "op": "in"},
        ]
   }

8. **Final Output**:
   - ONLY output valid JSON dictionary
   - NO additional text/explanations
   - Json schema is as following:
```json
{
  "type": "object",
  "properties": {
    "logic": {
      "type": "string",
      "description": "Logic relationship between all the conditions, the default is 'and'.",
      "enum": [
        "and",
        "or"
      ]
    },
    "conditions": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "key": {
            "type": "string",
            "description": "Metadata attribute name."
          },
          "value": {
            "type": "string",
            "description": "Value to compare."
          },
          "op": {
            "type": "string",
            "description": "Operator from allowed list.",
            "enum": [
              "contains",
              "not contains",
              "in",
              "not in",
              "start with",
              "end with",
              "empty",
              "not empty",
              "=",
              "≠",
              ">",
              "<",
              "≥",
              "≤"
            ]
          }
        },
        "required": [
          "key",
          "value",
          "op"
        ],
        "additionalProperties": false
      }
    }
  },
  "required": [
    "conditions"
  ],
  "additionalProperties": false
}
```

**Current Task**:
- Today's date: {current_date}
- Available metadata keys: {metadata_keys}
- User query: "{user_question}"
- Operator constraints: {constraints}"#;


// ── Fallback LLM ────────────────────────────────────────────────

/// Multi-LLM client with fallback strategy.
/// Tries primary LLM first, then falls back to alternatives on failure.
pub struct FallbackLlm {
    /// Primary LLM client (tried first)
    primary: LlmClient,
    /// Fallback LLM clients (tried in order)
    fallbacks: Vec<LlmClient>,
    /// Max retries per LLM
    max_retries: u32,
}

impl FallbackLlm {
    /// Create a fallback LLM chain.
    pub fn new(primary: LlmClient, fallbacks: Vec<LlmClient>) -> Self {
        Self {
            primary,
            fallbacks,
            max_retries: 2,
        }
    }

    /// Set max retries.
    pub fn with_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }

    /// Chat with fallback: try primary → fallbacks → error.
    pub async fn chat(&self, messages: &[ChatMessage]) -> crate::Result<String> {
        // Try primary with retries
        let mut last_err = String::new();
        for attempt in 0..self.max_retries {
            match self.primary.chat(messages).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    last_err = format!("{}", e);
                    if attempt < self.max_retries - 1 {
                        tracing::warn!(
                            "Primary LLM attempt {} failed: {}, retrying...",
                            attempt + 1,
                            last_err
                        );
                    }
                }
            }
        }
        tracing::warn!(
            "Primary LLM failed after {} attempts: {}",
            self.max_retries,
            last_err
        );

        // Try fallbacks
        for (i, fallback) in self.fallbacks.iter().enumerate() {
            for attempt in 0..self.max_retries {
                match fallback.chat(messages).await {
                    Ok(resp) => {
                        tracing::info!("Fallback LLM #{} succeeded", i + 1);
                        return Ok(resp);
                    }
                    Err(e) => {
                        last_err = format!("{}", e);
                        if attempt < self.max_retries - 1 {
                            tracing::warn!(
                                "Fallback #{} attempt {} failed, retrying...",
                                i + 1,
                                attempt + 1
                            );
                        }
                    }
                }
            }
            tracing::warn!("Fallback LLM #{} failed: {}", i + 1, last_err);
        }

        anyhow::bail!("All LLMs failed. Last error: {}", last_err)
    }

    /// RAG chat with fallback.
    pub async fn rag_chat(
        &self,
        question: &str,
        contexts: &[String],
        history: &[ChatMessage],
    ) -> crate::Result<String> {
        let context_text = if contexts.is_empty() {
            String::new()
        } else {
            contexts.iter().enumerate().fold(
                String::from("Relevant context:\n\n"),
                |mut acc, (i, c)| {
                    acc.push_str(&format!("[{}] {}\n\n", i + 1, c));
                    acc
                },
            )
        };

        let mut vars = HashMap::new();
        vars.insert("context", context_text.as_str());
        vars.insert("question", question);
        let system_prompt = PromptLibrary::rag_qa().render(&vars);

        let mut messages = vec![ChatMessage::new("system", system_prompt)];
        let recent = if history.len() > 10 {
            &history[history.len() - 10..]
        } else {
            history
        };
        messages.extend(recent.iter().cloned());
        messages.push(ChatMessage::new("user", question));

        self.chat(&messages).await
    }
}

/// Convenience: create fallback LLM from environment.
pub fn fallback_llm_from_env() -> Option<FallbackLlm> {
    let primary_key = std::env::var("LLM_API_KEY").ok()?;
    let primary = LlmClient::new(LlmConfig {
        api_base: std::env::var("LLM_API_BASE")
            .unwrap_or_else(|_| "https://api.minimaxi.com/v1".into()),
        api_key: primary_key.clone(),
        model: std::env::var("LLM_MODEL").unwrap_or_else(|_| "MiniMax-M3".into()),
        ..LlmConfig::default()
    });

    let mut fallbacks = Vec::new();

    // Qwen as fallback
    if let Ok(qwen_base) = std::env::var("FALLBACK_LLM_BASE") {
        fallbacks.push(LlmClient::new(LlmConfig {
            api_base: qwen_base,
            api_key: std::env::var("FALLBACK_LLM_KEY").unwrap_or(primary_key),
            model: std::env::var("FALLBACK_LLM_MODEL").unwrap_or_else(|_| "Qwen3.5-9B".into()),
            ..LlmConfig::default()
        }));
    }

    tracing::info!(
        "Fallback LLM configured: 1 primary + {} fallback(s)",
        fallbacks.len()
    );

    Some(FallbackLlm::new(primary, fallbacks).with_retries(3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_render() {
        let tmpl = PromptLibrary::rag_qa();
        let mut vars = HashMap::new();
        vars.insert("context", "Rust is fast.");
        vars.insert("question", "What is Rust?");
        let result = tmpl.render(&vars);
        assert!(result.contains("Rust is fast"));
        assert!(result.contains("What is Rust?"));
        assert!(!result.contains("{context}"));
    }

    #[test]
    fn test_prompt_list() {
        let names = PromptLibrary::list();
        assert!(names.len() >= 8);
        assert!(PromptLibrary::get("rag_qa").is_some());
        assert!(PromptLibrary::get("nonexistent").is_none());
    }

    #[test]
    fn test_graphrag_prompts() {
        // Graph entity extraction renders placeholders.
        let mut vars = HashMap::new();
        vars.insert("entity_types", "person, organization");
        vars.insert("input_text", "Alice works at Acme.");
        vars.insert("tuple_delimiter", "<|>");
        vars.insert("record_delimiter", "##");
        vars.insert("completion_delimiter", "<|COMPLETE|>");
        let out = PromptLibrary::graph_entity_extraction().render(&vars);
        assert!(out.contains("Alice works at Acme."));
        assert!(!out.contains("{input_text}"));

        // Community summary has literal JSON braces and entity_df placeholder.
        let mut vars2 = HashMap::new();
        vars2.insert("entity_df", "1,ACME,ACME is a company");
        vars2.insert("relation_df", "2,ACME,ALICE,employs");
        let out2 = PromptLibrary::graph_community_summary().render(&vars2);
        assert!(out2.contains("\"title\": <report_title>"));
        assert!(out2.contains("1,ACME,ACME is a company"));
        assert!(!out2.contains("{entity_df}"));

        // Question proposal / keyword extraction use {topn}/{content}.
        let mut vars3 = HashMap::new();
        vars3.insert("topn", "5");
        vars3.insert("content", "Rust is a systems language.");
        let out3 = PromptLibrary::question_proposal().render(&vars3);
        assert!(out3.contains("Propose 5 questions"));
        assert!(!out3.contains("{topn}"));

        // All new templates are registered in get()/list().
        for name in [
            "graph_entity_extraction",
            "graph_community_summary",
            "entity_extraction",
            "keywords_extraction",
            "minirag_query2kwd",
            "entity_resolution",
            "mind_map_extraction",
            "lightrag_response",
            "question_proposal",
            "keyword_extraction",
        ] {
            assert!(
                PromptLibrary::get(name).is_some(),
                "missing template: {name}"
            );
        }
    }

    #[test]
    fn test_lightrag_defaults_registered() {
        // Constants mirror rag/graphrag/light/graph_prompt.py PROMPTS defaults.
        assert_eq!(
            PromptLibrary::lightrag_default_language().content(),
            "English"
        );
        assert_eq!(
            PromptLibrary::lightrag_default_tuple_delimiter().content(),
            "<|>"
        );
        assert_eq!(
            PromptLibrary::lightrag_default_record_delimiter().content(),
            "##"
        );
        assert_eq!(
            PromptLibrary::lightrag_default_completion_delimiter().content(),
            "<|COMPLETE|>"
        );
        assert_eq!(
            PromptLibrary::lightrag_default_user_prompt().content(),
            "n/a"
        );
        assert_eq!(
            PromptLibrary::lightrag_fail_response().content(),
            "Sorry, I'm not able to provide an answer to that question.[no-context]"
        );
        assert_eq!(
            LIGHTRAG_DEFAULT_ENTITY_TYPES,
            ["organization", "person", "geo", "event", "category"]
        );
        let types_tmpl = PromptLibrary::lightrag_default_entity_types();
        let types = types_tmpl.content();
        assert!(types.contains("organization") && types.contains("category"));
        assert!(!types.contains('['));

        // Every new entry is resolvable via get() and listed by list().
        let listed = PromptLibrary::list();
        for name in [
            "lightrag_fail_response",
            "lightrag_default_language",
            "lightrag_default_tuple_delimiter",
            "lightrag_default_record_delimiter",
            "lightrag_default_completion_delimiter",
            "lightrag_default_entity_types",
            "lightrag_default_user_prompt",
        ] {
            assert!(
                PromptLibrary::get(name).is_some(),
                "missing template: {name}"
            );
            assert!(listed.contains(&name), "not listed: {name}");
        }
    }

    #[test]
    fn test_ragflow_qa_prompts_registered() {
        // Every RAGFlow QA-stage port is resolvable via get() and listed by list().
        let listed = PromptLibrary::list();
        for name in ["citation_prompt", "citation_plus", "full_question", "multi_queries_gen", "next_step", "reflect", "rank_memory", "content_tagging_prompt", "cross_languages_sys", "cross_languages_user", "analyze_task_system", "analyze_task_user", "ask_summary", "assign_toc_levels", "keyword_prompt", "question_prompt", "meta_data", "meta_filter"] {
            assert!(PromptLibrary::get(name).is_some(), "missing template: {name}");
            assert!(listed.contains(&name), "not listed: {name}");
        }
    }

    #[test]
    fn test_ragflow_prompt_render() {
        // citation_plus: sources/example substituted, no Jinja leftovers.
        let mut v1 = HashMap::new();
        v1.insert("sources", "ID: 1\nContent: Rust is fast.");
        v1.insert("example", "Example answer with [ID:1].");
        let out1 = PromptLibrary::citation_plus().render(&v1);
        assert!(out1.contains("ID: 1"));
        assert!(!out1.contains("{{"));
        assert!(!out1.contains("{{ sources }}"));

        // full_question: conversation + relative-date variables.
        let mut v2 = HashMap::new();
        v2.insert("conversation", "USER: What is the capital of France?\nASSISTANT: Paris.");
        v2.insert("today", "2026-08-05");
        v2.insert("yesterday", "2026-08-04");
        v2.insert("tomorrow", "2026-08-06");
        v2.insert("language", "");
        let out2 = PromptLibrary::full_question().render(&v2);
        assert!(out2.contains("Paris."));
        assert!(out2.contains("2026-08-05"));
        assert!(!out2.contains("{{ conversation }}"));

        // multi_queries_gen: all four variables render.
        let mut v3 = HashMap::new();
        v3.insert("original_query", "rust borrow checker");
        v3.insert("original_question", "How does the borrow checker work?");
        v3.insert("retrieved_docs", "[1] Borrow checker docs");
        v3.insert("missing_info", "lifetime elision rules");
        let out3 = PromptLibrary::multi_queries_gen().render(&v3);
        assert!(out3.contains("borrow checker"));
        assert!(out3.contains("lifetime elision rules"));
        assert!(!out3.contains("{{ original_query }}"));

        // content_tagging: topn/all_tags/examples placeholders.
        let mut v4 = HashMap::new();
        v4.insert("topn", "3");
        v4.insert("all_tags", "tech, business, tutorial");
        v4.insert("examples", "# Examples 0\n### Text Content\nExample text\n\nOutput:\n{\"tech\": 9}");
        v4.insert("content", "Rust systems programming.");
        let out4 = PromptLibrary::content_tagging_prompt().render(&v4);
        assert!(out4.contains("tech, business, tutorial"));
        assert!(out4.contains("Rust systems programming."));
        assert!(!out4.contains("{{ topn }}"));

        // meta_filter: date + metadata keys + constraints render; JSON schema braces survive.
        let mut v5 = HashMap::new();
        v5.insert("current_date", "2026-08-05");
        v5.insert("metadata_keys", "color, listing_date");
        v5.insert("user_question", "上市日期七月份的新品");
        v5.insert("constraints", "");
        let out5 = PromptLibrary::meta_filter().render(&v5);
        assert!(out5.contains("2026-08-05"));
        assert!(out5.contains("\"type\": \"object\""));
        assert!(!out5.contains("{{ current_date }}"));
    }

    #[test]
    fn test_ragflow_verbatim_content() {
        // Verbatim RAGFlow originals: citation rules + Chinese text preserved.
        let citation = PromptLibrary::citation_prompt();
        assert!(citation.content().contains("[ID:i] [ID:j]"));
        assert!(citation.content().contains("Maximum 4 citations per sentence"));

        let cross = PromptLibrary::cross_languages_sys();
        assert!(cross.content().contains("你好世界")); // zh example kept verbatim

        let ask = PromptLibrary::ask_summary();
        assert!(ask.content().contains("Sorry, no relevant information provided"));
        assert!(ask.content().contains("Miss R"));

        let meta = PromptLibrary::meta_data();
        assert!(meta.content().contains("Zero-Hallucination"));
        assert!(meta.content().contains("{schema}")); // placeholder form kept for render()

        let kw = PromptLibrary::keyword_prompt();
        assert!(kw.content().contains("top {topn}"));

        let qp = PromptLibrary::question_prompt();
        assert!(qp.content().contains("Propose {topn} questions"));
    }

}
