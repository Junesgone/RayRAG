//! Resume parser — extract structured sections from resume documents.
//!
//! Detects common resume sections (experience, education, skills, etc.)
//! by scanning for section headers. Works on PDF and DOCX resumes.

use crate::parser::{Parse, new_document};
use crate::{Document, Result};

#[derive(Default)]
pub struct ResumeParser;

/// Common resume section header patterns (case-insensitive).
const SECTION_PATTERNS: &[(&str, &str)] = &[
    ("experience", "Work Experience"),
    ("work experience", "Work Experience"),
    ("employment", "Work Experience"),
    ("professional experience", "Work Experience"),
    ("education", "Education"),
    ("academic background", "Education"),
    ("qualification", "Education"),
    ("skills", "Skills"),
    ("technical skills", "Skills"),
    ("core competencies", "Skills"),
    ("projects", "Projects"),
    ("project experience", "Projects"),
    ("certifications", "Certifications"),
    ("certificates", "Certifications"),
    ("languages", "Languages"),
    ("publications", "Publications"),
    ("summary", "Summary"),
    ("objective", "Objective"),
    ("contact", "Contact"),
];

impl ResumeParser {
    pub fn new() -> Self {
        Self
    }

    fn extract_text(&self, data: &[u8], mime_type: &str) -> Result<String> {
        // Delegate to appropriate parser for raw text
        let raw = match mime_type {
            "application/pdf" => {
                let parser = super::pdf::PdfParser::new();
                let doc = parser.parse("resume", data)?;
                doc.content
            }
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
                let parser = super::docx::DocxParser::new();
                let doc = parser.parse("resume", data)?;
                doc.content
            }
            _ => String::from_utf8(data.to_vec())?,
        };

        Ok(structure_resume(&raw))
    }
}

impl Parse for ResumeParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        // Try both PDF and DOCX
        let content = self
            .extract_text(data, "application/pdf")
            .or_else(|_| {
                self.extract_text(
                    data,
                    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                )
            })
            .unwrap_or_else(|_| structure_resume(&String::from_utf8_lossy(data)));

        Ok(new_document(name, content, "application/pdf", data.len()))
    }
}

/// Structure raw resume text into labeled sections.
fn structure_resume(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    if lines.is_empty() {
        return raw.to_string();
    }

    let mut result = String::new();
    let _current_section = String::from("Header");
    let mut seen_sections = std::collections::HashSet::new();

    // Try to extract name from first non-empty line
    if let Some(first) = lines.first() {
        let trimmed = first.trim();
        if !trimmed.is_empty() && trimmed.len() < 50 && !trimmed.contains(' ') {
            // RAGFlow resume parser (step_two.py) only treats a line as a
            // name when it starts with a known Chinese surname
            // (`surname.isit(nm[0]) or surname.isit(nm[:2])`).
            if crate::surname::is_chinese_surname(trimmed) {
                result.push_str(&format!("# 姓名: {}\n", trimmed));
            } else {
                result.push_str(&format!("# {}\n", trimmed));
            }
        } else {
            result.push_str(&format!("{}\n", trimmed));
        }
        seen_sections.insert("Header".to_string());
    }

    for line in &lines[1..] {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Check if this line is a section header
        let lower = trimmed.to_lowercase();
        let is_header = SECTION_PATTERNS
            .iter()
            .any(|(pat, _)| lower.contains(pat) && trimmed.len() < 40);

        if is_header {
            if let Some(lbl) = SECTION_PATTERNS
                .iter()
                .find(|(pat, _)| lower.contains(pat))
                .map(|(_, label)| *label)
            {
                if seen_sections.insert(lbl.to_string()) {
                    result.push_str(&format!("\n## {}\n", trimmed));
                } else {
                    // Duplicate section — merge content
                    result.push_str(&format!("\n{}", trimmed));
                }
            }
        } else {
            // Content line — add under current section
            result.push_str(&format!("{}\n", trimmed));
        }
    }

    result
}
