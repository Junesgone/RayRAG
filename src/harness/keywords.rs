//! Four-aspect keyword extraction with entity weighting — RAGFlow v0.27.2
//! `rag/advanced_rag/harness/keywords.py`.
//!
//! A keyword search matches only the surface forms you give it, so the LLM is
//! asked for FOUR aspects of the question: `entity` (what the fact is about),
//! `aliases` (surface variants), `fact_type` (words the corpus might use) and
//! `qualifiers` (year / edition / jurisdiction / revision). Entity and
//! qualifier terms are repeated (x3) in the retrieval query so BM25 weights
//! them up; the plain deduped union narrows retrieved chunks.

use serde_json::Value;

use super::HarnessChat;

/// Keywords are extracted into FOUR weighted aspects.
pub const KEYWORD_ASPECTS: [&str; 4] = ["entity", "aliases", "fact_type", "qualifiers"];
/// Copies of each entity term in the query, to weight it up.
pub const KEYWORD_ENTITY_REPEAT: usize = 3;
/// Copies of each qualifier (year / version / jurisdiction), weighted up like entity.
pub const KEYWORD_QUALIFIER_REPEAT: usize = 3;
/// Hard cap on the weighted query so it never pollutes.
pub const KEYWORD_MAX_CHARS: usize = 400;

/// `_KEYWORDS_SYSTEM`.
pub const KEYWORDS_SYSTEM: &str = r#"You turn ONE question into search terms for a keyword/BM25 search engine.

Emit the terms that would appear VERBATIM in a document that answers the question, sorted into FOUR
categories. Every term must come from the question itself or be a surface form of something in it.

A. "entity" — the specific thing the fact is ABOUT: proper nouns, titles, identifiers. Keep a
   multi-word entity whole, as ONE term ("Brown County", "Treaty of Versailles"); split across
   several terms its tokens match independently and drag in noise. A bare identifier — a serial,
   patent, catalogue or case number — is a complete entity on its own; never glue it to the words
   around it.
B. "aliases" — the engine matches ONLY the surface forms you supply, so emit the plausible variants
   of A: full vs. short name, native-language and transliterated forms, official vs. common name,
   acronym and its expansion, and the qualified form ("Brown County" -> "Brown County, Kansas").
C. "fact_type" — 3 to 6 words the corpus might use for this KIND of fact, since you cannot know how
   it is phrased. Spread them across registers:
     quantity of people -> population, inhabitants, residents, census, demographics, headcount
     time of an event   -> founded, established, opened, dated, began
     role of a person   -> served, appointed, elected, held, director
   SOURCES TABULATE WHAT QUESTIONS SPELL OUT: a statistic named in prose is usually written in a
   table as a column abbreviation, and the prose wording may not appear in the document at all. So
   include the abbreviation a table would use — "points per game" -> "PPG", "PTS"; "earnings per
   share" -> "EPS"; "games played" -> "GP" — and reach a superlative through its plain column too:
   "leading scorer" is found by looking for "PTS" and "PPG", not for the phrase itself.
D. "qualifiers" — year, edition, jurisdiction, revision. Worth emitting even when it looks
   redundant: the qualifier often sits in a table header or a document title that chunking has
   severed from the value. Include EVERY alternative expression of a DATE or NUMBER in the
   question — ordinals and their words ("21st" -> "twenty-first"), digits and their words
   ("2000000" -> "two million", "2 million"), and each common date format ("Aug 2nd" -> "August 2",
   "2 August", "08-02").

A and B are what FINDS the document; C and D only boost the ranking. So never withhold an entity
because you are unsure of it, and never pad C or D to reach a count.

DROP entirely: question words ("which", "who", "when", "how many"), relational scaffolding, and
generic high-frequency nouns ("year", "number", "city", "total", "list", "information"). They cost
ranking quality and retrieve nothing.

Output ONLY JSON, no prose, no code fences:
{"entity": ["<term>", ...], "aliases": ["<term>", ...], "fact_type": ["<term>", ...], "qualifiers": ["<term>", ...]}
Any category may be empty."#;

/// `_norm_keyword`: normalise a term for cross-category dedup.
pub fn norm_keyword(term: &str) -> String {
    term.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `_parse_aspects`: parse the LLM's JSON into one deduped list per aspect.
/// ONE dedup set spans all four categories — a term emitted as both an entity
/// and an alias must not collect a second share of the query's mass.
pub fn parse_aspects(raw: &str) -> Vec<(String, Vec<String>)> {
    let think = regex::Regex::new("(?s)^.*</think>").unwrap();
    let fences = regex::Regex::new(r"```(?:json)?\s*|\s*```").unwrap();
    let cleaned = fences
        .replace_all(&think.replace(raw, ""), "")
        .trim()
        .to_string();
    let data: Value =
        serde_json::from_str(&cleaned).unwrap_or_else(|_| Value::Object(Default::default()));

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut aspects: Vec<(String, Vec<String>)> = Vec::new();
    for aspect in KEYWORD_ASPECTS {
        let mut terms: Vec<String> = Vec::new();
        if let Some(items) = data.get(aspect).and_then(Value::as_array) {
            for item in items {
                let term = match item {
                    Value::String(text) => text.trim().to_string(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                let key = norm_keyword(&term);
                if !term.is_empty() && !key.is_empty() && seen.insert(key) {
                    terms.push(term);
                }
            }
        }
        aspects.push((aspect.to_string(), terms));
    }
    aspects
}

fn aspect_terms(aspects: &[(String, Vec<String>)], name: &str) -> Vec<String> {
    aspects
        .iter()
        .find(|(aspect, _)| aspect == name)
        .map(|(_, terms)| terms.clone())
        .unwrap_or_default()
}

/// Build `(query, keywords)` from parsed aspects — the weighting half of
/// `extract_weighted_keywords` without the LLM call.
pub fn weighted_query(aspects: &[(String, Vec<String>)], question: &str) -> (String, String) {
    let mut keywords = aspects
        .iter()
        .flat_map(|(_, terms)| terms.iter().cloned())
        .collect::<Vec<_>>()
        .join(", ");
    if keywords.is_empty() {
        keywords = question.to_string();
    }

    let mut weighted: Vec<String> = Vec::new();
    for term in aspect_terms(aspects, "entity") {
        for _ in 0..KEYWORD_ENTITY_REPEAT {
            weighted.push(term.clone());
        }
    }
    for term in aspect_terms(aspects, "qualifiers") {
        for _ in 0..KEYWORD_QUALIFIER_REPEAT {
            weighted.push(term.clone());
        }
    }
    for aspect in ["aliases", "fact_type"] {
        weighted.extend(aspect_terms(aspects, aspect));
    }
    let mut query = weighted.join(", ");
    if query.is_empty() {
        query = keywords.clone();
    }

    let cap = |text: String| -> String { text.chars().take(KEYWORD_MAX_CHARS).collect() };
    (cap(query), cap(keywords))
}

/// `extract_weighted_keywords`: ask the model for the four aspects and return
/// `(query, keywords)`. Falls back to `(question, question)` when extraction
/// fails.
pub async fn extract_weighted_keywords(chat: &dyn HarnessChat, question: &str) -> (String, String) {
    if question.is_empty() {
        return (String::new(), String::new());
    }
    let (_, messages) = super::message_fit_in(
        super::form_message(KEYWORDS_SYSTEM, question),
        chat.max_length(),
    );
    let user = messages
        .last()
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or(question)
        .to_string();
    let gen_conf = serde_json::json!({"temperature": 0.1});
    let raw = chat
        .chat(
            KEYWORDS_SYSTEM,
            &[serde_json::json!({"role": "user", "content": user})],
            &gen_conf,
        )
        .await
        .unwrap_or_default();
    let aspects = parse_aspects(&raw);
    weighted_query(&aspects, question)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing_dedups_across_the_four_aspects() {
        let raw = "<think>ignore</think>```json\n{\"entity\": [\"Brown County\", \"brown county\"], \"aliases\": [\"Brown County, Kansas\"], \"fact_type\": [\"population\"], \"qualifiers\": [\"2020\"]}\n```";
        let aspects = parse_aspects(raw);
        assert_eq!(aspect_terms(&aspects, "entity"), vec!["Brown County"]);
        assert_eq!(
            aspect_terms(&aspects, "aliases"),
            vec!["Brown County, Kansas"]
        );
        assert_eq!(aspect_terms(&aspects, "fact_type"), vec!["population"]);
        assert_eq!(aspect_terms(&aspects, "qualifiers"), vec!["2020"]);

        // A term named twice lands in the FIRST aspect only.
        let raw = "{\"entity\": [\"Alpha\"], \"aliases\": [\"alpha\"]}";
        let aspects = parse_aspects(raw);
        assert_eq!(aspect_terms(&aspects, "entity"), vec!["Alpha"]);
        assert!(aspect_terms(&aspects, "aliases").is_empty());
    }

    #[test]
    fn weighting_repeats_entities_and_qualifiers() {
        let aspects = parse_aspects(
            "{\"entity\": [\"Alpha\"], \"aliases\": [\"Beta\"], \"fact_type\": [\"population\"], \"qualifiers\": [\"1999\"]}",
        );
        let (query, keywords) = weighted_query(&aspects, "plain question");
        assert_eq!(
            query,
            "Alpha, Alpha, Alpha, 1999, 1999, 1999, Beta, population"
        );
        assert_eq!(keywords, "Alpha, Beta, population, 1999");

        // No aspects -> both sides fall back to the question.
        let (query, keywords) = weighted_query(&[], "the original question");
        assert_eq!(query, "the original question");
        assert_eq!(keywords, "the original question");

        // The 400-char cap holds for both outputs.
        let long = "x".repeat(500);
        let aspects = vec![("entity".to_string(), vec![long])];
        let (query, keywords) = weighted_query(&aspects, "q");
        assert_eq!(query.chars().count(), KEYWORD_MAX_CHARS);
        assert_eq!(keywords.chars().count(), KEYWORD_MAX_CHARS);
    }
}
