# RayRAG 运维台账

## 项目信息

- **名称**：RayRAG
- **路径**：`/vol1/1000/RayRAG/`
- **语言**：Rust（零 Python/JS，前端使用 RAGFlow React 构建产物）
- **依赖**：zvec-rust、Candle、lopdf、docx-rs、hf-hub、tokenizers、axum、tower-http
- **定位**：RAGFlow 解析管线 Rust 重写 + zvec 向量存储（替代 ES）
- **创建日期**：2026-07-10
- **Rust 版本**：cargo 1.97.0

---

## 架构概要

```
RAGFlow (Python)              → RayRAG (Rust)
─────────────────────────────────────────────────
deepdoc/parser/*              → parser/            文档解析（pdf/docx/txt/md/html）
rag/nlp/naive_merge + tokenizer → chunk/           分块策略（tokenizer + naive）
rag/llm/embedding_model.py    → embed/             向量化（三模：openai/candle/hybrid）
rag/utils/es_conn.py          → store/mod.rs       zvec 存储（替代 ES）
rag/app/naive.py              → pipeline/mod.rs    解析编排
nginx (web/dist)              → server.rs          Axum 静态服务 + API
```

---

## 版本记录

### v0.1.1 — Web UI + 无自动下载（2026-07-11）

**新增**：

| 模块 | 文件 | 功能 |
|------|------|------|
| `server.rs` | axum 0.8 + tower-http | HTTP 服务器（SPA 静态文件 + API stub） |
| `web/static/` | RAGFlow web/dist 副本 | 88MB React 前端（index.html + 资产） |

**变更**：
- Cargo.toml：新增 `axum`、`tower`、`tower-http` 依赖；`tokenizers` 禁用 `http` feature → `fancy-regex`
- main.rs：新增 `serve` 子命令；`--mode candle` 检查本地模型存在性
- candle.rs：新增 `download_model_to()` 方法（仅 UI 触发）；顶部大段注释说明下载策略
- embed/mod.rs：`default_embedder()` / `hybrid_embedder()` 标记 `#[deprecated]`

**Web 服务器路由**（兼容 RAGFlow 前端）：
```
GET  /api/v1/system/status   → {"status":"healthy"}
GET  /api/v1/version         → {"version":"0.1.0"}
POST /api/v1/auth/login      → stub token
GET  /api/v1/datasets        → []
POST /api/v1/retrieval       → []
/*                            → SPA 静态文件（index.html fallback）
```

**编译状态**：server/main 模块 ✅ 零错误零警告；全项目 17 个旧错误（candle/docx/uuid API）

### v0.1.0 — 初始骨架（2026-07-10）

**已完成（16 源文件 / 1544 行）**：

| 模块 | 文件 | 功能 |
|------|------|------|
| `parser/pdf.rs` | lopdf | PDF 文本提取 |
| `parser/docx.rs` | docx-rs | Word 段落 + 表格 |
| `parser/txt.rs` | std | 纯文本（含 UTF-16 兼容） |
| `parser/markdown.rs` | std | Markdown 透传 |
| `parser/html.rs` | std | HTML 标签剥离 |
| `chunk/tokenizer.rs` | unicode-segmentation | Unicode 分词计数（中英混合） |
| `chunk/naive.rs` | — | 令牌分块 + 重叠策略 |
| `embed/candle.rs` | Candle + hf-hub + tokenizers | **纯 Rust 本地嵌入（BERT 模型）** |
| `embed/openai.rs` | reqwest | 云端 API 嵌入（OpenAI/MiniMax/vLLM） |
| `embed/mod.rs` | — | **HybridEmbedder（先云后本地 fallback）** |
| `store/mod.rs` | zvec-rust | zvec 向量存储骨架 |
| `pipeline/mod.rs` | — | 解析→分块→嵌入→存储 编排 |
| `lib.rs` | — | 核心类型（Document/Chunk/ParserConfig） |
| `main.rs` | clap + tokio | CLI（`rayrag parse --embed --mode hybrid`） |
| `scripts/mirror.sh` | bash | 镜像加速自动检测 |

**嵌入模式**：
- `--mode candle`：本地 Candle（需先下载模型到本地目录，不自动下载）
- `--mode openai`：云端 API（需 `--api-base --api-key`）
- `--mode hybrid`（默认）：**先云后本地 fallback**（云端失败自动切 Candle）

**⚠️ 模型下载策略（v0.1.1）**：
- **构建时不下载任何模型** — `tokenizers` 已禁用 `http` feature
- **运行时不自动下载模型** — 所有 `default_embedder()` / `hybrid_embedder()` 已标记 deprecated
- **仅在用户界面选择 Candle 模式时才触发模型下载** — 通过 `CandleEmbedder::download_model_to()` 显式下载
- 模型下载到本地目录后，使用 `CandleConfig::from_local(path)` 加载

**镜像 fallback**：
- crates.io → `mirrors.ustc.edu.cn`（`.cargo/config.toml`）
- HuggingFace → `hf-mirror.com`（`embed/candle.rs` 代码级自动切换）
- GitHub git（Candle 源码）→ `ghproxy.com`（`scripts/mirror.sh` 自动检测）
- zvec 预编译库 → 禁用 bundled，手动设 `ZVEC_LIB_DIR`

**待完成**：
- ✅ Web 静态文件服务（v0.1.1）
- 🚧 API 对接 zvec/pipeline（当前为 stub，返回空数据）
- 🚧 OCR / 版面分析
- 🚧 RAPTOR / GraphRAG
- 🚧 zvec-rust 实际对接（等 libzvec_c_api.so）
- 🚧 图片/表格上下文提取
- 🚧 search CLI 命令
- 🚧 并发下载 HF_ENDPOINT 改用 `RwLock`
- 🚧 完整登录/用户系统（当前 auth/login 为 stub）

---

## 运维操作

### 构建
```bash
cd /vol1/1000/RayRAG
bash scripts/mirror.sh   # 先配置镜像加速
cargo build --release
```

### 运行
```bash
# 启动 Web 服务器（SPA 前端 + API，默认端口 9380）
cargo run -- serve
# 或指定端口/路径
cargo run -- serve --port 8080 --static-dir /path/to/web/static

# 解析 + 本地嵌入（纯 Rust，需先下载模型）
# 模型不会自动下载！先通过 UI 下载，或手动放到 models/ 目录
cargo run -- parse document.pdf --embed --mode candle --model-path /path/to/model

# 解析 + 云端嵌入
cargo run -- parse document.pdf --embed --mode openai \
  --api-base https://api.minimaxi.com/v1 --api-key *** --api-model MiniMax-M3

# 混合模式（云优先，失败切本地）
cargo run -- parse document.pdf --embed --mode hybrid --api-key *** \
  --model-path /path/to/model

# 纯解析（不分块）
cargo run -- parse document.pdf

# 自定义分块参数
cargo run -- parse document.pdf --chunk-size 1024 --overlap 0.1
```

### 测试
```bash
cargo test
```

### Web 服务器
```bash
# 启动（默认 9380 端口，使用 web/static/ 目录）
cargo run -- serve

# 自定义端口和静态目录
cargo run -- serve --port 8080 --static-dir /path/to/web

# 生产部署（release 构建 + 后台运行）
cargo build --release
nohup ./target/release/rayrag serve --port 9380 > /tmp/rayrag-server.log 2>&1 &
```

**静态文件更新**（当 RAGFlow 前端升级时）：
```bash
# 从 RAGFlow 容器同步最新前端
docker cp docker-ragflow-cpu-1:/ragflow/web/dist/. web/static/
echo '{"appName":"RayRAG"}' > web/static/conf.json
sed -i 's/<title>.*<\/title>/<title>RayRAG<\/title>/' web/static/index.html
```

---

## 关键依赖版本

| 依赖 | 来源 | 用途 |
|------|------|------|
| zvec-rust | crates.io 0.5 | 向量数据库（default-features=false） |
| candle-core | git: huggingface/candle:main | ML 推理核心 |
| candle-nn | git: huggingface/candle:main | 神经网络层 |
| candle-transformers | git: huggingface/candle:main | BERT 模型 |
| hf-hub | crates.io 0.4 | HuggingFace 模型下载 |
| tokenizers | crates.io 0.21 | HuggingFace 分词器（fancy-regex，无 http） |
| axum | crates.io 0.8 | HTTP 服务器（静态文件 + API） |
| tower | crates.io 0.5 | 中间件框架 |
| tower-http | crates.io 0.6 | CORS + 静态文件服务 |
| lopdf | crates.io 0.34 | PDF 解析 |
| docx-rs | crates.io 0.4 | DOCX 解析 |
| unicode-segmentation | crates.io 1.x | 分词 |
| tokio | crates.io 1.x | 异步运行时 |
| async-trait | crates.io 0.1 | 异步 trait |
| reqwest | crates.io 0.12 | HTTP 客户端（cloud API） |
| clap | crates.io 4.x | CLI |
| serde | crates.io 1.x | JSON 序列化 |

---

## 故障/注意事项

### Rust 工具链
- 需要在 `~/.bashrc` 中 source `~/.cargo/env`
- 首次构建会下载+编译所有依赖，Candle git clone 约 5-15 分钟
- zvec-rust 需要 `libzvec_c_api.so`（设置 `ZVEC_LIB_DIR` 环境变量）

### 镜像网络
- crates.io 已配置 ustc 镜像（`.cargo/config.toml`）
- HuggingFace 模型下载已内置 hf-mirror.com fallback
- GitHub git 代理需运行 `bash scripts/mirror.sh` 自动检测可用镜像
- 如所有镜像都不可用 → Candle git clone 直接走 github.com（慢但可用）

### 并发安全
- `candle.rs` 中 `HF_ENDPOINT` 环境变量切换使用 `std::sync::Mutex` 保护
- ⚠️ 这是 blocking mutex 在 async 上下文中，尚未造成死锁，但最佳实践应改用 `tokio::sync::Mutex`

### 与 RAGFlow 的区别
- 解析结果不应直接与 RAGFlow 对比（RAGFlow 有 OCR + 版面分析）
- zvec 与 ES 的检索精度可能有差异，需单独评测
- 当前 zvec 存储为骨架代码，需等待 zvec 服务部署

---

## 更新日志

| 日期 | 操作 | 说明 |
|------|------|------|
| 2026-07-11 07:13 | Web UI | 复制 RAGFlow web/dist → web/static/（88MB）；新增 Axum 服务器 `src/server.rs`；main.rs 加 `serve` 命令；API stub 兼容 RAGFlow 前端 |
| 2026-07-11 07:10 | 无自动下载 | 禁用 tokenizers http feature；default_embedder/hybrid_embedder 标记 deprecated；main.rs 检查本地模型存在性；仅 UI 触发下载 |
| 2026-07-10 22:51 | 镜像配置 | `.cargo/config.toml`（ustc 源）+ `scripts/mirror.sh`（github proxy 检测）+ zvec 禁用 bundled |
| 2026-07-10 22:49 | 兼容检查 | 全量代码兼容性审查（模块/trait/import/类型），并发 Mutex 修复 |
| 2026-07-10 22:46 | HF 镜像 | `candle.rs` 内置 hf-mirror.com fallback（主站→镜像自动切换） |
| 2026-07-10 22:29 | Hybrid | 新增 `HybridEmbedder`（先云后本地），三模 CLi（candle/openai/hybrid） |
| 2026-07-10 22:27 | Candle | 新增 `embed/candle.rs`，纯 Rust 本地嵌入（BERT via Candle），替换 HTTP API |
| 2026-07-10 22:15 | 骨架 | 12 源文件 / 1073 行，parser + chunk + embed(openai) + store + pipeline |
| 2026-07-10 21:47 | 创建 | 项目初始化 |
