# RayRAG

RAGFlow 解析管线 Rust 重写 + zvec 向量数据库。

## 架构

```
Document (.pdf/.docx/.txt/.md/.html)
    │
    ▼
┌─────────────────────────────────────┐
│  parser/  文档解析（端口 deepdoc）    │
│  ├── pdf.rs      PDF 文本提取        │
│  ├── docx.rs     Word 文档解析       │
│  ├── txt.rs      纯文本              │
│  ├── markdown.rs Markdown 透传       │
│  └── html.rs     HTML 标签剥离       │
└─────────────────────────────────────┘
    │ Document { content, metadata }
    ▼
┌─────────────────────────────────────┐
│  chunk/   分块（端口 rag/nlp）        │
│  ├── tokenizer.rs  Unicode 分词计数  │
│  └── naive.rs      令牌分块+重叠     │
└─────────────────────────────────────┘
    │ Vec<Chunk> { content, tokens }
    ▼
┌─────────────────────────────────────┐
│  embed/   向量化（端口 rag/llm）      │
│  └── openai.rs    OpenAI 兼容 API   │
└─────────────────────────────────────┘
    │ Chunk { embedding: Vec<f32> }
    ▼
┌─────────────────────────────────────┐
│  store/   存储（zvec 替代 ES）        │
│  └── mod.rs      zvec-rust 集成     │
└─────────────────────────────────────┘
```

## 与 RAGFlow 的对应关系

| RAGFlow (Python) | RayRAG (Rust) | 说明 |
|-----------------|---------------|------|
| `deepdoc/parser/pdf_parser.py` | `parser/pdf.rs` | PDF 解析 |
| `deepdoc/parser/docx_parser.py` | `parser/docx.rs` | Word 解析 |
| `deepdoc/parser/html_parser.py` | `parser/html.rs` | HTML 解析 |
| `rag/nlp/rag_tokenizer.py` | `chunk/tokenizer.rs` | 分词/计数 |
| `rag/nlp/__init__.py` (naive_merge) | `chunk/naive.rs` | 分块策略 |
| `rag/llm/embedding_model.py` | `embed/openai.rs` | 向量化 |
| `rag/utils/es_conn.py` | `store/mod.rs` | 向量存储（zvec 替代 ES） |
| `rag/app/naive.py` (chunk函数) | `pipeline/mod.rs` | 解析编排 |

## 特性

- ✅ PDF/DOCX/TXT/Markdown/HTML 解析
- ✅ Unicode 感知的令牌计数（中英混合）
- ✅ 令牌分块 + 重叠
- ✅ OpenAI 兼容 Embedding API
- ✅ zvec 向量存储集成
- 🚧 OCR/版面分析（计划中）
- 🚧 RAPTOR/GraphRAG（计划中）

## 构建

```bash
# 安装 Rust（如未安装）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 构建
cd /vol1/1000/RayRAG
cargo build --release

# 运行
cargo run -- parse document.pdf
```

## 依赖

- [zvec-rust](https://github.com/zvec-ai/zvec-rust) — 阿里 zvec 向量数据库 Rust 绑定
- [lopdf](https://crates.io/crates/lopdf) — PDF 解析
- [docx-rs](https://crates.io/crates/docx-rs) — DOCX 解析
