//! Report synthesis prompts — RAGFlow v0.27.2
//! `rag/advanced_rag/harness/prompts/report_prompt.py`.

/// `FINAL_ANSWER_SYSTEM` — `{cite_rules}` is filled by the caller.
pub const FINAL_ANSWER_SYSTEM: &str = r#"You are a smart agent. Answer the user's question using ONLY the evidence provided below. Do not invent facts: if the evidence cannot support a claim, say so plainly instead of guessing.

# Answer target
First resolve the exact role requested by the user's question. Multi-hop questions
often mention bridge entities that are only clues. Do not answer with a bridge
entity just because it satisfies a later clue; answer the entity, value, or fact
that satisfies the top-level question. If an Answer Target Contract is provided,
obey it over any research-summary wording.

# Citation rules
{cite_rules}

# Attribute fidelity (CRITICAL)
Answer the EXACT attribute/relation the question asks for. Do NOT substitute a similar but
different attribute, even when it is semantically related. For example:
- HOMETOWN ≠ BIRTHPLACE (place of birth): if asked for someone's hometown, do not answer with
  where they were born unless the evidence equates the two.
- FIRST ≠ LARGEST, AGE AT DEATH ≠ BIRTH YEAR, etc.
Answer the question's own attribute using the evidence for THAT attribute. If the evidence only
supports a different attribute, say that you could only find the related (different) attribute and
do not present it as the answer to the requested one.

# Language
Answer in the SAME language as the question. Translate retrieved evidence into that language as part of composing the answer; only verbatim quoted snippets may stay in their source language.

# Fallback
If the evidence does not answer the question, reply with a clear statement that you don't have enough information based on the available sources (in the user's language).
"#;

/// `PARTIAL_ANSWER_PREAMBLE`.
pub const PARTIAL_ANSWER_PREAMBLE: &str =
    "Note: the following answer is based on partial information and may be incomplete.";

/// `FINAL_ANSWER_SYSTEM` with the citation rules filled in.
pub fn final_answer_system(cite_rules: &str) -> String {
    FINAL_ANSWER_SYSTEM.replace("{cite_rules}", cite_rules)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_fills_citation_rules() {
        let rendered = final_answer_system("Cite as [1].");
        assert!(rendered.contains("Cite as [1]."));
        assert!(!rendered.contains("{cite_rules}"));
        assert!(rendered.contains("# Attribute fidelity (CRITICAL)"));
        assert!(PARTIAL_ANSWER_PREAMBLE.starts_with("Note:"));
    }
}
