# RayRAG

RAGFlow 解析管线 Rust 重写 + zvec 向量数据库。

## 项目状态

- 版本：**v0.3.4**，对齐切片 **v0.3.5v**（每次切片推进一个 RAGFlow 文件的逐行对齐，
  见 `CHANGELOG.md` 与 内部运维台账）。
- 测试基线：`cargo test --locked --features postgres-backend,zvec-backend --lib`
  **1473 passed / 0 failed / 27 ignored**；27 个 ignored 为需要显式 GPU 模型端点、
  凭据、公网或 MinIO 的 live 测试。
- 覆盖台账（RAGFlow v0.26.4 固定快照 `cb93883f`，4441 个文件，
  内部对标台账）：full **566** / aligned **275** /
  replaced **101** / N/A **61** / partial **41** / reference **3397** /
  unmapped **0**；`cargo run --locked --bin ragflow-coverage -- --check <ref-tree>`
  可复核（`--write` 刷新 Summary）。
- 运行形态：纯 Linux（直接运行 `target/release/rayrag`）与纯 Docker
  （`docker compose up -d`）均为一等支持；向量后端 zvec，元数据 PostgreSQL 18.4，
  不使用 MySQL。

## 许可证

Apache License 2.0，见 `LICENSE`；对标来源与第三方资产说明见 `NOTICE`。

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

完整逐文件清单见 内部对标台账。该矩阵固定对标 RAGFlow
`v0.26.4` commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，记录每个文件的
Git blob、RayRAG 对应实现及 `aligned/partial/replaced/reference/unmapped` 状态。当前仓库保留
固定提交的 4441 文件快照；纯 Rust coverage 门禁会直接读取固定 Git tree，校验 commit、
路径、blob、字节、文本行数、状态库存和摘要：

```bash
cargo run --locked --bin ragflow-coverage -- --check /path/to/ragflow-v0.26.4
# 有意修改状态后，以固定 Git blob 修复元数据并重算摘要：
cargo run --locked --bin ragflow-coverage -- --write /path/to/ragflow-v0.26.4
```

未知路径不会再被兜底伪装成 `reference`：生产运行时目录必须显式映射为
`aligned/partial/replaced`，测试与 fixture 由独立规则标为 `reference`，其余路径进入
`unmapped`。当前清单的 `unmapped=0` 因此代表固定树中的每个文件都有显式分类；
`partial` 仍是后续逐文件、逐行审计的真实待办，而不是完成声明。矩阵当前库存（v0.3.3bz）：
aligned 155 / full 553 / partial 55 / replaced 101 / N/A 61 / reference 3516 /
unmapped 0，摘要表与清单逐行一致。用户设置已有固定七路由顺序、根跳转、
303/64 px 响应式侧栏、精确 active 项、头像入口、账号/版本/主题/登出项，且已真实
Chrome 逐项点击；各子页的完整 CRUD、data-source detail route、精确图标与登出 token
revoke 仍为 partial。Model providers 已覆盖固定 63-card Available 目录、默认
模型、Added models、tenant Add/Verify/Create、10 类本地 provider 的 List models picker、
模型多选、Vision 与自定义模型 `is_tools` 持久化/运行时解析。默认项现为固定
provider→instance→model 三深度 tree dialog，六种 canonical PATCH、clear/required、失败回滚、
成功回读和 reload 已经真实 Chrome 复点。Bedrock 与 SoMark 卡片现路由到专属弹层：
Bedrock 三段 AWS 认证（Access Key/IAM Role/Assume Role）、37 区域带搜索选择器，密钥对象
以 JSON 持久化；SoMark 四个 Element Format 选择器与七个 Feature Config 开关，11 个
`somark_*` 字段作为 `ocr_config` 随 model_info 持久化并经实例 models API 回读。两者仍保留为
partial，因为固定 provider 图片资源、共享 tree 组件的其他消费者、Verify 的逐 capability
推理、编辑已有实例及精确 React 布局尚未完成。`/user-setting/api` 现有 Langfuse 卡与
配置弹层（Secret key / Public key / Host、文档外链、确认后 Delete），后端实现租户级
`GET/POST/PUT/DELETE /api/v1/langfuse/api-key`：保存时用 Basic auth 探测 Langfuse
health/projects 并缓存 project_id/name，无效凭证失败关闭；View 链接优先自托管 host
（大陆网络适配）。Provider modal 现按工厂渲染固定动态凭据字段：Azure-OpenAI 的
api_version/vision、VolcEngine 的 ARK_API_KEY 与 endpoint id、Google Cloud 三项、Tencent
Cloud 22 引擎 ASR 选择器与 sid/sk、XunFei Spark 四键（tts 条件显示）、BaiduYiYan ak/sk、
Fish Audio ak/refid，以及 OpenDataLoader/PaddleOCR/MinerU 的 OCR cfg（apiserver、algorithm、
backend/server-url/delete-output）；save 时按上游 `apikey_json` 语义合并进 api_key 对象，
OCR 端点从 api_key 对象读取做探测。Model providers 页现按固定 `LlmIcon` 规则渲染
provider 品牌图标：10 个 svgIcons 工厂提供 `assets/svg/llm/*.svg`，其余用打包 iconfont
的 IconMap symbol，未映射回退 `moxing-default`，Available 卡与 Added 组头部均不再用首
字母头像。`/user-setting/mcp` 现为完整 MCP servers 页：Search/Bulk manage/Import/+ Add
MCP、空态 Add Now、卡片与 hover Export/Edit/Delete、批量选择/导出/删除、Add/Edit 弹层
（Name 正则、URL、SSE/Streamable HTTP、Authorization Token、Test 拉取工具）与 JSON Import。
后端实现租户级 `GET/POST/PUT/DELETE /api/v1/mcp/servers`、`/import`、`/{id}/test`：
创建/更新/导入用打包 streamable-http MCP client 拉取并缓存工具表，URL 走
`assert_url_is_safe` 同款全局地址 SSRF 校验并支持 `ALLOW_ANY_HOST=1` 局域网旁路。
`/user-setting/team` 现为固定 Team 页：`{nickname} workspace` 标题、Team members 表
（Name/Date 排序/Email/State/Action，owner 行无删除）、Joined teams 表
（Accept/Decline/Quit）与 Invite member 弹层；后端补 GET/POST/DELETE
`/api/v1/tenants/{id}/users` 与 PATCH `/api/v1/tenants/{id}`（邀请接受），错误文案对齐
上游。`/user-setting/profile` 现为固定 Profile 页：Username / Avatar（内联上传）/
Time Zone / Email / Password 五行与 Edit Name / Edit Time Zone / Edit Password 弹层；
后端 `PATCH /api/v1/users/me` 支持昵称（上游 1-100 字符校验）、时区、头像与改密
（成功后失效全部 token）。`/user-setting/chat-channel` 现为固定 Chat channels 页：
Added channels 分组列表（Edit/Delete）与 7 张 Available 卡（hover Add），Add/Edit
弹层按渠道渲染固定凭据字段；后端新增 `GET/POST /api/v1/chat-channels` 与
`GET/PATCH/DELETE /{id}`。Provider Verify/创建在带 model_info 时现按上游
`verify_api_key` 做逐能力推理探测：chat / embedding / rerank 走 OpenAI 兼容协议真实
请求（非 2xx 失败关闭并返回 `Fail to access model(...)`），空 model_info 回退模型列表
探测；OCR/TTS/ASR 与厂商专有 REST 协议有意跳过。逐像素 React 组合与编辑/查看模式仍
为 partial。数据源列表页升级为上游布局：35 张 Available 卡（品牌 SVG icon/
描述、hover + Add、点击打开 `Create your {name} connector` schema 弹层）与按源
分组的 Added 卡（Settings→详情、Trash→删除确认弹层），数据源列表新增 Details 入口，落地
`/user-setting/data-source/{id}` 详情页：两处表单共用上游 35 套逐源动态字段集
（213 字段，编译期嵌入 schema）渲染 Text/Password/Number/Checkbox/Segmented/Select/
Textarea、必填星标、选项/默认值与条件显隐；详情页主按钮为 Save/Stop/Resume 状态机
（PATCH `reschedule`/`status`，RayRAG 无后台调度器、保存/恢复即时执行首轮同步），
Test 仅 REST API/BigQuery，Sync Logs 运行态每 15s 轮询。后端补
`GET/PUT/PATCH /api/v1/data_sources/{id}`、`/{id}/logs`、`/{id}/test`，`POST` 接受
嵌套 `config`（默认 prune=5/refresh=5/timeout=1740），记录嵌套持久化与状态归一化
（接受新旧两种拼写）；无 schema 的本地类型回退 URL/Target/Token 旧表单。
Box 数据源提供专用 web-OAuth 字段（Client ID/Secret/Redirect URI 弹层、
Configured/Authorized 徽章、Submit & Authorize 开窗 + Refresh status 轮询），
后端落地 `/api/v1/connectors/box/oauth/web/{start,callback,result}`（未完成返回
code 106；配置 `BOX_OAUTH_TOKEN_URL` 才执行真实令牌交换，大陆镜像/自托管可指向
镜像，未配置则持久化授权码原文）。

| RAGFlow (Python) | RayRAG (Rust) | 说明 |
|-----------------|---------------|------|
| `deepdoc/parser/pdf_parser.py` | `parser/pdf.rs` | PDF 解析 |
| `deepdoc/parser/docx_parser.py` | `parser/docx.rs` | Word 解析 |
| `deepdoc/parser/html_parser.py` | `parser/html.rs` | HTML 解析 |
| `deepdoc/parser/paddleocr_parser.py` | `ocr.rs` / `pipeline/mod.rs` | PaddleOCR 异步任务、JSONL 结果与解析接线 |
| `deepdoc/parser/mineru_parser.py` | `parser/mineru.rs` / `pipeline/mod.rs` | MinerU ZIP/content-list 与异步任务协议 |
| `deepdoc/parser/somark_parser.py` | `parser/somark.rs` / `pipeline/mod.rs` | SoMark SaaS/private async API 与结构化 block |
| `rag/nlp/rag_tokenizer.py` | `chunk/tokenizer.rs` | 分词/计数 |
| `rag/nlp/__init__.py` (naive_merge) | `chunk/naive.rs` | 分块策略 |
| `rag/llm/embedding_model.py` | `embed/openai.rs` | 向量化 |
| `rag/utils/es_conn.py` | `store/search_mapping.rs` / `store/mod.rs` | ES/OpenSearch 精确 mapping 构造与提交前 typed projection；JSON/PostgreSQL/zvec 替代引擎索引 |
| `rag/app/naive.py` (chunk函数) | `pipeline/mod.rs` | 解析编排 |

## Memory 消息兼容状态

Memory REST 查询现在保留固定 Quart `getlist` 合同：`agent_id`、`memory_id` 可重复传参，
也兼容仅一个参数时的逗号分隔；`page_size`、`limit`、`top_n` 的公开上限均为 100。
消息的持久身份是 `(memory_id,message_id)`，删除和 FIFO 驱逐不会再波及另一个 Memory
中复用相同整数 ID 的消息；FIFO 驱逐、upsert 与快照持久化也在同一次可回滚 mutation
内完成。`ES_INDEX_PREFIX` 命名 helper 已按固定服务动态读取并只 trim 前缀。

Memory 配置 wire 现在使用固定版 `permissions=me|team`、5 MiB 默认/10 MiB 上限、FIFO 与
temperature 0.5；embedding/type 只在实际已有消息时冻结，容量预算本身不再造成误锁。
创建时会按所选类型持久化固定 `PromptAssembler` system prompt；类型变更时，只有旧值仍是
旧类型默认、请求又没有显式提交 prompt，才自动切换为新默认，不覆盖自定义或显式空值。
固定的三类 instruction、JSON schema、examples、user conversation prompt 以及严格的小写
JSON fence/非法 JSON 回退合同均已有 Rust 单测与 Python 输出哈希门禁。混合类型在固定
Python 中受 hash-set 顺序影响，Rust 有意稳定为 semantic、episodic、procedural。
列表/config 响应返回 Owner nickname，页面提供 Owner 多选。创建与设置模型框从受鉴权的
tenant model catalog 按 embedding 与 chat/image2text capability 分流，提交固定
`model@instance@provider` selector（模型名可含 `@`）。RAGFlow tenant 模型目录位于
`GET /api/v1/models`，显式 Chat 默认位于 `GET /api/v1/models/default`；OpenAI protocol
模型目录使用 `GET /api/v1/openai/models`。

Memory 新增消息现在使用固定版 RAW-first 异步合同：请求线程完成 RAW embedding 和持久化，
随后发布持久 `task_type=memory`；worker 在文档 metadata 查询前分流，使用 Memory 的
temperature 调用 Chat/image2text 模型，把合法 JSON 扁平为 semantic/episodic/procedural
子消息，批量 embedding 后以 `source_id=RAW id` 原子保存。非法 JSON 视为空抽取成功；合法
JSON 的结构错误令任务失败但 RAW 保留。任务 payload/digest 可跨重启恢复，staging 状态只有
在 RAW 与 Memory 都存在时才发布；消息响应把最新 digest task 回绑父行，失败公开为
`progress=-1`。固定 v0.26.4 正式 worker 实际忽略配置中保存的自定义 prompts，当前 Rust
同样保留此合同，不虚报自定义 prompt 已参与抽取。

这仍不是 Memory 全栈完成声明：原生引擎 index create/delete、实际 prefix 资源隔离、
missing-token repair、engine cache repair、跨 task/message 文件的单事务以及私有 task payload
静态加密仍未替代。线上 Rust UI 已注册
`/memories`、`/memory/memory-message/:id`、`/memory/memory-setting/:id`：列表具备安全 DOM
卡片、搜索/类型/存储筛选、分页和 CRUD；消息页具备 sidebar、Agent/会话筛选、展开行、
启用/遗忘、内容/向量和分页；配置页具备基础、模型、容量与权限/FIFO/温度/提示词表单。
任务点现在始终可见并提供 32x32 点击区，Done/Running/Failed 对应绿/黄/红，日志使用
safe-text DOM，缺失时间显示 `—`，父行可展开新生成的子消息。这些路由仍保持 `partial`：
非 Chat 默认模型管理、固定头像上传协议和逐像素组件行为尚未完全等价。三个公开 HTML shell 不预嵌租户
业务数据，路径 ID 也使用 script-safe JSON 编码；实际数据继续由鉴权 API 加载。

## 密码传输与 TLS

RAGFlow 固定仓库中的 `conf/private.pem` / `conf/public.pem` 不是 JWT 密钥，而是
登录、注册和改密请求的 RSAES-PKCS1-v1_5 密码封装对。RayRAG 不复制这个公开已知的
私钥；如需兼容 RAGFlow 浏览器/CLI 的
`base64(RSA(base64(UTF-8 password)))` wire，可生成部署专用的至少 2048-bit RSA
PKCS#1/PKCS#8 私钥，以只读 secret 挂载，并设置：

```bash
RAYRAG_PASSWORD_PRIVATE_KEY_FILE=/run/secrets/rayrag-password-private.pem
```

服务启动时会解析、校验私钥并派生 SPKI 公钥；客户端可匿名读取
`GET /api/v1/system/password-public-key`。注册、登录、自助改密和管理员创建/重置用户均
接受该密文，同时保留固定 Go 服务的明文回退。未设置该变量时，密码是普通 JSON 字段；
默认 Compose 暴露的是 HTTP，只适合本机/受信网络，生产部署必须在反向代理终止 HTTPS。
无论传输形式如何，落盘只保存带随机盐的 Argon2id 哈希。不要使用上游仓库自带私钥或
固定 `Welcome` 口令。

解析任务终态会形成持久化操作历史，可按知识库、状态、任务类型、关键词和时间筛选：

```text
GET /api/v1/pipeline/operation-logs?kb_id=<id>&operation_status=done,failed&page=1&page_size=20
```

## Agent Canvas 执行器

RayRAG 会校验并执行 RAGFlow `components` DSL，已对标的核心组件为
`Begin`、`Agent/Generate/LLM`、`Message`、`Categorize`、`Retrieval`、
`TavilySearch`、`TavilyExtract`、`DuckDuckGo`、`Wikipedia`、`GoogleScholar`、`GitHub`、`YahooFinance`、`ArXiv`、`PubMed`、`Switch`、
`VariableAggregator`、`VariableAssigner`、`StringTransform`、`ListOperations` 和
`DataOperations`、`DocGenerator/DocsGenerator`、`ExcelProcessor`、`Invoke`、顶层及 Loop/Parallel body 内的 `UserFillUp`，以及作为运行时宏的
`Loop`、`Parallel`（前端仍显示为
`Iteration`）。运行时支持 `sys.*`、`env.*`、
`component@field.path` 变量引用，返回真实
执行路径、节点 trace 和结构化 `reference`。Retrieval 复用生产 ACL、知识库 embedding
selector、混合检索、可选 reranker、手工 metadata filter、top-N/top-K 及文档聚合；DSL
未指定 dataset 时回退 Agent/请求绑定的 KB。其 Memory、KG、跨语言、children/TOC 扩展
和 LLM 自动生成 metadata 条件仍未对齐，保存 DSL 时会明确拒绝。不支持的组件同样在
首个节点运行前失败，不会静默降级成普通 prompt 对话。旧版无 `components` 的 Canvas
记录继续使用 `prompt_template` 兼容路径。

Canvas 和运行时规范化边界会先执行固定版 `agent/dsl_migration.py` 的历史迁移：旧
`Splitter`、`HierarchicalMerger`、`PDFGenerator` 组件分别改名为 `TokenChunker`、
`TitleChunker`、`DocGenerator`，并同步重写组件 ID、上下游/父子拓扑、模板变量引用、
path、React-Flow 节点/边/form、history/messages/reference。迁移只匹配精确内置名称，
保留自定义标签与业务参数，输入不被原地修改且重复调用稳定。该结论仅表示 DSL 结构迁移
文件已对齐；这些 ingestion 组件是否可在 Agent Canvas 中直接执行仍由闭集组件目录独立
校验，未接线的节点继续在执行前明确失败。

Agent/Generate/LLM 组件现在统一兼容固定版缺省 `{sys.query}`、标量 prompt 和
`[{role,content}]` prompt：系统提示只发送一次，历史默认保留 13 轮并按
`message_history_window_size * 2` 截断，首条 prompt 与历史末条同角色时替换而不是重复追加
当前问题。组件采样参数接受固定 UI 的 `*Enabled` 开关，显式关闭时保留请求级/模型默认值，
同时继续兼容没有开关的旧 DSL；`max_tokens=0` 表示不覆盖。结构化输出从固定
`outputs.structured` JSON schema 启用，追加 schema 指令、按 `max_retries + 1` 尝试，
清理 `</think>` 与 JSON fence 后发布原生 `structured`，失败进入 `_ERROR` 或
`exception_default_value`。节点级 tenant `llm_id`/image2text 选择、`sys.files`/视觉输入、
引用提示、97% token context fitting、通用 JSON repair、chat-template kwargs、thinking、
延迟退避/exception-goto 和真实下游增量流仍未接线，因此 Python `llm.py` 与 Go `llm.go`
继续诚实标为 `partial`。

Agent 的工具路径已从“忽略 tools 后普通聊天”收紧为真实 ReAct 纵向链路。当前 Rust 会校验
固定 DSL 的 `{component_name,name,params}` 工具对象，把 Retrieval、TavilySearch、
TavilyExtract、DuckDuckGo、Wikipedia、Google、GoogleScholar、GitHub、YahooFinance、ArXiv 和 PubMed 的上游函数名 `search_my_dateset`、`tavily_search`、
`tavily_extract`、`duckduckgo_search`、`wikipedia_search`、`google_search`、`google_scholar_search`、`github_search`、`yahoo_finance`、`arxiv_search`、`pubmed_search` 按 DSL 顺序稳定编号为 `_idx`，通过 OpenAI-compatible `tools` /
`tool_choice=auto` 发给节点选定模型；模型返回的批量 `tool_calls` 会连同 assistant 消息和逐项
`tool` 结果续接到下一轮。Retrieval 复用同一生产检索后端、KB fallback、reference 和 usage
累计；Tavily 使用固定官方 Search/Extract 端点、Bearer JSON、禁 redirect/环境代理和 16 MiB
响应上限，优先读取工具静态 `api_key`，为空时回退 `TAVILY_API_KEY`。Search 强制关闭图片与
raw-content 大载荷，把结果转成 RAGFlow `kb_prompt` 风格正文、document aggregation 和结构化
reference；Extract 保留原始 result 数组，并兼容固定 Python 的逗号 URL 字符串。两者也可作为
顶层 Canvas 节点执行；`.env.example` 和 Compose 已公开空的 `TAVILY_API_KEY` 传入点，不包含
凭据。DuckDuckGo 同样不需要凭据，固定访问 HTML Search、vqd/news JSON 和 Go Instant Answer
端点，禁 redirect/环境代理并限制 16 MiB 响应；它统一前端 `text/news` 与 Agent
`general/news` 两套枚举，按 `top_n=10`、外层重试和 12 秒 deadline 生成 Python 原始 JSON、
summary chunk、aggregation、reference 与 `kb_prompt`。锁定 SDK 的随机 HTML/lite 后端、HTML
翻页、浏览器/TLS impersonation、cookie/`DDGS_PROXY` 和精确 rate-limit 分类仍保持 `partial`。
Wikipedia 不需要凭据，生产请求只能访问固定 68 个语言代码对应的
`https://<lang>.wikipedia.org/w/api.php`；它对齐 Python 的 `language=en`、`top_n=10`、外层重试和
60 秒 deadline，逐页取得摘要并跳过消歧义/缺失/单页错误，生成同样的 summary chunk、document
aggregation、reference 与 `kb_prompt` 正文，同时发布 Go 兼容的 `results` envelope 与
`title/snippet/url` JSON 行。
Python `wikipedia==1.4.0` 额外的 auto-suggest 请求、进程全局缓存/限流和 Go 通用 HTTP telemetry
仍未逐项复刻，因此对应生产文件保持 `partial`。Google Web Search 需要 SerpApi key；Canvas/Agent
固定访问 `https://serpapi.com/search`，严格对齐 `google-search-results==2.4.2` 自动补入的
`source=python`、`output=json` 以及 RAGFlow 的 `engine=google`、`google_domain=google.com`、`gl/hl`
字段、240 个国家代码和 156 个语言代码。Python 类缺省 `country=cn/language=en`，前端新节点实际种入
`country=us/num=12`；`q/start/num` schema 全部保留，但固定源码未把 `start/num` 传给 SDK，Rust 也不
伪造分页/条数语义。成功时保留原始 `organic_results`，优先取
`about_this_result.source.description` 并仍要求 `snippet`，生成 10,000 字符 chunk、aggregation、reference
与 `kb_prompt`；空 query 只清空 formalized content，外层重试、末次 delay 和 12 秒 deadline 已接线。
独立 Go 兼容方法则保留完全不同的 Google Programmable Search CSE 协议：`api_key/cx/query` 必填、
`max_results<=0` 缺省 5、上限 10，发布三字段 `results` envelope。两条协议均固定生产端点、禁环境代理/
redirect 并限制 16 MiB 响应；requests/Go HTTPHelper 的 proxy、OpenTelemetry、独立抖动重试与精确异常
分类仍为 `partial`。Google Scholar 同样无需 API key；Canvas/Agent
固定访问 `https://scholar.google.com/scholar`，对齐 Python 的 `top_n=12`、`relevance/date`、可选年份、
`patents=true`、分页、外层重试与 12 秒 deadline，解析 `scholarly` 核心 title/link/author/venue/year/abstract
字段后生成原始核心 publication JSON、reference chunk、aggregation、reference 与 `kb_prompt` 正文；独立
Go 兼容方法保留 `q/hl/num`、默认 5/请求上限 20、`ragflow/1.0` User-Agent、五字段结果和 `results`
envelope。锁定 `scholarly==1.7.11` 的随机 UA、1–2 秒节流、五次代理/会话轮换、redirect/cookie/CAPTCHA
浏览器解题与全部 citation/eprint/author-id 富元数据，以及 Go HTTPHelper telemetry/抖动重试仍未逐项复刻，
所以 Python/Go 两条生产文件保持 `partial`。GitHub 仓库检索也已成为独立 Canvas/Agent 工具：生产端点固定为
`https://api.github.com/search/repositories`，Python 路径保留 stars 降序、参数类 `top_n=10` 与前端
`top_n=5` 的真实差异、`requests/2.32.5` User-Agent、API version header、外层重试、原始 `items` JSON、
description/watchers chunk、aggregation、reference 与 12 秒 deadline；独立 Go 方法保留可选 Bearer token、
默认 5/上限 30 和四字段 `results` envelope。Canvas/Agent 的 Python query-only schema 不暴露 token，Go
HTTPHelper 的环境代理、OTel、30 秒 client/独立抖动重试，以及 Python requests redirect/proxy 与异常细分仍为
`partial`。YahooFinance 也无需用户凭据；Python/Canvas 路径按 `yfinance==0.2.65` 的 basic
cookie/crumb 流程固定访问 Yahoo 域名，保留 `info/news=true`、八个布尔参数、`financials` 实际输出
Calendar、`count/income_stmt` 只校验但不读取、外层重试与 60 秒 deadline；可按固定顺序输出信息、一个月
日线、日历、年度/季度资产负债表、年度/季度现金流和过滤广告后的新闻 Markdown，其中财务表使用完整
145/123 项 yfinance key。独立 Go 方法保留 symbols/fields query、`ragflow/1.0` User-Agent、null 标量零值和
四字段 `results` envelope。curl-cffi TLS impersonation、consent CSRF fallback、持久 cookie/cache、全部
history repair 与 pandas `to_markdown` 字节级格式，以及 Go HTTPHelper proxy/OTel/抖动重试仍未逐项复刻，
因此 Python/Go 生产文件保持 `partial`；公网可用性还可能受 Yahoo 对出口 IP 的 429 限流影响。ArXiv 同样无需凭据；Python/Canvas 路径固定访问
`https://export.arxiv.org/api/query`，对齐 `top_n=12`、`submittedDate/lastUpdatedDate/relevance`、
降序排序、外层重试和 12 秒 deadline，解析有界 Atom XML 后生成 summary chunk、document aggregation、
reference 与 `kb_prompt` 正文；独立 Go 兼容方法保留 `all:<query>`、默认 5、authors、PDF link/fallback 和
`results` envelope。锁定 `arxiv==2.1.3` 的内部三次重试、三秒分页节流、feedparser 容错细节和 Go
HTTP telemetry 尚未逐项复刻，所以 Python/Go 两条生产文件仍为 `partial`。PubMed 也无需 API key，
Canvas/Agent 走固定 NCBI E-utilities HTTPS `esearch` XML → `efetch` XML 路径，对齐 Python
`top_n=12`、contact email、外层重试、370ms 无 key 请求间隔和 12 秒 deadline；解析 PMID、标题、首段摘要、
期刊/卷期/页码、逐作者姓名与 DOI 后生成结构化 reference chunk、aggregation、reference 与 `kb_prompt`。
独立 Go 兼容方法走 JSON `esearch` → `esummary`，保留 `max_results=5`、上限 100、`ragflow/1.0`
User-Agent、PMID 顺序、三作者加 `et al.`、年份提取及五字段 `results` envelope。Biopython 1.86 的进程级
限流/三次内部重试/自动 GET-to-POST、DTD/ElementTree 边界，以及 Go HTTPHelper 的 proxy、OpenTelemetry
与抖动退避尚未逐项复刻，故两条 PubMed 生产文件同样保持 `partial`。最终节点额外发布 `tool_calls`；坏 JSON、非对象参数和模型幻觉的未知工具作为工具错误
回灌，不会 panic。达到 `max_rounds` 后追加固定 `Exceed max rounds` 消息并执行一次不携带 tools
的兜底生成。MCP、ExeSQL 等其余远程工具、嵌套 Agent、JSON repair、同轮并行调度、tool callback/
memory、artifact、citation 和真实 token delta stream 尚未完成，所以
`agent/component/agent_with_tools.py` 继续标为 `partial`；包含这些工具的画布会在运行前明确
拒绝，而不是伪装成功。

面向大陆网络环境的 9 个国内连接器已作为独立 Canvas/Agent 工具接入，全部
`.no_proxy()` 直连、15-30 秒超时、有界响应体，与既有 DuckDuckGo/Google 安全基线一致，
统一输出 RAGFlow `title/link/snippet` 形状的 tool 行，可在不改变 DSL 语义的前提下
无缝替换 Google/SerpApi：

- **Bing**（`bing`，`bing_search`）：无 key，固定访问 `https://cn.bing.com/search`，
  解析桌面结果页，对齐 `top_n=10`、外层重试与 12 秒 deadline；生成 summary chunk、
  aggregation、reference 与 `kb_prompt` 正文。
- **百度搜索**（`baidu`，`baidu_search`）：无 key，固定访问 `https://www.baidu.com/s`；
  跟随 302 安全跳转、启用 cookie store 与浏览器头（Accept/Sec-Fetch/sec-ch-ua），
  解析 `div#content_left` 下 `c-container` 结果。百度对自动化请求偶发返回
  "百度安全验证"（wappass）页，此时连接器返回明确错误（`safety verification
  page returned; retry later or use another provider`）供 Agent 层重试或切换，
  而不是静默返回空结果。
- **博查搜索**（`bocha`，`bocha_search`）：需 `BOCHA_API_KEY`（环境变量注入），
  固定访问博查 Web Search API，解析 JSON 结果。
- **腾讯财经**（`tencentfinance`，`tencent_finance`）：无 key，行情走 GBK 编码端点
  `https://qt.gtimg.cn/q=`（`v_sh600519="1~贵州茅台~..."`，`~` 分隔字段：3=现价、
  5=今开、31=涨跌额、32=涨跌%、33=最高、34=最低、36=成交量）；搜索提示走 UTF-8
  `https://smartbox.gtimg.cn/s3/`。生成单行行情正文 + Go 兼容 `results` envelope。
- **百度学术**（`baiduscholar`，`baidu_scholar`）：无 key，固定访问
  `https://xueshu.baidu.com/s`，解析 `sc_title/sc_abstract/sc_author/sc_year` 结果，
  与百度搜索同样处理 302/cookie/安全验证页。
- **东方财富 A股新闻**（`eastmoney`）：无 key，直连 `search-api-web.eastmoney.com`
  JSONP 搜索端点（与 akshare `stock_news_em` 同源），解析 `cmsArticleWebOld`
  新闻列表（标题/链接/正文/发布时间/来源），剥离 `<em>` 高亮标签。
- **金十财经**（`jin10`）：需 `JIN10_SECRET_KEY`，四模式——flash 快讯（category
  1-5）、calendar 财经日历（cj/qh/hk/us × data/event/holiday）、symbols 行情
  （GOODS/FOREX/FUTURE/CRYPTO × symbols/quotes，quotes 短键重命名为中文长名）、
  news 新闻（contain/filter 过滤）。
- **和风天气**（`qweather`）：需 `QWEATHER_API_KEY`，三模式——weather（now/
  3d/7d/10d/15d/30d）、indices 生活指数、airquality 空气质量；城市名先经
  geoapi 解析 location id；免费订阅走 devapi.qweather.com，付费走 api.qweather.com。
- **SearXNG**（`searxng`）：无 key，自建元搜索实例（`SEARXNG_URL` 或节点参数
  `searxng_url`），JSON API 聚合 Google/Bing/Baidu 等任意引擎，大陆/全球皆可。

9 个连接器均已注册进 `COMPONENT_REGISTRY` 工具目录（inputs/outputs 描述符），
`execute_node` 与 `execute_canvas_agent_tool_with_providers` 两条分发路径都含对应
分支；错误前缀映射、timeout 包装与重试语义与既有工具一致。Wikipedia 另已内置
`zh`/`yue` 语言预设（固定 68 语言代码表内），无需额外配置即可检索中文条目。

Message 组件兼容固定版前端 `content` 数组、v1 标量和 Go v2 `text` 别名；Canvas 保存/
编译时会拒绝缺失/空集合内容、非字符串选项和非布尔 `stream`。运行时随机选择一个消息，解析
`sys/env/component@path/item/index/result`，并用有限燃料、有限递归的 MiniJinja 沙箱处理
条件、循环和过滤器；模板错误保持上游 fail-soft 显示语义。上游值若是直接或 JSON 字符串
形式的 `{doc_id,filename,mime_type}` 描述符，会从正文分离到节点 `downloads`，移除内部
`include_download_info_in_content` 标志，重复引用不会重复收集。`output_format` 的
markdown/html/pdf/docx/xlsx 附件导出、Excel 数值类型转换、组件级异步生成器流、TTS 和
memory save 尚未接线，因此 Python `message.py` 与 Go `message.go` 仍诚实标为 `partial`。

Invoke 按固定 Python Canvas/前端合同支持 GET、POST、PUT，URL、参数和值、header 都可解析
`sys/env/component@path/item/index/result`，header 另兼容单花括号变量；JSON 模式会把合法的
字符串化 JSON 恢复为原生值，GET 使用 query，POST/PUT 使用 JSON 或 formdata。请求禁用
redirect 和环境代理，应用节点/请求双层 timeout 与 16 MiB 响应上限；目标及显式代理的全部
DNS 结果都必须为公网地址并固定到已校验 IP。代理无法约束其再次解析 hostname，因此沿用固定
Go 安全收紧：代理模式只允许公网字面 IP 目标。SSRF 拒绝统一写 `_ERROR="URL not valid"`，
普通网络错误按 `max_retries + 1` 与 `delay_after_error` 重试；`clean_html=true` 输出纯文本。
Python `requests` 的 charset 猜测、完整 DeepDOC block/table 清洗、partial stream 拼接、动态
input-form/thought，以及 Go v2 独有 DELETE、status/body/headers envelope 和 OpenTelemetry
transport 尚未一一对齐，所以 Python/Go 两个 Invoke production 文件继续标为 `partial`。

```text
POST /api/v1/agents/{id}/completions
{
  "question": "请处理这个请求",
  "inputs": {"language": "zh-CN"},
  "stream": true
}
```

`stream: true` 返回 RAGFlow Agent Canvas 的扁平 SSE envelope：每帧为
`data:{"event":"...","message_id":"...","created_at":0,"task_id":"...","session_id":"...","data":{}}`，成功路径依次
包含 `workflow_started`、每个顶层调度节点的 `node_started/node_finished`、
`message/message_end`、`workflow_finished`，等待路径以 `waiting_for_user` 结束，所有路径均
以 `data: [DONE]` 收尾。节点 start 会在组件 `await` 前实时写入 body stream，finish 携带
本次 runtime inputs/outputs/error/elapsed timing；message 与 workflow 终态在 checkpoint 和
conversation exchange 持久化成功后发送。恢复流不再发 `workflow_started`，只观察本次继续
执行的节点；等待帧额外携带服务端 checkpoint 所需的不透明 `resume_token`。丢弃 HTTP body
会结构化 drop 正在运行的 future；同一 Agent 的新 stream 在 ACL/DSL/checkpoint/model
preflight 通过后会静默替换旧流，旧 lease 不会删除新运行 registry 项。SSE message_id 与
conversation 中持久化的消息 ID 相同。

当前仍不是完整 runner 对齐：LLM 只在完整回答生成后发送一个 `message`，Loop/Parallel
child 只暴露外层 macro 生命周期，registry 仅在当前进程且只覆盖 `stream: true`，task_id
尚未绑定发布版本，spawned task panic 也尚未转换为 error + done。省略 `stream` 仍返回原
JSON 以兼容既有客户端；这些差异继续在覆盖矩阵中标为 `partial`。

当任一顶层或 Loop/Parallel body 内的 `UserFillUp` 不能从本轮初始输入完成表单时，接口返回
`data.event="waiting_for_user"`、`waiting_for_user.kind/cpn_id/tips/inputs` 和不透明
`resume_token`，并把完整 scheduler cursor 与 Canvas 状态仅保存在服务端
`agent_checkpoints.json`（启用 PostgreSQL backend 时同步进入 snapshot mirror）。同一
`conversation_id` 的下一次请求会原子 claim 该 checkpoint；可显式回传 token，并用
`resume_data` 或 `inputs` 提交标量/对象：

```text
POST /api/v1/agents/{id}/completions
{
  "conversation_id": "<waiting response>",
  "question": "表单已提交",
  "resume_token": "<waiting response>",
  "inputs": {"name": "Ada", "age": 37}
}
```

恢复从暂停节点继续，不会重新运行其上游 Begin、Retrieval 或 LLM；等待回合仍持久化成
user + 空 assistant 消息对。过期/不匹配 token、并发二次 claim、等待期间 DSL 变化均返回
HTTP 409。Loop checkpoint 会保存当前迭代和 body 游标；Parallel checkpoint 会保存原始
items、已完成 item 快照及所有暂停 item 的独立 Canvas 游标，并通过稳定的复合
`interrupt_id` 区分展示叶子与实际恢复地址。真实文件服务/layout recognition、多叶子一次
提交、Redis TTL/跨进程 claim，以及实时、可取消的 SSE 执行流继续明确标为 `partial`。

响应中的 `workflow_path` 和 `workflow_trace` 可用于调试分支与变量传递。完整组件目录
（检索扩展、工具、沙箱、复合 checkpoint 恢复和分布式取消）仍按逐文件覆盖表继续对标。

Begin 支持 RAGFlow 的 `conversational`、`task`、`Webhook` mode、显式 runtime
inputs、单字段 question fallback 以及 scalar/object descriptor 解码；尚未接线的非空文件输入
会明确失败。StringTransform 的 split 支持多个 literal delimiter 并保留空片段，merge 会用
首个 delimiter 连接数组。Jinja2 statement/control-flow、文件/layout 解析和协作取消仍
标记为 partial，详见覆盖矩阵，不会被报告为已完整对齐。

ListOperations 已逐行对标固定版 Python 与 Go 两套生产实现，支持 `nth`、`head`
（含旧 DSL 的 `topN` 别名）、`tail`、`filter`、`sort`、`drop_duplicates`，保留
正负索引、strict 范围错误、稳定多字段对象排序，以及原生 `result/first/last` 输出。

DataOperations 已接入不可变组件目录和纯 Rust Canvas 分派，覆盖 `select_keys`、
`literal_eval`、`combine`、`filter_values`、`append_or_update`、`remove_keys`、
`rename_keys` 七种 JSON 变换；兼容 Python/Go 字符串 query、Go CSV 以及前端
`[{ "input": "component@field" }]` / `[{ "name": "key" }]` 表单形态。JSON literal、
Python `True/False/None` 和前导小数可递归解码；Python `ast.literal_eval` 独有的单引号
容器、tuple/set，以及动态 input-form/thought/cancellation 接口继续标记为 partial。

ExcelProcessor 已接入不可变组件目录和纯 Rust Canvas 分派：兼容 Python/前端的
`read/merge/transform/output` 与 Go 的 `write` 别名，支持 Canvas selector、base64 和
`data:` URI 内联文件，读取 CSV/XLSX 的全部、首个或具名工作表，输出原始行、工作表名、
records、摘要和有界 Markdown 预览；可作 concat/基础 outer join，并由已有 ZIP + OOXML
栈生成多工作表 XLSX 或首工作表 CSV。输入和生成文件都受 16 MiB 上限约束，工作表名称会
清洗、截断并去重。当前生成附件保留在 trace 的内联 base64 descriptor 中，尚未接入上游
FileService/STORAGE_IMPL 的持久化 `doc_id`；pandas 的日期/公式/dtype、精确 outer-join
冲突/空值语义、组件 thought/cancellation/decorator timeout 也仍未一一对齐，因此固定版
Python/Go 两个 production 文件继续诚实标为 `partial`。

DocGenerator 已按固定版 Python/前端的 `DocGenerator` 名称接入，并兼容同提交 Go 端的
`DocsGenerator` 别名。组件编译期校验非空 content、`pdf/docx/txt/markdown/html`、至少
12pt 字号及布尔开关；运行时解析 Canvas selector、删除完整或悬空 `<think>` 内容、按固定
规则清洗文件名并替换扩展名。TXT/Markdown/HTML 复现 Go writer 的字节布局，DOCX 由 ZIP +
OOXML 直接生成段落、字体、页眉页脚、水印、页码字段和时间戳，PDF 由 lopdf 生成 A4 页面、
Unicode ToUnicode map、页眉页脚、水印和真实页码；五种格式都只走 Rust 进程并受 16 MiB
上限约束。输出同时提供 Python 风格 JSON `download`、Go/Rust 风格 base64 bytes 和内联
attachment，Message 可按 `include_download_info_in_content` 消费描述符。当前 `doc_id` 尚未
落入 tenant STORAGE_IMPL，preview URL 没有持久化对象支撑；Pandoc/XeLaTeX 的 Markdown AST、
嵌入 Noto 字体及精确视觉版式也未一一复现，所以 Python/Go 主文件、PDF/DOCX writer 继续
标为 `partial`，仅固定 Go 的 TXT/Markdown/HTML writer 标为 `aligned`。

Loop 已实现固定 Go 端的核心执行合同：优先按 `parentId` 收集 body，兼容折叠并回写旧
`LoopItem/IterationItem` 入口；无分组旧 DSL 回退到遇回边停止的 descendants。每次至少
执行一次 body，再对共享 Canvas 状态计算 AND/OR 终止条件；支持 constant、实时 variable
引用和按类型零值初始化、完整 string/bool/number/object/list/null 操作符、Switch 分支、
多终端，以及仅作 no-op 终端的 `ExitLoop`。`maximum_loop_count` 未满足条件时返回可区分
错误并保留已完成迭代状态；0 采用固定 Go driver 的 1,024 次安全上限。UserFillUp 会在
同一迭代的精确 body 游标恢复，序列化往返不会重跑已经完成的迭代前缀。逐迭代 stream、
通用 Eino serializer/child-store 格式和并发 sibling wave 仍明确为 partial。

Parallel 已实现固定 Go 宏的核心 fan-out：按 `parentId` 隔离 body，兼容折叠
`IterationItem` 及旧 `@result`/回边引用；`items_ref` 为 null 时返回空批，非数组明确报错。
每项克隆独立 Canvas 状态并注入 `item/index`，`max_concurrency=0/1` 严格顺序，较大值先在
调用路径完成第 0 项，再对其余项作有界异步并发。输出始终按输入下标排序，公开 `_result`
完整逐项快照，并把声明的 `item`、`index`、`component@object.path` ref 汇总为数组，缺失
路径为 null。UserFillUp 复合暂停会持久化原始 items、完成/未完成 partition 及每个未完成
item 的局部 Canvas 状态；每次恢复只消费当前稳定 item 地址，已完成项不重跑，最终仍按原
索引聚合。一次提交同时恢复多个叶子、上游原始 bridge-store 字节格式、panic recovery、
嵌套 Loop/Parallel 和逐项 stream 仍诚实标记为 partial。

## 特性

- ✅ PDF/DOCX/TXT/Markdown/HTML 解析（PDF 含大纲/书签提取：
  OutlineEntry title/depth/page 镜像 RAGFlow extract_pdf_outlines，
  挂 Document.metadata["__outline__"]，reportlab 嵌套书签 PDF 实测）
- ✅ Unicode 感知的令牌计数（中英混合）
- ✅ 令牌分块 + 重叠
- ✅ OpenAI 兼容 Embedding API
- ✅ zvec 向量存储集成（原生 `zvec-rust`，构建 `--features zvec-backend`）
- ✅ RAGFlow Agent Canvas 核心工作流执行
- ✅ 国内搜索连接器：Bing/百度/博查/腾讯财经/百度学术（Agent 工具目录已注册）
- ✅ Wikipedia zh/yue 中文预设
- ✅ PaddleOCR v0.26.4 异步 submit/poll/JSONL 与图片/PDF 管线接线
- ✅ 视觉模型客户端 `VisionClient`（对标 RAGFlow cv_model.py：OpenAI 兼容
  image_url data URL，describe/describe_with_prompt，GPU 8088 Qwen3.5-9B
  视觉投影实测通过）
- ✅ ASR/TTS 客户端 `src/audio.rs`（对标 sequence2txt_model.py +
  tts_model.py：AsrClient multipart 转写 / TtsClient 语音合成，覆盖
  OpenAI/StepFun/SiliconFlow/Xinference/Ollama 等全部 OpenAI 兼容端点）
- ✅ S3 兼容对象存储 `src/storage.rs`（对标 rag/utils/storage_factory.py
  族：AWS SigV4 签名客户端，MinIO/AWS S3/阿里云 OSS/腾讯云 COS 一套覆盖，
  真实 MinIO 容器 round-trip 实测通过；`RAYRAG_STORAGE_*` 环境配置）
- ✅ OpenDataLoader v0.26.4 health/multipart/JSON/Markdown 与 PDF 管线接线
- ✅ MinerU v0.26.4 同步 ZIP/content-list、Go 异步任务协议与 PDF 管线接线
- ✅ SoMark v0.26.4 SaaS/private submit/retry/poll、结构化 block 与 PDF 管线接线
- ✅ PostgreSQL 18.4 状态快照镜像（构建 `--features postgres-backend`）
- 🚧 完整 OCR/版面坐标、裁剪、tenant provider 与视觉增强
- ✅ RAPTOR/GraphRAG 持久化轻量实现 + **LLM 摘要闭环**（ClusterSummarizer
  trait 镜像 RAGFlow _summarize_texts 协议：{cluster_content} prompt 模板、
  同语言标题行、max_token≥512；LlmClusterSummarizer 逐块截断预算；GPU 8088
  Qwen3.5-9B 实测润色摘要，LLM 失败自动回退抽取式）
- ✅ 编译模板执行器（对标 chunk_post_processor.run_tree_templates +
  structure.py 知识编译）：POST /api/v1/compilation_template_groups/{id}/execute
  按 kind 分双桶——tree 模板建 RAPTOR 树（config.raptor
  prompt/max_token/threshold/max_cluster），非 tree 模板走两阶段 LLM
  超图抽取 + LLM 判定合并去重（cosine ≥ 0.9 前置过滤）；单模板失败
  跳过；带 ?doc_id=&kb_id= 时合并落库为 knowledge_compile 图
  checkpoint（tree 图 + 超图实体/关系，镜像 _struct_upsert_graph_json）；
  GPU 端到端实测通过
- ✅ LLM 超图抽取（对标 structure.py 知识编译非 tree 模板）：两阶段
  JSON 抽取（实体 → 已知实体约束的关系），prompt 渲染镜像
  _struct_hypergraph_prompts（编译模板 shape + 旧 output shape 双兼容、
  list/set/hypergraph 三态 Auto-type）；gen_json 容错解析 + compile_hypergraph
  逐 chunk 抽取去重；GPU 8088 中文两阶段实测通过
- ✅ 知识编译合并（对标 structure.py merge 阶段）：LLM 判定合并
  （_struct_merge_pair 三 prompt 逐字镜像、temperature 0.0、
  duplicated/merged 协议）+ relation 端点不变式（_struct_apply_merge_invariants）
  + cosine ≥ 0.9 前置过滤 + chunk_ids 并集；GPU 8088 重复合并/无关拒绝
  实测通过

## 构建

```bash
# 安装 Rust（如未安装）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 构建
cd /vol1/1000/RayRAG
cargo build --release

# 运行
cargo run --bin rayrag -- parse document.pdf
```

## PaddleOCR provider

外部 OCR 默认关闭。启用时，RayRAG 使用固定 RAGFlow v0.26.4 的
`POST /api/v2/ocr/jobs` multipart 提交、Bearer 鉴权、指数退避轮询和 JSONL 结果协议；
支持 `PaddleOCR-VL`、`PaddleOCR-VL-1.6`、`PP-OCRv5`、`PP-OCRv6`、
`PP-StructureV3`、`PaddleOCR-VL-1.5`。配置缺失、未知算法、空结果和服务错误均明确失败，
不会把文件名占位文本伪装成 OCR 成功。

```bash
export RAYRAG_OCR_PROVIDER=paddleocr
export PADDLEOCR_BASE_URL=https://paddleocr.aistudio-app.com
export PADDLEOCR_ACCESS_TOKEN='<secret>'
export PADDLEOCR_ALGORITHM=PaddleOCR-VL

cargo run --bin rayrag -- parse scan.png --layout-recognize PaddleOCR
```

服务端知识库的 `parser_config.layout_recognize` 可使用 `PaddleOCR`，也兼容上游
`<model>@<instance>@PaddleOCR` 三段 selector。Docker Compose 会透传同名环境变量。
旧 `paddleocr-gpu-proxy` 可用 `RAYRAG_OCR_PROVIDER=legacy` 与
`RAYRAG_OCR_BASE_URL=http://...` 保持兼容。当前 provider 仍来自进程环境，尚未统一到
tenant Provider 实例；PDF 空间 bbox、页面图像裁剪、progress callback 与本地 DeepDOC
OCR fallback 继续标记为 `partial`。

## OpenDataLoader PDF provider

OpenDataLoader 默认关闭。设置 API server 后，RayRAG 会先探测固定的 `GET /health`，
再向 `POST /file_parse` 发送 `application/pdf` multipart；可选 Bearer 鉴权，并按上游
协议透传 `hybrid`、`image_output`、`sanitize`，提交失败最多即时尝试 3 次。响应优先递归
转换 `json_doc` 的文本、表格、图片和公式节点，没有文本 section 时回退 `md_text`。

```bash
export OPENDATALOADER_APISERVER=http://127.0.0.1:9383
export OPENDATALOADER_API_KEY='<optional-secret>'
export OPENDATALOADER_TIMEOUT=600

cargo run --bin rayrag -- parse document.pdf --layout-recognize OpenDataLoader
```

服务端知识库也可把 `parser_config.layout_recognize` 设为 `OpenDataLoader` 或
`<model>@<instance>@OpenDataLoader`，并在同一配置中使用 `hybrid`、`image_output`、
`sanitize`。Compose 会透传三个 `OPENDATALOADER_*` 环境变量。当前仍未实现上游本地
PDF 页面渲染/outline、bbox 坐标 tag、crop 图片、from/to page、`parse_method` tuple 形态、
progress callback 和 tenant Provider 实例取密，因此该文件仍诚实标为 `partial`。

## MinerU PDF provider

MinerU 默认关闭。设置 API server 后，RayRAG 会先对固定的
`HEAD /openapi.json` 做五秒可用性探测，再向 `POST /file_parse` 提交
`application/pdf` multipart。Rust client 同时支持固定版本中的两种返回合同：Python
路径的同步 `application/zip`（从 ZIP 内直接读取 content-list JSON，不落盘解压），以及
Go 路径返回 task ID 后轮询 `GET /tasks/{id}/result` 的 Markdown。可选 Bearer token 是
Go local driver 能力扩展；Python 路径无需 token 时保持空值即可。

```bash
export MINERU_APISERVER=http://127.0.0.1:8000
export MINERU_API_KEY='<optional-secret>'
export MINERU_BACKEND=pipeline
export MINERU_REQUEST_TIMEOUT_SECONDS=1800
export MINERU_TIMEOUT_SECONDS=30

cargo run --bin rayrag -- parse document.pdf --layout-recognize MinerU
```

`MINERU_BACKEND` 严格接受固定七种 backend；`vlm-http-client` 还要求
`MINERU_SERVER_URL`。服务端知识库可把 `parser_config.layout_recognize` 设为 `MinerU`
或 `<model>@<instance>@MinerU`，并通过 `mineru_lang`、`mineru_parse_method`、
`mineru_formula_enable`、`mineru_table_enable` 控制每份 PDF。Compose 也会透传
`MINERU_OUTPUT_DIR`、`MINERU_DELETE_OUTPUT` 等 wrapper 兼容配置；当前 Rust ZIP 路径在
内存中读取 JSON，不产生需要清理的输出目录。

当前实现会过滤 header/footer/page number/discarded/未知 block，转换文本、表格、图片
caption、公式、代码和列表，并保留 table/image marker。尚未实现 PDF page rendering 与
outline、bbox/tag/crop、ZIP 图片落盘和 VLM 图片描述、page range、manual/pipeline/paper
tuple 返回形态、progress callback、tenant Provider 实例取密，以及 MinerU.Net 托管版的
public-URL 工作流，因此三份目标文件继续标为 `partial`。

## SoMark PDF provider

SoMark 默认关闭。私有部署设置 `SOMARK_BASE_URL`；使用托管服务时可只设置
`SOMARK_API_KEY`，base URL 会采用固定的 `https://somark.tech/api/v1`。私有地址先执行
`HEAD` 探针并接受任意小于 500 的状态；托管地址则调用 `POST /usage` 校验 API key 和
剩余付费/免费页数。

```bash
export SOMARK_BASE_URL=http://127.0.0.1:8088/api/v1
export SOMARK_API_KEY='<optional-for-private-deployment>'
export SOMARK_IMAGE_FORMAT=url
export SOMARK_FORMULA_FORMAT=latex
export SOMARK_TABLE_FORMAT=html
export SOMARK_CS_FORMAT=image

cargo run --bin rayrag -- parse document.pdf --layout-recognize SoMark
```

PDF 通过 `POST /parse/async` multipart 提交，包含 `file`、重复值
`output_formats=json`、JSON `element_formats`、JSON `feature_config`，SaaS key 作为表单
字段发送，私有部署留空时完全省略。业务码 `1124` 按固定源码在十分钟预算内指数退避并加入
jitter；提交成功后先等待两秒，再对 `POST /parse/async_check` 使用 1.5 倍、单次最多十秒
的轮询间隔，直到 `SUCCESS`、`FAILED` 或十分钟总预算耗尽。

服务端 `parser_config.layout_recognize` 支持 `SoMark` 和
`<model>@<instance>@SoMark`。四个 `somark_*_format` 与七个 `somark_enable_*` /
`somark_keep_header_footer` 字段可按文档覆盖 provider 默认值；Compose 透传同名环境变量。
结果会映射固定 21 种 block：TOC/blank 丢弃、header/footer 可选保留、未知类型回退 text、
有效 title level 转为 Markdown 标题、无 bbox 图片跳过，其余图片获得唯一 caption，并输出
chunker 可识别的 table/image marker。每个保留 block 的原始 bbox 会生成固定格式的
`@@<一基页码>\t<x0>\t<x1>\t<top>\t<bottom>##` 标签；SoMark parser 会给文档添加仅内部
可见的位置感知标记，chunker 只在该标记存在时于 embedding 前移除标签，按 Python `int()`
语义截断坐标，并把有序坐标写入 `position_int`、`page_num_int` 和 `top_int` 元数据。
普通文本中形似 `@@…##` 的字面量不会被改写，chunk API 与检索引用可直接返回真实解析位置。

当前仍未实现本地 PDF outline/page rendering、将 raw bbox 缩放到实际渲染页面、crop
图片对象、raw/manual/pipeline tuple 形态、progress callback、Go JSON/Markdown
ParseResult 与精确 page/file metadata，以及 tenant Provider 实例持久化/选择；因此
Python/Go 两份目标 parser、`rag/nlp/__init__.py` 和四-provider wrapper 继续诚实标为
`partial`。

## zvec Rust 后端

默认使用可移植的 JSON 索引。启用 `zvec-ai/zvec-rust` 原生后端时，需要提供
`libzvec_c_api` 并显式开启 feature。当前锁定 **v0.7.1**：crate 走 git tag
`v0.7.1`（该 patch 版本尚未发布到 crates.io），native 库取同名 release 预编译包，
`ZVEC_LIB_DIR` 指向其解压目录（内含 `libzvec_c_api.so`、`TARGET` 与
`data/jieba_dict/`）：

```bash
export ZVEC_LIB_DIR=$HOME/.local/lib/zvec/0.7.1
export RAYRAG_VECTOR_BACKEND=zvec
export RAYRAG_ZVEC_DIR=./zvec-data
cargo run --bin rayrag --features zvec-backend -- parse document.pdf \
  --embed --mode openai --api-key "$EMBED_API_KEY" --collection knowledge-base
```

不设置 `RAYRAG_VECTOR_BACKEND` 时继续使用 JSON，便于无原生库环境运行和回滚。

服务端启用 zvec 后，HTTP 文档解析、文档删除、手工 chunk CRUD 和反馈权重更新会
在同一提交边界内同步 JSON 混合检索索引与 zvec。zvec 按知识库创建独立 collection，
避免不同 embedding 模型共享向量空间；服务启动时以 JSON 为真源执行全量对账，清理
中断提交遗留的陈旧主键。JSON 仍负责 BM25、metadata filter 和 rank feature。

## PostgreSQL 18.4 状态镜像

PostgreSQL 保存 RayRAG JSON 状态快照，zvec 仍负责向量索引。该模式保持本地原子文件
为运行真源，同时提供数据库恢复副本：

```bash
cp .env.example .env
# 设置 RAYRAG_POSTGRES_PASSWORD
docker compose --env-file .env up -d postgres
cargo run --bin rayrag --features postgres-backend -- serve
```

服务启动会执行 `SELECT VERSION()`；设置
`RAYRAG_POSTGRES_REQUIRED_VERSION=18.4` 后，版本不匹配会直接拒绝启动。快照写入使用
PostgreSQL `ON CONFLICT` 原子 upsert，并按快照键获取事务级 advisory lock；校验和不一致时
拒绝恢复损坏数据。

## 部署

纯 Linux 模式使用宿主 Rust 二进制、外部或容器 PostgreSQL，以及宿主 zvec native library：

```bash
export RAYRAG_POSTGRES_URL=postgresql://rayrag:password@127.0.0.1:5432/rayrag
export RAYRAG_VECTOR_BACKEND=zvec
export RAYRAG_ZVEC_DIR=/var/lib/rayrag/zvec
export ZVEC_LIB_DIR=/path/to/zvec/build/lib
export RAYRAG_ADMIN_EMAIL=admin@rayrag.local
export RAYRAG_ADMIN_PASSWORD='replace-with-a-strong-admin-password'
cargo build --release --locked --features postgres-backend,zvec-backend
./target/release/rayrag serve --port 9380
```

纯 Docker 模式默认构建 PostgreSQL + zvec 双后端。`zvec-rust` 在 builder 阶段通过
Xget 下载对应 Linux 架构的预编译 `libzvec_c_api`，运行镜像将其安装到
`/opt/zvec/lib`；builder 和运行层的 APT 默认使用清华 TUNA Debian 镜像：

```bash
cp .env.example .env
# 先在 .env 中替换 PostgreSQL 和管理员密码；管理员密码至少 12 个字符。
docker compose --env-file .env up -d --build
```

Compose 默认从 DaoCloud 国内加速地址拉取 `postgres:18.4`、
`rust:1.97.1-bookworm` 和 `debian:bookworm-slim`，镜像名可分别通过
`POSTGRES_IMAGE`、`RUST_IMAGE`、`RUNTIME_IMAGE` 覆盖。Dockerfile 直接构建时仍以这
三个官方镜像标签为默认值。应用镜像和 Compose 都配置了
`GET /api/v1/system/healthz` 健康检查；该探针会实际访问 PostgreSQL snapshot worker
并检查 zvec 数据目录及已打开 collection，而不是返回固定成功。鉴权后的
`GET /api/v1/system/status` 还会返回与 RAGFlow 对齐的 doc engine、storage、database
和 task executor heartbeat 结构。

需要禁用 native zvec 时，显式设置 `RAYRAG_FEATURES=postgres-backend` 和
`RAYRAG_VECTOR_BACKEND=json` 后重新构建。RayRAG 不会在请求 zvec 但二进制未编译该
feature 时静默回退。下载源均可通过 `.env` 的 `DEBIAN_MIRROR`、
`DEBIAN_SECURITY_MIRROR`、`ZVEC_RELEASE_BASE` 和 `ZVEC_RELEASE_FALLBACK_BASE`
覆盖。zvec 默认先尝试 Xget，失败后自动回退官方 GitHub，不修改宿主 Docker daemon。

同理，若设置了 `RAYRAG_POSTGRES_URL`，二进制必须包含 `postgres-backend` feature；
配置与编译能力不匹配时启动即失败，避免误以为状态已镜像到 PostgreSQL。

## UI 与 OpenAI 兼容层

- ✅ 16 页面 Rust 手写 UI（dashboard/kbs/search/chat/agents/memories/files/skills/
  providers/settings/admin/status/api-docs/login/404 + dataset/document-viewer），
  全面对标 RAGFlow 页面层级，每个功能均有真实交互按钮（批量上传/删除/清空/重命名、
  chunk 分页编辑、会话管理、知识图谱环形可视化、搜索多 KB 过滤、模型选择器）
- ✅ OpenAI 兼容端点：`POST /api/v1/chat/completions`（含 SSE 流式）、
  `GET /api/v1/openai/models`、`POST /api/v1/embeddings`、`POST /api/v1/rerank`——
  LangChain/OpenWebUI 等外部生态可直接接入
- ✅ 聊天 SSE 流式（打字机效果 + 推理模型思考占位 + 引用卡片）
- ✅ SearXNG 自托管接入（`SEARXNG_URL`，cn.bing 引擎国内网络适配）
- ✅ 容器性能开关：`RAYRAG_NO_FSYNC=1`（overlay 文件系统上传提速 30%）

## 快速开始

```bash
# 纯 Linux（宿主机）
cargo build --release --features postgres-backend,zvec-backend
RAYRAG_PORT=9390 RAYRAG_ADMIN_PASSWORD=xxx ./target/release/rayrag

# 纯 Docker
RAYRAG_FEATURES=postgres-backend,zvec-backend \
RAYRAG_POSTGRES_PASSWORD=xxx RAYRAG_ADMIN_PASSWORD=xxx \
docker compose up -d --build
# 浏览器打开 http://<host>:9390 （默认 admin@rayrag.local / 上面设置的密码）
```

GPU 四链路（embedding/LLM/rerank/OCR）通过环境变量注入：
`EMBED_API_BASE`/`LLM_API_BASE`/`RERANK_API_BASE`/`RAYRAG_OCR_BASE_URL`。

## 依赖

- [zvec-rust](https://github.com/zvec-ai/zvec-rust) — 阿里 zvec 向量数据库 Rust 绑定
- [lopdf](https://crates.io/crates/lopdf) — PDF 解析与 DocGenerator 纯 Rust 生成
- [docx-rs](https://crates.io/crates/docx-rs) — DOCX 解析及生成结果回读验证
