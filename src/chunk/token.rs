//! Token chunker — RAGFlow `rag/flow/chunker/token_chunker.py` 的 Rust 实现。
//!
//! 三种 delimiter_mode：
//!   - `one`：整篇文档作为一个 chunk。
//!   - `token_size`：无自定义分隔符时按 token 阈值合并（naive_merge 语义），
//!     阈值 = chunk_token_size * (100 - overlapped_percent) / 100；
//!     有自定义分隔符时先按分隔符拆成 section 再按 token 阈值合并。
//!   - `delimiter`：按编译后的分隔符正则拆分，分隔符保留在各 chunk 尾部。
//!
//! 自定义分隔符：反引号包裹（`` `...` ``）编译为正则，`|` 连接、按长度倒序，
//! 保证最长匹配优先（对齐 token_chunker.py `_compile_delimiter_pattern`）。
//! 重叠率 normalize_overlapped_percent：0<p<1 → ×100 → int → clamp [0,90]（百分数）。

use super::tokenizer::token_count;
use super::{ChunkStrategy, chunk_id};
use crate::{Chunk, Document, ParserConfig, Result};
use regex::Regex;
use std::collections::HashMap;

/// TokenChunker。
#[derive(Default)]
pub struct TokenChunker;

impl TokenChunker {
    pub fn new() -> Self {
        Self
    }

    /// 编译反引号包裹的自定义分隔符 → `|` 连接的正则（最长优先）。
    fn compile_delimiters(delimiters: &[String]) -> Option<Regex> {
        let mut custom: Vec<&str> = Vec::new();
        for raw in delimiters {
            for m in Regex::new(r"`([^`]+)`").unwrap().captures_iter(raw) {
                if let Some(c) = m.get(1) {
                    custom.push(c.as_str());
                }
            }
        }
        if custom.is_empty() {
            return None;
        }
        custom.sort_by_key(|s| std::cmp::Reverse(s.len()));
        let mut set: Vec<String> = Vec::new();
        for c in custom {
            if !set.iter().any(|s| s == c) {
                set.push(c.to_string());
            }
        }
        Regex::new(
            &set.iter()
                .map(|s| regex::escape(s))
                .collect::<Vec<_>>()
                .join("|"),
        )
        .ok()
    }

    /// 按模式拆分，分隔符并入其前一段末尾（对齐 `_split_text_by_pattern`：
    /// `re.split(r"(%s)" % pattern)` 两两配对，chunk += delimiter）。
    fn split_keep_delimiters(text: &str, pattern: Option<&Regex>) -> Vec<String> {
        let Some(re) = pattern else {
            return vec![text.to_string()];
        };
        let mut out = Vec::new();
        let mut last = 0;
        for cap in re.find_iter(text) {
            let end = cap.end();
            out.push(text[last..end].to_string());
            last = end;
        }
        if last < text.len() {
            out.push(text[last..].to_string());
        }
        out
    }

    /// 编译子分隔符：裸模式 escape 后按长度倒序 join（对齐 token_chunker.py
    /// 的 `custom_pattern = "|".join(re.escape(t) for t in sorted(set(...), key=len, reverse=True))`）。
    fn compile_children_pattern(delimiters: &[String]) -> Option<Regex> {
        let mut set: Vec<String> = Vec::new();
        for raw in delimiters {
            let t = raw.trim();
            if !t.is_empty() && !set.iter().any(|s| s == t) {
                set.push(t.to_string());
            }
        }
        if set.is_empty() {
            return None;
        }
        set.sort_by_key(|s| std::cmp::Reverse(s.len()));
        Regex::new(
            &set.iter()
                .map(|s| regex::escape(s))
                .collect::<Vec<_>>()
                .join("|"),
        )
        .ok()
    }

    /// normalize_overlapped_percent：0<p<1 → ×100 → int；其余按百分数直接 int
    /// （对齐 RAGFlow：int(p)，clamp [0,90]；1.5 视为 1.5% → 1）。
    fn normalize_overlapped(value: f32) -> usize {
        let mut v = value as f64;
        if (0.0..1.0).contains(&v) {
            v *= 100.0;
        }
        let v = v as i64;
        v.clamp(0, 90) as usize
    }

    /// token_size 模式的合并逻辑（naive_merge 语义）。
    fn merge_by_token_size(
        &self,
        doc: &Document,
        config: &ParserConfig,
        sections: Vec<String>,
    ) -> Vec<Chunk> {
        let max_tokens = config.chunk_token_num;
        let overlap_pct = Self::normalize_overlapped(config.overlapped_percent);
        let _threshold = max_tokens * (100 - overlap_pct) / 100;
        let overlap_tokens = max_tokens * overlap_pct / 100;

        let mut chunks: Vec<Chunk> = Vec::new();
        let mut current = String::new();
        let mut pos = 0usize;
        for section in &sections {
            if current.is_empty() && section.is_empty() {
                continue;
            }
            if !current.is_empty() && token_count(&current) + token_count(section) > max_tokens {
                chunks.push(self.build(doc, &current, pos));
                pos += 1;
                // 携带重叠尾部
                current = if overlap_tokens > 0 {
                    carry_overlap(&current, overlap_tokens)
                } else {
                    String::new()
                };
            }
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(section);
        }
        if !current.trim().is_empty() {
            chunks.push(self.build(doc, &current, pos));
        }
        chunks
    }

    fn build(&self, doc: &Document, content: &str, position: usize) -> Chunk {
        Chunk {
            id: chunk_id(&doc.id, position),
            content: content.to_string(),
            content_type: "text".into(),
            doc_id: doc.id,
            position,
            token_count: token_count(content),
            embedding: None,
            metadata: HashMap::new(),
        }
    }

    /// children_delimiters 二次拆分：文本块按子分隔符拆分，父块原文挂 `mom`
    /// （对齐 token_chunker.py：`{"text": text, "mom": c}`）。
    fn apply_children(
        &self,
        doc: &Document,
        chunks: Vec<Chunk>,
        children: &[String],
    ) -> Vec<Chunk> {
        let Some(pattern) = Self::compile_children_pattern(children) else {
            return chunks;
        };
        let mut out = Vec::new();
        let mut next = chunks.len() + 1;
        for chunk in chunks {
            let parts = Self::split_keep_delimiters(&chunk.content, Some(&pattern));
            if parts.len() <= 1 {
                out.push(chunk);
                continue;
            }
            for part in parts {
                if part.trim().is_empty() {
                    continue;
                }
                let mut meta = HashMap::new();
                meta.insert("mom".into(), chunk.content.clone());
                let mut child = self.build(doc, &part, next);
                child.metadata = meta;
                next += 1;
                out.push(child);
            }
        }
        out
    }
}

/// 从尾部截取 overlap_tokens 个 token 的近似文本（CJK 按 1.5 字符/token）。
fn carry_overlap(text: &str, overlap_tokens: usize) -> String {
    let mut count = 0usize;
    let mut cut = text.len();
    for (idx, ch) in text.char_indices().rev() {
        let t = if ch.is_ascii_alphanumeric() { 1 } else { 1 };
        count += t;
        if count >= overlap_tokens {
            cut = idx;
            break;
        }
    }
    text[cut..].to_string()
}

impl ChunkStrategy for TokenChunker {
    fn chunk(&self, doc: &Document, config: &ParserConfig) -> Result<Vec<Chunk>> {
        // 对齐 token_chunker.py：one → 整篇一块；有编译分隔符 → 按分隔符拆分
        // （分隔符并入段尾）；否则 → token 阈值合并（naive_merge 语义）。
        // delimiter_mode 仅影响 schema 校验，token_size/delimiter 行为一致。
        let chunks = if config.delimiter_mode == "one" {
            let content = doc.content.trim().to_string();
            if content.is_empty() {
                Vec::new()
            } else {
                vec![self.build(doc, &content, 0)]
            }
        } else {
            let pattern = Self::compile_delimiters(&config.delimiters);
            let has_delimiters =
                pattern.is_some() || config.delimiters.iter().any(|d| !d.trim().is_empty());
            if has_delimiters {
                let sections = Self::split_keep_delimiters(&doc.content, pattern.as_ref());
                let mut out = Vec::new();
                let mut pos = 0usize;
                for s in sections {
                    if s.trim().is_empty() {
                        continue;
                    }
                    out.push(self.build(doc, &s, pos));
                    pos += 1;
                }
                out
            } else {
                let sections: Vec<String> = doc
                    .content
                    .split(&config.delimiter)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if sections.is_empty() {
                    Vec::new()
                } else {
                    self.merge_by_token_size(doc, config, sections)
                }
            }
        };

        if !config.children_delimiters.is_empty() {
            Ok(self.apply_children(doc, chunks, &config.children_delimiters))
        } else {
            Ok(chunks)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, ParserConfig};
    use uuid::Uuid;

    fn doc(content: &str) -> Document {
        Document {
            id: Uuid::new_v4(),
            name: "test.txt".into(),
            content: content.into(),
            mime_type: "text/plain".into(),
            size: content.len(),
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn one_mode_returns_single_chunk() {
        let d = doc("hello world");
        let mut cfg = ParserConfig::default();
        cfg.delimiter_mode = "one".into();
        let out = TokenChunker.chunk(&d, &cfg).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, "hello world");
    }

    #[test]
    fn token_size_merges_until_limit() {
        // 每行一段（~1 token/段），阈值 2 → 每块 2 段 → 3 块
        let d = doc("aaa\nbbb\nccc\nddd\neee\nfff");
        let mut cfg = ParserConfig::default();
        cfg.delimiter_mode = "token_size".into();
        cfg.chunk_token_num = 2;
        cfg.overlapped_percent = 0.0;
        let out = TokenChunker.chunk(&d, &cfg).unwrap();
        assert_eq!(out.len(), 3);
        assert!(out[0].content.contains("aaa"));
        assert!(out[0].content.contains("bbb"));
        assert!(out[2].content.contains("fff"));
    }
    #[test]
    fn delimiter_mode_splits_keeping_delimiters() {
        let d = doc("第一节内容。第二节内容。第三节内容。");
        let mut cfg = ParserConfig::default();
        cfg.delimiter_mode = "delimiter".into();
        cfg.delimiters = vec!["`。`".into()];
        let out = TokenChunker.chunk(&d, &cfg).unwrap();
        assert!(out.len() >= 3);
        assert!(out[0].content.ends_with("。"));
    }

    #[test]
    fn normalize_overlapped_clamps() {
        assert_eq!(TokenChunker::normalize_overlapped(0.5), 50);
        assert_eq!(TokenChunker::normalize_overlapped(0.0), 0);
        assert_eq!(TokenChunker::normalize_overlapped(0.95), 90);
        assert_eq!(TokenChunker::normalize_overlapped(1.5), 1);
        assert_eq!(TokenChunker::normalize_overlapped(-1.0), 0);
    }

    #[test]
    fn children_delimiters_attach_mom() {
        let d = doc("甲、内容一。乙、内容二。");
        let mut cfg = ParserConfig::default();
        cfg.delimiter_mode = "one".into();
        cfg.children_delimiters = vec!["。".into()];
        let out = TokenChunker.chunk(&d, &cfg).unwrap();
        assert!(out.len() >= 2);
        assert!(out[0].metadata.contains_key("mom"));
    }

    #[test]
    fn backtick_pattern_longest_first() {
        let re = TokenChunker::compile_delimiters(&["`ab` `abc`".into()]).unwrap();
        // 长匹配优先：abc 而非 ab；分隔符并入前一段
        let parts = TokenChunker::split_keep_delimiters("xxabcyy", Some(&re));
        assert_eq!(parts.len(), 2);
        assert!(parts[0].ends_with("abc"));
        assert_eq!(parts[1], "yy");
    }
}
