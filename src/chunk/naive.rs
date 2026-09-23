//! Naive chunker — token-based splitting with overlap.
//!
//! Ported from RAGFlow's `rag/nlp/__init__.py` `naive_merge` function.
//! Splits text by delimiter, then merges sections until token limit
//! is reached, with configurable overlap between chunks.

use super::tokenizer::token_count;
use super::{ChunkStrategy, chunk_id};
use crate::{Chunk, Document, ParserConfig, Result};

#[derive(Default)]
pub struct NaiveChunker;

impl NaiveChunker {
    pub fn new() -> Self {
        Self
    }
}

impl ChunkStrategy for NaiveChunker {
    fn chunk(&self, doc: &Document, config: &ParserConfig) -> Result<Vec<Chunk>> {
        let position_aware = doc
            .metadata
            .contains_key(super::RAGFLOW_POSITION_TAGS_METADATA);
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
            let section_tokens = visible_token_count(section, position_aware);
            let current_tokens = visible_token_count(&current, position_aware);

            // If adding this section would exceed the limit, flush current chunk
            if current_tokens + section_tokens > max_tokens && !current.is_empty() {
                chunks.push(build_chunk(doc, &current, pos, max_tokens, position_aware));
                pos += 1;

                // Start new chunk with overlap: carry over last N tokens
                current = if overlap_tokens > 0 && current_tokens > overlap_tokens {
                    extract_overlap(&current, overlap_tokens, position_aware)
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
            chunks.push(build_chunk(doc, &current, pos, max_tokens, position_aware));
        }

        Ok(chunks)
    }
}

fn visible_token_count(text: &str, position_aware: bool) -> usize {
    if position_aware {
        token_count(&super::position::remove(text))
    } else {
        token_count(text)
    }
}

fn build_chunk(
    doc: &Document,
    content: &str,
    position: usize,
    _max_tokens: usize,
    position_aware: bool,
) -> Chunk {
    let position_metadata = position_aware.then(|| super::position::extract(content));
    let content = if position_aware {
        super::position::remove(content)
    } else {
        content.to_owned()
    };
    let mut metadata = doc.metadata.clone();
    metadata.remove(super::RAGFLOW_POSITION_TAGS_METADATA);
    metadata.insert("file_name".to_string(), doc.name.clone());
    if let Some(position_metadata) =
        position_metadata.filter(|metadata| !metadata.positions.is_empty())
    {
        metadata.insert(
            "position_int".to_string(),
            serde_json::to_string(&position_metadata.positions)
                .expect("fixed position arrays always serialize"),
        );
        metadata.insert(
            "page_num_int".to_string(),
            serde_json::to_string(&position_metadata.page_numbers)
                .expect("page numbers always serialize"),
        );
        metadata.insert(
            "top_int".to_string(),
            serde_json::to_string(&position_metadata.tops)
                .expect("top coordinates always serialize"),
        );
    }
    let token_count = token_count(&content);
    Chunk {
        id: chunk_id(&doc.id, position),
        content,
        content_type: "text".into(),
        doc_id: doc.id,
        position,
        token_count,
        embedding: None,
        metadata,
    }
}

/// Extract the last ~overlap_tokens tokens worth of text.
fn extract_overlap(text: &str, overlap_tokens: usize, position_aware: bool) -> String {
    let visible = if position_aware {
        super::position::remove(text)
    } else {
        text.to_owned()
    };
    let sections: Vec<&str> = visible.split('\n').collect();
    let mut result = String::new();
    for section in sections.iter().rev() {
        if token_count(&result) >= overlap_tokens {
            break;
        }
        if !result.is_empty() {
            result.insert(0, '\n');
        }
        result.insert_str(0, section);
    }
    result
}

#[cfg(test)]
mod single_tests {
    use super::*;
    use crate::{Document, ParserConfig};

    fn doc(content: &str) -> Document {
        Document {
            id: uuid::Uuid::new_v4(),
            name: "sample.txt".to_string(),
            content: content.to_string(),
            mime_type: "text/plain".to_string(),
            size: content.len(),
            metadata: Default::default(),
        }
    }

    #[test]
    fn single_chunker_returns_whole_document_as_one_chunk() {
        let d = doc("第一段。\n第二段。\n第三段。");
        let config = ParserConfig::default();
        let chunks = SingleChunker::new().chunk(&d, &config).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "第一段。\n第二段。\n第三段。");
        assert_eq!(chunks[0].position, 0);
    }

    #[test]
    fn single_chunker_skips_blank_documents() {
        let d = doc("   ");
        let config = ParserConfig::default();
        assert!(SingleChunker::new().chunk(&d, &config).unwrap().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
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

    #[test]
    fn ragflow_position_tags_become_chunk_metadata_and_leave_visible_text() {
        let chunker = NaiveChunker::new();
        let mut doc = test_doc(
            "First positioned paragraph@@1\t10.9\t100.2\t20.8\t40.7##\n\
             Second positioned paragraph@@2\t50.0\t150.0\t60.0\t90.0##",
        );
        doc.metadata.insert(
            crate::chunk::RAGFLOW_POSITION_TAGS_METADATA.to_string(),
            "true".to_string(),
        );
        let chunks = chunker.chunk(&doc, &ParserConfig::default()).unwrap();

        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].content,
            "First positioned paragraph\nSecond positioned paragraph"
        );
        assert_eq!(
            chunks[0].metadata.get("position_int").map(String::as_str),
            Some("[[1,10,100,20,40],[2,50,150,60,90]]")
        );
        assert_eq!(
            chunks[0].metadata.get("page_num_int").map(String::as_str),
            Some("[1,2]")
        );
        assert_eq!(
            chunks[0].metadata.get("top_int").map(String::as_str),
            Some("[20,60]")
        );
        assert!(
            !chunks[0]
                .metadata
                .contains_key(crate::chunk::RAGFLOW_POSITION_TAGS_METADATA)
        );
        assert_eq!(chunks[0].token_count, token_count(&chunks[0].content));
    }

    #[test]
    fn ordinary_text_preserves_position_like_literal_without_parser_marker() {
        let chunker = NaiveChunker::new();
        let literal = "Protocol example: @@1\t10.0\t20.0\t30.0\t40.0##";
        let doc = test_doc(literal);
        let chunks = chunker.chunk(&doc, &ParserConfig::default()).unwrap();

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, literal);
        assert!(!chunks[0].metadata.contains_key("position_int"));
        assert!(!chunks[0].metadata.contains_key("page_num_int"));
        assert!(!chunks[0].metadata.contains_key("top_int"));
    }
}


/// Whole-document-as-one-chunk strategy — mirrors RAGFlow `rag/app/one.py`
/// ("One file forms a chunk which maintains original text order").
pub struct SingleChunker;

impl Default for SingleChunker {
    fn default() -> Self {
        Self::new()
    }
}

impl SingleChunker {
    pub fn new() -> Self {
        Self
    }
}

impl ChunkStrategy for SingleChunker {
    fn chunk(&self, doc: &Document, _config: &ParserConfig) -> Result<Vec<Chunk>> {
        let content = doc.content.trim();
        if content.is_empty() {
            return Ok(vec![]);
        }
        let position_aware = doc
            .metadata
            .contains_key(super::RAGFLOW_POSITION_TAGS_METADATA);
        Ok(vec![Chunk {
            id: chunk_id(&doc.id, 0),
            content: content.to_string(),
            content_type: "text".to_string(),
            doc_id: doc.id,
            position: 0,
            token_count: token_count(content),
            embedding: None,
            metadata: if position_aware {
                let mut meta = doc.metadata.clone();
                meta.insert("page_num_int".to_string(), "[]".to_string());
                meta.insert("top_int".to_string(), "[]".to_string());
                meta
            } else {
                doc.metadata.clone()
            },
        }])
    }
}
