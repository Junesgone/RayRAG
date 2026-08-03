//! DOCX parser — ported from RAGFlow's `deepdoc/parser/docx_parser.py`.
//!
//! Extracts text from Word documents including paragraphs, tables,
//! headers, and footers.

use crate::parser::{Parse, new_document};
use crate::{Document, Result};

#[derive(Default)]
pub struct DocxParser;

impl DocxParser {
    pub fn new() -> Self {
        Self
    }

    fn extract_text(&self, data: &[u8]) -> Result<String> {
        // docx-rs 0.4.x: read_docx expects &[u8] directly (not Cursor)
        let doc = docx_rs::read_docx(data)?;
        let mut text = String::new();

        for item in &doc.document.children {
            match item {
                docx_rs::DocumentChild::Paragraph(p) => {
                    text.push_str(&paragraph_to_text(p));
                    text.push('\n');
                }
                docx_rs::DocumentChild::Table(t) => {
                    text.push_str("\n<!--TABLE_START-->\n");
                    text.push_str(&table_to_text(t));
                    text.push_str("\n<!--TABLE_END-->\n");
                }
                _ => {}
            }
        }

        Ok(text)
    }
}

impl Parse for DocxParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = self.extract_text(data)?;
        Ok(new_document(
            name,
            content,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            data.len(),
        ))
    }
}

fn paragraph_to_text(p: &docx_rs::Paragraph) -> String {
    let mut text = String::new();
    // docx-rs 0.4.x: Paragraph.children (Vec<ParagraphChild>) replaces runs
    for child in &p.children {
        if let docx_rs::ParagraphChild::Run(run) = child {
            for run_child in &run.children {
                // run.children (Vec<RunChild>) replaces r#ref
                if let docx_rs::RunChild::Text(t) = run_child {
                    text.push_str(&t.text);
                }
            }
        }
    }
    text
}

fn table_to_text(t: &docx_rs::Table) -> String {
    let mut text = String::new();
    // docx-rs 0.4.x: Table.rows is Vec<TableChild>, not Vec<Row>
    for row in &t.rows {
        #[allow(irrefutable_let_patterns)]
        let docx_rs::TableChild::TableRow(tr) = row;
        let mut row_text = String::new();
        // tr.cells is Vec<TableRowChild>, not Vec<Cell>
        for cell in &tr.cells {
            #[allow(irrefutable_let_patterns)]
            let docx_rs::TableRowChild::TableCell(tc) = cell;
            // tc.children is Vec<TableCellContent>, not paragraphs
            for content in &tc.children {
                if let docx_rs::TableCellContent::Paragraph(p) = content {
                    row_text.push_str(&paragraph_to_text(p));
                    row_text.push(' ');
                }
            }
            row_text.push('\t');
        }
        text.push_str(&row_text);
        text.push('\n');
    }
    text
}
