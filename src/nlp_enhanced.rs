//! Enhanced NLP pipeline — semantic chunking, sentence window, query tree.
//! Replaces RAGFlow's `rag/nlp/__init__.py` (1627 lines) chunking strategies.

use crate::chunk::token_count;
use crate::{Chunk, Document, ParserConfig};
use std::collections::HashMap;

/// Enhanced chunking strategies.
pub enum ChunkMethod {
    /// Naive token-based (existing)
    Naive,
    /// Sentence-based: split on sentence boundaries
    Sentence,
    /// Paragraph-based: split on double newlines
    Paragraph,
    /// Sliding window: overlapping windows of sentences
    SlidingWindow,
    /// Semantic: split on markdown headers (# ## ###)
    Semantic,
    /// Pattern-based: split on custom delimiter
    Pattern(String),
}

/// Apply enhanced chunking to a document.
pub fn chunk_document(
    doc: &Document,
    config: &ParserConfig,
    method: ChunkMethod,
) -> crate::Result<Vec<Chunk>> {
    match method {
        ChunkMethod::Naive => chunk_naive(doc, config),
        ChunkMethod::Sentence => chunk_sentence(doc, config),
        ChunkMethod::Paragraph => chunk_paragraph(doc, config),
        ChunkMethod::SlidingWindow => chunk_sliding_window(doc, config),
        ChunkMethod::Semantic => chunk_semantic(doc, config),
        ChunkMethod::Pattern(pat) => chunk_pattern(doc, config, &pat),
    }
}

// ── Naive (existing behavior) ──────────────────────────────────

fn chunk_naive(doc: &Document, config: &ParserConfig) -> crate::Result<Vec<Chunk>> {
    let max_tokens = config.chunk_token_num;
    let overlap_ratio = config.overlapped_percent;
    let overlap_tokens = (max_tokens as f32 * overlap_ratio) as usize;

    let mut chunks = Vec::new();
    let lines: Vec<&str> = doc.content.lines().collect();
    let mut current = String::new();
    let mut pos = 0;

    for line in &lines {
        let new_tokens = token_count(current.as_str()) + token_count(line);
        if new_tokens > max_tokens && !current.is_empty() {
            chunks.push(build_chunk(doc, &current, pos));
            pos += 1;

            // Overlap: keep last portion
            let overlap = extract_overlap(&current, overlap_tokens);
            current = overlap;
            current.push_str(line);
            current.push('\n');
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }

    if !current.trim().is_empty() {
        chunks.push(build_chunk(doc, &current, pos));
    }

    Ok(chunks)
}

// ── Sentence-based ─────────────────────────────────────────────

fn chunk_sentence(doc: &Document, config: &ParserConfig) -> crate::Result<Vec<Chunk>> {
    let max_chars = config.chunk_token_num * 4; // approximate: 4 chars per token
    let sentences = split_sentences(&doc.content);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut pos = 0;

    for sentence in sentences {
        if current.len() + sentence.len() > max_chars && !current.is_empty() {
            chunks.push(build_chunk(doc, &current, pos));
            pos += 1;
            current.clear();
        }
        current.push_str(&sentence);
        current.push(' ');
    }

    if !current.trim().is_empty() {
        chunks.push(build_chunk(doc, &current, pos));
    }

    Ok(chunks)
}

/// Split text into sentences (handles . ! ? 。！？).
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?' | '。' | '！' | '？') && current.len() > 5 {
            sentences.push(current.trim().to_string());
            current.clear();
        }
    }

    if !current.trim().is_empty() {
        sentences.push(current.trim().to_string());
    }

    sentences
}

// ── Paragraph-based ────────────────────────────────────────────

fn chunk_paragraph(doc: &Document, config: &ParserConfig) -> crate::Result<Vec<Chunk>> {
    let max_chars = config.chunk_token_num * 4;
    let paragraphs: Vec<&str> = doc.content.split("\n\n").collect();
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut pos = 0;

    for para in paragraphs {
        let trimmed = para.trim();
        if trimmed.is_empty() {
            continue;
        }

        if current.len() + trimmed.len() > max_chars && !current.is_empty() {
            chunks.push(build_chunk(doc, &current, pos));
            pos += 1;
            current.clear();
        }

        current.push_str(trimmed);
        current.push_str("\n\n");
    }

    if !current.trim().is_empty() {
        chunks.push(build_chunk(doc, &current, pos));
    }

    Ok(chunks)
}

// ── Sliding Window ─────────────────────────────────────────────

fn chunk_sliding_window(doc: &Document, config: &ParserConfig) -> crate::Result<Vec<Chunk>> {
    let max_chars = config.chunk_token_num * 4;
    let window_size = max_chars;
    let stride = (max_chars as f32 * (1.0 - config.overlapped_percent)) as usize;
    let text = &doc.content;
    let mut pos = 0;

    if text.len() <= window_size {
        return Ok(vec![build_chunk(doc, text, 0)]);
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = (start + window_size).min(text.len());
        let slice = &text[start..end];
        chunks.push(build_chunk(doc, slice, pos));
        pos += 1;
        start += stride;
        if stride == 0 {
            break;
        }
    }

    Ok(chunks)
}

// ── Semantic (markdown-header aware) ───────────────────────────

fn chunk_semantic(doc: &Document, config: &ParserConfig) -> crate::Result<Vec<Chunk>> {
    let max_chars = config.chunk_token_num * 4;
    let text = &doc.content;
    let re = regex::Regex::new(r"(?m)^#{1,3}\s+.+$")?;
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut pos = 0;

    // Split at markdown headers
    let sections = re.split(text);
    let headers: Vec<&str> = re.find_iter(text).map(|m| m.as_str()).collect();

    for (i, section) in sections.enumerate() {
        let content = if i < headers.len() {
            format!("{}\n{}", headers[i], section.trim())
        } else {
            section.trim().to_string()
        };

        if current.len() + content.len() > max_chars && !current.is_empty() {
            chunks.push(build_chunk(doc, &current, pos));
            pos += 1;
            current.clear();
        }

        current.push_str(&content);
        current.push('\n');
    }

    if !current.trim().is_empty() {
        chunks.push(build_chunk(doc, &current, pos));
    }

    Ok(chunks)
}

// ── Pattern-based ──────────────────────────────────────────────

fn chunk_pattern(
    doc: &Document,
    _config: &ParserConfig,
    pattern: &str,
) -> crate::Result<Vec<Chunk>> {
    let re = regex::Regex::new(pattern)?;
    let mut pos = 0;
    let chunks: Vec<Chunk> = re
        .split(&doc.content)
        .filter(|s| !s.trim().is_empty())
        .enumerate()
        .map(|(i, s)| {
            pos = i;
            build_chunk(doc, s.trim(), i)
        })
        .collect();
    Ok(chunks)
}

// ── Helpers ────────────────────────────────────────────────────

fn build_chunk(doc: &Document, content: &str, position: usize) -> Chunk {
    Chunk {
        id: uuid::Uuid::new_v4().to_string(),
        content: content.to_string(),
        content_type: "text".into(),
        doc_id: doc.id,
        position,
        token_count: token_count(content),
        embedding: None,
        metadata: {
            let mut m = HashMap::new();
            m.insert("file_name".to_string(), doc.name.clone());
            m.insert("chunk_method".to_string(), "enhanced".into());
            m
        },
    }
}

fn extract_overlap(text: &str, overlap_tokens: usize) -> String {
    let lines: Vec<&str> = text.lines().rev().collect();
    let mut overlap = String::new();
    let mut count = 0;
    for line in lines {
        let tk = token_count(line);
        if count + tk > overlap_tokens {
            break;
        }
        overlap = format!("{}\n{}", line, overlap);
        count += tk;
    }
    overlap.trim().to_string()
}
