# Changelog

All notable changes to RayRAG are documented in this file.

## [0.3.1] — 2026-08-05

### UI 按钮逐项核对补齐（RAGFlow JSX vs RayRAG 渲染 diff）

- 补 12 个按钮事件：batchDelSel/batchLinkSel（文档批量删除/关联）、
  clearConv/tempConv（清空/临时对话）、emptyAllDocs（清空文档）、
  exportAgentJson/exportChunks（导出 agent JSON/chunk）、inviteUser
  （邀请用户）、openVersions（版本）、sendTemp（临时发送）、
  showTemplates（模板）、uploadF（上传文件）
- web.rs +169/-12；新增 8 个 UI 测试（含按钮 diff 断言）

## [0.3.0] — 2026-08-05

### UI：页面按钮全面对齐 RAGFlow（web/src/pages 19 页面层级）

- **知识库组**：列表页创建按钮/搜索/排序/卡片操作菜单（重命名/导出/删除）
  + 弹窗；详情页文档表格（状态/进度/chunk 数）+ 批量操作栏（解析/禁用/
  删除）+ 上传按钮；dataset 5 tab（文档/配置/检索测试/chunk/概览）+ 配置
  表单（chunk 方法下拉/embedding/检索参数）+ 保存按钮 + 检索测试（输入/
  滑块/结果卡片）。
- **对话检索组**：对话页新建/重命名/删除/分享 + 消息区（发送/停止）+ 引用
  折叠；检索页知识库选择器/过滤/结果卡片；记忆页添加/编辑/删除；404 页。
- **Agent 管理组**：agents 列表新建/卡片操作；agent 详情运行/保存/发布 +
  组件面板 + 日志区；admin 用户表格 + 添加/删除/密码重置；files 上传/下载/
  删除；skills 安装/卸载；settings API Key 生成/删除。
- 所有按钮接现有 API（HTML + JS 事件）；web.rs +796 / server.rs +198 /
  kb.rs +28；新增 6 个 UI 渲染测试。

## [0.2.5] — 2026-08-05

### Changed

- **覆盖矩阵最终收尾**（纯文档，`docs/ragflow-v0.26.4-coverage.md`）：矩阵终态
  aligned 128 / full 541 / partial 18 / replaced 92 / N/A 61 / reference 3601 /
  unmapped 0（4441 文件快照）。摘要表按清单逐行重算，与库存行完全一致。
  agent/sandbox 22 个非代码文件、deepdoc/README×2、deepdoc/__init__.py、
  admin/build_cli_release.sh 由 partial 转为 N/A-by-design（Python 沙箱镜像/
  文档/构建脚本不属于 Rust 移植范围；等价语义在 src/sandbox.rs + Dockerfile
  多阶段构建中表达）。

## [0.2.5] — 2026-08-05

### Added

- **conf/ 配置处理**：all_models 逐字节一致核对（+27 个新模型回归锁定：
  gpt-5.2-pro/deepseek-v4-flash/glm-5/kimi-k2.6/minimax-m2.7/grok-4 等）、
  llm_factories 工厂映射（FactoryLlmInfo/factory_endpoint）、system_settings
  种子（14 行常量 + coerce_setting_value）、ServiceConf（Mysql/Minio/Es/Os/
  Infinity/Redis/UserDefaultLlm 子结构 + apply_to_settings）。
- **MCP SSE 服务器**（`src/mcp_server.rs` 新建）：ToolRegistry（ragflow_retrieval/
  search_knowledge/list_datasets）、McpServerCore（JSON-RPC 2.0 dispatch：
  initialize/notifications/ping/tools/list/tools/call，-32600/-32601/-32602 错误码）、
  RetrievalBackend trait + InMemory 后端；接入主 router `/mcp/sse` + `/mcp/messages`。
- **消息记忆服务**（`src/memory.rs` + `src/api/joint_services.rs`）：消息读写/查询、
  中文双引号正则对齐（Unicode 弯引号）。
- **矩阵终态**：partial 44 → **15**；sandbox 非代码 22 → N/A-by-design（等价语义
  在 src/sandbox.rs seccomp/超时 + Dockerfile）；whatsapp Node 网关/Go CLI → N/A。

## [0.2.4] — 2026-08-05

### Added

- **common 工具层**（`src/common.rs` ~1800 行）：常量表（RetCode/TaskStatus/
  StatusEnum/ParserType 15 种 chunk method/FileSource 31 种/LLMType/Storage/
  MemoryType 位标志）、string/text/time/float/misc/parser_config/query_base/
  connection_utils、crypto（AES-128/256-CBC + PKCS7 + RAGF magic 头 +
  PBKDF2-HMAC-SHA256，与 Python wire format 逐字节一致；SM4 标记 TODO）、
  ssrf_guard、参数校验；api/constants + api/validation 语义。
- **settings 层**（`src/settings.rs` 新建）：Settings::from_env/from_reader、
  StorageImpl/DocEngine 枚举、parse_model_entry（name@factory）、init_secret_key
  （≥32 字符）、FLOAT_ZERO/PARAM_MAXDEPTH。
- **benchmark**（`src/benchmark.rs` 新建）：BenchmarkDataset/Config、ndcg@10/
  map@5/mrr@10（rank-ordered）、LatencyStats（avg/p50/p95/max）、
  RetrievalRunner + run_retrieval_benchmark、to_markdown。
- **canvas 语义**（`src/agent.rs` 追加）：reset_sys_globals/reset_env_globals、
  get_history_window、is_canvas_reference、add_retrieval_reference、
  ToolUseTrace + merge_tool_use_trace、clean_tts_text。

## [0.2.3] — 2026-08-05

### Added

- **flow 组件契约**（`src/pipeline/flow.rs` ~700 行）：FromUpstream/check_payloads
  （alias 反序列化）、OutputFormat、ComponentOutput（ProcessBase 语义）、
  file_outputs、FlowLog/TraceEntry/ComponentTrace（trace 累积 + 加权进度）、
  ChunkDoc（REST 契约 + From<&Chunk> 转换）。
- **resume 实体资源修复**（`src/resume.rs`）：2 个 regex 崩溃 bug
  （`[—-]+` 非法区间、反斜杠转义层级）+ CORP_TKS 贪心最长匹配（对齐 jieba）。
- **服务层语义**（`src/api/joint_services.rs` +700）：seed_system_settings、
  SuperuserSeedPlan、LLM 工厂种子/清理（novita.ai 删除/QAnything→Youdao）、
  tenant_model 修复计划、canvas 模板规范化、langfuse/mcp_server/conversation/
  task 服务（SyncLog 单调计数 + 100 条裁剪 + resume 状态机）。
- **MCP 客户端**（`src/mcp_client.rs`）：SSE + Streamable HTTP 传输、
  initialize/list_tools/call_tool。
- **Helm chart**（`helm/` 新建）：最小可用 chart（deployment/service/configmap，
  镜像 rayrag:local，9390→8080，外部 PG18 依赖说明）；helm lint/template 通过。

## [0.2.2] — 2026-08-05

### Added

- **api/common**（`src/api/common.rs`）：ApiError/ApiErrorKind（400/403/404/409/500
  信封输出 `{"code","message"}`）、ok_json、base64、TeamPermission 检查。
- **运行时配置**（`src/api/runtime_config.rs`）：RwLock 热更新（init/get/get_env/
  set_env/load_config_manager/set_service_db，对齐 reload_config_base）。
- **联合服务**：memory_message（消息记忆读写）/tenant_model（租户模型绑定）/
  user_account 语义补入 api 层。
- **dialog.rs**：is_aggregate_sql/is_row_count_question/add_kb_filter（UUID 注入
  防护 + RAGFlow 两处怪癖忠实复刻）。
- **工具契约核对**（`src/runtime.rs` +278 / `src/agent.rs` +34）：17/19 工具已对齐；
  JIN10_INPUTS 补 7 参数、QWEATHER 补 web_apikey、retrieval 补 15 输入 4 输出、
  **wencai 别名**（canvas component_name=WenCai 此前匹配失败）。
- **资源加载器**（`src/resources.rs` 新建）：ner.json（235KB）+ synonym.json
  （268KB）磁盘加载（RAYRAG_RES_PATH 环境解析、缺失降级空表、值→键镜像）；
  CvModel 页面转写 prompt + figure 描述调用流补全。

## [0.2.1] — 2026-08-05

### Added

- **远程沙箱契约**（`src/sandbox.rs` +456）：RemoteSandboxClient/Config（task 提交/
  结果拉取、超时=执行超时、重试、/healthz）、CodeExecutionResult 系列、
  unsafe/unhandled_exception 失败分支。
- **插件机制**（`src/plugin.rs` 新建）：PluginManager（发现/加载/执行、env 注入、
  错误隔离）、LLM 工具插件（OpenAI tool 转换）、内置插件 1-2 个。
- **NLP 工具**（`src/nlp.rs` +658）：rag_tokenize（CJK 单字+二元组）、全角→半角、
  ~450 对繁→简映射（替代 OpenCC）、sub_special_char、QueryBase（rm_www/
  normalize_query 管线）、TermWeightComputer（NER×postag）。
- **LLM 工厂**（`src/llm.rs` +655）：ModelKind 7 类 + Chat/Ocr/Cv/Seq2txt/Tts
  trait 抽象 + OpenAi 系列实现（vision/audio/tts 多模态 payload）。
- **RAPTOR 检索**（`src/advanced_rag.rs`）：问题分解→子检索→汇总（复用已有
  Retrieval/Chunk 组件）。
- **doc_store 抽象**（`src/doc_store.rs` 新建）：DocStore trait（insert/delete/
  search/update/sql 全契约）+ Memory/Postgres/Zvec 三后端 + 连接池
  （es_conn_pool 语义：2 次重试/5s 间隔/按 URL 选后端）。
- **memory utils**（`src/memory.rs` 新建）：highlight_text（按句切分/<em> 标记）、
  aggregate_by_field。
- **admin 端点**：users CRUD/keys/activate/admin 授权/version/ping（对齐
  admin/server/routes.py）。

### Fixed

- `/api/v1/admin/users/{username}` 与既有 `{user_id}` 路由冲突 → 删除旧
  重复路由与 handler（新路由功能更全）。

## [0.2.0] — 2026-08-05

### Added

- **剩余 prompts 全部移植**（`src/prompts.rs` +1390 行）：26 个常量——
  related_question、resume 系列双语（system/basic_info/education/project_exp/
  work_exp）、structured_output/sufficiency_check/summary4memory/tool_call_summary、
  TOC 管线（detection/extraction/continue/index/relevance×2）、vision describe 系列
  （vision_llm_describe/figure_describe×2，Jinja 语法转 {var} 约定）。
- **问答模式分派**（`src/app_parsers.rs`）：naive/advanced/graph 模式选择逻辑
  （显式标记优先、全 KG 才走 graph）。
- **api/utils 服务层**（`src/api/utils.rs` ~600 行）：FileType/filename_type
  （扩展名逐字对齐）、sanitize_path、常量（255 限制/50MiB/100MiB）、
  hash128（xxh3）/md5 hex、get_parser_config（chunk_method 默认 + 递归合并）、
  init_dataset_defaults（parser_id=naive、llm_id 注入）。
- **解析器行为补齐**：`excel.rs`（多 sheet `表头:值;` 行格式、sheet_html 分块、
  >10000 行二分、row_number 计数）、`docx.rs`（blip rId→rels→media 图片提取）、
  `paddleocr_layout.rs`（table/figure/image 区域分组 + 位置 tag）。

### Fixed

- channels 测试断言 JSON map 遍历顺序不稳定 → 集合匹配（顺序无关）。

## [0.1.9] — 2026-08-05

### Added

- **连接器框架**（`src/connectors.rs` ~1480 行，对标 `common/data_source`）：
  Connector trait（load_credentials/list_files/fetch_file/normalize/fetch_all 分批
  runner）、RemoteFile/SyncBatch 契约、LocalConnector/WebDavConnector/BlobAdapter
  基础连接器 + FeishuConnector（tenant_access_token 授权、云空间递归列举、
  docx raw_content 导出）/ ConfluenceConnector（spaceKey 分页、HTML→markdown）；
  ConnectorRegistry 按类型分发；datasources 页面连接/断开/拉取按钮。
- **渠道框架**（`src/channels.rs` ~1200 行，对标 `api/channels`）：
  Channel trait（send_text/send_markdown/start/stop/dispatch 错误隔离）、
  ChannelRegistry（register/build_channels/start_all/stop_all/dispatch_inbound，
  bootstrap 语义）、webhook 渠道（OutgoingMessage 契约 POST）+ feishu 渠道
  （tenant_access_token 缓存、text/interactive-card 发送、v2.0 事件归一化 +
  URL verification challenge）；server.rs 启动/停止钩子。
- **rag/svr 语义补齐**：`src/api/file_mgr.rs` +236（DocFileCache：Redis 式
  `{kb_id}/{location}` 键、TTL 过期清理）；`src/task_executor.rs` +960
  （SyncTask 同步任务类型：fingerprint 跳过、poll_range_start 窗口推进、
  快照删除对账、超时永久 FAIL；ChunkEnrichment：auto_questions 两阶段 LLM
  缓存 + 离线 fallback、important_kwd/question_kwd 元数据、进度消息对齐）。

## [0.1.8] — 2026-08-05

### Added

- **TaskExecutor**（`src/task_executor.rs`，~680 行，对标 `task_executor.py`）：
  异步任务队列（tokio::mpsc + Semaphore 限流）、状态机 Pending→Processing→
  Done/Failed（对齐 RAGFlow doc 状态 UNSTARTED/RUNNING/DONE/FAILED）、
  进度回调（set_progress：`[ERROR]` 前缀/`Page(a~b):` 页前缀/HH:MM:SS 时间戳）、
  失败重试（retryable 标记 + 指数退避 + max_retries）、status/stats 上报、
  submit/shutdown 生命周期；接入 document.rs 上传队列。
- **ChatPipeline**（`src/chat.rs`，835 行，对标 `generator.py`）：
  问题改写（full_question：绝对 ISO 日期 + USER:/ASSISTANT: 会话，**ERROR** 回退）、
  多路查询（multi_queries_gen：<2 命中时生成 2-3 路补充查询，JSON 解析容错）、
  引用格式化（citation_plus 二次 LLM 生成 + repair_bad_citation_formats 规范化
  [ID:n] + references 按被引过滤）、format_knowledge（kb_prompt 树形渲染）。

## [0.1.7] — 2026-08-05

### Added

- **RAG 问答阶段 prompts**（`src/prompts.rs` +1194 行）：18 个 RAGFLOW_* 常量
  （citation/citation_plus/full_question/multi_queries_gen/next_step/reflect/
  rank_memory/content_tagging/cross_languages×2/analyze_task×2/ask_summary/
  assign_toc_levels/keyword/question_prompt/meta_data/meta_filter），注册
  PromptLibrary get()/list()。
- **graph_extractor NER 语义**（`src/structure_compile.rs` +1033 行）：实体/关系
  JSON 解析容错（LLM 非严格输出提取/修复）、upsert 语义（if not exists）、
  general/light extractor 剩余常量。
- **组件参数契约**（`src/runtime.rs`）：新增 iteration/iterationitem/exitloop/
  loopitem descriptor；扩展 categorize/listoperations/docsgenerator/excelprocessor
  参数（对齐 RAGFlow Param schema 的 camelCase 语义）。

## [0.1.6] — 2026-08-05

### Added

- **translate 工具**（`src/translate.rs` + `src/runtime.rs`）：注册 translate descriptor
  （对齐 RAGFlow deepl 参数契约），百度翻译 VIP 签名 API + 免 key sug 降级双路径
  （大陆可用）；3 个 mock-HTTP 测试（SUG 解析/VIP MD5 签名表单校验/错误码透传）。
- **KeyTerm Extractor**（`src/extractor.rs`，402 行）：RAGFlow KeyTermExtractor 语义的
  统计实现——词频/位置/POS 代理权重、CJK n-gram（跳过虚词开头、单字过滤）、
  停用词表；`extract_key_terms` / `extract_key_terms_with_top_n` / chunk 批量接口；
  pipeline 可选集成。
- **title_chunker.py 主文件语义**：标题连续性检查/章节序号处理补入 `src/chunk/title.rs`。

### Fixed

- extractor CJK 测试断言（默认 top-N 截断遮蔽 4 字短语 → 显式 wide top_n 验证）。

## [0.1.5] — 2026-08-05

### Added

- **Provider Model Catalog**（`src/model_meta.rs` + `src/api/fixtures/models/*.json`）：
  RAGFlow conf/models 全部 20 家厂商 / **157 个静态模型**种子化（include_str! 内嵌）；
  `list_models_for` / `resolve_model_factory`（zhipu→zhipu-ai、qwen→aliyun 别名）/
  `provider_models_payload`；providers 页面新增 Model Catalog 卡片 + 点击弹窗
  （模型名 + 能力 badge：chat/embedding/rerank/image2text/thinking）。
- **沙箱 executor_manager 契约补齐**（`src/sandbox.rs` +1031 行）：
  SupportLanguage/ResultStatus/ResourceLimitType/UnauthorizedAccessType/RuntimeErrorType
  枚举、失败协议（exit code + stderr 分类）、运行时环境约定（容器池/镜像/run 参数/
  bundle）、artifact 契约、安全校验契约。
- **GraphRAG 管线补齐**（`src/structure_compile.rs` +321 行、`src/prompts.rs` +116 行）：
  phase_markers 阶段标记（PHASE_RESOLUTION/PHASE_COMMUNITY、`graphrag:phase:{kb}:{phase}`
  键格式、7d TTL）、实体消解确定性门控（levenshtein/2gram 数字差/字符集重叠、
  entity_resolution_candidates 分组排序、parse_resolution_results）、light graph
  默认常量（LIGHTRAG_DEFAULT_LANGUAGE/TUPLE_DELIMITER/ENTITY_TYPES/FAIL_RESPONSE）。

### Fixed

- 子 agent 并发期间 prompts.rs 临时借用 E0716（I 已修，测试全绿）。

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
