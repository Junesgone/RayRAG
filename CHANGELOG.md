# Changelog

All notable changes to RayRAG are documented in this file.

## [0.1.4] — 2026-08-05

### Added

- **RAGFlow 25 个 Agent 工作流模板种子化**（`src/agent_templates.rs` + `src/api/fixtures/agent_templates/*.json`）：
  include_str! 编译期嵌入（零转义风险），agents 页面模板库 3 → **28**（web_search_assistant、
  deep_research、text2sql_data_expert、trip_planner、ingestion_pipeline_* 系列等）。
- **GraphRAG prompts 移植**（`src/prompts.rs`，18 个常量 + PromptLibrary 方法）：
  GRAPH_EXTRACTION_PROMPT/CONTINUE/LOOP、GRAPH_SUMMARIZE_DESCRIPTIONS_PROMPT、
  GRAPH_COMMUNITY_REPORT_PROMPT、ENTITY_EXTRACTION_PROMPT 系列、KEYWORDS_EXTRACTION_PROMPT、
  LIGHTRAG_RESPONSE_PROMPT、NAIVE_RAG_RESPONSE_PROMPT、QUERY_KEYWORDS_EXTRACTION_PROMPT、
  MINIRAG_QUERY2KWD_PROMPT、ENTITY_RESOLUTION_PROMPT、MIND_MAP_EXTRACTION_PROMPT、
  QUESTION_PROPOSAL_PROMPT、KEYWORD_EXTRACTION_PROMPT。
- **paddleocr_layout 补齐**（`src/parser/paddleocr_layout.rs`，66 行 stub → 完整 layout 流程）：
  blocks→bbox→position tags、parse_methods、extract_positions（对齐 paddleocr_parser.py）。
- **figure.rs 补齐**：figure-describe prompts + 上下文感知 describe helper（对齐 figure_parser.py/picture.py）。

### Fixed

- `exact_dedup_by_key` canonical 名称保留组内首现（对齐 RAGFlow 语义，而非频次最高）。
- `parse_run_response` 缺省 result 时始终注入 result_present=false / result_value=null / result_type=json。

## [0.1.3] — 2026-08-05

### Added

- **Token Chunker**（`src/chunk/token.rs`，对标 RAGFlow `token_chunker.py`）：
  `one` / `token_size` / `delimiter` 三模式；反引号分隔符编译（最长优先）；
  分隔符并入段尾（re.split capture-group 语义）；重叠率 normalize（0<p<1→×100，
  clamp [0,90]）；children_delimiters 二次拆分（裸模式 escape，`mom` 挂父块原文）。
- **Title Chunker**（`src/chunk/title.rs`，对标 `title_chunker/{hierarchy,group,common}.py`）：
  hierarchy 目标层级合并、group 32/1024 token 常量合并、include_heading_content、
  root_chunk_as_heading；标题来源优先 PDF 大纲 `__outline__` 元数据，fallback 正文
  markdown/枚举标题行解析。
- **iwencai 问财工具**（`src/wencai.rs`，对齐 RAGFlow `agent/tools/wencai.py`）：
  get-robot-data 请求契约（perpage 字符串/page 整数）、xuangu_tableV1 landing
  分页数据接口、多组件 Markdown 渲染、`agent.rs` 工具注册与 provider 执行链。
- **沙箱 executor_manager 远程客户端**（`src/code_exec.rs`）：`parse_run_response`
  缺省 result 元数据兜底（result_present=false / result_value=null / result_type=json）；
  `process_result` 结构化结果优先、stderr-only 错误分支、canonical 渲染。
- **KB 详情页路由** `/kbs/{id}` → `kb_detail_page`（文档/配置/测试/知识图谱 tab，
  对齐 RAGFlow 数据集详情页；与旧 `/dataset/{id}` 并存）。

### Changed

- `Pipeline::new` 按 `config.chunk_method`（naive | token | title）选择 chunker。
- `ParserConfig` 新增 chunk_method / delimiter_mode / delimiters /
  children_delimiters / title_levels / hierarchy / include_heading_content /
  root_chunk_as_heading 字段（serde alias 兼容 RAGFlow camelCase）。
- 修复上一会话遗留 `src/structure_compile.rs` 编译错误（as_any ×2、
  semaphore 借用、Arc 包装、downcast and_then）。

### Fixed

- 子 agent 半成品编译错误（wencai `Map` 类型、code_exec `base64::Engine`、
  agent `SandboxResult` 缺字段）与测试缺陷（重复 bind listener、serde_json
  Map Index panic、process_result 断言语义）。

## [0.1.2] — 2026-08-04

### Added

- **UI 全面对标 RAGFlow**（16 页面 Rust 手写）：dashboard（统计卡片/最近会话）、
  knowledge base（批量上传/删除/清空/重命名）、search（多 KB 过滤/分页/图谱上下文
  卡片/导出 Markdown）、chat（模型选择器/流式/引用/会话管理）、agents（模板库
  3 种工作流/DSL 编辑器/画布拓扑/搜索过滤）、files（搜索）、admin（用户搜索）、
  status（Refresh/Copy Report）、api-docs、login（注册/cookie 会话）
- **Agent 画布完整闭环**：components 格式 DSL（RAGFlow 标准）、模板创建
  （RAG 问答/网络搜索/RAG 对话）、JSON 编辑保存、错误传播（11 个工具函数统一
  formalized_content 注入 + `{Provider} error:` 前缀）
- **OpenAI 兼容层**：`/api/v1/models`、`/api/v1/embeddings`（租户表空回落 env）、
  `/api/v1/chat/completions`（SSE 流式 + references 帧）、`/api/v1/rerank`
  （Jina 风格）
- **GraphRAG 生命周期**：Build（LLM 实体抽取）→ 持久化（graphrag.json 重启保留）
  → Query API → RAG 对话注入 → KB 删除清理
- **SearXNG 自托管**：Docker 部署 + cn.bing 引擎中国适配 + `SEARXNG_URL` 透传
- **性能优化**：zvec 增量 upsert（仅写变更）、`RAYRAG_NO_FSYNC` 开关
  （容器 overlay 上传 53s→37s）、docx 正则静态缓存 + HashMap tie-break
  （偶发测试失败根治）

### Fixed

- docx 并行偶发测试失败（OnceLock 缓存 + 确定性 tie-break）
- Agent 工具静默失败（错误传播到 formalized_content）
- 模板 JS 错位（showTemplates 误入 kbs_page）
- Retrieval 模板字段（formalized_content 非 result）
- 上传性能（容器 overlay fsync 瓶颈 + 增量 upsert）
- 知识图谱不落盘（build 只返回统计）

### Verified (container)

- 844 passed / 0 failed / 26 ignored
- GPU 四链路实连（embedding 8888 / LLM 8088 / rerank 8081 / OCR 8097）
- OpenAI 兼容 4 端点 + Agent 模板 3/3 + SearXNG 成功/失败路径
