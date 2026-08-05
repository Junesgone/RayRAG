//! Title chunker — RAGFlow `rag/flow/chunker/title_chunker/` 的 Rust 实现。
//!
//! 两种方法：
//!   - `hierarchy`：按标题层级构建树，每个标题节点与其正文构成 chunk；
//!     目标层级由 `hierarchy` 参数决定（clamp 1..=levels.len()），
//!     深度超过目标层级的标题并入正文。`include_heading_content` 控制
//!     标题文本是否包含在 chunk 内容里。
//!   - `group`：按标题分组，相邻同组内容合并，直到 token 数达到下限 32
//!     或（上限 1024 且处于同一 section）。对齐 RAGFlow group_chunker 的
//!     固定常量（不随 chunk_token_size 变化）。
//!
//! 标题来源：
//!   1. PDF 大纲：`doc.metadata["__outline__"]`（JSON 数组
//!      `[{title, depth, page}]`，depth 0-based，来自 pdf.rs extract_outlines）。
//!   2. fallback：从正文行解析标题（`# ` / `## `、`一、`/`1.` 等枚举行）。

use super::tokenizer::token_count;
use super::{ChunkStrategy, chunk_id};
use crate::{Chunk, Document, ParserConfig, Result};
use serde_json::Value;
use std::collections::HashMap;

/// 标题条目（来自 PDF 大纲或正文解析）。
struct Heading {
    title: String,
    depth: usize, // 0-based
}

/// 一个标题节点及其正文。
struct Section {
    heading: String,
    depth: usize,
    body: String,
}

impl Section {
    fn render(&self, include_heading: bool) -> String {
        if include_heading && !self.heading.is_empty() {
            format!("{}\n{}", self.heading, self.body.trim())
        } else {
            self.body.trim().to_string()
        }
    }
}

/// TitleChunker。
#[derive(Default)]
pub struct TitleChunker;

impl TitleChunker {
    pub fn new() -> Self {
        Self
    }

    /// 从 PDF 大纲元数据解析标题（`__outline__` JSON）。
    fn outlines_from_metadata(doc: &Document) -> Vec<Heading> {
        let Some(raw) = doc.metadata.get(super::OUTLINE_METADATA) else {
            return vec![];
        };
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            return vec![];
        };
        let Some(arr) = value.as_array() else {
            return vec![];
        };
        arr.iter()
            .filter_map(|v| {
                let title = v.get("title").and_then(Value::as_str)?.to_string();
                let depth = v.get("depth").and_then(Value::as_u64).unwrap_or(0) as usize;
                Some(Heading { title, depth })
            })
            .collect()
    }

    /// 正文行解析标题：`# `、`## `、`一、`/`（一）`、`1.`、`1、`。
    fn outlines_from_content(doc: &Document) -> Vec<Heading> {
        let mut out = Vec::new();
        for line in doc.content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let depth = if let Some(rest) = trimmed.strip_prefix("### ") {
                Some((3, rest))
            } else if let Some(rest) = trimmed.strip_prefix("## ") {
                Some((2, rest))
            } else if let Some(rest) = trimmed.strip_prefix("# ") {
                Some((1, rest))
            } else if let Some(rest) = trimmed.strip_prefix("（") {
                rest.split('）').next().and_then(|n| {
                    n.parse::<u32>().ok().map(|_| (2, rest))
                })
            } else if let Some(rest) = trimmed.strip_prefix("一、")
                .or_else(|| trimmed.strip_prefix("二、"))
                .or_else(|| trimmed.strip_prefix("三、"))
                .or_else(|| trimmed.strip_prefix("四、"))
                .or_else(|| trimmed.strip_prefix("五、"))
            {
                Some((1, rest))
            } else {
                None
            };
            if let Some((d, rest)) = depth {
                let title = rest.trim().trim_end_matches('：').trim().to_string();
                if !title.is_empty() {
                    out.push(Heading { title, depth: d - 1 });
                }
            }
        }
        out
    }

    /// 获取标题列表：优先 PDF 大纲，fallback 正文解析。
    fn headings(doc: &Document) -> Vec<Heading> {
        let from_meta = Self::outlines_from_metadata(doc);
        if !from_meta.is_empty() {
            return from_meta;
        }
        Self::outlines_from_content(doc)
    }

    /// 归一化行文本用于标题匹配：去 markdown 前缀（# / ## / ###）、列表符。
    fn normalize_line(line: &str) -> &str {
        let t = line.trim();
        t.strip_prefix("### ")
            .or_else(|| t.strip_prefix("## "))
            .or_else(|| t.strip_prefix("# "))
            .or_else(|| t.strip_prefix("- "))
            .or_else(|| t.strip_prefix("* "))
            .unwrap_or(t)
    }

    /// 把文档切分为 (标题, 正文) 段。无标题时整篇作为一个段。
    fn sections(doc: &Document, headings: &[Heading]) -> Vec<Section> {
        if headings.is_empty() {
            return vec![Section {
                heading: String::new(),
                depth: 0,
                body: doc.content.clone(),
            }];
        }
        let mut out: Vec<Section> = Vec::new();
        let mut body_lines: Vec<&str> = Vec::new();
        let mut pending_heading: Option<&Heading> = None;
        let mut heading_iter = headings.iter().peekable();

        for line in doc.content.lines() {
            let trimmed = line.trim();
            let normalized = Self::normalize_line(line);
            // 命中标题行 → 开启新段（始终匹配下一个未消费的标题）
            let is_heading = heading_iter
                .peek()
                .map(|h| normalized == h.title)
                .unwrap_or(false);
            if is_heading {
                // flush 上一段
                if let Some(h) = pending_heading.take() {
                    out.push(Section {
                        heading: h.title.clone(),
                        depth: h.depth,
                        body: body_lines.join("\n"),
                    });
                    body_lines.clear();
                }
                pending_heading = Some(heading_iter.next().unwrap());
                continue;
            }
            if pending_heading.is_none() {
                // 第一个标题前的引言
                body_lines.push(line);
                continue;
            }
            body_lines.push(line);
        }
        if let Some(h) = pending_heading {
            out.push(Section {
                heading: h.title.clone(),
                depth: h.depth,
                body: body_lines.join("\n"),
            });
        } else if !body_lines.is_empty() {
            // 只有引言没有标题
            out.push(Section {
                heading: String::new(),
                depth: 0,
                body: body_lines.join("\n"),
            });
        }
        out
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

    /// hierarchy 模式：目标层级内的标题各成一块，更深的并入父块正文。
    fn chunk_hierarchy(&self, doc: &Document, config: &ParserConfig) -> Vec<Chunk> {
        let headings = Self::headings(doc);
        let sections = Self::sections(doc, &headings);

        // 目标层级：hierarchy clamp 1..=levels.len()（无 levels 时取 1）
        let levels_len = if config.title_levels.is_empty() {
            1
        } else {
            config.title_levels.len()
        };
        let target = config
            .hierarchy
            .unwrap_or(1)
            .clamp(1, levels_len);

        // 组装：深度 >= target 的段并入最近的深度 < target 的父段。
        let mut merged: Vec<Section> = Vec::new();
        for section in sections {
            if section.depth < target {
                merged.push(section);
            } else if let Some(last) = merged.last_mut() {
                if !last.body.is_empty() {
                    last.body.push('\n');
                }
                last.body.push_str(&section.render(true));
            } else {
                merged.push(section);
            }
        }
        if merged.is_empty() {
            return vec![];
        }

        let mut chunks: Vec<Chunk> = Vec::new();
        for (i, section) in merged.iter().enumerate() {
            let content = section.render(config.include_heading_content);
            if content.trim().is_empty() {
                continue;
            }
            chunks.push(self.build(doc, &content, i));
        }

        if config.root_chunk_as_heading {
            Self::apply_root_as_heading(chunks, doc)
        } else {
            chunks
        }
    }

    /// group 模式：相邻段按 section 归属合并，直到下限 32 token 或
    /// （上限 1024 token 且同 section）。
    fn chunk_group(&self, doc: &Document, config: &ParserConfig) -> Vec<Chunk> {
        let headings = Self::headings(doc);
        let sections = Self::sections(doc, &headings);
        let mut chunks: Vec<Chunk> = Vec::new();
        let mut current = String::new();
        let mut pos = 0usize;
        let mut section_id = String::new();

        for section in sections {
            let text = section.render(config.include_heading_content);
            if text.trim().is_empty() {
                continue;
            }
            let sid = if section.heading.is_empty() {
                String::new()
            } else {
                section.heading.clone()
            };
            let tk = token_count(&current);
            if !current.is_empty()
                && ((tk >= 32 && sid != section_id) || tk >= 1024)
            {
                chunks.push(self.build(doc, &current, pos));
                pos += 1;
                current.clear();
            }
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(&text);
            section_id = sid;
        }
        if !current.trim().is_empty() {
            chunks.push(self.build(doc, &current, pos));
        }
        if config.root_chunk_as_heading {
            Self::apply_root_as_heading(chunks, doc)
        } else {
            chunks
        }
    }

    /// root_chunk_as_heading：第一块文本前置到所有后续块，丢弃根块。
    fn apply_root_as_heading(mut chunks: Vec<Chunk>, doc: &Document) -> Vec<Chunk> {
        if chunks.len() <= 1 {
            return chunks;
        }
        let root = chunks.remove(0);
        let root_text = root.content.trim().to_string();
        if root_text.is_empty() {
            return chunks;
        }
        let mut out = Vec::new();
        for (i, mut chunk) in chunks.into_iter().enumerate() {
            chunk.content = format!("{}\n{}", root_text, chunk.content);
            chunk.id = chunk_id(&doc.id, i);
            chunk.token_count = token_count(&chunk.content);
            out.push(chunk);
        }
        out
    }
}

impl ChunkStrategy for TitleChunker {
    fn chunk(&self, doc: &Document, config: &ParserConfig) -> Result<Vec<Chunk>> {
        let method = if config.chunk_method == "title" {
            // chunk_method 已由 chunker_for 分发；此处按 delimiter_mode 细分
            config.delimiter_mode.as_str()
        } else {
            "hierarchy"
        };
        let chunks = if method == "group" {
            self.chunk_group(doc, config)
        } else {
            self.chunk_hierarchy(doc, config)
        };
        Ok(chunks)
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
            name: "test.md".into(),
            content: content.into(),
            mime_type: "text/markdown".into(),
            size: content.len(),
            metadata: HashMap::new(),
        }
    }

    const SAMPLE: &str = "# 第一章\n\n这是第一章的正文内容。\n\n## 1.1 小节\n\n这是小节正文。\n\n# 第二章\n\n这是第二章的正文。\n";

    #[test]
    fn hierarchy_splits_by_heading_levels() {
        let d = doc(SAMPLE);
        let mut cfg = ParserConfig::default();
        cfg.chunk_method = "title".into();
        cfg.delimiter_mode = "hierarchy".into();
        cfg.hierarchy = Some(2);
        let out = TitleChunker.chunk(&d, &cfg).unwrap();
        // 第一章(含1.1) + 第二章 = 2 块
        assert_eq!(out.len(), 2);
        assert!(out[0].content.contains("第一章"));
        assert!(out[0].content.contains("1.1 小节"));
        assert!(out[1].content.contains("第二章"));
    }

    #[test]
    fn hierarchy_depth_target_merges_deeper_heads() {
        let d = doc(SAMPLE);
        let mut cfg = ParserConfig::default();
        cfg.chunk_method = "title".into();
        cfg.delimiter_mode = "hierarchy".into();
        cfg.hierarchy = Some(1); // 只有一级标题成块，1.1 并入第一章
        let out = TitleChunker.chunk(&d, &cfg).unwrap();
        assert_eq!(out.len(), 2);
        assert!(out[0].content.contains("1.1 小节"));
    }

    #[test]
    fn group_merges_small_sections() {
        let d = doc(SAMPLE);
        let mut cfg = ParserConfig::default();
        cfg.chunk_method = "title".into();
        cfg.delimiter_mode = "group".into();
        let out = TitleChunker.chunk(&d, &cfg).unwrap();
        // 内容很少 → 全部合并为一块
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn include_heading_content_controls_heading_in_body() {
        let d = doc(SAMPLE);
        let mut cfg = ParserConfig::default();
        cfg.chunk_method = "title".into();
        cfg.delimiter_mode = "hierarchy".into();
        cfg.include_heading_content = true;
        let out = TitleChunker.chunk(&d, &cfg).unwrap();
        assert!(out[0].content.starts_with("第一章"));
    }

    #[test]
    fn root_as_heading_prepends_root_text() {
        let d = doc(SAMPLE);
        let mut cfg = ParserConfig::default();
        cfg.chunk_method = "title".into();
        cfg.delimiter_mode = "hierarchy".into();
        cfg.hierarchy = Some(2);
        cfg.root_chunk_as_heading = true;
        let out = TitleChunker.chunk(&d, &cfg).unwrap();
        // 根块被前置到后续块并丢弃 → 只剩第二章块
        assert_eq!(out.len(), 1);
        assert!(out[0].content.contains("第一章"));
        assert!(out[0].content.contains("第二章"));
    }

    #[test]
    fn pdf_outline_metadata_is_preferred() {
        let mut d = doc("正文内容");
        d.metadata.insert(
            super::super::OUTLINE_METADATA.into(),
            r#"[{"title":"节A","depth":0,"page":1},{"title":"节B","depth":0,"page":2}]"#.into(),
        );
        d.content = "节A\n内容A\n节B\n内容B\n".into();
        let mut cfg = ParserConfig::default();
        cfg.chunk_method = "title".into();
        let out = TitleChunker.chunk(&d, &cfg).unwrap();
        assert!(out.len() >= 2);
    }
}
