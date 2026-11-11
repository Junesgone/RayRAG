//! Specialized app parsers — domain-specific document extraction.
//! Replaces RAGFlow's `rag/app/` (audio/book/email/laws/manual/paper/picture/qa/table/tag).
//! Each parser applies domain knowledge for better extraction.

use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use regex::Regex;

/// Route a `chunk_method` (RAGFlow `parser_id`) to its domain parser —
/// mirrors `rag/app/app_parsers.py get_parser`. Returns `None` for the
/// generic methods (`naive`/`token`/`title`/`manual`/`one`/`knowledge_graph`)
/// and unknown values so the MIME-based pipeline keeps handling them.
pub fn parser_for_method(method: &str) -> Option<Box<dyn Parse>> {
    match method.to_ascii_lowercase().as_str() {
        "email" => Some(Box::new(EmailParser::new())),
        "paper" => Some(Box::new(PaperParser::new())),
        "laws" => Some(Box::new(LawsParser::new())),
        "book" => Some(Box::new(BookParser::new())),
        "picture" => Some(Box::new(PictureParser::new())),
        "table" => Some(Box::new(TableParser::new())),
        "tag" => Some(Box::new(TagParser::new())),
        "presentation" => Some(Box::new(PresentationParser::new())),
        "audio" => Some(Box::new(AudioParser::new())),
        "qa" => Some(Box::new(QaParser::new())),
        _ => None,
    }
}

// ── Email Parser ────────────────────────────────────────────────

#[derive(Default)]
pub struct EmailParser;
impl EmailParser {
    pub fn new() -> Self {
        Self
    }
    fn extract(&self, data: &[u8]) -> Result<String> {
        let content = String::from_utf8_lossy(data);
        let mut text = String::from("<!--EMAIL_START-->\n");
        let mut in_headers = true;
        for line in content.lines() {
            if in_headers && line.is_empty() {
                in_headers = false;
                continue;
            }
            if in_headers {
                if let Some((k, v)) = line.split_once(':') {
                    text.push_str(&format!("{}: {}\n", k.trim(), v.trim()));
                }
            } else {
                text.push_str(line);
                text.push('\n');
            }
        }
        text.push_str("<!--EMAIL_END-->\n");
        Ok(text)
    }
}
impl Parse for EmailParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        Ok(new_document(
            name,
            self.extract(data)?,
            "message/rfc822",
            data.len(),
        ))
    }
}

// ── Academic Paper Parser ──────────────────────────────────────

#[derive(Default)]
pub struct PaperParser;
impl PaperParser {
    pub fn new() -> Self {
        Self
    }
    fn extract(&self, data: &[u8]) -> Result<String> {
        let content = String::from_utf8_lossy(data);
        let mut text = String::from("<!--PAPER_START-->\n");
        let sections = Regex::new(
            r"(?im)^(?:Abstract|Introduction|Related Work|Method|Experiment|Result|Conclusion|Reference|Discussion|Background|Approach|Evaluation)\b",
        )?;

        let parts: Vec<&str> = sections.split(&content).collect();
        let headers: Vec<&str> = sections.find_iter(&content).map(|m| m.as_str()).collect();

        for (i, part) in parts.iter().enumerate() {
            if i == 0 && !part.trim().is_empty() {
                text.push_str(&format!("# Title/Abstract\n{}\n\n", part.trim()));
            } else if i <= headers.len() {
                text.push_str(&format!("## {}\n{}\n\n", headers[i - 1], part.trim()));
            }
        }
        text.push_str("<!--PAPER_END-->\n");
        Ok(text)
    }
}
impl Parse for PaperParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        Ok(new_document(
            name,
            self.extract(data)?,
            "application/pdf",
            data.len(),
        ))
    }
}

// ── Legal Document Parser ──────────────────────────────────────

#[derive(Default)]
pub struct LawsParser;
impl LawsParser {
    pub fn new() -> Self {
        Self
    }
    fn extract(&self, data: &[u8]) -> Result<String> {
        let content = String::from_utf8_lossy(data);
        let mut text = String::from("<!--LAWS_START-->\n");
        let article_re =
            Regex::new(r"(?im)^(?:Article|第[一二三四五六七八九十百千]+条|Section)\s+\d+")?;
        let parts: Vec<&str> = article_re.split(&content).collect();
        let headers: Vec<&str> = article_re.find_iter(&content).map(|m| m.as_str()).collect();
        for (i, part) in parts.iter().enumerate() {
            if i == 0 && !part.trim().is_empty() {
                text.push_str(&format!("{}\n\n", part.trim()));
            } else if i <= headers.len() {
                text.push_str(&format!("## {}\n{}\n\n", headers[i - 1], part.trim()));
            }
        }
        text.push_str("<!--LAWS_END-->\n");
        Ok(text)
    }
}
impl Parse for LawsParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        Ok(new_document(
            name,
            self.extract(data)?,
            "application/pdf",
            data.len(),
        ))
    }
}

// ── Manual / Book Parser ───────────────────────────────────────

#[derive(Default)]
pub struct BookParser;
impl BookParser {
    pub fn new() -> Self {
        Self
    }
    fn extract(&self, data: &[u8]) -> Result<String> {
        let content = String::from_utf8_lossy(data);
        let mut text = String::from("<!--BOOK_START-->\n");
        let ch_re =
            Regex::new(r"(?im)^(?:Chapter|CHAPTER|第[一二三四五六七八九十百千]+章)\s+\d*.*$")?;
        let parts: Vec<&str> = ch_re.split(&content).collect();
        let headers: Vec<&str> = ch_re.find_iter(&content).map(|m| m.as_str()).collect();
        for (i, part) in parts.iter().enumerate() {
            if i == 0 && !part.trim().is_empty() {
                text.push_str(&format!("{}\n\n", part.trim()));
            } else if i <= headers.len() {
                text.push_str(&format!("## {}\n{}\n\n", headers[i - 1], part.trim()));
            }
        }
        text.push_str("<!--BOOK_END-->\n");
        Ok(text)
    }
}
impl Parse for BookParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        Ok(new_document(
            name,
            self.extract(data)?,
            "application/pdf",
            data.len(),
        ))
    }
}

// ── Picture / Image Content Parser ─────────────────────────────

#[derive(Default)]
pub struct PictureParser;
impl PictureParser {
    pub fn new() -> Self {
        Self
    }
    fn extract(&self, name: &str, data: &[u8]) -> Result<String> {
        let basename = std::path::Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("image");
        let size_mb = data.len() as f64 / 1_048_576.0;
        let fmt = detect_format(data);
        let text = format!(
            "<!--PICTURE_START-->\n[Image: {}] [{:.1}MB] [Format: {}]\n<!--PICTURE_END-->\n",
            basename, size_mb, fmt
        );
        Ok(text)
    }
}
impl Parse for PictureParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        Ok(new_document(
            name,
            self.extract(name, data)?,
            "image/png",
            data.len(),
        ))
    }
}

fn detect_format(data: &[u8]) -> &'static str {
    if data.len() < 4 {
        return "unknown";
    }
    match &data[0..4] {
        [0x89, b'P', b'N', b'G'] => "PNG",
        [0xFF, 0xD8, 0xFF, _] => "JPEG",
        [b'G', b'I', b'F', b'8'] => "GIF",
        [b'R', b'I', b'F', b'F'] => "WEBP",
        _ => "unknown",
    }
}

// ── Table-specific Parser ──────────────────────────────────────

#[derive(Default)]
pub struct TableParser;
impl TableParser {
    pub fn new() -> Self {
        Self
    }
    fn extract(&self, data: &[u8]) -> Result<String> {
        let content = String::from_utf8_lossy(data);
        let text = format!("<!--TABLE_START-->\n{}\n<!--TABLE_END-->\n", content.trim());
        Ok(text)
    }
}
impl Parse for TableParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        Ok(new_document(
            name,
            self.extract(data)?,
            "text/csv",
            data.len(),
        ))
    }
}

// ── Tag / Metadata Generator ───────────────────────────────────

#[derive(Default)]
pub struct TagParser;
impl TagParser {
    pub fn new() -> Self {
        Self
    }
    fn extract(&self, data: &[u8]) -> Result<String> {
        let content = String::from_utf8_lossy(data).to_lowercase();
        let mut tags = Vec::new();
        let keywords = [
            ("api", "API"),
            ("rust", "Rust"),
            ("python", "Python"),
            ("database", "Database"),
            ("machine learning", "ML"),
            ("ai", "AI"),
            ("deep learning", "Deep Learning"),
            ("web", "Web"),
            ("mobile", "Mobile"),
            ("security", "Security"),
            ("cloud", "Cloud"),
            ("devops", "DevOps"),
            ("testing", "Testing"),
            ("frontend", "Frontend"),
            ("backend", "Backend"),
            ("algorithm", "Algorithm"),
        ];
        for (kw, tag) in &keywords {
            if content.contains(kw) {
                tags.push(*tag);
            }
        }
        Ok(format!(
            "<!--TAG_START-->\n{}\n<!--TAG_END-->\n",
            tags.join(", ")
        ))
    }
}
impl Parse for TagParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        Ok(new_document(
            name,
            self.extract(data)?,
            "text/plain",
            data.len(),
        ))
    }
}

// ── Presentation (non-PPTX) Parser ─────────────────────────────

#[derive(Default)]
pub struct PresentationParser;
impl PresentationParser {
    pub fn new() -> Self {
        Self
    }
    fn extract(&self, data: &[u8]) -> Result<String> {
        let content = String::from_utf8_lossy(data);
        let mut text = String::from("<!--PRESENTATION_START-->\n");
        for (i, line) in content.lines().enumerate() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if t.len() < 80
                && t.chars()
                    .all(|c| c.is_uppercase() || c.is_whitespace() || c.is_numeric())
            {
                text.push_str(&format!("## Slide {}: {}\n", i + 1, t));
            } else {
                text.push_str(&format!("{}\n", t));
            }
        }
        text.push_str("<!--PRESENTATION_END-->\n");
        Ok(text)
    }
}
impl Parse for PresentationParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        Ok(new_document(
            name,
            self.extract(data)?,
            "text/plain",
            data.len(),
        ))
    }
}

// ── Audio Parser (metadata) ───────────────────────────────────

#[derive(Default)]
pub struct AudioParser;
impl AudioParser {
    pub fn new() -> Self {
        Self
    }
    fn extract(&self, name: &str, data: &[u8]) -> Result<String> {
        let basename = std::path::Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("audio");
        let duration = format!("[{:.1}s]", data.len() as f64 / 16000.0);
        Ok(format!(
            "<!--AUDIO_START-->\n[Audio: {}] {}\n<!--AUDIO_END-->\n",
            basename, duration
        ))
    }
}
impl Parse for AudioParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        Ok(new_document(
            name,
            self.extract(name, data)?,
            "audio/mpeg",
            data.len(),
        ))
    }
}

// ── Q&A / Interview Parser ────────────────────────────────────

#[derive(Default)]
pub struct QaParser;
impl QaParser {
    pub fn new() -> Self {
        Self
    }
    fn extract(&self, data: &[u8]) -> Result<String> {
        let content = String::from_utf8_lossy(data);
        let qa_re = Regex::new(r"(?im)^(?:Q:|Question:|A:|Answer:)")?;
        let mut text = String::from("<!--QA_START-->\n");
        let mut last_q = String::new();
        for line in content.lines() {
            let t = line.trim();
            if qa_re.is_match(t) {
                if !last_q.is_empty() {
                    text.push_str(&format!("A: {}\n\n", last_q));
                    last_q.clear();
                }
                text.push_str(&format!("{}\n", t));
            } else if !t.is_empty() {
                last_q.push_str(t);
                last_q.push(' ');
            }
        }
        if !last_q.is_empty() {
            text.push_str(&format!("A: {}\n", last_q));
        }
        text.push_str("<!--QA_END-->\n");
        Ok(text)
    }
}
impl Parse for QaParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        Ok(new_document(
            name,
            self.extract(data)?,
            "text/plain",
            data.len(),
        ))
    }
}

// ── QA-mode dispatch (rag/app application-layer semantics) ───────
// Mirrors RAGFlow's chat-mode selection:
//   - `api/db/services/dialog_service.py::async_ask`:
//       is_knowledge_graph = all(kb.parser_id == ParserType.KG)
//       retriever = settings.kg_retriever if is_knowledge_graph else settings.retriever
//   - `rag/advanced_rag.py` (DeepResearcher) is the "advanced" pipeline; every
//     other parser id (naive/manual/paper/book/…) routes to the hybrid retriever.
// The RAGFlow parser vocabulary (common/constants.py ParserType): naive, laws,
// manual, paper, resume, book, qa, table, picture, one, audio, email, tag,
// knowledge_graph, presentation.

/// RAGFlow chat QA mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QaMode {
    /// Hybrid retrieval → plain LLM answer (`settings.retriever`).
    Naive,
    /// Deep-research pipeline (`rag/advanced_rag.py` DeepResearcher).
    Advanced,
    /// Knowledge-graph retrieval (`settings.kg_retriever`, all KBs use ParserType.KG).
    Graph,
}

impl QaMode {
    /// RAGFlow-style mode name.
    pub fn as_str(self) -> &'static str {
        match self {
            QaMode::Naive => "naive",
            QaMode::Advanced => "advanced",
            QaMode::Graph => "graph",
        }
    }

    /// True when every knowledge base uses the knowledge-graph parser.
    pub fn is_graph_mode(parser_ids: &[&str]) -> bool {
        !parser_ids.is_empty() && parser_ids.iter().all(|p| *p == "knowledge_graph")
    }

    /// True when any knowledge base opts into the deep-research pipeline.
    pub fn is_advanced_mode(parser_ids: &[&str]) -> bool {
        parser_ids
            .iter()
            .any(|p| matches!(*p, "advanced" | "advanced_rag" | "deep_researcher"))
    }

    /// Resolve the QA mode for a set of KB parser ids.
    ///
    /// Priority mirrors RAGFlow: graph mode when *all* KBs are KG-parsed
    /// (dialog_service.async_ask), advanced when the pipeline is explicitly
    /// requested, otherwise naive hybrid retrieval.
    pub fn resolve(parser_ids: &[&str]) -> QaMode {
        if Self::is_graph_mode(parser_ids) {
            QaMode::Graph
        } else if Self::is_advanced_mode(parser_ids) {
            QaMode::Advanced
        } else {
            QaMode::Naive
        }
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;

    #[test]
    fn parser_for_method_routes_domain_parsers_and_returns_none_for_generic() {
        assert!(parser_for_method("paper").is_some());
        assert!(parser_for_method("book").is_some());
        assert!(parser_for_method("laws").is_some());
        assert!(parser_for_method("qa").is_some());
        assert!(parser_for_method("table").is_some());
        assert!(parser_for_method("tag").is_some());
        assert!(parser_for_method("presentation").is_some());
        assert!(parser_for_method("picture").is_some());
        assert!(parser_for_method("audio").is_some());
        assert!(parser_for_method("email").is_some());
        // Generic methods and unknown values stay on the MIME pipeline.
        assert!(parser_for_method("naive").is_none());
        assert!(parser_for_method("token").is_none());
        assert!(parser_for_method("title").is_none());
        assert!(parser_for_method("manual").is_none());
        assert!(parser_for_method("one").is_none());
        assert!(parser_for_method("knowledge_graph").is_none());
        assert!(parser_for_method("nonsense").is_none());
    }

    #[test]
    fn email_parser_extracts_headers_and_body() {
        let parser = EmailParser::new();
        let raw = b"From: a@b.c\nSubject: test\n\nhello world\n";
        let doc = parser.parse("mail.eml", raw).unwrap();
        assert!(doc.content.contains("From: a@b.c"));
        assert!(doc.content.contains("hello world"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qa_mode_resolve_naive_default() {
        // Mixed / non-KG parsers → naive hybrid retrieval.
        assert_eq!(QaMode::resolve(&["naive"]), QaMode::Naive);
        assert_eq!(QaMode::resolve(&["paper", "manual"]), QaMode::Naive);
        assert_eq!(
            QaMode::resolve(&["knowledge_graph", "naive"]),
            QaMode::Naive
        );
        assert_eq!(QaMode::resolve(&[]), QaMode::Naive);
        assert_eq!(QaMode::Naive.as_str(), "naive");
    }

    #[test]
    fn test_qa_mode_resolve_graph() {
        // All KBs must be KG-parsed (RAGFlow `all(kb.parser_id == ParserType.KG)`).
        assert_eq!(QaMode::resolve(&["knowledge_graph"]), QaMode::Graph);
        assert_eq!(
            QaMode::resolve(&["knowledge_graph", "knowledge_graph"]),
            QaMode::Graph
        );
        assert!(QaMode::is_graph_mode(&["knowledge_graph"]));
        assert!(!QaMode::is_graph_mode(&["knowledge_graph", "naive"]));
        assert!(!QaMode::is_graph_mode(&[]));
        assert_eq!(QaMode::Graph.as_str(), "graph");
    }

    #[test]
    fn test_qa_mode_resolve_advanced() {
        // Explicit deep-research marker wins over naive but not over all-KG graph.
        assert_eq!(QaMode::resolve(&["advanced"]), QaMode::Advanced);
        assert_eq!(
            QaMode::resolve(&["deep_researcher", "naive"]),
            QaMode::Advanced
        );
        assert_eq!(QaMode::Advanced.as_str(), "advanced");
        // Mixed KG + advanced is not "all KG", so the explicit marker wins.
        assert_eq!(
            QaMode::resolve(&["knowledge_graph", "advanced"]),
            QaMode::Advanced
        );
    }
}
