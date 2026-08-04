# Changelog

All notable changes to RayRAG are documented in this file.

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
