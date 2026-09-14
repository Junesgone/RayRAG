//! Seeded RAGFlow agent workflow templates.
//! Mirrors `/root/ragflow/agent/templates/*.json` (25 templates). Each entry is
//! `(slug, name, description, dsl_json)`; the DSL is embedded at compile time via
//! `include_str!` from `src/api/fixtures/agent_templates/<slug>.json` so the raw
//! JSON survives untouched (no escaping/truncation risk in Rust source).

/// (slug, name, description, dsl_json) — 25 seeded RAGFlow workflow templates.
pub const AGENT_TEMPLATES: &[(&str, &str, &str, &str)] = &[
    (
        "advanced_ingestion_pipeline",
        "Advanced Ingestion Pipeline",
        "This template demonstrates how to use an LLM to generate summaries, keywords, Q&A, and metadata for each chunk to support diverse retrieval needs.",
        include_str!("api/fixtures/agent_templates/advanced_ingestion_pipeline.json"),
    ),
    (
        "chunk_summary",
        "Chunk Summary",
        "This template uses an LLM to generate chunk summaries for building text and vector indexes. During retrieval, summaries enhance matching, and the original chunks are returned as results.",
        include_str!("api/fixtures/agent_templates/chunk_summary.json"),
    ),
    (
        "customer_feedback_dispatcher",
        "Customer feedback disptacher",
        "Automatically classify customer reviews using LLM (Large Language Model) and route them via email to the relevant departments.",
        include_str!("api/fixtures/agent_templates/customer_feedback_dispatcher.json"),
    ),
    (
        "cv_analysis_and_candidate_evaluation",
        "CV Analysis and Candidate Evaluation",
        "This is a workflow that helps companies evaluate resumes, HR uploads a job description first, then submits multiple resumes via the chat window for evaluation.",
        include_str!("api/fixtures/agent_templates/cv_analysis_and_candidate_evaluation.json"),
    ),
    (
        "data_analysis_beginner_assistant",
        "Beginner's data analytics assistant",
        "A beginner-friendly data analysis assistant that guides you through exploring datasets step-by-step, automatically generating code and visualizations while explaining the logic behind each insight. ",
        include_str!("api/fixtures/agent_templates/data_analysis_beginner_assistant.json"),
    ),
    (
        "deep_research",
        "Deep research",
        "For professionals in sales, marketing, policy, or consulting, the Multi-Agent Deep research Agentic workflow conducts structured, multi-step investigations across diverse sources and delivers consulting-style reports with clear citations.",
        include_str!("api/fixtures/agent_templates/deep_research.json"),
    ),
    (
        "ingestion_pipeline_book",
        "Book",
        "This template segments parsed files by book structure. Best for books, long-form manuscripts, literary works, and other documents with defined chapters and sections.",
        include_str!("api/fixtures/agent_templates/ingestion_pipeline_book.json"),
    ),
    (
        "ingestion_pipeline_general",
        "General",
        "This general-purpose template segments parsed files by token count. Ideal for unstructured documents that lack a fixed layout.",
        include_str!("api/fixtures/agent_templates/ingestion_pipeline_general.json"),
    ),
    (
        "ingestion_pipeline_laws",
        "Laws",
        "This template segments parsed files by legal provision hierarchy. Best for documents with clearly defined articles, such as laws, regulations, judicial interpretations, and compliance policies.",
        include_str!("api/fixtures/agent_templates/ingestion_pipeline_laws.json"),
    ),
    (
        "ingestion_pipeline_manual",
        "Manual",
        "This template segments parsed files by manual structure. Best for technical documents with clearly defined sections and operational guidance, such as product manuals, user guides, and installation instructions.",
        include_str!("api/fixtures/agent_templates/ingestion_pipeline_manual.json"),
    ),
    (
        "ingestion_pipeline_one",
        "One",
        "This template treats the entire parsed file as one segment. Best for short documents or highly coherent text where maintaining full context is critical.",
        include_str!("api/fixtures/agent_templates/ingestion_pipeline_one.json"),
    ),
    (
        "ingestion_pipeline_paper",
        "Paper",
        "This template segments parsed files by paper structure. Best for documents with clearly defined sections, such as scholarly works, conference articles, and research studies.",
        include_str!("api/fixtures/agent_templates/ingestion_pipeline_paper.json"),
    ),
    (
        "ingestion_pipeline_resume",
        "Resume",
        "This template segments parsed files into resume-specific sections. Best for career-related documents with clearly defined categories such as experience, education, projects, and skills.",
        include_str!("api/fixtures/agent_templates/ingestion_pipeline_resume.json"),
    ),
    (
        "market_seo_article_writer",
        "SEO article writer",
        "This SEO article writer automatically generates a complete SEO-optimized blog article based on a simple user input. You don't need any writing experience. Just provide a topic or short request — the system will handle the rest.",
        include_str!("api/fixtures/agent_templates/market_seo_article_writer.json"),
    ),
    (
        "photo_text_translator",
        "Photo text translator",
        "Photo text translator lets you snap any photo containing text—menus, signs, or documents—and instantly recognize and translate it into your language of choice using advanced AI-powered translation technology.",
        include_str!("api/fixtures/agent_templates/photo_text_translator.json"),
    ),
    (
        "reflective_academic_paper_generator",
        "Reflective academic paper generator",
        "A reflective academic paper generator using local knowledge base, with advanced capabilities in task planning, reasoning, and reflective analysis. Recommended for academic research paper Q&A",
        include_str!("api/fixtures/agent_templates/reflective_academic_paper_generator.json"),
    ),
    (
        "seo_article_writer",
        "SEO article writer",
        "This is a multi-agent version of the SEO blog generation workflow. It simulates a small team of AI “writers”, where each agent plays a specialized role — just like a real editorial team.",
        include_str!("api/fixtures/agent_templates/seo_article_writer.json"),
    ),
    (
        "smart_customer_service_specialist",
        "Smart customer service specialist",
        "This template helps address complex customer needs, such as comparing product features, providing usage support, and coordinating home installation services.",
        include_str!("api/fixtures/agent_templates/smart_customer_service_specialist.json"),
    ),
    (
        "stock_market_research_assistant",
        "Stock market research assistant",
        "This template helps financial analysts quickly organize information — it can automatically retrieve company data, consolidate financial metrics, and integrate research report insights.",
        include_str!("api/fixtures/agent_templates/stock_market_research_assistant.json"),
    ),
    (
        "text2sql_data_expert",
        "Text-to-SQL data expert",
        "Text-to-SQL data expert lets business users turn plain-English questions into fully formed SQL queries. Simply type your question (e.g., 'Show me last quarter's top 10 products by revenue') and Text-to-SQL data expert generates the exact SQL, runs it against your database, and returns the results in seconds. ",
        include_str!("api/fixtures/agent_templates/text2sql_data_expert.json"),
    ),
    (
        "title_chunker",
        "Title Chunker",
        "This template slices the parsed file based on its title structure. It is ideal for documents with well-defined headings, such as product manuals, legal contracts, research reports, and academic papers.",
        include_str!("api/fixtures/agent_templates/title_chunker.json"),
    ),
    (
        "trip_planner",
        "Trip planner",
        "This smart trip planner utilizes LLM technology to automatically generate customized travel itineraries, with optional tool integration for enhanced reliability.",
        include_str!("api/fixtures/agent_templates/trip_planner.json"),
    ),
    (
        "user_interaction",
        "Interactive Agent",
        "During the Agent’s execution, users can actively intervene and interact with the Agent to adjust or guide its output, ensuring the final result aligns with their intentions.",
        include_str!("api/fixtures/agent_templates/user_interaction.json"),
    ),
    (
        "web_search_assistant",
        "WebSearch Assistant",
        "A chat assistant template that integrates information extracted from a knowledge base and web searches to respond to queries. Let's start by setting up your knowledge base in 'Retrieval'!",
        include_str!("api/fixtures/agent_templates/web_search_assistant.json"),
    ),
    (
        "your_starter_dataset_chatbot",
        "Your starter dataset chatbot",
        "This is a document question-and-answer system based on a knowledge base. When a user asks a question, it retrieves relevant document content to provide accurate answers.",
        include_str!("api/fixtures/agent_templates/your_starter_dataset_chatbot.json"),
    ),
];

/// Number of seeded templates.
pub const AGENT_TEMPLATE_COUNT: usize = AGENT_TEMPLATES.len();

/// Fixed template metadata uses `canvas_type = "Ingestion Pipeline"` for
/// these ten templates; the Agent API persists that distinction through
/// `CanvasCategory.DataFlow` (`dataflow_canvas`).
pub fn template_canvas_category(slug: &str) -> &'static str {
    match slug {
        "advanced_ingestion_pipeline"
        | "chunk_summary"
        | "ingestion_pipeline_book"
        | "ingestion_pipeline_general"
        | "ingestion_pipeline_laws"
        | "ingestion_pipeline_manual"
        | "ingestion_pipeline_one"
        | "ingestion_pipeline_paper"
        | "ingestion_pipeline_resume"
        | "title_chunker" => "dataflow_canvas",
        _ => "agent_canvas",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_template_categories_preserve_ingestion_pipelines() {
        assert_eq!(
            template_canvas_category("advanced_ingestion_pipeline"),
            "dataflow_canvas"
        );
        assert_eq!(template_canvas_category("chunk_summary"), "dataflow_canvas");
        assert_eq!(template_canvas_category("title_chunker"), "dataflow_canvas");
        assert_eq!(
            template_canvas_category("customer_feedback_dispatcher"),
            "agent_canvas"
        );
    }
}

/// Template metadata fixture (`id`, localized title/description, canvas type),
/// extracted verbatim from the upstream `agent/templates/*.json` catalogue.
const AGENT_TEMPLATE_META: &str = include_str!("api/fixtures/agent_templates/meta.json");

fn template_meta() -> Vec<serde_json::Value> {
    serde_json::from_str(AGENT_TEMPLATE_META).unwrap_or_default()
}

/// Upstream `CanvasTemplateService.get_all()` payload without the DSL.
pub fn template_metadata() -> Vec<serde_json::Value> {
    template_meta()
}

/// One template including the seeded DSL.
pub fn template_detail(id: &str) -> Option<serde_json::Value> {
    let meta = template_meta()
        .into_iter()
        .find(|item| item.get("id").and_then(|value| value.as_str()) == Some(id))?;
    let dsl = AGENT_TEMPLATES
        .iter()
        .find(|(slug, _, _, _)| *slug == id)
        .map(|(_, _, _, dsl)| *dsl)?;
    let mut value = meta;
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "dsl".to_string(),
            serde_json::from_str(dsl).unwrap_or(serde_json::Value::Null),
        );
        let title = object
            .get("title")
            .and_then(|title| title.get("en"))
            .and_then(|title| title.as_str())
            .unwrap_or(id)
            .to_string();
        object.insert(
            "slug".to_string(),
            serde_json::Value::String(id.to_string()),
        );
        object.insert("name".to_string(), serde_json::Value::String(title));
    }
    Some(value)
}
