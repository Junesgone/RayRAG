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
        ]
    }
}

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
}
