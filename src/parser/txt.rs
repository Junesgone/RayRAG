//! Plain text parser — ported from RAGFlow's `deepdoc/parser/txt_parser.py`.

use crate::parser::{Parse, new_document};
use crate::{Document, Result};

#[derive(Default)]
pub struct TxtParser;

impl TxtParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parse for TxtParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = String::from_utf8(data.to_vec()).or_else(|_| {
            // Try with BOM removal for UTF-16
            if data.len() >= 2 {
                let bom = u16::from_le_bytes([data[0], data[1]]);
                if bom == 0xFEFF {
                    let utf16: Vec<u16> = data[2..]
                        .chunks(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    return String::from_utf16(&utf16)
                        .map_err(|e| anyhow::anyhow!("UTF-16 decode error: {}", e));
                }
            }
            Err(anyhow::anyhow!("Failed to decode text"))
        })?;
        Ok(new_document(name, content, "text/plain", data.len()))
    }
}
