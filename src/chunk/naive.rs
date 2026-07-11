//! Naive chunker — token-based splitting with overlap.
//!
//! Ported from RAGFlow's `rag/nlp/__init__.py` `naive_merge` function.
//! Splits text by delimiter, then merges sections until token limit
//! is reached, with configurable overlap between chunks.

use super::tokenizer::token_count;
use super::{chunk_id, ChunkStrategy};
use crate::{Chunk, Document, ParserConfig, Result};
use std::collections::HashMap;

pub struct NaiveChunker;

impl NaiveChunker {
    pub fn new() -> Self {
        Self
    }
}

impl ChunkStrategy for NaiveChunker {
    fn chunk(&self, doc: &Document, config: &ParserConfig) -> Result<Vec<Chunk>> {
        let sections: Vec<&str> = doc
            .content
            .split(&config.delimiter)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        if sections.is_empty() {
            return Ok(vec![]);
        }

        let mut chunks = Vec::new();
        let mut current = String::new();
        let mut pos = 0;
        let max_tokens = config.chunk_token_num;
        let overlap_tokens = (max_tokens as f32 * config.overlapped_percent) as usize;

        for section in &sections {
            let section_tokens = token_count(section);
            let current_tokens = token_count(&current);

            // If adding this section would exceed the limit, flush current chunk
            if current_tokens + section_tokens > max_tokens && !current.is_empty() {
                chunks.push(build_chunk(doc, &current, pos, max_tokens));
                pos += 1;

                // Start new chunk with overlap: carry over last N tokens
                current = if overlap_tokens > 0 && current_tokens > overlap_tokens {
                    extract_overlap(&current, overlap_tokens)
                } else {
                    String::new()
                };
            }

            if !current.is_empty() {
                current.push_str(&config.delimiter);
            }
            current.push_str(section);
        }

        // Don't forget the last chunk
        if !current.trim().is_empty() {
            chunks.push(build_chunk(doc, &current, pos, max_tokens));
        }

        Ok(chunks)
    }
}

fn build_chunk(doc: &Document, content: &str, position: usize, _max_tokens: usize) -> Chunk {
    Chunk {
        id: chunk_id(&doc.id, position),
        content: content.to_string(),
        doc_id: doc.id,
        position,
        token_count: token_count(content),
        embedding: None,
        metadata: {
            let mut m = HashMap::new();
            m.insert("file_name".to_string(), doc.name.clone());
            m
        },
    }
}

/// Extract the last ~overlap_tokens tokens worth of text.
fn extract_overlap(text: &str, overlap_tokens: usize) -> String {
    let sections: Vec<&str> = text.split('\n').collect();
    let mut result = String::new();
    for section in sections.iter().rev() {
        if token_count(&result) >= overlap_tokens {
            break;
        }
        if !result.is_empty() {
            result.insert_str(0, "\n");
        }
        result.insert_str(0, section);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn test_doc(text: &str) -> Document {
        Document {
            id: Uuid::new_v4(),
            name: "test.txt".to_string(),
            content: text.to_string(),
            mime_type: "text/plain".to_string(),
            size: text.len(),
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn test_empty_document() {
        let chunker = NaiveChunker::new();
        let doc = test_doc("");
        let chunks = chunker.chunk(&doc, &ParserConfig::default()).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_single_chunk() {
        let chunker = NaiveChunker::new();
        let doc = test_doc("Short text that fits in one chunk.");
        let chunks = chunker.chunk(&doc, &ParserConfig::default()).unwrap();
        assert_eq!(chunks.len(), 1);
    }
}
