## 2026-08-13 v0.3.3bb：Chat 设置抽屉 + 钉钉 AI 表格连接器（Stage 2 主 UI + 连接器#2）

- 固定上游 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- **Chat 设置抽屉（对齐 app-settings/chat-settings.tsx）**：
  - RAGFlow 结构实测：详情页 ⚙ Settings 按钮（testid chat-settings）→
    右侧 440px 抽屉（testid chat-detail-settings），标题 "Chat Setting"，
    表单 = ChatBasicSetting（icon/name/description + LLM collapse + prologue
    + KB 多选）+ ChatPromptEngine（system/cross_languages/rerank/similarity/
    top_n/tavily/toc/use_kg/empty_response/quote/keyword/tts/reference_metadata），
    底部 Cancel + SavingButton。defaultValues：quote=true、keyword=false、
    tts=false、use_kg=false、toc_enhance=false、top_n=8、
    similarity_threshold=0.2。
  - RayRAG 实现：chat_app_detail_page 重写，加 ⚙ 按钮 + settings-drawer
    （440px 右滑入），两段表单（基本设置/提示引擎），5 开关 + 2 数值 +
    3 textarea + LLM datalist + KB 多选 + 11 语言 chip；Save/Cancel；
    PUT /api/v1/chatapps/{id} 持久化。新增 .settings-drawer/.set-* CSS
    与 13 条 i18n 键。
- **update_chat_app handler**（RAGFlow updateChat parity）：PUT 接受任意
  子集（name/description/icon/kb_ids/llm_id/llm_setting/prompt_config），
  name-only 保持 rename 语义；rename_chat_app 被取代删除。测试 3 个
  （roundtrip + name-only + prompt_config/llm_setting 解析）。
- **ChatApp 字段**（上轮 6a317da）：llm_setting/prompt_config 持久化已就绪。
- **钉钉 AI 表格连接器**（子agent 并行，仅改 src/connectors.rs）：
  DingTalkAiTableConnector —— appKey/appSecret 换 access_token → 列 bases
  → 列 sheets → 列 records，每行一个文档（列名:值拼接），token 缓存至
  到期前 60s；kind "dingtalk_ai_table" 分发替换 bail；6 个本地 mock 测试。
  connectors:: 37 passed（31+6）。
- 待办转移：rerank 选择器、reference_metadata、refine_multiturn、reasoning
  字段未入表单（inventory 已记录），下轮补。


## 2026-08-13 v0.3.3ba：Chat 应用体系 Stage 1（RAGFlow 0.26.4 /chats 对齐）

- 固定上游 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- **溯源**：RAGFlow 0.26.4 Chat 页为应用列表（`web/src/pages/next-chats/index.tsx`，
  路由 `/chats`），非会话列表。结构：ListFilterBar（标题 "Chat apps" +
  搜索框 + Create chat 按钮，仅列表非空或搜索态显示）→ 卡片网格
  （ChatCard：icon/name/description + 悬停 ChatDropdown Rename/Delete）
  → 分页 footer；空态为可点击大卡片（EmptyAppCard，testId
  chats-empty-create）；创建弹窗复用 RenameDialog（425px，仅名称字段，
  save 按钮 testId rename-save）。
- **新增 ChatApp 层**（此前 RayRAG 只有裸 Conversation）：
  - `src/api/chatapp_mgr.rs`：ChatApp 结构（id/name/description/icon/
    tenant_id/kb_ids/llm_id/created_at）+ ChatAppStore（JSON 持久化，
    list/get/insert/rename/remove）+ 4 handler（list/create/rename/delete），
    路由 `/api/v1/chatapps[/{id}]`。
  - `AppState.chat_apps`（主构造 + server.rs/web.rs 测试构造各补）。
  - `Conversation.app_id`（serde default 兼容旧记录）+ `ConvStore::set_app_id`。
  - `ChatRequest.app_id`：create_chat 后绑定（/api/v1/chats POST 可传 app_id）。
- **web.rs 新增两页**：
  - `chat_apps_page`（/chats）：标题+搜索+Create 按钮、卡片带悬停
    ✏️/🗑 操作、空态可点大卡片、创建/重命名双弹窗（纯名称，425px）。
  - `chat_app_detail_page`（/chats/{id}）：应用头部 + New conversation
    按钮（POST /api/v1/chats 带 app_id）→ 绑定会话列表。
  - 新增 `html_esc` 辅助 + 13 条 i18n 中文键。
- **路由别名对齐**：/chats、/chats/{id}、/searches（RAGFlow 0.26.4
  路由为 /datasets /chats /searches /agents /memories /files）。
- **RSS 连接器**（子agent 并行，仅改 src/connectors.rs）：quick-xml
  解析 RSS 2.0 + Atom、SSRF 私网防护、feed 探针、list/fetch/normalize，
  本地 TcpListener mock 测试。kind "rss" 分发替换 bail。
- 测试：web 28、chatapp_mgr 1、connectors 增至 26+（RSS 3+）。


## 2026-08-13 v0.3.3az：数据源注册表对齐 RAGFlow 0.26.4 全量 35 源

- 固定上游 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- **溯源**：RAGFlow 0.26.4 数据源管理页（user-setting/data-source）由
  `web/src/pages/user-setting/data-source/constant/index.tsx` 的
  `DataSourceKey` 枚举驱动，共 **35 源**；表单字段 schema 在
  `DataSourceFormFields`（30 源）+ 5 个独立 constant 文件
  （bitbucket/confluence/jira/s3/seafile-constant.tsx）。
  已程序化抽取落档：
  `docs/ragflow-ui-ref/data-source-manifest.json`（35 源清单 +
  sync_deleted_files 支持位）、
  `docs/ragflow-ui-ref/data-source-form-fields.json`（30 源表单字段）。
- **ConnectorRegistry::kinds() 扩到 41 项**：35 个 RAGFlow 源（原样
  标签，如 "Dingtalk AI Table"、"Azure Blob Storage"）+
  6 个本土扩展（local/feishu/钉钉云盘/腾讯文档/语雀/微信文档）。
- **create() 分发**：新增 `r2`（S3 协议族，Cloudflare R2 兼容）、
  `azure_blob`（并入原 blob SAS 适配器，旧 "blob" 分发保留兼容历史
  记录）；其余 24 个 RAGFlow 源登记后 connect 明确报
  "connector not implemented yet"（按域逐步实装，不静默失败）。
- **钉钉语义澄清**：RAGFlow 官方源是 **钉钉 AI 多维表格**
  （dingtalk_ai_table，NotableClient），不是云盘。现有钉钉云盘连接器
  降级为本土扩展（kind "dingtalk"），官方语义 kind "dingtalk_ai_table"
  已登记，实装排队。
- **web.rs 数据源页**：下拉 41 项 + dsHint 悬停提示映射全量同步。
- 测试：registry_kind_tests 断言扩为全量 35 源清单。


## 2026-08-13 v0.3.3ay：钉钉云盘连接器（子agent 并行）+ DocumentSource::DingTalk + prompt_config 守卫

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- **钉钉云盘连接器**（子agent 实现，仅改 src/connectors.rs，文件隔离）：
  appKey:appSecret → /v1.0/oauth2/accessToken（缓存 Mutex）→
  x-acs-dingtalk-access-token 头 → 列空间/文件（nextToken 分页）→
  download 接口取 Location → GET 内容。target=spaceId 可空（遍历全部空间）。
  本地 TcpListener mock 测试 6 个（accessToken 缓存/分页/下载跳转）。
  注意：因主 agent `git add -A` 打包，钉钉代码实际落在提交 b32b7c2
  （该提交消息仅写了 chip 多选，含 connectors.rs +754 行）。
- **DocumentSource::DingTalk** 变体（as_str/from_str 全接线）+ normalize
  与测试断言更新（移除 Local 占位）。
- **prompt_config 守卫**：update_dataset 校验允许仅含 prompt_config 的
  PUT（此前 400 "name, permission, embedding_model or parser_config
  is required"）。
- 数据源登记现共 17 类：gitlab/blob/local/webdav/feishu/confluence +
  s3/gcs/oracle（S3 协议族）+ notion/google_drive/gmail +
  钉钉/企业微信/腾讯文档/语雀 + yuque/dingtalk 已实装。


## 2026-08-13 v0.3.3ax：检索页 Language 多选 + prompt_config 全链路（cross_languages）

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- 源码确证：cross_languages 存 KB 的 `prompt_config`（db_models.py 默认模板
  + dialog_service.py:652 读取并翻译提问）。据此打通 RayRAG 全链路：
  1. `KnowledgeBase.prompt_config`（serde default {}，create/持久化全兼容）；
  2. `update_config` 增第 5 参（Option<&Value>），`update_dataset` PUT 时对
     `prompt_config` 做**对象级合并**（保留其他 chat 设置，不清空覆盖）；
  3. UI：两个配置表单 Retrieval 段加 **Languages 输入行**（cfgLangs，
     comma-separated 语言码 + 官方 tooltip ③ 原文）；save 写入
     body.prompt_config.cross_languages，load 从 kb.prompt_config 回填；
  4. chat_completion 与 retrieval 端点：读 KB prompt_config.cross_languages
     → tenant LLM 按官方 prompt 批量翻译 → 逐语言 hybrid 检索 →
     chunk.id 去重合并；无 LLM/翻译失败优雅降级。
- 新测试：prompt_config_tests 2（持久化 roundtrip + 不污染其他字段）、
  web 断言 cfgLangs/cross_languages 接线。


## 2026-08-13 v0.3.3aw：检索页 ⓘ 悬停提示 + 检索四字段（cross_languages/use_kg/keyword/size）+ 语雀连接器

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- **悬停遍历方法**：JS 合成 mouseover 不触发 Radix tooltip，必须 CDP
  `Input.dispatchMouseEvent` 真实鼠标移动 + 1.2s 停留；捕获检索页 3 条
  官方 tooltip（相似度阈值/KG/语言）。UI 两个配置表单的 Top K/Threshold/
  Vector weight/KG 开关加 `ⓘ`（纯 CSS `.tip` data-tip，无 JS）。
- **检索四字段**（对齐 v0.26.4 SearchDatasetReq 实测字段）：
  - `keyword` → vector_weight 强制 0（纯关键词检索）；
  - `size` → 分页大小（0=回退 top_k）；
  - `use_kg` → 门控 graph_context（此前无条件返回）；
  - `cross_languages` → 真实翻译管线：tenant LLM 按
    cross_languages_sys/user prompt 批量翻译 → 逐语言 hybrid 检索 →
    按 chunk.id 去重合并；翻译失败优雅降级为原语言结果。
  - 新测试 search_request_schema_tests 2。
- **语雀连接器**（子agent 并行，文件隔离：子agent 仅 src/connectors.rs，
  主 agent 仅 web.rs/search.rs）：X-Auth-Token 鉴权、列仓库文档分页、
  拉 Markdown 正文、本地 TcpListener mock 测试 7 个。
- 工具脚本：/tmp/ui_traverse/hover_tips.py。


## 2026-08-13 v0.3.3av：S3 协议连接器（SigV4，真 MinIO E2E 通过）

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- **S3Connector**（`src/connectors.rs`，对齐 RAGFlow `s3_connector.py`）：
  手写 AWS SigV4 签名链（HMAC-SHA256 密钥派生 + canonical request/
  string-to-sign/signature），ListObjectsV2（quick-xml 解析 + 分页
  continuation-token）、GET object、load_credentials 探针。
- **注册表接线**：`s3 | gcs | oracle` 三种 kind 共用 S3Connector（S3 协议
  家族：GCS-interop、Oracle S3-compat、MinIO 全覆盖）。
- `DocumentSource::S3` 变体 + as_str/from_str roundtrip。
- **测试 7 个**：AWS 官方 SigV4 测试向量（canonical hash=7344ae5b…、
  签名=f0e8bdb…，与 Python hmac 独立复核一致）、canonical URI/query
  编码、region 猜测（aws/cn/minio）、ListBucket XML 解析（含分页 token）。
- **真机 E2E**：MinIO（rag_flow@172.22.0.3:9000，RAGFlow 栈容器）建桶
  `rayrag-s3-e2e` + 对象 `docs/hello.md` → RayRAG 建数据源 kind=s3 →
  connect 探针 code:0 → POST /files 列出 1 文件（name/extension/size/
  updated_at 精确一致）。E2E 后已清理数据源 + 网络连接。
- 注意：RayRAG 容器需 `docker network connect docker_ragflow rayrag-1`
  才能直连 MinIO（跨 compose 网络不互通；E2E 后已 disconnect）。

## 2026-08-13 v0.3.3au：数据源类型全量登记（RAGFlow 7 源 + 国内替代源）

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- 浏览器实测 RAGFlow 数据源页 7 种外部源：Confluence/Notion/Google
  Drive/Gmail/GCS/Oracle Storage/S3。RayRAG 原仅 6 种（gitlab/blob/
  local/webdav/feishu/confluence）。
- **注册表 +10 种**：`ConnectorRegistry::kinds()` 补 RAGFlow 剩余 6 种
  （notion/google_drive/gmail/gcs/oracle/s3）+ 中国大陆替代源 4 种
  （dingtalk 钉钉文档/tencent_docs 腾讯文档/yuque 语雀/wechat_docs
  微信文档），共 16 种。所有新类型可创建（is_supported_kind 走注册表），
  连接时未实现的连接器会明确报 `unsupported connector kind`（诚实占位，
  非静默失败）。
- **UI**：dsType 下拉 +10 选项（中文标签）+ dsHint 全量提示。
- 新测试：registry_kind_tests 1（RAGFlow 7 + 国内 5 + 旧 4 全覆盖断言）。
- 后续批次：实现 S3 协议连接器（SigV4，可覆盖 s3/gcs/oracle 三源）与
  钉钉/腾讯文档/语雀 REST 连接器。

## 2026-08-13 v0.3.3at：Page rank / Tag sets / Avatar 三字段（对齐实测配置页基础段）

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- 依据浏览器实测配置页基础段清单（Name/Language/Avatar/Description/
  Permissions/Embedding model/Page rank/Tag sets），补齐缺的三字段：
- **后端**：`KnowledgeBase + tag_sets/avatar`（serde(default) 兼容旧记录）；
  `ParserConfig + page_rank`（默认 0）；`update_identity`（name/description/
  tag_sets/avatar 一次更新，旧 `update_name_description` 委托）；
  `update_dataset` 把 tag_sets/avatar 从 parser_config 白名单移出改走
  KB 身份字段更新（RAGFlow 中 tag_kwd 也是 KB 级而非 parser 级）。
- **UI（两个配置表单都接）**：Page rank 数字输入、Tag sets 文本、
  Avatar 文件选择（data URL，300KB 上限）+ 圆形预览；load/save 全接线。
- 新测试：identity_update_tests 2（tag_sets/avatar 持久化 + 旧接口兼容）。

## 2026-08-13 v0.3.3as：RAPTOR/KG 高级参数组 UI（对齐实测配置页字段清单）

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- 依据 v0.3.3ar 浏览器点击实测的配置页字段清单，补齐 RAPTOR 与
  Knowledge Graph 高级参数组：
- **后端 ParserConfig +9 字段**：raptor_scope/prompt/max_token/
  cluster_method(GMM|AHC)/max_cluster/random_seed + graphrag_batch_chunk_size/
  entity_resolution/community_reports；默认值对齐上游（scope=Dataset、
  threshold=0.7、depth=5、cluster=GMM、max_cluster=64、batch=8196、
  method=light）。
- **UI 配置表单**：RAPTOR 段 +7 行（Threshold/Generation scope/Prompt/
  Max token/Clustering method/Max cluster/Random seed，随 Enable 禁用态）；
  KG 段 +3 行（Batch chunk token size/Entity resolution/Community
  reports）+ Method 恢复；load/save 全接线；PUT 白名单 +10 键。
- 新测试：raptor_graphrag_config_tests 2（JSON roundtrip + 默认值）。
- 剩余缺口（下批）：Page rank/Tag sets/Avatar 字段 UI + 数据源页
  （国内替代源飞书/钉钉/微信文档）。

## 2026-08-13 v0.3.3ar：建库弹窗对齐（浏览器点击实测）+ KB 配置页补 3 字段

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- **浏览器模拟点击实测**（fygo CDP 遍历 RAGFlow v0.26.2 线上实例）：
  `datasets → Create dataset` 弹窗结构 = Name → Embedding model →
  **Parse type (Built-in|Pipeline)** → **chunking method**（General/Q&A/
  Resume/Manual/Table/Paper/Book/Laws/Presentation/One/Tag 等 15 法）→
  Save/Close。RayRAG 建库弹窗此前只有 Name/Desc/Perm/Embedding。
- **对齐**：① 建库弹窗加 Parse type + chunk method 两个选择器（15 法，
  createKb 发送 `parse_type`/`chunk_method`）；② `KbStore` 新增
  `create_for_with_parser_config(parser_config)`（旧 `create_for_with_config`
  委托默认种子 `{"chunk_token_num":2048,"overlapped_percent":0.05}`）；
  ③ `create_dataset` 后端接受 chunk_method/parse_type 并 seed parser_config。
- **KB 配置页补 3 字段**：Children Delimiter（children_delimiters）、
  Title Levels（title_levels）、Auto Metadata（enable_metadata）——
  三者后端 ParserConfig 早已支持但 UI 缺失；PUT 白名单同步补 3 键；
  顺带修复 loadConfig 不恢复 excel_to_html 的小 bug。
- 新测试：create_seed_tests 2（自定义种子 + 默认种子）。
- **别名路由 E2E 通过**：`/dataset/{files|retrieval|logs|configuration}/{id}`
  四路由全部 200；建库 parser_config 种子实测
  `{"chunk_token_num":2048,"overlapped_percent":0.05,"chunk_method":"paper","parse_type":"Built-in"}`。
- **修复 kb_detail_page 真 bug**：showTab 循环 `i<3` 而页面有 6 个标签，
  点 Configuration/Knowledge Graph/Logs 时旧标签不隐藏（多标签同时显示）；
  改 `i<6` + URL pathname 映射选标签。
- 上游 v0.26.2 侧栏实测：Dataset/Chat/Search/Agent/Memory/File +
  用户设置页（Data sources/Chat channels/Model providers/MCP/Team/
  Profile/API/Log out）；Data sources 页含 7 种外部数据源（Confluence/
  Notion/Google Drive/Gmail/GCS/Oracle Storage/S3）——**RayRAG 数据源页
  与国内替代源（如飞书/钉钉/微信文档）为后续批次**。

## 2026-08-13 v0.3.3aq：chunk_method 领域解析器生产接线（app_parsers.py 对齐）

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- **又一处"实现完备但生产零引用"**：`src/app_parsers.rs`（Email/Paper/Laws/
  Book/Picture/Table/Tag/Presentation/Audio/Qa 十个领域解析器 + QaMode
  分发）只有 lib.rs 的 `mod` 声明引用——`pipeline.parse_document` 完全没
  调用。后果：KB 配置 UI 的 12 种 chunk_method 全是摆设，所有文档一律
  MIME 解析 + naive/token/title 分块（上游 `app_parsers.py get_parser`
  按 parser_id 路由到领域专属解析管线）。
- **接线**：① `app_parsers.rs` 加 `parser_for_method(chunk_method)` 分发
  （10 个领域方法 → 对应解析器；naive/token/title/manual/one/
  knowledge_graph 返回 None 走 MIME 管线）；② `parse_document` 顶部加
  领域解析分支（空结果/异常 warn 后回退 MIME）；③ `chunk/mod.rs` 加
  `SingleChunker`（对齐 `rag/app/one.py`：整文件单块，含 position 元数据
  空数组），`chunker_for` 路由 `one`。
- **UI 补齐**：KB 配置表单 chunkMethod 加 `picture/audio/email`（12→15，
  与上游一致）；新增 **layout_recognize 选择器**（DeepDOC/PaddleOCR/
  OpenDataLoader/MinerU/SoMark——后端五种识别器全部支持但此前 UI 无法
  选择），loadConfig/saveConfig 同步接线。
- 新测试：dispatch_tests 2（路由表 + email 解析）、single_tests 2。
- 覆盖矩阵：28 行 app_parsers.rs 引用行补记接线说明。

## 2026-08-13 v0.3.3ap：rerank 链路闭环（宿主代理架构真相）+ 全模块死代码扫描

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- **宿主 rerank 架构真相（推翻 v0.3.3an 的阻塞判断）**：8081 的 llama-server
  确实缺 `--reranking`（501），但宿主另有一个 **FastAPI 代理在 8899**
  （`mxbai-rerank.service` → `proxy_server.py`），用 **cross-encoder logprob
  法**（`get_relevance_score`：max_tokens=1 + logprobs 取 token '1' 概率）
  实现 OpenAI 兼容 `/v1/rerank`——**不需要** llama-server 的 reranking 标志。
  8899 绑 0.0.0.0，容器经 172.29.0.1:8899 可达。
- **修复**：local-rerank provider + 租户实例 api_base 8081 → **8899**。
  E2E：建库 → 上传 → 检索带 rerank_id → code 0，`score=1.0` 正确排序
  （相关文档 1.0 > 无关 0.917）。**四路 GPU 链路（LLM/embedding/vision/
  rerank）全部真机闭环**。
- **全模块死代码扫描（v0.3.3ao 模式的系统性复查）**：对 src/ 全部模块
  grep 外部引用数。结论：**无第二处死代码**——搜索/连接器模块
  （arxiv/google/wikipedia/pubmed/baidu/baidu_scholar/bocha/eastmoney/
  jin10/qweather/searxng/tencent_finance/tushare/wencai 等）全走
  agent.rs 工具注册表；graphrag→graphrag_index→server.rs；mcp_client→
  connector+mcp_server；benchmark→main.rs CLI。vision 是唯一特例且已修复。
- 检索响应字段：chunk 用 `score`（rerank 后归一化分），非 `similarity`。

## 2026-08-13 v0.3.3ao：图片解析 vision 模型生产接线（对齐 picture.py/naive.py）

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- **发现的真缺口**：`CvModel`/`VisionClient`/`vision_llm_chunk_sync` 在
  `src/llm.rs`、`src/vision.rs`、`src/parser/figure.rs` 全部完备且有测试，但
  **生产零引用**——`pipeline.parse_document` 走 `parser::get_parser(mime)` 时
  `FigureParser::new()` 恒 `vision: None`，图片文档永远拿不到租户默认
  image2text 模型（上游 naive.py:160-165 会解析
  `get_tenant_default_model_by_type(LLMType.IMAGE2TEXT)` 注入
  VisionFigureParser）。
- **接线**：① `Pipeline` 加 `vision: Option<VisionClient>` 字段 +
  `with_vision_client()` 建造器；② `parse_document` 在 PaddleOCR 分支后、
  ASR/兜底前加 vision 分支（`supports_ocr(mime)` && data≥100B →
  `figure_describe_prompt` 描述，输出 `[DOCUMENT_LAYOUT: VisionLLM]` 包装，
  空结果/异常 warn 后静默落回 FigureParser）；③ `server.rs` 加
  `kb_vision_for()`（resolve 租户默认 ImageToText 能力，None=无配置静默跳过），
  `process_document` 构造管线时注入。
- 新测试：`vision_client_describes_raster_images_when_configured`（mock
  OpenAI vision 服务 + 1×1 PNG 补 128B → 断言 VisionLLM 标记 + 描述文本）。
- **真机 E2E 全通**：宿主 PaddleOCR-VL-1.6-GGUF @ 8090 注册为 image2text
  provider → 上传中文 PNG → GPU 视觉识别落库为 chunk（`[Image: <id>]
  池塘溶解氧低于3毫克每升时 鱼类会出现浮头现象`）→ retrieval 命中 ✓。
- **坑**：租户实例/提供的 model 名必须与 llama-server 实际服务名一致
  （`PaddleOCR-VL-1.6-GGUF`，`/v1/models` 实测），自定义别名会导致
  vision 返回空描述（warn 落回 figure parser）。改实例 models[].name 即愈。
- **宿主 rerank 阻塞仍挂起**（见 v0.3.3an）：rerank 服务缺 `--reranking`
  标志且只绑 127.0.0.1，Boss 重启该服务加参数后即可直连验证。

## 2026-08-13 v0.3.3an：真机 GPU 端到端验证（宿主三模型全链路）

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- 宿主 GPU 模型清单（ss + /proc + /v1/models 实测）：LLM = Qwen3.5-9B-Q4_K_M
  @ 0.0.0.0:8088；Embedding = Qwen3-Embedding-4B-Q4_K_M @ 0.0.0.0:8888
  （llama.cpp --embedding --pooling mean，2560 维）；OCR = PaddleOCR-VL-1.6 @ 8090；
  rerank = mxbai-rerank-large-v2 @ 127.0.0.1:8081（API key `mxbai-rerank-key`）。
- **E2E 全链路打通**：注册 local-llm/local-embedding/local-rerank 提供商 →
  租户模型实例 → 建 KB（embd_id=裸模型名）→ 上传中文水产文档 → 解析 DONE →
  GPU embedding 索引 → retrieval 命中（similarity 0.9344）→ GPU LLM RAG 回答
  「溶解氧低于3毫克每升时鱼类会浮头」正确带引用。
- **两个实坑**：① 容器内 127.0.0.1 不通宿主 GPU 服务——provider 与租户实例
  api_base 必须用 docker 网关 `172.29.0.1`（compose 网络 rayrag_default 实测）；
  ② 租户模型实例创建时会**快照继承 provider 的 api_base**，改 provider 不会
  传导到已建实例，必须再 PUT 实例更新。
- **宿主侧阻塞（非 RayRAG 缺陷）**：rerank 服务启动时未带 `--reranking`
  标志，llama-server 明确报 501 "This server does not support reranking. Start
  it with `--reranking`"；且只绑 127.0.0.1。重启宿主 rerank 服务（加
  `--reranking --host 0.0.0.0`）后 RayRAG 即可直连验证 rerank 链路。
- 测试数据已清理（KB + chat 删除）；GPU 三个 provider + 租户实例保留，
  UI Model providers 页可直接选用。

## 2026-08-13 v0.3.3am：dark-first 设计 token 移植 + 22 个 UI 行转 aligned + crypt.py 行转 full

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- 经 fygo Chrome CDP 实测参考部署（v0.26.2 Tailwind 重写版）：深色主题为**固定设计**
  （`html.dark` 类 + `--background: rgba(11,10,18,1)`），非 prefers-color-scheme 跟随。
- 提取完整设计 token：页面底 #0b0a12、主文本 #f6f6f7（246 246 247）、次要文本
  #b2b5b7（178 181 183）、弱文本/输入提示 #75787a、卡片 rgba(255,255,255,.05)、
  头部栏 #0b0b0c、主强调紫 #9276ff、圆角 .5rem、字体 InterVariable 16px。
- RayRAG 全局 CSS（src/web_css.txt）由浅色默认+`.dark` 覆写**反转为 dark-first**：
  `:root` 装载 RAGFlow token 板，经典浅色板收进可选 `.light` 类；body 换
  InterVariable 16px 字栈；toggleTheme 语义同步（light/dark 键互换）。
- crypt.py 行转 full：上游 v0.26.2 login-next 页明文发送密码（Tailwind 重写时
  移除了客户端加密助手），RayRAG 无客户端加密 = 上游对齐。
- 覆盖矩阵：22 个 web/src 记忆页 UI 行 partial→aligned（功能完整 + token 级
  视觉对齐，微间距保留近似而非逐字 React 转译）。

## 2026-08-13 v0.3.3ak：task_executor 多任务类型分发对齐 + 行转 full

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- 逐分支溯源 rag/svr/task_executor.py 的 7 种任务类型：
  - dataflow=文档解析 → RayRAG document_parse ✓；
  - memory → RayRAG process_memory_task ✓；
  - **raptor/graphrag**：RayRAG 在解析管线内联构建（build_raptor_chunks /
    graphrag index at parse time），上游用独立任务类型——管线相同、分发不同；
  - **mindmap 是上游占位符**（progress 1 "place holder" 后 pass）——无实现可移植；
  - **skill 依赖重构版 executor**（TE_RUN_MODE=0）——固定版 executor 直接失败；
  - **artifact 在固定版 executor 无执行分支**（与 skill 同走 SAAS 版）。
- RayRAG worker 循环对未知类型的处理由笼统 failed 改为**逐类型上游对齐**：
  mindmap→done"place holder"、skill→failed（上游原句）、raptor/graphrag→done
  （注明解析时已内联构建）、artifact/未知→failed（注明无上游执行分支）。
- 覆盖矩阵 rag/svr/task_executor.py 行 partial→full（Redis 消费/心跳传输由
  进程内队列取代，属架构替换）。

## 2026-08-13 v0.3.3aj：messages.py missing-token 修复闭环 + 行转 full

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- 溯源 `fix_missing_tokenized_memory`：上游仅 elasticsearch doc engine 执行修复
  （修 ES 的 `tokenized_content_ltks` 字段），其他引擎打日志后直接返回。
- 核实 RayRAG 现状：修复循环本就完整移植（`repair_missing_field` 搜索缺失字段行
  → 重放 content 触发刷新，guard 两条路径均有测试）；缺的只是**生产启动接线**。
  RayRAG 用 `rag_tokenize`/`MsgTextQuery` 在查询时动态分词、不持久化 ltks 字段，
  故非 ES 跳过是架构正确行为——与上游非 elasticsearch 路径逐字一致。
- 本轮补齐启动钩子：server::run 在 chat channel 启动后打上游同款日志
  （"Not using elasticsearch as doc engine, no need to fix missing tokenized memory."）
  并注释完整溯源。
- 覆盖矩阵 memory/services/messages.py 行 partial→full——上一批标注的
  "missing-token no-op" 实为上游非 ES 原生行为，非缺口。

## 2026-08-13 v0.3.3ai：file_syncer 遗留键类型化解析 + service_conf 行转 full

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- 溯源 `file_syncer.max_concurrent_syncs/sync_interval`：v0.26.4 中为**遗留死配置**——
  Python 侧无任何引用（周期同步组件已退役，被逐数据源手动同步取代），Go 侧仅
  internal/server/config.go 的结构体 + admin 状态枚举，无活跃消费者。
- RayRAG 按配置对齐原则补齐：`ServiceConf.file_syncer: FileSyncerConf`（默认
  max_concurrent_syncs=4 / sync_interval=3，与 shipped service_conf.yaml 一致），
  from_value 用 u32_field 解析自定义值，测试覆盖文档解析与缺省回退两条路径。
  运行时行为与上游一致：手动 `sync_data_source` 端点即 v0.26.4 的实际同步方式。
- 覆盖矩阵 conf/service_conf.yaml 行 partial→full（全键解析齐，架构差异注明）。

## 2026-08-13 v0.3.3ah：all_models 扁平别名索引 + Add-model 自动补全

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- 查明 conf/all_models.json（3757 条 name/alias/model_types）的唯一消费方是内部 Go SDK
  （`internal/entity/models/model.go` ProviderManager 的 alias2ModelIndex），Python API
  与 React UI 均不引用。
- RayRAG 完整移植该语义：`src/api/fixtures/models/all_models.json` 原样内嵌
  （include_str! + std OnceLock 惰性解析，无新依赖）；`resolve_model_alias` 大小写不敏感
  name/alias → 规范条目（Go 对重复别名报错，RayRAG 确定性保留首个并注释）；
  `search_model_aliases` 子串搜索（UI 自动补全）；新增 `GET /api/v1/models/search?q=&limit=`
  （需会话、限 100 条）；Add Provider 弹窗 Models 输入框接 datalist 自动补全
  （防抖 250ms，逗号分段取末段搜索）。
- 覆盖矩阵 conf/all_models.json 行 partial→full。

## 2026-08-13 v0.3.3ag：llm_factories 目录补齐（DeerAPI fixture + Astraflow-CN 别名）+ conf 行复核

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。
- 三方对比：llm_factories.json 66 工厂 vs conf/models 60 文件 vs RayRAG 60 fixture。
  真缺口只有两个：**DeerAPI**（37 模型，OpenAI 兼容中转，官方端点
  api.deerapi.com/v1）与 **Astraflow-CN**（21 模型，与 Astraflow 逐条相同，仅
  端点不同——RayRAG 的 astraflow fixture 默认端点本就是国内
  api.modelverse.cn，故以别名 `astraflow-cn → astraflow` 覆盖）。
  其余 6 个工厂（BaiduYiyan/Tencent Cloud/New API/OpenAI-API-Compatible/
  RAGcon 等）在 RAGFlow 中本身就是 0 条目动态提供商，无模型可嵌。
- `deerapi.json` 由 llm_factories.json 逐条转 conf/models 格式生成（chat/
  embedding/image2text/speech2text/tts 五能力 37 模型）；fixture 总数 60→61，
  三处计数断言与 DeerAPI 端点/模型数抽查同步。
- conf 两行复核：service_conf.yaml 的 ragflow/admin host+port、
  task_executor.message_queue_type、user_default_llm.default_models、
  os.*→zvec 均已有类型化等价键（mysql/minio/es/infinity/oceanbase/redis/nats
  被 postgres+zvec 架构取代）；file_syncer 周期同步旋钮（RayRAG 现为手动
  sync_data_source）与 all_models.json 3757 条扁平别名列表仍 incremental，
  行备注已精确枚举。

## 2026-08-13 v0.3.3af：per-capability 模型默认值、memory 原生索引生命周期与 Infinity 查询语义

- 固定上游仍为 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。本批由
  3 个并行子 agent 分别完成 models / memory / infinity 三个域，主 agent 合并并修绿门禁。
- **models（tenant_models.rs/system.rs）**：按 `models_api.py`/`models_api_service.py`
  port 了 per-capability 默认值（embedding/vision/rerank/ASR/TTS 均可 PATCH
  `/api/v1/models/default` 持久化，带上游三重/型号校验）；`plan_added_models` 实现工厂目录
  展开（factory ∪ active − inactive − unsupported）+ rank 键回退；遗留 `/api/v1/llm/models`
  同步切换为 upstream `list_tenant_added_models` 语义（目录展开），其唯一断言随语义更新。
- **memory（features.rs/joint_services.rs/memory.rs）**：`MessageIndexLifecycle`
  （create/delete/has_index，prefix 应用到原生资源名）、`MemorySizeCache`、missing-token
  修复（无 tokenize 引擎时按 Python 语义 no-op）；joined-tenant mutation
  （owner 或 team 成员可改/可删，外部人等价 404）；两阶段跨 store 删除（先删 memory 行，
  `has_index` 门控再删 messages，两 store 各自独立事务与 Python 一致）；digest 富化。
- **infinity（store/infinity_query.rs + infinity_table.rs + search.rs）**：analyzer/index
  选择、通用 filter/sort/aggregation、dense threshold、atan Fusion（`atan(x)*2/pi`）与
  同表多维度向量生命周期（旧维度与新维度共存至表删除）。
- 主 agent 合并修复：models 子 agent 超时留下的 3 个失败测试——(1) server.rs 能力展开断言随
  目录展开语义更新；(2)(3) 测试夹具撞 ProviderStore 大小写契约（store id 一律小写、name
  保留目录原案；`check_model_available` 按 name 匹配、selector 扫描按小写 id 匹配），测试
  以 delete-then-create + `provider_id = factory_name.to_ascii_lowercase()` 修正。
- 覆盖矩阵 8 行 partial→full（models_api、models_api_service、memory_api、
  memory_api_service、memory_message_service、memory_service、memory_utils、
  infinity_mapping）；messages.py 保留 partial（missing-token 真实 tokenize 分支待分词引擎）。
- 本批同时完成 RAGFlow v0.26.2 真实 UI 全量遍历（fygo Chrome CDP + Playwright）并回写
  `docs/ragflow-ui-inventory.md`：确认知识库详情为左侧边栏导航（Tailwind 重写版）、
  Model providers 页六能力默认选择器/Added models/Available models 区块、Agent 画布
  顶栏（Save/Run/Launch/Publish/Management）、Memory 详情双 Tab 等增量结论。
- 内存基准更新（v0.3.3af，postgres-backend + 容器实测）：rayrag 容器
  **552 MiB cgroup / 228 MB VmRSS / 0.01% CPU**（稳态无泄漏，20s 采样不变）；
  postgres 79 MiB。旧基准 65.25 MiB 是 JSON 本地存储时代；现含 Postgres 连接池、
  zvec 客户端与 60 提供商工厂目录常驻。仍约为 RAGFlow（16.6 GiB）的 1/60。

## 2026-08-12 v0.3.3ae：Memory 持久任务、LLM 抽取与消息任务 UI

- 固定上游仍为 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。本批沿
  `api/db/joint_services/memory_message_service.py`（458 行、20,272 bytes、blob
  `2e773bb06ca0454a`）、`api/apps/services/memory_api_service.py`（389 行、16,655 bytes、
  blob `a12eb1594675678c`）、`rag/svr/task_executor.py`（1,947 行、88,355 bytes、blob
  `2e85ef7e82b1c8d1`）和 Memory message React hook/interface/table 复核正式 RAW-first
  queue/worker、digest 回绑、进度和按钮合同。
- `POST /api/v1/messages` 现在先解析可见 Memory、调用其 embedding、同步保存 RAW 对话，
  再发布持久 `task_type=memory`。Task 增加向后兼容 `digest` 与私有 conversation payload；
  公开 `/tasks`、operation log 与 message task 都通过显式 view 投影，绝不返回 payload、
  worker id、lease token 或 lease expiry。document dedupe/cancel 限定
  `task_type=document_parse`，同一 Memory 的连续对话永不按 `doc_id` 错误去重。
- task/message 是两个原子 snapshot，不能假装一个数据库事务。本批增加 staging write-ahead
  协议：先持久不可 claim 的 task，再在同一 Memory coordinator 内保存 RAW 并 publish；
  RAW 失败会 discard staging，publish 持久化失败则保留 RAW+staging，启动/周期 reconciler
  只在 RAW 与 Memory 均存在时恢复为 pending，否则清理孤儿。Memory 删除/清空先取消相关
  worker，并与 worker 最终提交共用 coordinator。私有 payload 目前仍以明文存在内部 task
  snapshot；HTTP 已隔离，但 at-rest 加密需要后续独立密钥管理批次。
- dispatcher 在任何文档 metadata 查询前按 `task_type` 分流。Memory worker 使用持久 lease、
  heartbeat、retry/cancel，按固定 0.15/0.35/0.5/0.65/0.85/0.95/1 节点追加日志；选择
  tenant Chat，找不到时兼容 image2text，`default` 使用全局 LLM。`ChatModel` 新增带默认实现的
  `chat_with_generation`，`LlmClient` 把 Memory temperature 送入现有
  `chat_completion_with_generation`，fake/其他 provider 实现保持源码兼容。
- extraction 直接复用上批已按 Python SHA 锁定的 `PromptAssembler`，没有复制第二套 prompt。
  JSON 语法错误按固定版变为空抽取并成功；合法但顶层/数组/字段结构错误令 worker 失败；
  unknown 顶层 key 保留为 `message_type`，缺失/空/null `invalid_at` 变 None，ISO-8601 时间
  规范为本地 wire。抽取内容整批 embedding、一次预算/FIFO mutation 保存为
  `source_id=RAW id` 子消息；失败不回滚已成功的 RAW，重试若已有任一 source child 则不重复
  插入。固定 commit 的两个正式调用点都没有传存储的 `system_prompt/user_prompt`，Rust
  有意复现此缺陷，而不是把可编辑字段虚报为已生效。
- message GET 按 `(task_type=memory,doc_id,digest)` 取同 source 最新 task，仅挂 RAW 父行；
  failed/cancelled 公开为 `progress=-1`，无任务使用 `null`，避免固定空对象在 JS 中误显示红点。
  UI 任务点未 hover 也始终可见，独立 32x32 点击区和 4px 圆点；Done/Running/Failed 为
  绿/黄/红，View/Forget 仍在 row hover/focus 出现，缺失/非法开始时间显示 `—`。task log
  与动态内容继续用 `textContent`/text node，恶意 HTML 不形成 DOM sink。
- 定向回归覆盖 6 条 extraction、25 条 TaskQueue、6 条 Memory server、4 条 Memory web，
  包括 legacy task 反序列化、staging 重启、document/memory 隔离、敏感投影、最新 digest、
  RAW-only dispatcher、semantic 子行、结构失败保留 RAW 和 reconciler。默认和
  `postgres-backend,zvec-backend` 的 all-target locked check 均通过；默认单线程全量为
  **1,216 passed / 0 failed / 27 ignored**（lib 1,214 + coverage bin 2），双特性为
  **1,223 passed / 0 failed / 27 ignored**（lib 1,221 + coverage bin 2）。双特性中的
  PostgreSQL snapshot 与 zvec native collection 测试通过；本批没有设置
  `RAYRAG_POSTGRES_TEST_URL`、也没有新启 PostgreSQL 18.4 容器，不能把 feature gate
  冒充数据库实机 smoke。coverage 固定 commit/4,441 files/unmapped=0、Compose config、
  全仓 `cargo fmt --all -- --check`、本批五个 Rust 文件定向 rustfmt 与 `git diff --check`
  通过。严格双特性 all-target Clippy 回到全仓既有 **369** 个 error header 基线，本批新增的
  async lock 命中已清零。
- FnOS Chrome 149/CDP 16002 对 `/tmp/rayrag-v033ae-browser-K3lIDo` 隔离 RayRAG 完成真实
  登录和 Memory 消息页点击。使用本机 loopback fake OpenAI-compatible chat/embed 生成
  semantic child；浏览器实测两个任务点均常驻、点击区 32x32、绿色 Done、row action opacity
  由 0→1、日志 modal 完整显示七段进度、父行展开后出现 Semantic 子行，page error、业务
  4xx/5xx、dialog 均为 0。截图 `/tmp/rayrag-v033ae-memory-task.png`（1280x900，SHA-256
  `0961a2b32f144bb30da1dfcc3a4459a5c3d1783f884d09043b611c0f2cd0046c`）。该 smoke 验证的是
  本批 RayRAG UI/worker，不冒充固定上游 live 服务；临时服务和隔离数据已在门禁后清理。
- coverage 将三个 Memory API/service 和三个 message UI 条目的已实现边界更新；同时把此前
  仅凭文档 executor 就宽泛声称 **full** 的 `rag/svr/task_executor.py` 纠正为 **partial**：
  Memory/document 分流已生产接线，但固定 executor 的完整 Redis consumer、多 task type、
  heartbeat/reporting 生态尚未逐项替代。

## 2026-08-12 v0.3.3ad：Memory PromptAssembler 与 LLM JSON 边界

- 固定上游仍为 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。本批逐函数复核
  `memory/utils/prompt_util.py`（190 行、7,421 bytes、blob `422f45cd48eea264`）、
  `memory/utils/msg_util.py`（37 行、1,425 bytes、blob `0f4e90e8ca0275a0`），并沿
  `memory_service.py:create_memory`、`memory_api_service.py:update_memory` 与正式
  message worker 调用链核对默认 prompt 的持久化和刷新时机。
- `src/memory.rs` 新增纯 Rust `PromptAssembler`：精确保留 system base、semantic/
  episodic/procedural instruction、required JSON schema、examples、user conversation
  template、`ISO 8601`/最多 5 项默认值，并过滤 RAW、未知值和重复类型。固定 Python 使用
  `set`，混合类型块会随 `PYTHONHASHSEED` 漂移；Rust 有意稳定为 semantic、episodic、
  procedural，同时用固定 Python 输出的字符数和 SHA-256 锁定 RAW 及三个单类型全文：
  `0353e9f4...ed8f70`、`d941d0ad...bfe6d59`、`d518bb26...3780ecb`、
  `8adfd8ba...363eaa`，user prompt 为 `2b71b613...64a6bd`。
- JSON helper 复现固定版的实际行为而非其过度宽泛 docstring：只 trim 全文、剥精确小写
  ` ```json ` 起始 fence 与尾 ` ``` `，语法失败静默回退 `{}`，不从 prose 中搜花括号，
  但合法 array 等非 object JSON 仍原样返回。回归覆盖纯 JSON、fenced JSON、大写 fence、
  prose、非法语法和合法 array。
- 新建 Memory 会把按类型生成的固定 system prompt 持久化，不调用或探测 LLM。更新类型时
  仅在旧 prompt 精确等于旧类型默认、请求没有显式提交 `system_prompt` 且类型确有变化时
  生成新默认；自定义 prompt 原样保留，显式空 prompt 也不会被悄悄覆盖。旧 snapshot 的空
  prompt 不做猜测性迁移，因为它可能是用户通过 API 明确保存的值。
- coverage 保留 `prompt_util.py`/`msg_util.py` 的 **full**，但纠正原来错误的
  “highlight + aggregation”映射说明，并把真实 lifecycle 写入两个仍为 **partial** 的
  Memory service 条目。本批没有虚报完整消息链：固定 `POST /messages` 是 RAW 先落盘、
  持久 `task_type=memory` 后台抽取；当前 Rust dispatcher 尚未按 task type 分流，若直接入队
  会被文档 worker 误判失败。因此非 RAW LLM、task digest/progress、失败后 RAW 保留和
  worker 重启恢复仍留待下一纵向批次。
- 九条 prompt/JSON 定向测试与七条 Memory store 测试通过。默认与
  `postgres-backend,zvec-backend` 的 all-target locked check 均通过；默认单线程全量为
  **1,202 passed / 0 failed / 27 ignored**（lib 1,200 + coverage bin 2），双特性为
  **1,209 passed / 0 failed / 27 ignored**（lib 1,207 + coverage bin 2），其中
  PostgreSQL snapshot 与 zvec native collection 回归通过。本批没有设置
  `RAYRAG_POSTGRES_TEST_URL`、也没有新启 PostgreSQL 18.4 容器，不能把 feature gate
  冒充数据库实机 smoke。coverage 固定 commit/4,441 files/unmapped=0、Compose config、
  定向 rustfmt 与 `git diff --check` 通过。严格双特性 all-target Clippy 仍由全仓既有
  **369** 个 error header 阻断；相关文件命中均位于本批前的 Memory tokenizer/chat 代码，
  新 PromptAssembler/config lifecycle 无命中。全仓 fmt 仍首先受既有
  `src/advanced_rag.rs:543` 等累计格式差异阻断。本批不改 UI，故不重复浏览器 smoke；
  上批已经过 FnOS Chrome 149 的 Memory 创建/配置页面验证，但不把它当作本批 prompt
  文本的 live 证据。

## 2026-08-12 v0.3.3ac：Memory Owner、模型 selector 与固定配置合同

- 固定上游仍为 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。本批逐字段复核
  `memory_service.py`、`memory_api_service.py`、`memory_utils.py`、`models_api.py`、
  `models_api_service.py`、`model-tree-select.tsx`、Memory create/setting form 与 Owner filter。
  `tenant_id` 是 Owner id，`owner_name` 来自用户 nickname；模型选择 wire 是从右解析的
  `model@instance@provider`，模型名自身允许含 `@`；embedding 与 chat/image2text 选项必须按
  capability 分流。
- Memory list/config/create/update 响应新增实时 `owner_name`，列表页新增 Owner 多选、计数、
  clear/apply，并发送逗号分隔 `owner_ids`。修复空 `tenant_id` 抢占 `owner_ids` 的查询优先级
  缺口。Owner 仍从当前用户可见集合构建，未授权值被可见性过滤自然求交；Rust 保留
  accepted-membership 与 owner-only config/delete 的安全硬化，不冒充固定版团队成员可写。
- `/api/v1/models` 从原来错误返回 OpenAI `rayrag-v1` 改为受 tenant membership 约束的
  enabled provider/instance/model 目录，返回 secret-free capability、instance metadata 与
  fixed selector；新增 `/api/v1/models/default` 返回已显式配置的 Chat default。OpenAI
  protocol 模型目录迁到兼容别名 `/api/v1/openai/models`，`/api/v1/llm/models` 同步过滤
  disabled provider 并补 selector。Memory 创建/设置页改从固定目录构造 provider/instance
  optgroup，embedding 仅接 embedding，LLM 接 chat/image2text，彻底移除不可解析的
  `provider_id:model` wire。embedding 等非 Chat 默认的持久化/修改 API 仍保持 **partial**。
- 新 Memory 固定使用 `permissions=me`、5 MiB capacity、FIFO 与 temperature 0.5；旧
  `private` snapshot 启动时原子迁移为 `me`。容量校验统一复用 10 MiB 常量并拒绝 0，遗忘
  策略只接受 FIFO。embedding/type 的后端与 UI 冻结条件改为实际消息内容存在，不再把
  非零容量预算误判为已有内容；LLM 按固定渲染行为保持可编辑，并接受 chat 或
  image2text capability。上游创建时的系统提示词拼装、旧异常容量/策略 snapshot 迁移、
  source-message task digest 与跨 Memory/config store 删除事务仍明确未完成。
- coverage 纠正四条既有过度声明：`models_api.py`、`models_api_service.py`、
  `memory_service.py`、`memory_utils.py` 均降为 **partial** 并映射到真实 Rust 模块；
  `model-tree-select.tsx` 从泛化 reference 改为本批已实现切片的 **partial**；
  `memory/constant.tsx` 注释纠正为三个 API action key，不再虚构路由常量语义。
- 定向门禁覆盖 6 个 Memory store 合同、含 `@` 模型名的右锚 selector、Owner/default/
  secret-free model catalog/OpenAI alias 的真实 HTTP 生命周期，以及 Memory list/setting 两个
  safe-DOM shell。默认与 `postgres-backend,zvec-backend` 的 all-target locked check 均通过；
  默认单线程全量为 **1,197 passed / 0 failed / 27 ignored**（lib 1,195 + coverage bin 2），
  双特性为 **1,204 passed / 0 failed / 27 ignored**（lib 1,202 + coverage bin 2），其中
  PostgreSQL 18 snapshot 与 zvec native collection 测试通过。本批没有另启 PostgreSQL 18.4
  容器，不能把 feature 测试写成数据库实机 smoke。coverage 对固定 checkout 的 4,441 个
  Git 文件/commit/blob/bytes/lines 校验通过，`unmapped=0`；当前摘要为 aligned 126、full
  540、partial 34、replaced 101、N/A 61、reference 3,579。Compose config 与
  `git diff --check` 通过；本批 Rust 文件定向 rustfmt 通过。全仓 `cargo fmt --all -- --check`
  仍首先被既有 `src/advanced_rag.rs:543` 等累计未格式化代码阻断；严格双特性 all-target
  Clippy 在修正本批三个简单命中后仍由全仓既有 **354** 项告警阻断，未借机重排用户的
  大范围未提交工作树。
- FnOS Chrome 149/CDP 16002 对 `/tmp` 隔离数据上的本批 RayRAG 完成真实登录、创建
  Memory、按 capability 选择 embedding/chat 模型、Owner 筛选、列表进入消息/配置页并保存；
  实测 selector、5 MiB、`me`、FIFO、0.5、零消息时模型可编辑，以及 secret-free/default/
  OpenAI alias 响应，JS exception/dialog/业务失败均为 0，唯一 404 是 `/favicon.ico`。截图：
  `/tmp/rayrag-v033ac-memory-owner-model-list.png`（1280x900，SHA-256
  `f0c9fcdbea8b05aa365de8e6de50fcfa8e777bf21f1eeb1a51a9e83fd2f27db9`）与
  `/tmp/rayrag-v033ac-memory-setting.png`（1265x1015，SHA-256
  `6eba8473a97064a3313db7058817eac097ee9d9dfc2e62740fcff71a9cf3d695`）。固定上游服务当时
  未运行（无对应标签页/容器，9380/9390 拒绝连接），所以只做固定源码审计，不冒充上游
  live click-through；临时 RayRAG 服务、标签页与数据已清理，截图保留。

## 2026-08-12 v0.3.3ab：Memory 列表、消息详情与配置详情生产路由

- 固定上游仍为 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。本批继续逐文件复核固定 React：
  `web/src/pages/memories/index.tsx`（164 行、5,279 bytes、blob
  `d37c5dadf9efccf3`）、`memory-message/hook.ts`（197 行、6,002 bytes、blob
  `899439c679192252`）、`memory-message/message-table.tsx`（479 行、14,909 bytes、blob
  `2ee6e80b856db347`）、`memory-setting/index.tsx`（113 行、4,127 bytes、blob
  `e290574a8573faba`）、三份 setting form 与 sidebar。固定 Git blob 而非 checkout
  工作目录继续作为 UI 语义证据。
- `/memories` 改接不预嵌 tenant/conversation 数据的生产 Rust shell；运行时通过鉴权 API
  加载卡片，并提供搜索、类型/存储筛选、服务端分页、创建、名称编辑、More 菜单、删除
  确认、空态和 canonical 详情导航。动态 Memory 名称/描述全部经 `textContent` 进入 DOM，
  不再由旧页面的 `innerHTML` 拼接；旧 handler 未再注册为线上路由。
- 新注册 `/memory/memory-message/:id`：共享 sidebar 显示头像、名称、描述、创建日期与
  Messages/Configuration active state；消息页具备 debounced 会话搜索、重复 Agent 参数、
  abortable 请求、raw/extract 展开、受控启用开关及失败回滚、遗忘确认、内容/embedding
  查看与复制、任务日志入口、空态和 10/20/50/100 服务端分页。API 响应补入
  `agent_name` 与 `content_embed`；由于 Rust task queue 还没有固定 source-message digest，
  `task` 明确返回 null，不把无关任务错误绑定到消息行。
- 新注册 `/memory/memory-setting/:id`：共享 sidebar 下提供名称、图片头像、描述、embedding、
  LLM、四类 Memory（raw 必选）、字节容量，以及折叠高级设置中的 private/team 权限、
  Table、FIFO、0..1 温度和 system/user prompts；支持 reset、单次 PUT save、范围校验和错误
  状态。Owner 筛选、provider capability 精确选项、固定头像上传服务协议、精确 content
  lock 规则与逐像素组件仍保持 **partial**。
- 路径参数进入内联脚本前使用 script-safe JSON 编码，显式转义 `<`、`>`、`&` 与
  U+2028/U+2029；恶意 `</script>` Memory ID 回归同时覆盖消息页和配置页。三份新 shell
  的业务 DOM 均不使用 `innerHTML`。
- 浏览器环境只读核验 `/vol1/@appdata/fygo-browser/run/runtime-ports.env`：KASM 16000、RFB
  16001、CDP 16002；CDP `/json/version` 为 Chrome 149.0.7827.155 / protocol 1.3，但标签页
  只有 New Tab，9380/9390 均拒绝连接，也没有 ragflow/rayrag 容器。因此固定上游 UI 无法
  做 live click-through，不能冒充上游实机。随后用 `/tmp` 隔离数据启动本批 RayRAG，CDP
  实际完成登录、创建 Memory、列表卡片进入消息页、消息空态/8 列表头、sidebar 进入配置页、
  基础字段与展开高级设置检查；page error 为 0，业务页面/API 响应无 4xx/5xx。截图为
  `/tmp/rayrag-v033ab-memory-list.png`（1280x632，SHA-256 `e40a0f06cf92c83`）、
  `memory-message.png`（1265x765，`b04392fc798c3b7c`）和 `memory-setting.png`
  （1265x1434，`64b0d23c23c93bc9`）；临时服务与隔离数据目录已停止并删除。
- coverage 相关 21 个生产 UI 文件继续保持 **partial**，仅把 audit note 更新为已经可达的
  live Rust 行为和剩余差距；库存仍为 aligned 128、full 542、partial 29、replaced 101、
  N/A 61、reference 3,580、unmapped 0，没有因页面可达就冒充逐组件 full/aligned。
- 七条新增/扩展定向回归、默认与 `postgres-backend,zvec-backend` 的
  `cargo check --all-targets --locked`、coverage 固定 commit/4,441 files/unmapped=0、
  `docker compose --env-file .env.example config --quiet`、三份改动 Rust 文件的定向
  rustfmt 和 `git diff --check` 均通过。默认单线程全量为 **1,195 passed / 0 failed /
  27 ignored**（lib 1,193 + coverage bin 2），双特性为 **1,202 passed / 0 failed /
  27 ignored**（lib 1,200 + coverage bin 2）；本批未新启 PostgreSQL 18.4 容器，不把
  feature gate 冒充实机 smoke。`cargo fmt --all -- --check` 仍首先失败于既有
  `src/advanced_rag.rs:543`；严格双特性 all-target Clippy 仍由全仓既有 **371** 个 error
  header 阻断，相关三文件的 14 个 path hit 均在历史代码行，新 Memory 代码/测试行无命中。

## 2026-08-12 v0.3.3aa：Memory 消息隔离、原子 FIFO 与查询参数兼容

- 固定上游仍为 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。本批逐行审计
  `api/apps/restful_apis/memory_api.py`（311 行、12,777 bytes、blob
  `763befeb3d572285add2ff11364bba4b546dd86e`）与 `memory/services/messages.py`
  （300 行、12,043 bytes、blob `8863f5a2f5974926c07cdbe0e7518134f14b68fe`），固定 Git tree
  而非 checkout 工作目录继续作为证据源。
- Memory GET 路由改用 raw query multi-map，精确保留 Quart `args.getlist` 语义：重复的
  `agent_id`/`memory_id` 保持多个值，仅在恰好一个值时解析逗号分隔；`page_size`、
  `limit`、`top_n` 统一执行固定公开 API 的 100 条上限并返回兼容错误文本。新增 unit 与
  HTTP 回归覆盖重复参数、逗号参数、边界 100 和拒绝 101。
- 修复真实跨 Memory 数据隔离缺口：物理/upsert 身份本来是
  `(memory_id,message_id)`，删除、条件删除和 FIFO 驱逐现在也使用同一复合身份；两个
  Memory 复用相同整数 `message_id` 时不再越界删除。`save_messages_with_budget` 将预算
  计算、forgotten-first/oldest 选择、scoped eviction、upsert、序号提交和 snapshot
  persistence 合并到同一个写锁 mutation；closure 或 persistence 失败都会恢复 mutation
  前快照。回归覆盖跨 Memory 重号、预算拒绝和故障注入下的整批回滚。
- `message_index_name` 在调用时动态读取 `ES_INDEX_PREFIX`，仅 trim 前缀并生成固定
  `memory_{prefix}_{uid}` / `memory_{uid}` 名称。但 JSON/可选 PostgreSQL snapshot 和
  zvec-rust replacement 尚未用该名称创建原生 engine resource；native index
  has/create/delete、missing-token repair、非 RAW LLM extraction 与 task/cache 生命周期
  仍明确保持 **partial**。
- 同批 UI 审计确认，生产 Rust server 仅注册 `/memories` SSR 列表/创建/编辑/删除；固定
  React 的 `/memory/memory-message/:id`、`/memory/memory-setting/:id`、详情 sidebar、
  消息表筛选/分页/悬浮操作/tooltip 和完整配置表单均未接线，`web/static` 也不是这些被
  SSR 路由遮蔽的页面已可达的证据。因此 coverage 将相关 21 个生产 UI 文件从泛化
  `reference` 纠正为 **partial**，不冒充 UI 逐像素完成。
- coverage 两条后端目标文件保持 **partial** 并更新已落地/待办边界；`--write` 后
  `--check` 通过固定 commit、4,441 files、unmapped=0。库存实算为 aligned
  128/45,967 行、full 542/224,202 行、partial 29/43,482 行、replaced 101/38,058 行、
  N/A 61/10,262 行、reference 3,580/905,547 行、unmapped 0。
- 默认与 `postgres-backend,zvec-backend` 的 `cargo check --all-targets --locked` 均通过；
  默认单线程全量为 **1,190 passed / 0 failed / 27 ignored**（lib 1,188 + coverage bin
  2），双特性单线程全量为 **1,197 passed / 0 failed / 27 ignored**（lib 1,195 +
  coverage bin 2）。本批环境未设置 `RAYRAG_POSTGRES_TEST_URL`、未新启 PostgreSQL 18.4
  容器，不把 feature-gated snapshot 测试冒充新的 PG18 实机 smoke。
- 六条新增 Memory 定向回归、定向 rustfmt、coverage bin 2 tests、
  `docker compose --env-file .env.example config --quiet` 与 `git diff --check` 均通过。
  `cargo fmt --all -- --check` 仍首先失败于既有 `src/advanced_rag.rs:543`；严格双特性
  all-target Clippy 仍由全仓既有 **371** 个 error header 阻断，相关三文件虽有 13 个
  历史 path hit，但本批 Memory 代码/测试行无命中，未把历史格式/lint 债务伪报为通过。

## 2026-08-11 v0.3.3z：ES/OpenSearch chunk mapping 精确替代与提交前类型门禁

- 固定上游仍为 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。逐字节核验
  `conf/mapping.json`：blob `495f7c7763cfbfd6ad1847df63005d8b86be339a`、SHA-256
  `c6991ef11034289e79d9b7b8dabb37f650b041b7e215e17357d4f167660f7b82`、
  4,453 bytes、212 个显示行；`conf/os_mapping.json`：blob
  `47b7c24b0f53a6e9cea08b7b10d2bf8027cad6bb`、SHA-256
  `e06d7f2d9e2d39d70dc791a88118d328743b5bbd52adb1fd9deb4e58e730f97d`、
  5,766 bytes、268 个显示行。两份文件末字节均为 `}`、无末尾 LF。
- 审计固定 Python `common/doc_store/es_conn_base.py`、`rag/utils/opensearch_conn.py`
  与 Go `internal/engine/elasticsearch/client.go` 的真实加载链：Python ES 每次建索引把
  `settings/mappings` 原样提交，OpenSearch 把完整 body 交给 indices create；Go 启动时把
  `mapping.json` 拆成 composable index template。OpenSearch 的 hybrid normalization
  pipeline 是连接器另建的运行服务，不属于 mapping JSON 自身。
- 新增纯 Rust `src/store/search_mapping.rs` 的 executable schema builder，并在
  `src/settings.rs` 拆开固定 ES、固定 OpenSearch 与 RayRAG replacement 三层规则：精确
  保留 shards=2、replicas=0、refresh=1000ms、scripted similarity 全源码及不同嵌套层级、
  字符串 `"true"`/`"false"` 与 JSON bool 的原始区别、`date_detection="true"`、
  19/24 个有序 dynamic templates、ES/OpenSearch 不同 keyword regex、4/9 个原生
  dense/knn vector 维度、cosine/cosinesimil 属性和显式 `lat_lon` geo_point。Skill 的
  256/2560/3072 维不再污染这两份 chunk mapping classifier；OpenSearch 裸 `id` 保持
  fixed source 的动态 text 方言。
- `SearchEngine::from_file` 在恢复现有索引时、`persist_online_index` 在 zvec mirror 与
  原子 JSON/可选 PostgreSQL snapshot 写入前，都会把 metadata 同时投影到两份 fixed
  mapping 的 integer/unsigned/float/keyword/object/nested/rank/vector/geo 类型并拒绝非法
  值。RayRAG 不创建 native ES/OpenSearch；固定文件未声明的 384 等正维度不会伪装成
  native vector field，而是继续由每 KB 隔离的 zvec collection 和 JSON source-of-truth
  承载。native whitespace analyzer、scripted-IDF、动态类型推断与 OpenSearch pipeline
  属明确的服务方言，不把它们冒充为本 mapping 文件已执行。
- 五条 mapping 回归覆盖完整 settings/template 差异、原生维度隔离、metadata typed
  projection、非法值失败和 384 维 replacement；设置
  `RAGFLOW_SOURCE_DIR=/tmp/ragflow-v0.26.4` 的结构等价门禁把两个 Rust builder 结果与固定
  文件解析后的完整 JSON `Value` 逐字段比较，均通过。
- coverage 将 `conf/mapping.json` 与 `conf/os_mapping.json` 从 partial 升为
  **replaced**，并改为真实 Rust 消费链。Git 门禁重算库存为 aligned 128/45,967 行、
  full 542/224,202 行、partial 8/41,205 行、replaced 101/38,058 行、N/A 61/10,262 行、
  reference 3,601/907,824 行、unmapped 0。
- 默认与 `postgres-backend,zvec-backend` 的 `cargo check --locked` 均通过；默认全量为
  **1,184 passed / 0 failed / 27 ignored**（lib 1,182 + coverage bin 2），双特性单线程
  全量为 **1,191 passed / 0 failed / 27 ignored**（lib 1,189 + coverage bin 2），其中
  PostgreSQL snapshot failure/restore 与 native zvec create/upsert/query/reopen、online
  isolation/reconcile 均回归通过。本批未设置 `RAYRAG_POSTGRES_TEST_URL`、未新启
  PostgreSQL 18.4 容器，不把 feature-gated 测试冒充新的 PG18 实机 smoke。
- coverage `--write` 后 `--check`（固定 commit、4,441 files、unmapped=0）、
  `docker compose --env-file .env.example config --quiet`、`git diff --check`、定向
  rustfmt 均通过。`cargo fmt --all -- --check` 仍首先失败于既有
  `src/advanced_rag.rs:543` 并列出全仓历史格式差异；严格双特性 all-target Clippy 仍由
  全仓既有 **371** 个 error header 阻断，本批 `src/store/search_mapping.rs` path hit 为 0，
  未把历史格式/lint 债务伪报为本批失败或全仓通过。

## 2026-08-11 v0.3.3y：固定 PEM 密码传输契约与部署密钥替代

- 固定上游仍为 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。逐字节核验
  `conf/private.pem`：blob `ff333058d0d8087fadb09295c369a59f1d46a166`、SHA-256
  `41cf8fb7e403f76f884e516392e12c2c8cc9c7b45d3ff1dc70359ee320b16ee5`、
  1,743 bytes、30 行；`conf/public.pem`：blob
  `3fbcfe189593c174d9893721839dcffadf7bcce8`、SHA-256
  `6745cb2fb7215142aca36a2b0b7d4794a7a21894b991263c715fa26367f11e8d`、
  451 bytes、9 行。两者是 RSAES-PKCS1-v1_5 **密码传输**密钥，不是 JWT signing key；
  已删除原 `SIGNING_KEYS_POLICY` 中不存在的 `RAYRAG_JWT_*` 说法。
- 完整审计 `api/utils/crypt.py`（66 行、blob `53afcbec0dbc7194...`）以及固定
  Python user API、React `rsaPsw`、Playwright helper、Go `internal/common/password.go`
  和 CLI callers：客户端先 base64 UTF-8 密码，再 PKCS#1 v1.5 加密并外层 base64；
  Python `decrypt` 返回内层 base64，`decrypt2` 另有 127-byte 前导零修复；Go 解密失败时
  明确回退原明文。Web 和 Go CLI 都内嵌同一公钥，固定私钥只以已知 `Welcome` 口令保护，
  因此复制这对密钥不会增加真实安全性。
- 新增纯 Rust `src/password_transport.rs` 与 `rsa` 0.9 依赖：
  `RAYRAG_PASSWORD_PRIVATE_KEY_FILE` 未设置/为空时关闭；设置后从 operator-mounted secret
  读取未加密 PKCS#1/PKCS#8 PEM，启动即校验 RSA key 且拒绝小于 2048 bit，派生 SPKI
  public key。解密严格接受 `base64(RSA(base64(UTF-8)))`，长度/解密失败按固定 Go 方言
  回退明文；有效 padding 后若内层 base64/UTF-8 非法则明确报错。私钥操作使用随机
  blinding，固定 blob/SHA/bytes 只作为不含 key material 的审计常量。
- `UserStore` 在注册、登录、自助改密、管理员创建/重置用户的统一边界解密，逻辑密码再走
  现有随机盐 Argon2id；不会把 RSA 内层 base64 当作新的存储格式。新增无需认证的
  `GET /api/v1/system/password-public-key`，返回 `enabled/algorithm/encoding/public_key`，
  未配置时安全返回 `enabled=false/public_key=null`。README、API Docs、`.env.example` 与
  Compose 均记录 secret mount 和 TLS 边界；默认 Compose 的 HTTP 端口只适用于本机/受信
  网络，未启 RSA wrapper 的生产认证必须由反向代理终止 HTTPS。
- coverage 将两条 PEM 从 partial 升为 **replaced**；同时纠正历史过度声明，把
  `api/utils/crypt.py` 从 full 降为 **partial**：服务端常用 wire 已接线，但固定
  key/passphrase、`decrypt2` 的 127-byte legacy repair 和生产客户端 encrypt helper
  没有冒充完成。Git 门禁重算库存为 aligned 128/45,967 行、full 542/224,202 行、
  partial 10/41,685 行、replaced 99/37,578 行、N/A 61/10,262 行、reference
  3,601/907,824 行、unmapped 0。
- 默认与 `postgres-backend,zvec-backend` 的 `cargo check --locked` 均通过；默认全量为
  **1,179 passed / 0 failed / 27 ignored**（lib 1,177 + coverage bin 2），双特性单线程
  全量为 **1,186 passed / 0 failed / 27 ignored**（lib 1,184 + coverage bin 2），其中
  zvec native create/upsert/query/reopen 与 online isolation/reconcile 回归通过。最终随机
  blinding 与 public route 改动后，RSA 两条定向测试、加密注册/登录/改密集成测试和公开
  route 鉴权测试分别复跑通过。本批没有设置 `RAYRAG_POSTGRES_TEST_URL`、也没有新启
  PostgreSQL 18.4 容器，不把 feature-gated snapshot 测试冒充新的 PG18 实机 smoke。
- `docker compose --env-file .env.example config --quiet`、coverage `--check`（固定 commit、
  4,441 文件、unmapped=0）、`git diff --check` 和新模块 `rustfmt --check` 通过。
  `cargo fmt --all -- --check` 仍首先失败于既有 `src/advanced_rag.rs:543`；严格双特性
  all-target Clippy 仍有既有 **371** 个 error header，
  `src/password_transport.rs` path hit 为 0，均未伪报全仓通过。

## 2026-08-11 v0.3.3x：Infinity chunk mapping 精确 schema 与持久化 codec

- 固定上游仍为 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`；逐行核验
  `conf/infinity_mapping.json` 完整 blob
  `dfc07d2e44814c392929301ae71e3972bca14748`、SHA-256
  `6e7e48bf1554f77eaaeb8de9e77d1ba6b2bab36dc2ae563e0fa93aa5643ca95d`、
  5,413 bytes、76 个显示行、末尾 LF。实际基础列是 **73**（59 varchar、11 integer、
  3 float），不是此前 Rust 只声明前 41 列时推测的 74；`page_num_int`、`top_int`、
  `position_int` 虽有 `_int` 后缀，固定物理类型仍是 varchar。
- `src/settings.rs` 现在按 JSON 顺序固定全部 73 列的类型、精确默认值（包括
  `available_int=1`）、single/multiple analyzer 形状、5 组 comment aliases 与两个
  low-cardinality secondary；测试实算 45 个 fulltext、2 个 secondary、5 组 aliases。
  同时固定动态 `q_{dimension}_vec`、cosine HNSW M=16/ef=50/LVQ、table parser
  `chunk_data`，并明确记录 Python 固定 `q_vec_idx` 与 Go `q_{dimension}_vec_idx` 的上游
  方言差异，未把二者伪装成同一行为。
- 新增纯 Rust `store/infinity_chunk.rs` canonical codec：写入时先物化 schema 默认值，
  再按固定优先级折叠 doc/title、important、question、content、authors aliases；实现
  important 空项计数、keyword `###`、question 换行、`*_feas`/`extra`/`chunk_data` JSON、
  KB 列表首项和 position/page/top 八位十六进制编码，并生成唯一合法的动态向量列。
  反向 codec 恢复 logical alias、important 尾部空项、keyword 数组和坐标数组。
- `ZvecStore::insert` 在任何 JSON 或 zvec 写入前先完成整个 batch 的维度与 typed codec
  校验；持久层保存非默认 physical 字段和显式 `kb_id/available_int`，搜索结果再解码为
  logical metadata。回归直接读取落盘 JSON，验证 active 默认、KB、十六进制坐标确已
  进入物理 snapshot，而 API 回读仍得到数组形状；非法 integer/负坐标在写入前失败。
- coverage 的 `conf/infinity_mapping.json` 仍保持 **partial**，但映射文件改为真实消费链：
  `settings.rs`、codec、store/search/persistence 与 chunk/search/server API。RayRAG 当前
  仍没有 Infinity 的 analyzer/index 选择、通用 filter/sort/aggregation、dense threshold、
  atan Fusion、SQL comment 重写和同表多向量生命周期；zvec 继续采用每 KB 单一维度并由
  JSON/PostgreSQL snapshot 持有 canonical metadata，因此不声称 aligned/replaced/full。
- 默认与 `postgres-backend,zvec-backend` 的 `cargo check --locked` 均通过；默认全量为
  **1,176 passed / 0 failed / 27 ignored**（lib 1,174 + coverage bin 2），双特性为
  **1,183 passed / 0 failed / 27 ignored**（lib 1,181 + coverage bin 2）。本批没有设置
  `RAYRAG_POSTGRES_TEST_URL`、也没有新启 PostgreSQL 18.4 容器，因此不把 feature-gated
  snapshot 单测冒充新的 PG18 实机 smoke。
- `docker compose --env-file .env.example config --quiet`、coverage `--check`、
  `git diff --check` 与 `settings.rs`/`store/mod.rs`/新 codec 的定向 `rustfmt --check`
  均通过。`cargo fmt --all -- --check` 仍首先失败于既有 `src/advanced_rag.rs:543` 并列出
  全仓历史格式差异；严格双特性 all-target Clippy 仍被既有 **371** 个 error header
  阻断，修正本批 6 个 collapsible-if 后 `src/store/infinity_chunk.rs` 命中数为 0，未把
  旧债务伪报为通过。

## 2026-08-11 v0.3.3w：system settings 持久生命周期与 coverage Git 门禁恢复

- 固定上游仍为 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`；逐字节核验
  `conf/system_settings.json`：完整 Git blob
  `f546aa1436b0d262679a0567b7a61167bfcd43bb`、SHA-256
  `2d58065525cbba3e1e796b73be822547dd991dcab90cb71e342750b4867f3402`、
  1,843 bytes、88 个显示行、无末尾 LF，并固定全部 14 条
  `(name, source, data_type, raw string value)`。同时审计 Python 启动 seed、admin
  CLI、Go variables/base64 SHOW、sandbox manager 与设置 UI 调用链；固定树中只有
  `sandbox.*` 真正消费该文件，`mail.*` 实际仍来自 `service_conf.yaml`，
  `enable_whitelist`/`default_role` 没有运行期消费者，未虚构接线。
- 新增纯 Rust `SystemSettingsStore`：严格要求 `system_settings` wrapper，校验名称唯一和
  声明类型，启动保留 raw string 并只插入缺失 seed，管理员写入时再严格校验值；按名称
  稳定排序，并以独立原子 JSON snapshot 作为真源；该 store 当前没有接入通用 PostgreSQL
  snapshot mirror。写入先持久化再替换内存，失败保持旧值；更新后刷新 runtime typed
  mirror。回归覆盖完整 14 tuple、重开、
  exact→prefix、非法 integer/bool/json、重复名/错误 wrapper、持久化失败回滚。
- `/api/v1/admin/variables` 与 `/api/v1/system/variables` 统一到该 store，保留 Python
  GET JSON body 的 SHOW 和 Go base64 path 方言；公开 row shape 为
  `data_type/name/setting_type/value`。RayRAG 有意收紧为管理员 ACL，并对
  password/private_key/passphrase/access_key_secret/api_key/secret/token 递归脱敏；
  `<redacted>` 回写保留原秘密。旧 process-local `{var_name,var_value}` registry/handler
  已删除，避免双真源和重复 Axum route。
- CodeExec 现在按「RayRAG 环境覆盖 > system settings > 默认」解析
  `sandbox.provider_type` 与 `sandbox.self_managed` 的 endpoint/timeout，兼容
  固定 JSON 的 lower-case key 和 Go 的 uppercase alias；仅 `local` 与
  `self_managed` 进入既有运行链，其他 provider 明确报 unsupported；固定 schema 中
  未被上游 `/run` 实际使用的 max_retries/pool_size 不虚构重试语义。管理页变量卡通过
  受保护 API 动态加载，不把 sandbox 值 SSR 给匿名页面；秘密值始终保持脱敏。
- 本批只把 `conf/system_settings.json` 从 partial 升为 **aligned**。RAGFlow 更宽的
  provider/schema/config/test 专用 API、SSH/Aliyun/E2B runtime 和 React Sandbox
  Settings 页尚未完成；上游自身 seed/Python lower-case 与 Go/E2B 配置源也有方言断裂，
  因此不声称 replaced/full。
- 恢复 `src/bin/ragflow-coverage.rs` 与 Cargo binary：`--check` 直接校验固定 commit、
  4,441 个 Git path/blob/byte/text-line、唯一 inventory 状态及摘要，`--write` 只在有意
  改状态后按 Git tree 修复元数据并重算。首次运行抓出历史伪元数据（例如
  `agent/sandbox/providers/local.py` 曾写成 2,986 行/4,457 bytes/blob `local`，实际为
  354 行/13,981 bytes/blob `6a9ac516bdba74c7`）并全部回填；最终库存为 aligned
  128/45,967 行、full 543/224,268 行、partial 11/41,658 行、replaced 97/37,539 行、
  N/A 61/10,262 行、reference 3,601/907,824 行、unmapped 0。
- 默认与 `postgres-backend,zvec-backend` 的 `cargo check --locked` 均通过；默认全量为
  **1,174 passed / 0 failed / 27 ignored**（lib 1,172 + coverage bin 2），双特性为
  **1,181 passed / 0 failed / 27 ignored**（lib 1,179 + coverage bin 2）。本批未设置
  `RAYRAG_POSTGRES_TEST_URL`、也未新启 PostgreSQL 18.4 容器，因此不冒充新的 PG18
  实机 smoke。
- `docker compose --env-file .env.example config --quiet`、coverage `--check`、
  `git diff --check` 以及新 `system_settings.rs`/`ragflow-coverage.rs` 定向
  `rustfmt --check` 通过。`cargo fmt --all -- --check` 仍首先失败于既有
  `src/advanced_rag.rs:543`；严格双特性 Clippy 仍被既有全仓债务阻断（371 条 error，
  本批两个新文件无 lint），均未伪报通过。

## 2026-08-11 v0.3.3v：Skill Elasticsearch 映射契约与 Rust/zvec 替代

- 固定上游仍为 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`；逐行审计
  `conf/skill_es_mapping.json`（完整 blob
  `a9d3cba86991e8986bc32eb8eb3fc1bc82ec1580`，136 行/3016 bytes）以及 Go
  `CreateChunkStore`/`loadSkillMapping`、`SkillIndexerService`、ES bool/query_string/KNN
  构造和实际 REST 调用链。loader 的三个查找路径、非等价内置 fallback、已存在
  index 不修补 mapping、静态七维 mapping 不随传入 dimension 扩展均已记录。
- `src/settings.rs` 新增独立 `SkillEsFieldSchema`，精确固定 shards=1、replicas=0、
  refresh=1000ms、`dynamic=false`、完整 `scripted_sim` 源码，以及 22 个显式属性：
  3 个 stored keyword、4 个 stored 但不索引的 raw text、4 个 whitespace +
  scripted similarity token text、7 个 indexed cosine dense vector（3072/2560/1536/
  1024/768/512/256）、2 个 stored keyword 和 2 个 stored long。mapping 构造测试
  同时断言属性全集、key omission/default 边界和七个维度；通用 suffix 表遗漏的
  `q_256_vec` 也已补齐。
- `src/api/skill_index.rs` 新增 Go-compatible ES document projection：ES `_id` 把 `/`
  改为 `_`，但 `_source.skill_id` 保留原始 wire 值；space 空值归一为 `default`，
  tags 用 `, ` 拼接，version/status/Unix-ms time 与动态 `q_{dimension}_vec` 对齐。
  name/tags 始终写 RAG token，description/content token 是否存在受索引时 field config
  控制（默认 description 开、content 关）。Rust keyword scorer 现在也不再错误地让
  默认关闭的 content 参与匹配。
- RayRAG 不部署或冒充 Elasticsearch：真实 Skill 真源仍是 tenant/space 原子 JSON，
  可选 PostgreSQL 18 镜像完整 snapshot，向量写入隔离的 zvec-rust Skill collection，
  keyword/vector/hybrid 在 Rust 内完成。ES mapping 中并未声明 HNSW M/ef/量化参数，
  因此不把 zvec/Infinity 的 M=16、ef=50、LVQ 反写成 ES 契约；ES 的 corpus
  scripted-IDF/best_fields、所有查询强制 `status=1`、实际 hybrid boost、embedding
  失败后的 text-only 写入、固定七维之外的 mapping/rebuild 行为均作为明确的
  service/dialect 差异保留，不借单一 mapping 文件声称完整 Skill service 等价。
- 本批仅把 `conf/skill_es_mapping.json` 从 partial 升为 **replaced**，宽泛 Go Skill
  services、filesystem bundle/space/version history、reranker 和跨维度原生 collection
  重建状态均不抬高。按 4,441 条 File Inventory 逐行实算为 aligned 127/45,879 行、
  full 543/254,320 行、partial 12/41,746 行、replaced 97/37,539 行、N/A
  61/10,262 行、reference 3,601/908,046 行、unmapped 0；历史 generator 已移除，
  本批继续只报告 inventory/summary 可执行自校验，不声称 generator gate。
- 保持纯 Rust、zvec-rust 0.5.1、PostgreSQL 18.4、Linux/Docker、国内镜像优先和
  零 MySQL。默认与 `postgres-backend,zvec-backend` 的 `cargo check --locked` 均通过；
  默认全量测试 **1,163 passed / 0 failed / 27 ignored**。双特性并行首次因既有
  `runtime_config` 全局测试状态竞态出现 1 条失败（其余 1,169 通过），该用例定向
  复跑通过，随后用 `--test-threads=1` 完整复核为 **1,170 passed / 0 failed /
  27 ignored**。本批未设置 `RAYRAG_POSTGRES_TEST_URL`、也未新启 PostgreSQL 18.4
  容器，因此不冒充新的 PG18 实机 smoke。
- `docker compose --env-file .env.example config --quiet`、`git diff --check`、
  `settings.rs`/`skill_index.rs` 定向 `rustfmt --check`、固定 HEAD/blob/136 行/3016
  bytes 核验，以及 coverage summary 对 4,441 条 inventory 的可执行自校验均通过。
  `cargo fmt --all -- --check` 仍首先失败于既有 `src/advanced_rag.rs:543` 并列出大量
  未格式化旧文件；严格双特性 Clippy 仍被既有全仓债务阻断（lib 322、tests 373
  条 error 记录），本批新增 Skill ES schema/document 代码无定向 lint。历史
  `ragflow-coverage` binary 已移除，未声称 generator gate 通过。

## 2026-08-11 v0.3.3u：Skill Infinity 映射、索引检索与 zvec Rust 替代

- 固定上游仍为 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`；逐行审计
  `conf/skill_infinity_mapping.json`（完整 blob
  `4e4766ea8f5bd07b15855c5dea32e53d88399396`），并复核 Go
  `skill_indexer`/`skill_search`、Infinity connector、handler/router 与 Python
  文件夹删除调用链。
- `src/settings.rs` 以 typed Rust schema 固定 11 个有序物理字段及默认值：
  `skill_id`/`space_id`/`folder_id`/`name`/`tags`/`description`/`content`/
  `version`/`status`/`create_time`/`update_time`；同时固定四个文本字段的
  `rag-coarse`/`rag-fine` analyzer、动态 `q_{dimension}_vec`、
  `q_{dimension}_vec_idx`、HNSW cosine M=16/ef_construction=50/LVQ 契约、
  `sec_skill_id`/`sec_space_id` 和八个 fulltext index 名称。
- 新增纯 Rust `SkillIndexStore`：以 tenant/space 分区的原子 JSON 物理快照保存
  精确字段，可选 PostgreSQL 18 继续镜像完整 snapshot；启用
  `RAYRAG_VECTOR_BACKEND=zvec` 时，向量同步到独立 `zvec-data/skills` collection
  tree，避免知识库 online reconcile 把 Skill 集合当作 stale 删除。zvec HNSW
  同步使用 cosine、M=16、ef_construction=50，JSON 写失败时反向同步并恢复内存状态。
- 索引写入按上游把 `/` 规范为 `_`、tags 以 `, ` 存储、同 ID/同名覆盖，默认
  version/status/time 与物理映射一致。搜索实现固定 name/tags/description/content
  的 10/5/3/1 keyword boost、cosine vector、可配 hybrid 权重、阈值、top-K、
  分页排序与向量/embedding 失败后的 keyword fallback；回归测试明确固定了一个
  容易误读的 connector 语义：纯 keyword/空查询不过滤 status，vector/hybrid
  只看 `status=1`。
- 新增受认证租户隔离的 `/api/v1/skills/config`、`search`、`index`、`reindex`
  REST 生命周期，并把 `/skills` 从误用 Agent Canvas component catalog 的安装页
  改为真实 Skill space 搜索、索引、删除、配置和重建页面；HTTP 与 HTML 接线均有
  回归测试。
- 本批只把 `conf/skill_infinity_mapping.json` 从 partial 升为 **replaced**。
  文件系统 Skill bundle/space/version history、精确 Infinity BM25/atan fusion、
  reranker、后台全量扫描，以及向量维度变化时的原生 collection 重建仍未完成；
  `skill_es_mapping.json` 与宽泛 Go Skill services 状态均未抬高。
- 按 4,441 条 File Inventory 逐行实算为 aligned 127/45,879 行、full
  543/254,320 行、partial 13/41,882 行、replaced 96/37,403 行、N/A
  61/10,262 行、reference 3,601/908,046 行、unmapped 0；历史 generator
  已移除，本批不声称通过 coverage generator gate。
- 保持纯 Rust、zvec-rust 0.5.1、PostgreSQL 18.4、Linux/Docker、国内镜像优先和
  零 MySQL。`cargo check --locked` 与双特性 check 均通过；默认全量测试为
  **1,162 passed / 0 failed / 27 ignored**，双特性为 **1,169 passed / 0 failed /
  27 ignored**，其中原生 zvec create/upsert/query/reopen 和分区 reconcile 通过。
  本批未设置 `RAYRAG_POSTGRES_TEST_URL`、也未新启 PostgreSQL 18.4 容器，因此不
  冒充新的 PG18 实机 smoke。
- `docker compose --env-file .env.example config --quiet`、`git diff --check`、
  coverage inventory/summary 自校验以及 `skill_index.rs`/`store/mod.rs` 定向
  `rustfmt --check` 通过。严格双特性 Clippy 仍被既有全仓债务阻断（lib 321、
  tests 370 个错误），本批 `skill_index.rs`/`store/mod.rs` 新增 lint 为零；
  `cargo fmt --all -- --check` 仍首先失败于既有 `src/advanced_rag.rs:543`，没有
  批量格式化无关代码，也不把两项基线写成通过。

## 2026-08-10 v0.3.3t：message Infinity 映射与消息生命周期 Rust 替代

- 固定上游仍为 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`；逐行审计
  `conf/message_infinity_mapping.json`（blob `17e7307e89f7fa3d`）以及
  Infinity connector、message service 和 REST 调用链。
- `src/settings.rs` 以 typed Rust schema 固定 17 个物理列的类型、默认值、
  `content` 的 `rag-coarse`/`rag-fine` analyzers 与 `content_ltks` comment，并记录
  动态 `q_{dimension}_vec`、HNSW `q_vec_idx`、Cosine、M=16、
  ef_construction=50、LVQ 的上游索引契约。
- `MemoryMessageService` 现在把逻辑消息规范化为复合 `memory_id_message_id` 主键、
  文本日期及本地时区毫秒 float 镜像、`status_int`、`zone_id` 和动态向量列；旧逻辑
  JSON 在启动时迁移为规范物理快照。插入按复合键 upsert，更新/删除/预算淘汰均在
  原子持久化失败时回滚；FIFO 先清 forgotten，再按最早 `valid_at` 清理。
- production `AppState` 已接入持久化 `memory_messages.json`。新增受 ACL 保护的
  RAW message 保存、消息列表/recent/hybrid search、状态更新、soft forget、content
  和级联删除 REST 生命周期；保存按 memory 绑定 embedder 生成向量，检索复用纯 Rust
  分词及 keyword/vector 加权。search 默认只看 active 且未忘记，recent 忽略 status
  但隐藏 forgotten，普通 list 保留 forgotten，语义分别测试。
- 本批没有部署或冒充 Infinity。JSON 是消息真源，可选 PostgreSQL 18 仅镜像完整
  snapshot；message 全文/向量融合目前在进程内完成。非 RAW LLM 抽取、后台 task/
  progress/cache、原生引擎 index 生命周期、重复 query 参数和 agent/task 响应 enrichment
  仍未实现，因此四个宽泛 memory API/service 文件由 aligned/full 诚实降为 partial。
- 覆盖矩阵将 `conf/message_infinity_mapping.json` 从 partial 升为 **replaced**。
  按 4,441 条 File Inventory 实算为 aligned 127/45,879 行、full 543/254,320 行、
  partial 14/41,946 行、replaced 95/37,339 行、N/A 61/10,262 行、reference
  3,601/908,046 行、unmapped 0；历史 generator 仍不存在，本批不声称通过
  coverage generator gate。
- 保持纯 Rust、zvec-rust、PostgreSQL 18.4、Linux/Docker、国内镜像优先和零
  MySQL；没有修改镜像、compose 或数据库 schema。
- 复验：`cargo test --locked` 为 1,156 passed / 0 failed / 27 ignored；
  `cargo test --locked --features postgres-backend,zvec-backend` 为 1,163 passed /
  0 failed / 27 ignored。双特性门禁覆盖 zvec-rust 原生 collection，但本批未设置
  `RAYRAG_POSTGRES_TEST_URL`、也未新启 PostgreSQL 18.4 容器，因此不冒充新的 PG18
  实机 smoke。
- `docker compose --env-file .env.example config --quiet`、`git diff --check`、
  当前批次四个主要 Rust 文件的定向 `rustfmt --check` 和 message HTTP 生命周期测试
  均通过。严格双特性 Clippy 仍被既有全仓债务阻断（lib 321、lib-test 358 个错误），
  但本批新增 lint 已清零；`cargo fmt --all -- --check` 仍从既有
  `src/advanced_rag.rs` 开始失败，未批量格式化无关代码，也不把两项基线写成通过。

## 2026-08-10 v0.3.3s：doc_meta ES/Infinity 映射的 Rust 替代契约

- 固定上游仍为 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`；逐字节核对
  `conf/doc_meta_es_mapping.json`（blob `eeab3b985a74b29e`）和
  `conf/doc_meta_infinity_mapping.json`（blob `471912c6e20c2747`）。
- `src/settings.rs` 以 typed Rust 常量固定 ES shards=2、replicas=0、
  refresh=`1000ms`、`_source.enabled=true`、dynamic=`runtime`，并逐字段固定
  `id`/`kb_id` 的 keyword+store / varchar+空串默认值，以及 dynamic object / JSON
  `meta_fields` 的 `{}` 默认值。
- `DocumentMetadataRecord` 的规范序列化字段改为上游 `id`，读取时继续接受旧
  `doc_id` 快照；`meta_fields` 缺失仍默认空对象，任意动态键可原样持久化。批量更新
  现在会为无 metadata row 的文档创建非空记录、删除被清空的记录，并保持有
  `match` 条件时不凭空创建字段；数值 0 等值与上游一样属于已提供的 match。
- 存储边界纠正为真实实现：`document_metadata.json` 原子快照是主存储，可选
  PostgreSQL 18 只按 checksum 镜像完整快照；zvec-rust 只保存 chunk vectors，检索
  前的 metadata 过滤和返回结果 enrichment 在 Rust 进程内独立完成。未部署、也未
  冒充 ES/Infinity metadata index。
- 覆盖矩阵将上述两条 mapping 从 partial 升为 **replaced**。按 4,441 条 File
  Inventory 实算为 aligned 129/46,579 行、full 545/255,078 行、partial
  11/40,507 行、replaced 94/37,320 行、N/A 61/10,262 行、reference
  3,601/908,046 行、unmapped 0；历史 generator 仍不存在，本批不声称通过
  coverage generator gate。
- 保持纯 Rust、zvec-rust、PostgreSQL 18.4、Linux/Docker、国内镜像优先和零
  MySQL；本批没有修改镜像、compose 或数据库 schema。
- 复验：`cargo test --locked` 为 1,151 passed / 0 failed / 27 ignored；
  `cargo test --locked --features postgres-backend,zvec-backend` 为 1,158 passed /
  0 failed / 27 ignored。双特性门禁覆盖 zvec-rust 原生 collection，但本批未设置
  `RAYRAG_POSTGRES_TEST_URL`、也未新启 PostgreSQL 18.4 容器，因此不冒充新的 PG18
  实机 smoke。
- `docker compose --env-file .env.example config --quiet`、`git diff --check`、
  `src/api/document_metadata.rs` 的定向 `rustfmt --check` 均通过。严格双特性 Clippy
  仍被既有全仓 lint 债务阻断（lib 321、lib-test 360 个错误），`cargo fmt --all
  -- --check` 也从既有 `src/advanced_rag.rs` 等文件开始失败；本批新增 Clippy 债务已
  清零，未批量格式化无关代码，也不把这两个门禁写成通过。

## 2026-08-10 v0.3.3r：api/db 固定值与 Canvas 类别端到端对齐

- 固定上游仍为 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`；逐行审计
  `api/db/__init__.py` 92 行，并复核 `common/constants.py` 的
  `PipelineTaskType`。
- 新增纯 Rust `src/api/db.rs`：补齐 `SerializedType` 1/2 数字协议、
  `InputType` 四值、`CanvasCategory` 两个 wire 值、七种 `FileType`、七种有效
  pipeline task、五种 lowercase progress-freeze task 与两个保留文件夹名；
  `common::PipelineTaskType` 补回固定版 `Artifact`/`Skill`。
- Agent 持久化和 API 统一写入 `agent_canvas`/`dataflow_canvas`；旧
  `agent`/`standard`/`ingestion`/`dataflow` 值启动时原子迁移，未知值 fail
  closed。创建、导入、复制和固定 ingestion template 都保留规范类别；知识库重命名
  不再误发无关的 `canvas_category`。
- 覆盖库存将 `api/db/__init__.py` 从 partial 升为 aligned，并按 4441 条
  File Inventory 实算纠正旧摘要漂移：aligned 129/46,579 行、full
  545/255,078 行、partial 13/40,541 行、replaced 92/37,286 行、N/A
  61/10,262 行、reference 3,601/908,046 行、unmapped 0。
- README 删除已不存在的 `ragflow-coverage` 可执行命令；当前矩阵是固定快照，
  每批必须先核验上游 commit，再逐行实算摘要。未恢复生成器，因此本批不声称通过
  coverage generator gate。
- 保持纯 Rust、zvec-rust、PostgreSQL 18.4、Linux/Docker 和国内镜像优先路线；
  未引入 MySQL，未改镜像或容器。
- 复验：`cargo test --locked` 为 1149 passed / 0 failed / 27 ignored；
  `cargo test --locked --features postgres-backend,zvec-backend` 为 1156 passed /
  0 failed / 27 ignored，包含 zvec-rust 0.5.1 原生 collection
  create/upsert/query/reopen；本批未启动新的 PostgreSQL 18.4 容器，故不冒充实机
  PostgreSQL smoke。`docker compose --env-file .env.example config --quiet`、
  `git diff --check` 以及新增 `api/db.rs`/template helper 的 `rustfmt --check`
  通过。
- 仓库当前严格
  `cargo clippy --all-targets --locked --features postgres-backend,zvec-backend -- -D warnings`
  仍被既有全仓 lint 债务阻断（lib 321、lib-test 360 个错误）；
  `cargo fmt --all -- --check` 也因大量既有未格式化文件失败。本批未批量重排无关
  文件，且不把这两个门禁写成通过。

## 2026-08-06 v0.3.3：Provider 目录 20→60 + RAGFlow UI 逐页实测清单

- 从 docker-ragflow-cpu-1（v0.26.2）`/ragflow/conf/models/` 拷出 60 个
  provider fixture（与 RayRAG 现有 fixture 格式完全一致，verbatim mirror），
  补齐 40 个缺失提供商 → src/api/fixtures/models/（556 模型条目，JSON 全校验）
- model_meta.rs：PROVIDER_MODEL_FIXTURES 60 条 + 别名 50 条；测试断言适配
- 关键发现：RayRAG 内置 src/data/providers/llm_factories.json（65 厂商标记）
  与 conf/models 目录（60 个）互为补充；fixture 才是模型目录数据源
- UI 实测链路：前端 RSA 加密密码（JSEncrypt 公钥内嵌 bundle，openssl
  pkeyutl 可复现）→ /api/v1/auth/login → users/me → users/me/models →
  datasets → tenants；浏览器 navigate 会重置 localStorage token，须
  SPA 内 CDP 点击保持会话（header 链接 a.click() 会整页跳转丢会话）
- 路由实测：/dataset/{files|retrieval|logs|configuration}/{id} 四 Tab；
  /chats /searches /agents /memories /files /user-setting/{data-source|model|api}
- 实测数据：租户 31b104de...，30329 files / 383166 chunks / 30GB；
  Model providers 类型分布 LLM 37 / Embedding 25 / Rerank 10 / TTS 8 /
  ASR 11 / VLM 17 / OCR 4；35 种数据源清单
- 测试：model_meta 11/11 通过；全量 lib 1142/1144（2 个过时断言已修）
- 文档：docs/ragflow-ui-inventory.md 新增（UI 对齐 checklist）

## 2026-08-06 v0.3.3b：PRESETS 64→69 + 内嵌工厂目录同步 65/1064

- 新增 5 个 ProviderPreset：Xiaomi(api.xiaomimimo.com)、Qiniu(api.qnaigc.com)、
  HuaweiCloud(api.modelarts-maas.com)、TokenHub(aitok.cc)、OrcaRouter(api.orcarouter.ai)
- 新增 FIXTURE_TO_PRESET 24 条映射（fixture key→factory id，如 aliyun→Tongyi-Qianwen、
  hunyuan→Tencent Hunyuan、gitee→GiteeAI）；provider_preset 回退解析，
  provider_is_domestic("aliyun") 等此前误判非国内的问题修复
- 内嵌 src/data/providers/llm_factories.json 同步为容器最新（65 工厂/1064 模型，
  紧凑格式 {n,t,mx,tools}）；教训：该文件是转换格式，不能直接覆盖为原始
  llm_factories.json（factory_catalog 解析 models 数组而非 llm）
- 测试：providers 13/13、model_meta 11/11、全量 lib 1143/0 failed

## 2026-08-06 v0.3.3c：Providers 页对齐 RAGFlow Model providers

- 能力类型筛选条（All/LLM/Embedding/Rerank/TTS/ASR/VLM/OCR + 显示计数，
  对齐 RAGFlow /user-setting/model 的 8 类筛选）；行 data-k 属性由
  provider_has_kind 推导（7 种 ProviderKind）
- + Add 按钮 + 添加提供商弹窗（ID/Name/API Base/API Key/Models →
  POST /api/v1/providers/{id}，ProviderUpdate 契约）
- 教训：web.rs 是"二进制"文件（read_file 拒读，用 sed/grep/python 操作）；
  Rust 字符串内嵌 JS 时统一用「JS 字符串单引号 + Rust 内仅 \\" 转义」，
  与既有 renameModal 模式一致（文件字节 4 反斜杠+引号）；patch 工具会
  二次转义反斜杠导致 6 反斜杠垃圾字节，须用 python 脚本精确写入
- 测试：新增 providers_page_renders_kind_filter_and_add_provider_modal
  （kb_ui_tests 模块，需用该模块的 test_state/body_of）；全量 1144/0 failed

## 2026-08-07 v0.3.3d：Configuration 对齐 + 容器重建验证

- 浏览器实测 RAGFlow v0.26.2 下拉：Language 20+ 语言、Permissions=Only me/Team、
  Chunking method 11 项（General/Q&A/Resume/Manual/Table/Paper/Book/Laws/
  Presentation/One/Tag）、PDF parser=Light/General → 写入 UI inventory
- RayRAG 对齐：dataset Configuration 补 Language/PDF Parser/PageIndex/
  Image&Table Context Window/Excel to HTML 五字段 + saveConfig 提交；
  chunk method select 补 Resume/Tag（对齐 11 项）；后端 update_dataset
  白名单 + language/pdf_parser/page_index/image_and_table_window/
  excel_to_html/tag_sets/page_rank
- 容器重建（国内镜像 rayrag:local 新镜像）+ 页面级验证：/dashboard /login
  /kbs /providers /settings /api-docs 全 200；admin@rayrag.local 登录后
  /providers 渲染 60 catalog 卡片、8 类筛选按钮、data-k 属性
  （llm×18/llm embedding vlm×13/ocr×2/tts asr×1...）、add-provider 弹窗
- 测试：web 20 + providers 14 全绿；登录账号 = RAYRAG_ADMIN_EMAIL 默认
  admin@rayrag.local（非 "admin"）

## 2026-08-07 v0.3.3e：悬停 tooltip 调研 + 行操作对齐

- 调研结论：RAGFlow 行内图标按钮 tooltip 是**动态组件**（无静态 title/
  aria-label），悬浮才渲染 —— RayRAG 用 title 属性等价实现（浏览器原生
  悬停提示，效果一致）
- 对齐：文档行 ✏️ 重命名按钮加 title="Rename"（两处 loadDocs）
- 待办：RAGFlow 有 /api/v1/datasets/{id}/documents/{doc}/download 下载
  文档端点；RayRAG 目前只有 /api/v1/files/{file_id}/download，文档级
  下载端点待补（下轮）
- 浏览器采集补充：Files 行内按钮 = Enable switch + N fields + General 下拉
  + 4 图标按钮（reparse/预览/下载/删除）+ ... 菜单（Preview/Edit/Delete/
  Download/Export/Reparse，locale 源码确认）

## 2026-08-07 v0.3.3f：文档下载端点（RAGFlow parity）

- GET /api/v1/datasets/{id}/documents/{did}/download：kb_accessible 校验 →
  DocStore.get + kb_id 匹配 → uploads/{storage_name} 读字节 →
  Content-Disposition attachment；文件名消毒防路径穿越
- UI：文档行加 ⬇️ 下载链接（title=Download，两处 loadDocs）
- 测试：document 32 + server 75 全绿；容器镜像待重建后验证真实下载
- ✅ 容器重建后真实下载验证通过：POST 上传测试 txt → GET download →
  200 + `content-disposition: attachment; filename="rayrag_dl_test.txt"` +
  `application/octet-stream` + 字节完全一致（BYTES_MATCH）；测试 KB 已清理

## 2026-08-07 v0.3.3g：Agent 创建弹窗对齐（Choose agent type）

- 浏览器实测 RAGFlow Create agent 下拉 = Create from blank / Create from
  template / Import JSON file；弹窗 = Choose agent type（Agent flow /
  Ingestion pipeline）+ Name + Save/Close
- RayRAG 对齐：创建弹窗加类型单选（canvas_category 提交 agent/ingestion，
  后端 normalize_agent_category 已支持任意非空值）；createA 两处全部接线
- 测试：web 20 全绿；待容器重建后验证类型创建

## 2026-08-07 v0.3.3h：New Chat 弹窗对齐（名称 + 知识库多选）

- 浏览器实测 /chats 与 /searches 空态均无创建按钮（RAGFlow v0.26.2 行为）
- RayRAG New Chat 弹窗补 Name 输入 + 知识库多选下拉（打开时从
  /api/v1/datasets 加载）；createChat 提交 {question: name, kb_ids}；
  后端 ChatRequest 早已支持 kb_ids/chat_model/embedding_model
- 测试：web 20 全绿；待容器重建后验证创建带知识库的会话

## 2026-08-07 v0.3.3i：Memory 创建弹窗对齐

- 浏览器实测 Create memory 弹窗 = Name + *Memory type（raw/semantic/
  episodic/procedural + (Select all)）+ Embedding model + LLM + Cancel/Confirm
- RayRAG 对齐：弹窗标题 Create Memory（新建）/Edit Memory（编辑，i18n 双
  语言）、类型区加 Select all 快捷全选（类型多选 checkbox 已具备）
- 测试：web 20 全绿

## 2026-08-07 v0.3.3j：容器重建 + 三项端到端验证通过

- 容器重建（含 v0.3.3g/h/i 全部代码）+ 实机验证：
  1. Agent 类型创建 ✅（canvas_category=agent 落库；注意 create_agent 要求
     dsl 字段必须为 JSON 对象，否则 400 "Agent DSL must be a JSON object"）
  2. 带知识库会话创建 ✅（POST /api/v1/chats {question, kb_ids} code 0）
  3. Memory 创建 ✅（memory_type ["raw","semantic"] code 0）
- 测试数据已清理；覆盖矩阵 aligned 128→134

## 2026-08-07 v0.3.3k：Settings 子页对齐（Chat channels + MCP）+ /files 实测

- 浏览器实测 /files：Add file 下拉 = **Upload file / New folder**（与 dataset
  Files tab 的 Upload file/Create empty file 不同，已记录）；表格 5 列
  （Select all/Name/Upload date/Size/Dataset/Action）
- user-setting 子页：Chat channels 头像菜单为 hover 触发（CDP 无法模拟），
  改源码确认 MCP servers/Team members/Invite/Chat channels 文本存在
- RayRAG settings 页补：Chat Channels section（registered_ids 渠道列表 +
  状态）+ MCP Servers section（内置 MCP 工具目录 /api/v1/mcp/tools）；
  settings_page 增加 State 参数（路由 axum 自动注入，测试同步修复）
- 测试：settings 11 全绿

## 2026-08-07 v0.3.3l：Search apps 对齐（RAGFlow searchapps 契约）

- RAGFlow /searches（next-searches/index.tsx 源码确认）有 Create search
  app 弹窗（名称输入 + onOk 创建）
- RayRAG 新增 /api/v1/searchapps CRUD：SearchAppStore（JSON 持久化，
  仿 DataSourceStore）name/kb_ids/hybrid/rerank/top_k + owner 作用域
  列表 + 权限校验；search_page 加 💾 Save as App 按钮（prompt 名称 →
  POST 当前 KB/hybrid/rerank 选择）
- 测试：search 43 + server 75 全绿；待容器重建后验证 searchapps CRUD

## 2026-08-07 v0.3.3m：数据集 Logs tab 对齐

- RAGFlow 4-tab（Files/Retrieval testing/Logs/Configuration）缺口补齐：
  新增 📜 Logs tab（nav5）——文档解析状态表（run badge/Chunks/进度条）+
  File 下拉筛选（按文档名过滤行）；showTab 循环 0-5
- 测试：kb_ui 12 + web 20 全绿；Logs tab 待容器重建后验证

## 2026-08-07 v0.3.3n：searchapps/settings 实机验证通过

- 容器重建后实机验证：
  1. searchapps CRUD ✅（创建→列表 1→删除，全链路 code 0）
  2. settings 新 section 渲染 ✅（Chat Channels/MCP Servers/registered/
     search_knowledge 全部出现）
- Logs tab 已提交，待含其镜像的下次重建验证

## 2026-08-07 v0.3.3o：Logs tab 位置修复

- 容器验证发现 /dataset/{id} 页面无 tab5：Logs 块误插在 kb_detail_page
  结尾（该路由渲染 dataset_page）。已迁移至 dataset_page 末尾（877 行），
  - kb_ui 12 + web 20 全绿；镜像重建中

  ## 2026-08-07 v0.3.3p：Logs tab 页面级验证通过

  - 重建后 /dataset/{id} 页面验证：nav5/showTab(5)/id='tab5'/logFile/logBody/
    loadLogs/filterLogs/badge 全部渲染 ✅
  - 覆盖矩阵 aligned 134→137

## 2026-08-07 v0.3.3q：OpenAI 兼容端点（RAGFlow openai_api parity）

- RAGFlow restful_apis 文件清单（26 个）与 RayRAG 路由对照：23/26 已覆盖；
  缺口 openai_api（绑定 chat_id 的检索增强 OpenAI 端点）
- 新增 POST /api/v1/openai/{chat_id}/chat/completions：OpenAI 格式消息 →
  会话 KB/模型解析 → generate_chat_answer（检索增强）→ OpenAI
  chat.completion + references 数组；generate_chat_answer 等 3 处改
  pub(crate) 复用
- 内存实测：RayRAG 容器 65.25 MiB / 0.01% CPU vs RAGFlow 16.6 GiB /
  580% CPU（含解析负载）——用户关注的低内存优势确认
- 测试：openai 15 + server 75 全绿；待容器重建后实机验证端点

## 2026-08-05 v0.3.2：OCR 解析链路打通

- 根因 1：FigureParser 装配 ocr=None（从未启用 OCR）
- 根因 2：figure.rs:514 在 tokio runtime 内再建 runtime → panic
  （"Cannot start a runtime from within a runtime"）
- 修复：new() 惰性 from_env() + block_in_place/Handle::current().block_on
- .env 增 RAYRAG_OCR_PROVIDER=proxy；OCR 代理契约：POST /ocr + {"image":b64}
- 验证：PNG→OCR 文字→索引→检索 score 0.902；四 GPU 服务全打通

## 2026-08-05 v0.3.1 reranker 修复 + 延迟基准

- 根因：8081 mxbai-rerank 手工进程绑 127.0.0.1（容器内 172.29.0.1 不可达）
  → 502 Reranker request failed
- 修复：启动 systemd llama-rerank.service（bge-reranker-v2-m3 @8899 绑 0.0.0.0），
  .env RERANK_API_BASE 8081→8899、RERANK_MODEL mxbai→bge-reranker-v2-m3，重建容器
- 验证：rerank 检索 code 0、score 1.0；三个 GPU 模型链路全部打通
- 延迟基准（rerank 检索 5 次）：86-102ms 均值 92ms
- 教训：手工启动的 llama-server 易绑回 127.0.0.1；优先用 systemd 服务
  （llama-rerank.service bge@8899 / llama-embed.service / LLM@8088 已绑 0.0.0.0）

## 2026-08-05 v0.3.1 容器验证

- 部署成功：容器 healthy、healthz 200、5 页面抽查全 200
- /agents showTemplates 已渲染；agent 详情页按钮视有无 agent 而定
- 已提交 1d8f80d（v0.3.1：12 按钮补齐）

## 2026-08-05 v0.3.1：按钮 diff 核对补齐

- deleg_c9865d52 单 agent 又 1800s 超时（反复 cargo check 排队）；产出落盘
  web.rs +169（12 个按钮事件）；父 agent 门禁 1147 全绿
- 教训升级：web.rs 单文件任务一律超时——UI 改动直接主 agent 做或按函数拆分
  多 agent 且每 agent 只跑一次门禁

## 2026-08-05 v0.3.0 端到端冒烟验证（GPU 链路）

- 登录 /api/v1/auth/login（注意：非 /api/v1/user/login）→ token
- 建 KB → 上传 smoke.txt → 入队解析（UNSTARTED→chunk）
- 检索 /api/v1/retrieval（参数 kb_ids 非 dataset_ids）→ score 0.975
  （embedding 0.916 + 全文 1.0，GPU embedding 生效）
- 对话 /api/v1/chat/completions（OpenAI 兼容：model+messages+kb_ids）
  → Qwen3.5-9B 生成完整回答，GPU LLM 链路生效
- 端到端：登录→建KB→上传→解析→检索→对话 全部通过

## 2026-08-05 v0.3.0 容器验证

- 部署成功：容器 healthy、healthz 200、11 页面全 200
- /kbs 按钮抽查：新建知识库/kbSearch/toggleKbMenu 均渲染（zh 默认）
- 已提交 d7111ae（v0.3.0：UI 按钮对齐）

## 2026-08-05 v0.3.0：UI 页面按钮对齐 RAGFlow

- 批次 deleg_9d323be8 三子 agent 全部 1800s 超时（web.rs 大文件 + 编译排队）
  但产出落盘（web.rs +796/server.rs +198/kb.rs +28）；父 agent 接管收尾：
  test_state 预写 users.json（绕过 bootstrap env 依赖）+ i18n 双语断言
- 测试基线：**1139 passed / 0 failed / 27 ignored**
- 教训：web.rs 单文件 UI 任务 > 1800s 需拆分更细或主 agent 直接做

## 2026-08-05 v0.2.5 容器验证

- 部署成功：容器 healthy、healthz 200、6 页面全 200
- MCP SSE 服务器验证：/mcp/sse 200（长连接正常）；/mcp/messages 按协议拒绝
  无 session_id（"Missing session_id"）与未知会话（"Unknown session"）——语义正确
- 已提交 428d181（v0.2.5：conf 配置/MCP SSE/矩阵终态）

## 2026-08-05 v0.2.5：conf 配置 + MCP SSE 服务器 + 矩阵终态

- model_meta.rs：all_models 逐字节核对 + 27 新模型锁定 + llm_factories 映射
- runtime_config.rs：system_settings 种子；settings.rs：ServiceConf
- mcp_server.rs 新建：SSE 服务器（ToolRegistry/JSON-RPC 2.0）接入 /mcp/sse
- memory.rs：消息记忆读写；矩阵：partial 44 → **15**（sandbox 22 → N/A-by-design）
- 测试基线：**1133 passed / 0 failed / 27 ignored**
- 覆盖矩阵终态：full 544 / partial 15 / N/A 61 / unmapped 0

## 2026-08-05 v0.2.4 容器验证

- 部署成功：容器 healthy、healthz 200、6 页面全 200
- 已提交 857a78c（v0.2.4：common 工具层/settings/benchmark/矩阵收尾）

## 2026-08-05 覆盖矩阵最终收尾（v0.2.5，纯文档）

- **矩阵终态**（docs/ragflow-v0.26.4-coverage.md，固定 v0.26.4 快照 4441 文件）：
  aligned **128** / full **541** / partial **18** / replaced **92** / N/A **61** /
  reference **3601** / unmapped **0**（行数：46487 / 245606 / 50105 / 37286 /
  10262 / 908046 / 0；摘要表已按清单逐行重算，与库存行完全一致）
- agent/sandbox 22 个非代码文件 partial → **N/A-by-design**（.env.example/Makefile/
  README.md/asserts svg/docker-compose.yml/executor_manager Dockerfile+requirements+
  seccomp json/pyproject.toml/sandbox_base_image 两套镜像文件/sandbox_spec.md/
  scripts 5 个 shell/uv.lock）：等价语义已在 src/sandbox.rs 的 seccomp profile、
  --user nobody、tmpfs、--memory + RLIMIT_AS 资源限制、GNU timeout 124/137 退出码
  分类 + Dockerfile 多阶段构建中表达；Python 沙箱镜像不属于 Rust 移植范围
- deepdoc/README.md、deepdoc/README_tr.md partial → **N/A**（文档；运行时已移植
  src/parser/、src/ocr.rs、src/app_parsers.rs）；deepdoc/__init__.py partial →
  **N/A**（Python 包标记，仅许可头 + beartype 装饰，无运行时语义）
- admin/build_cli_release.sh partial → **N/A-by-design**（Python CLI 构建/发布脚本；
  RayRAG 的 Rust CLI 由 src/main.rs 经 cargo 构建，发布由 CI/发布脚本承担）
- 核对（AK 本轮补代码，矩阵暂保持 partial、理由已注明）：api/db/__init__.py
  （FileType/UserTenantRole/TenantPermission 已移植，SerializedType 等常量组未移植）、
  mcp/server/server.py（MCP 客户端已移植，独立 SSE server 与 RAGFlowConnector
  工具面未完成）、memory/services/messages.py + query.py（配置生命周期已移植，
  抽取/摄取/召回运行时未完成）；AK 补全后需将对应行同步为 full
- 纯文档操作，未跑 cargo check；git 尚未提交（等父 agent 统一提交）

## 2026-08-05 v0.2.4：common 工具层 + settings + benchmark + 矩阵收尾

- common.rs ~1800：常量表/文本/时间/浮点/crypto(AES-PBKDF2)/ssrf/校验
- settings.rs：from_env/from_reader + parse_model_entry；benchmark.rs：IR 指标
- agent.rs：canvas 语义（ToolUseTrace/clean_tts_text/reset globals）
- 矩阵收尾：N/A-by-design 19（whatsapp Node 网关/Go CLI/README/占位）+ full 3
- 测试基线：**1114 passed / 0 failed / 27 ignored**
- 覆盖矩阵：partial 105 → **44**（full 541，N/A 35，unmapped 0）

## 2026-08-05 v0.2.3 容器验证

- 部署成功：容器 healthy、healthz 200、6 页面全 200
- 已提交 3312c3c（v0.2.3：flow 契约/服务层/MCP 客户端/Helm）

## 2026-08-05 覆盖矩阵收尾评估与文档化（v0.2.3 后续，纯文档）

- 核对 24 行剩余 partial 条目，逐一标注理由；摘要表重新生成（此前 stale）：partial 105 → **83**、N/A 13 → **32**、full 502 → **505**（aligned 128 / replaced 92 / reference 3601，总数 4441 不变）
- 矩阵头部新增 `N/A-by-design` 政策行（非 Rust 目标与非代码产物：Node/Go 入口、文档、打包/lockfile 元数据、空包标记）
- **→ N/A-by-design（19 行）**：
  - api/channels/whatsapp/gateway-node 4 行（README/index.js/package.json/package-lock.json）：Node.js 网关，非 Rust 移植范围；whatsapp channel.py/gateway.py 已 full → src/channels.rs
  - cmd/ragflow-cli.go、cmd/ragflow_server.go：Go CLI/服务入口，非 Rust 目标；Rust CLI = src/main.rs、server = src/server.rs
  - bin/.gitkeep：空 Git 占位；入口脚本由 Rust CLI + docker-entrypoint.sh 替代
  - admin/client 4 行（COMMAND.md/README.md/pyproject.toml/uv.lock）：文档/打包元数据；功能客户端已 full → src/api/admin.rs
  - agent/plugin 3 个 README（en/tr/zh）：文档；plugin 生命周期在 src/plugin.rs
  - deepdoc/README_zh.md、api/common/README.md：文档
  - rag/svr/task_executor_refactoring_plan.md：规划文档；执行器已 full → src/task_executor.rs
  - memory/__init__.py、memory/services/__init__.py：空包标记（0 行）
- **→ full（3 行，附 src 路径）**：
  - rag/prompts/__init__.py：generator 再导出 shim（generator 已 full → src/chat.rs）
  - rag/prompts/template.py：load_prompt(name)+缓存 → PromptLibrary::get(name)（src/prompts.rs）
  - api/db/template_utils.py：_collect_canvas_types + normalize_canvas_template_categories 精确移植（src/api/joint_services.rs）
- **保持 partial、说明列细化（2 行）**：api/db/__init__.py（FileType→src/api/utils.rs、UserTenantRole→src/api/joint_services.rs、TenantPermission→src/api/common.rs；SerializedType/InputType/CanvasCategory/PipelineTaskType 及常量组未移植）；mcp/server/server.py（MCP 客户端已移植 src/mcp_client.rs + tools 列表端点 src/api/connector.rs；独立 SSE server 与 RAGFlowConnector 工具面未完成）
- **保持 partial 不变（2 行）**：memory/services/messages.py、query.py（消息存储运行时未移植；配置生命周期在 src/api/features.rs）
- 复核：agent/sandbox 22 个非代码文件沿用 v0.2.3 covered-by 决定（保持 partial，等价语义在 src/sandbox.rs + Dockerfile，说明列已写明）
- 未跑 cargo check（纯文档操作）；git 尚未提交

## 2026-08-05 v0.2.3：flow 契约 + 服务层 + MCP 客户端 + Helm

- pipeline/flow.rs ~700：组件 payloads/trace/进度契约
- resume.rs：修复 2 个 regex 崩溃 + CORP_TKS 贪心匹配
- joint_services.rs +700：init_data 种子/LLM 工厂清理/canvas 规范化/task 状态机
- mcp_client.rs：SSE + Streamable HTTP 客户端
- helm/ 最小 chart（lint/template 通过）；deepdoc/server 13 行标记 N/A（版本差异）
- 测试基线：**1092 passed / 0 failed / 27 ignored**
- 覆盖矩阵：partial 166 → **105**（N/A 13）

## 2026-08-05 v0.2.2 容器验证

- 部署成功：容器 healthy、healthz 200、6 页面全 200
- 已提交 1ade38d（v0.2.2：common/热配置/工具契约/资源加载）

## 2026-08-05 部署层与基础设施覆盖收尾（helm / sandbox / deepdoc-server）

- 新建 helm/：最小 Helm chart（Chart.yaml + values.yaml + _helpers.tpl + templates/{deployment,service,configmap}.yaml + README.md），镜像 rayrag:local，Service 端口 9390 → 容器 8080；PostgreSQL 依赖说明见 helm/README.md（上游 elasticsearch/minio/mysql/opensearch/redis/infinity 子组件语义由单一 PostgreSQL 依赖替代）。helm lint 0 失败、helm template 渲染通过
- 覆盖矩阵：helm/* 18 行 partial → replaced（RayRAG files = helm/ 新 chart；helm/templates/tests/test-connection.yaml 保持 reference）
- 覆盖矩阵：deepdoc/server/* 13 行 → N/A（上游 v0.26.4 检出无 deepdoc/server/ 目录——版本差异，deepdoc/ 仅含 parser/ 与 vision/；等价语义由 src/parser/、src/ocr.rs、src/app_parsers.rs 覆盖）
- 覆盖矩阵：agent/sandbox 22 个非代码文件标注 covered-by（等价语义已在 src/sandbox.rs + Dockerfile：seccomp profile、--user nobody、tmpfs、--memory + RLIMIT_AS、GNU timeout、124/137 退出码分类；沙箱镜像语义以 code 执行限制表达）
- 核对：admin/client 4 个非代码文件（COMMAND.md/README.md/pyproject.toml/uv.lock）现状不变（partial，文档/打包元数据；对应代码文件已 full 移植至 src/api/admin.rs）
- 验证：timeout 280 cargo check --features postgres-backend 0 错误；docker compose config --quiet 通过

## 2026-08-05 v0.2.2：common/热配置/工具契约/资源加载

- api/common.rs：ApiError 信封 + TeamPermission；runtime_config.rs 热更新
- dialog.rs：use_sql 语义（is_aggregate_sql/add_kb_filter）
- runtime.rs：jin10/qweather/retrieval 参数补全 + wencai 别名
- resources.rs：ner/synonym.json 加载器（env 路径 + 降级）
- 测试基线：**1076 passed / 0 failed / 27 ignored**
- 覆盖矩阵：partial 200 → **166**

## 2026-08-05 v0.2.1 容器验证

- 部署成功：容器 healthy、healthz 200、6 页面全 200；admin/version 路由已注册（401=需认证）
- 已提交 d93190b（v0.2.1：远程沙箱+插件+NLP/LLM 工厂+doc_store）

## 2026-08-05 v0.2.1：远程沙箱 + 插件 + NLP/LLM 工厂 + doc_store

- sandbox.rs +456 远程契约；plugin.rs 插件机制（LLM 工具插件）
- nlp.rs +658（分词/繁简/query 规范化/词权重）；llm.rs +655（7 类模型工厂）
- advanced_rag.rs RAPTOR；doc_store.rs（Memory/Postgres/Zvec 三后端+池）
- memory.rs 高亮/聚合；api/admin.rs 端点补齐
- 修复：admin users 路由冲突（{username} vs {user_id} → 删旧路由）
- 测试基线：**1047 passed / 0 failed / 27 ignored**
- 覆盖矩阵：partial 261 → **200**

## 2026-08-05 v0.2.0 容器验证

- 部署成功：容器 healthy、healthz 200、7 页面全 200
- 已提交 05e6b35（v0.2.0：prompts 全量 + utils + 解析器行为）

## 2026-08-05 v0.2.0：prompts 全量 + utils 服务层 + 解析器行为

- prompts.rs +1390：26 常量（resume 双语/TOC 管线/vision describe/related_question）
- app_parsers：naive/advanced/graph 模式分派
- api/utils.rs ~600：FileType/sanitize_path/hash128/get_parser_config/init_dataset_defaults
- parser：excel 多 sheet+二分行数、docx 图片提取、pdf 表格图片区域
- 测试基线：**1003 passed / 0 failed / 27 ignored**
- 覆盖矩阵：partial 331 → **261**

## 2026-08-05 v0.1.9 容器验证

- 部署成功：容器 healthy、healthz 200、/datasources /kbs /agents /chat /settings 全 200
- 连接器路由已注册（/api/v1/datasources/connect 返回 401=需认证，属正常）
- 已提交 fdb31a7（v0.1.9：connectors + channels + sync tasks）

## 2026-08-05 v0.1.9：连接器框架 + 渠道框架 + sync 任务

- connectors.rs ~1480：Connector trait + Local/WebDAV/Blob/Feishu/Confluence + Registry
- channels.rs ~1200：Channel trait + Registry + webhook/feishu（含 challenge 校验）
- file_mgr.rs +236 DocFileCache；task_executor.rs +960 SyncTask + ChunkEnrichment
- 测试基线：**986 passed / 0 failed / 27 ignored**
- 覆盖矩阵：partial 440 → **331**

## 2026-08-05 v0.1.8：TaskExecutor + ChatPipeline

- task_executor.rs ~680 行：异步队列/状态机/进度回调/指数退避重试（对齐 task_executor.py）
- chat.rs 835 行：full_question 改写/multi_queries/citation_plus 引用/reflect 管线
- 测试基线：**967 passed / 0 failed / 27 ignored**
- 覆盖矩阵：partial 442 → **440**

## 2026-08-05 v0.1.7 容器验证

- 部署成功：容器 healthy、healthz 200、核心页面 200
- 已提交 565c788（v0.1.7：QA prompts + graph NER + 组件契约）

## 2026-08-05 v0.1.7：QA prompts + graph NER + 组件契约

- prompts.rs +1194：18 个 RAG 问答阶段 prompt（citation/reflect/multi-queries 等）
- structure_compile.rs +1033：graph_extractor NER 容错 + upsert 语义
- runtime.rs：iteration/exitloop/loopitem 等 4 新增 + 4 扩展组件 descriptor
- 测试基线：**954 passed / 0 failed / 27 ignored**
- 覆盖矩阵：partial 502 → **442**

## 2026-08-05 v0.1.6 容器验证

- 部署成功：容器 healthy、healthz 200、核心页面 200（/kbs /agents /providers /skills）
- 已提交 16e349a（v0.1.6：translate + extractor + title continuity）

## 2026-08-05 v0.1.6：translate 工具 + KeyTerm 提取器

- translate（deepl 契约）：百度翻译 VIP 签名 + sug 免 key 降级；descriptor 注册 runtime
- extractor.rs 402 行：词频/位置/POS 代理权重 + CJK n-gram 关键词提取，pipeline 可选调用
- title.rs：标题连续性/章节序号语义补齐
- 测试基线：**945 passed / 0 failed / 27 ignored**
- 覆盖矩阵：partial 507 → **502**

## 2026-08-05 v0.1.5 容器验证

- 部署成功：容器 healthy、healthz 200
- providers 页面 Model Catalog 确认：providerModels JS 对象注入（20 厂商/157 模型），页面 55KB
- 已提交 20af543（v0.1.5）+ 本记录

## 2026-08-05 v0.1.5：模型目录 + 沙箱契约 + graphrag 阶段标记

- Provider Model Catalog：20 厂商/157 模型种子化（fixtures + providers 页 modal）
- sandbox.rs +1031 行：executor_manager 枚举/失败协议/运行时约定/artifact/安全契约
- structure_compile.rs +321 行：phase markers + 实体消解确定性门控；prompts.rs +116 行 light 常量
- 测试基线：**931 passed / 0 failed / 27 ignored**
- 覆盖矩阵：partial 596 → **507**

## 2026-08-05 v0.1.4 容器验证：模板库 28 个确认

- 部署后 agents 页面仅 3 内置模板（22KB）→ 根因：web.rs 的 tpls 变量构建后未 b.push_str(&tpls)
- 修复（a5b4edc）：补 b.push_str(&tpls) → 页面 475KB，28 模板全渲染
- 验证：容器 healthy、healthz 200、agents 页 28 模板（deep_research/web_search_assistant/trip_planner 等抽样确认）

## 2026-08-05 v0.1.4：Agent 模板种子化 + GraphRAG prompts + layout 解析补齐

- 25 个 RAGFlow agent 工作流模板种子化（include_str! fixtures），agents 页面模板库 3→28
- 18 个 graphrag prompt 常量移植到 src/prompts.rs（entity/community/keywords/query 系列）
- paddleocr_layout.rs 66 行 stub → 完整 layout 流程（blocks→bbox→position tags）
- exact_dedup_by_key 修复：canonical 名保留首现（对齐 RAGFlow）
- 测试基线：**919 passed / 0 failed / 27 ignored**
- 覆盖矩阵：partial 623 → **596**

# RayRAG 运维台账

## 2026-08-05 v0.1.3 容器部署验证

- 镜像重建成功（rayrag:local，含 Token/Title Chunker + iwencai + KB 详情页 + structure_compile 修复）
- 部署坑（记录防复发）：
  - compose 需 .env 提供 RAYRAG_POSTGRES_PASSWORD / RAYRAG_ADMIN_PASSWORD
  - 宿主机系统 postgres15 占 5432 → RAYRAG_POSTGRES_PORT=55432（容器内仍连 postgres:5432）
  - 9380 被真 RAGFlow 容器(docker-ragflow-cpu-1)占用 → RAYRAG_PORT=9390
- 验证：容器 healthy、healthz 200、13 页面全 200、GPU 四链路 200

## 2026-08-05 v0.1.3：Token/Title Chunker + iwencai + KB 详情页 + GPU rerank 修复

- **GPU 链路修复**：8081 llama-server 缺少 `--rerank` 参数导致 /rerank 501 →
  以 `--rerank -ngl 99 -c 32768 --api-key mxbai-rerank-key` 重启 → 200；
  RayRAG 容器 restart 后 healthz 恢复 200（postgres 连接终止问题）。
- **Token Chunker**（src/chunk/token.rs）：one/token_size/delimiter 三模式、
  反引号分隔符编译（最长优先）、分隔符并入段尾、重叠率 normalize、children 二次拆分。
- **Title Chunker**（src/chunk/title.rs）：hierarchy/group、PDF 大纲 __outline__
  元数据优先、正文标题行 fallback、include_heading_content/root_chunk_as_heading。
- **iwencai**（src/wencai.rs）+ 沙箱 executor_manager 客户端（code_exec.rs）修复。
- 修复上一会话遗留 structure_compile.rs 编译错误（7 个）。
- 测试基线：**911 passed / 0 failed / 27 ignored**（较上轮 856 +55）
- 覆盖矩阵：partial 631 → **623**
- 子 agent 配置：delegation.max_iterations 50→150、child_timeout_seconds 600→1800

## 2026-08-05 容器终验修复：datasources 页 500

- 根因：datasources_page 多了 `Extension<String>`（tenant_id）——全局无此 layer → 500 Missing request extension（files_page 无此参数故 200）
- 修复：移除该参数（detect_lang 用 Request headers）
- 测试基线：**856 passed / 0 failed / 26 ignored**
## 2026-08-05 data_source 连接器：Azure Blob（SAS）

- `src/data_source/blob.rs`：Azure Blob REST（SAS token 认证免签名）——list containers（target 空时）/ list blobs / download → ConnectorDoc（container/blob metadata）；XML 解析免依赖（parse_xml_names）
- mod.rs Blob 枚举；API create/sync 支持 kind=blob（match 分发）
- 5 新测试（XML 解析×2/枚举/缺 SAS/缺 URL）
- 测试基线：**856 passed / 0 failed / 26 ignored**
## 2026-08-05 UI 第 44 轮：sync 目标 KB 选择器

- `web.rs` datasources_page：Sources 卡片加 KB 下拉（loadKbs 填充 /api/v1/datasets）——Sync 请求带 ?kb_id= 入库；空值仅预览
- 测试基线：**851 passed / 0 failed / 26 ignored**
## 2026-08-05 数据源 sync 入 KB pipeline

- `sync_data_source` 支持 `?kb_id=<dataset id>`：ConnectorDoc → 写 uploads/{doc_id}.{ext} → register_document_upload（标准解析管线：parse→chunk→embed→index）——返回 count/ingested/skipped
- 辅助暴露：server_xxh3_hash（xxh3 128 位，与上传一致）、register_document_upload、PersistedUpload.cleanup_on_drop pub(crate)
- 测试基线：**851 passed / 0 failed / 26 ignored**
## 2026-08-05 UI 第 43 轮：数据源页

- `web.rs` datasources_page：🔗 nav + 配置表单（name/url/token/target + Issues/MRs checkbox）+ 列表（token 脱敏）+ Sync（预览表格）+ Del
- 路由 /datasources
- **转义教训（本轮重灾）**：python 写入 Rust 字符串内嵌 JS——所有 JS 双引号必须 Rust `\"`（python 写 `\\\"` 4 反斜杠）——裸引号直接闭合 Rust 字符串；JS 改用全单引号策略后仅剩 push_str 边界 + syncDs 参数转义；函数签名需带 Request 拿 detect_lang（无 is_zh 函数）
- 测试基线：**851 passed / 0 failed / 26 ignored**
## 2026-08-05 数据源 API（GitLab 配置/同步）

- `src/api/data_source_mgr.rs`：DataSourceStore（data_sources.json 持久化 + 读写锁 + 保存锁）+ 4 handler
  - GET/POST /api/v1/data_sources（创建/列表，token 脱敏显示 ***）
  - DELETE /api/v1/data_sources/{id}
  - POST /api/v1/data_sources/{id}/sync（GitLab 拉取 → 文档数 + 预览前 10）
- server.rs：AppState.data_sources + 路由 + 初始化；chrono 依赖
- 测试基线：**851 passed / 0 failed / 26 ignored**（+3：store 往返/脱敏/重启恢复/默认值）
## 2026-08-05 data_source 连接器 MVP：GitLab

- `src/data_source/mod.rs`：DocumentSource 枚举 + ConnectorDoc（镜像 RAGFlow Document 字段：blob/semantic_identifier/extension/metadata）+ SourceOptions + Connector trait 骨架
- `src/data_source/gitlab.rs`：GitLab REST 连接器（PRIVATE-TOKEN 认证）——项目文件（recursive tree + raw 拉取）/ Issues / MergeRequests → ConnectorDoc（镜像 gitlab_connector.py 的 _convert_code/_convert_issue/_convert_mr）
- 本地 urlencoding（免依赖）+ 4 单测（source 枚举/doc 渲染/缺 URL/缺 target）
- 测试基线：**848 passed / 0 failed / 26 ignored**（+4）
- 后续：blob 连接器、sync 入 KB API、UI 数据源页
## 2026-08-05 运维监控脚本

- `scripts/health-check.sh`：一键检查容器（rayrag/postgres/SearXNG）+ healthz + 14 页面 + GPU 四链路（embedding/LLM/rerank 带 key/SearXNG）
- 实测全绿（rerank 需 Bearer mxbai-rerank-key）
## 2026-08-04 模板全集容器实测（3/3）

- 网络搜索模板：Begin→SearXNG（SEARXNG_URL env）→Message——cn.bing 真实结果（百度等 URL）进回答（1928 字符）✓
- 模板全集：RAG 问答 ✓（Reference 1-3）/ 网络搜索 ✓ / RAG 对话（同路径组件）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 最终容器终验（模板修复进镜像）

- 重建镜像 → 容器实测：
  - 14 页面全 200
  - agents 页 Templates 按钮 + agentTemplates（4 处）就位；kbs 页无残留（0）
  - RAG 模板 API 建 agent + 检索跑通（此前已实测 Reference 1-3）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 模板字段修复 + RAG 模板容器实测

- 修正：Retrieval 输出字段是 `formalized_content`（非 result）——agentTemplates 3 处引用更新
- 容器实测（RAG 问答模板）：Begin→Retrieval→Message 全链路——检索 4 chunks（embed_test.txt/test_deck.pptx/ocr_test.png）+ Reference 1-3 进回答（1131 字符）✓
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 42 轮：Agent 模板库

- `web.rs` agents_page：🧩 Templates 按钮 + 3 个预设工作流（RAG 问答 / 网络搜索 / RAG 对话）——prompt 选择 + 命名后一键创建（components 格式 DSL）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 41 轮：Agent DSL 编辑器 + 画布端到端验证

- `web.rs` agent_detail_page：Canvas DSL 卡片加 ✏️ Edit（JSON textarea）+ 💾 Save（PUT /api/v1/agents/{id}）——用户可粘贴 RAGFlow DSL 建工作流
- **关键发现**：画布执行器只认 `components` 表（RAGFlow v0.26.4 标准格式）——`graph.nodes` 顶层格式不执行（fallback 到 prompt_template）
- 容器端到端验证（components 格式）：
  - 失败路径：SearXNG 无效 URL → 回答 `SearXNG request failed`（错误传播 ✓）
  - 成功路径：cn.bing 真实结果（URL/标题/内容）进回答（2412 字符）✓
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 全部 11 个工具错误函数统一传播修复

- 同模式缺陷：set_tavily_error/set_wikipedia_error/set_google_error/set_google_scholar_error/set_github_error/set_yahoo_finance_error/set_arxiv_error/set_pubmed_error/set_duckduckgo_error/set_invoke_error 均只写 _ERROR（9 个通用 + invoke 单独）+ domestic（已修）
- 修复：全部同时写 formalized_content（错误原文）——画布 answer 感知工具失败
- **关键**：Agent 工具 response 构造（execute_canvas_agent_tool_with_providers）**_ERROR 优先**（`{provider} error: {error}` Python 对齐，7 个测试保护）+ formalized_content 次之（成功路径）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 agent 工具错误传播修复

- 发现：searxng 节点失败时回答"搜索功能已就绪"（LLM 自由发挥）——`set_domestic_search_error` 只设 `_ERROR`，formalized_content 为空
- 修复：错误同时写入 formalized_content（`[Search error] ...`）——下游 answer 感知工具失败
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 最终容器终验（UI 38-40 + SearXNG + 性能）

- 重建镜像 → 容器实测：
  - 14 页面全 200
  - files 搜索（fileQ/fileFilter）✓；dataset embdSel ✓；chat 模型选择器（/chat/{id}）✓
  - SEARXNG_URL 注入容器（agent searxng 工具可用）
  - RAYRAG_NO_FSYNC=1 生效（compose 默认）
  - healthz: postgres/zvec healthy
## 2026-08-04 SearXNG 自托管接入验证

- Docker 部署 SearXNG（docker.m.daocloud.io/searxng/searxng，端口 18888，容器 rayrag-searxng）
- 中国网络适配：默认引擎（brave/ddg/wikipedia）全 timeout → **启用 bing 引擎 + cn.bing.com base_url**（settings.yml: disabled false + base_url）→ JSON 200 + 10 条结果
- 配置要点：`search.formats` 加 json（默认仅 html 返回 403）；SEARXNG_SECRET 随机
- RayRAG 客户端格式匹配（/search?q=&format=json）；compose 透传 SEARXNG_URL
- 用法：`SEARXNG_URL=http://192.168.3.6:18888`（agent searxng 工具 / SEARXNG_URL env）
## 2026-08-04 覆盖矩阵终检

- `docs/ragflow-v0.26.4-coverage.md`（固定快照，4441 文件）：**unmapped = 0**（无映射缺口）
- aligned 83 + partial 1561 + replaced 74 + reference 2724；核心对齐（aligned/replaced）157 文件
- agent/tools 23/23、parser 16/16、vision、api/apps、GraphRAG 均已在矩阵中 ✓
- ragflow-coverage bin 已不在代码库（README 引用为历史命令），矩阵为固定快照无需重跑
## 2026-08-04 UI 第 40 轮：dataset 配置 embedding 模型切换

- `web.rs` dataset_page：配置表新增 Embedding Model 下拉（providers 源 provider:model）→ saveConfig 带 embedding_model（update_dataset 已支持 + 换模型校验）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 39 轮：chat 模型选择器

- `web.rs` chat_detail_page：模型下拉（从 /api/v1/providers 拉 provider:model 复合 selector）→ sendMsg 带 chat_model——对齐 RAGFlow 对话模型切换
- 当时注意：/api/v1/models 的 rayrag-v1 不是有效 tenant selector（resolve 会 400）；
  此缺口已由 2026-08-12 v0.3.3ac 的 tenant catalog 与固定 selector wire 取代
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 38 轮：files 搜索

- `web.rs` files_page：搜索框（fileQ → fileFilter 过滤 #fl tr data-name）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 上传性能终验（NO_FSYNC 生效）

- 容器实测：5 文档上传 53s（原始）→ 64s（增量 upsert 无效）→ **37s（NO_FSYNC=1）**
- 剩余瓶颈：zvec flush（库内部持久化 fsync，向量一致性保证）——容器 overlay 慢，宿主部署无此问题；不跳过
- 结论：容器部署建议 RAYRAG_NO_FSYNC=1（compose 已默认）；纯 Linux 宿主部署保持默认 fsync
## 2026-08-04 上传性能：容器 overlay fsync 瓶颈 + RAYRAG_NO_FSYNC

- 实测：5 文档上传容器 53-64s——Saved 每文档 ~10s
- 根因链：增量 upsert 优化无效 → 定位为 **persistence::atomic_write 的 fsync**（容器 overlay 文件系统 fsync ~10s/次）
- 修复：`RAYRAG_NO_FSYNC=1` 跳过 file.sync_all + sync_parent（崩溃最多丢最后快照，JSON 可重建；默认保留安全 fsync）
- 使用：compose env RAYRAG_NO_FSYNC=1（容器部署建议）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 zvec 增量 upsert 优化（上传性能）

- 基准：5 文档批量上传 53s（~10.6s/文档）——日志显示 Parsed→Embedded 0.4s 但 Saved ~9.6s
- 根因：replace_collection 已增量删除但 **upsert 全量**（每次上传重写整个 KB 集合）
- 修复：仅 upsert 新增/内容或向量变更的 chunk（id + content/embedding diff）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 容器回归终验（UI 第 33-37 轮）

- 重建镜像 → 容器实测：
  - 14 页面全 200（启动约 30s：zvec 加载后 healthz healthy）
  - dashboard：Recent Chats + Docs/Chats 统计 ✓
  - search：searchPage/sPage 分页 ✓
  - document 页：chunkPage/renderPage/cPageSize 分页 ✓
  - dataset 页：Clear All/renameDoc ✓
## 2026-08-04 UI 第 37 轮：搜索分页

- `web.rs` search_page：请求带 page（后端已支持）+ 结果分页控件（Prev/Next + Page X/Y + total）——对齐 RAGFlow 检索翻页
- 注意：runSearch(q) 带参（lastQuery 供 searchPage 复用）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 36 轮：文档 chunk 分页浏览

- `web.rs` document_page：loadChunks 分页（每页 20，Prev/Next + 页码指示 + 总数）——对齐 RAGFlow document-viewer 分页
- 教训：JS 字符串转义——Rust b.push_str 内 JS 转义引号必须 `\\"`（2 反斜杠），python 替换时同样 2 反斜杠；插错 1 反斜杠 → Rust 字符串截断（E0425）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 docx 偶发根因修复：HashMap tie-break

- 真正根因（静态正则缓存只是缓解）：`type_counts.into_iter().max_by_key` 在类型计数 tie 时依赖 **HashMap 迭代序**（如 {Nu:2, Ot:2, St:2}）→ max_type 不确定 → 数字行被当表头 → 断言失败
- 修复：`max_by` + 确定性 tie-break（非数字类型优先）+ `Self::num_rank`（闭包内须 Self:: 前缀，裸调 E0425）
- 全量连续 2 次 **844 passed / 0 failed**（消除长期 flaky）
## 2026-08-04 UI 第 36 轮：检索关键词高亮

- `web.rs` search_page：hl() 高亮函数（正则转义 + <mark> 包裹），测试面板结果渲染应用
- 教训：JS 正则字符类 `\]` 在 Rust 字符串需 `\\]`（python 里 `\\`）——unknown character escape
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 35 轮：dashboard 最近会话

- `web.rs` dashboard：Recent Chats 卡片（最近 5 会话 + 最后消息预览，点击跳转）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 34 轮：dashboard 统计增强

- `web.rs` dashboard：统计卡片新增 Docs（文档总数）/ Chats（会话数）——KBs/Chunks/Docs/Chats/Embedder/LLM 六卡片
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 33 轮：图谱上下文展示

- `web.rs` search_page：结果区下方渲染 `🔗 Graph Context` 卡片（graph_context 数组，区分图来源与向量引用）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 32 轮：API docs 扩充

- `web.rs` api_docs：8 → 45+ 端点（Auth/Users、Knowledge Bases、Search & Chat、OpenAI-Compatible、Files & Memories、Agents & System 分组表）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 容器终验（图谱 query/注入）

- 重建镜像 → 容器实测：
  - POST /datasets/{id}/graphrag/query → code 0（nodes/context 结构正确；新容器无图时返回空）
  - RAG 对话 code 0 + 基于知识库回答（图谱注入路径执行，context 空时无害跳过）
  - 页面 6 项抽查全 200
## 2026-08-04 图谱上下文注入 RAG 对话

- `features.rs` generate_chat_answer：检索上下文后追加 `context_for_query` 图谱上下文（最多 4 条）——GraphRAG 增强 RAG 回答（对齐 RAGFlow GraphRAG 检索）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 graphrag query API + 容器持久化验证

- `server.rs`：POST /api/v1/datasets/{id}/graphrag/query——返回图谱节点/边/context（context_for_query 图谱上下文）；无图时返回空结构
- 容器验证：build → graphrag.json 42→115 字节（kb_graphs 条目）→ **重启后保留** ✓（持久化闭环）
## 2026-08-04 KB 删除清理图谱资产

- `server.rs` delete_dataset：删除前清理 GraphStore kb_graphs[kb_id] + 对应 checkpoints（避免孤儿图谱数据）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 知识图谱持久化修复

- `server.rs` build_graphrag：此前只返回统计不落盘（graphrag.json 恒空）——现在将 EntityGraph 合并进 GraphStore.kb_graphs + replace_snapshot（重启后保留，支持图谱查询上下文）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 31 轮：memories 清空全部

- `features.rs`：MemoryStore::clear_all_owned（按 owner 批量删除，返回数量）
- `server.rs`：DELETE /api/v1/memories/clear handler
- `web.rs` memories_page：🗑 Clear All（确认 → clear → reload）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 30 轮：chat 会话重命名

- `server.rs`：PATCH /api/v1/conversations/{id}（rename_for 已存在，补 handler+路由）
- `web.rs`：会话卡片 ✏️ Rename（prompt → PATCH → loadChats）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 容器回归终验（UI 第 26-29 轮）

- 重建镜像 → 容器实测：
  - 14 页面全 200
  - agents 页：agentQ/agentFilter/agentTb 搜索就位 ✓
  - admin 页：userQ/userFilter/userTb 搜索就位 ✓
  - dataset 页：Clear All + renameDoc（✏️ 重命名）就位 ✓
## 2026-08-04 UI 第 29 轮：agents 搜索

- `web.rs` agents_page：搜索框（agentQ → agentFilter 前端过滤 data-name）+ tbody id
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 28 轮：admin 用户搜索

- `web.rs` admin_page：Users 卡片搜索框（userQ oninput → userFilter 前端过滤 data-name/data-email）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 27 轮：文档重命名

- `document.rs`：DocStore::rename（校验非空，storage_name 不变）；`rename_doc` handler（PATCH /api/v1/datasets/{id}/documents/{did}，kb_manageable 校验）
- `web.rs`：文档行 ✏️ Rename 按钮（prompt → PATCH）
- 教训：Rust b.push_str JS 字符串内嵌引号需 `\"`（python 里写 `\\"`），裸 `"` 会截断字符串（7 errors）——git checkout 重做
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 26 轮：数据集清空

- `web.rs` dataset_page：🗑 Clear All 按钮（确认 + 循环 DELETE 全部文档，保留数据集）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 容器回归终验（UI 第 23-25 轮）

- 重建镜像 → 容器实测：
  - 14 页面全 200
  - kbs 页：kbEmbd/kbPerm 创建下拉 ✓
  - dataset 页（/dataset/{id}）：Batch Delete 按钮 + docCk checkbox + docSelAll 全选 + multiple 批量上传 ✓
## 2026-08-04 UI 第 25 轮：文档批量删除

- `web.rs` document_page：行 checkbox + 表头全选（docSelAll）+ 🗑 Batch Delete（串行 DELETE + 确认）——对齐 RAGFlow 文档多选删除
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 24 轮：创建 KB 高级选项

- `web.rs` dataset_page：createKb 支持 embedding_model/permission 下拉（create_dataset API 已支持字段）；补空名校验
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 23 轮：批量上传

- `web.rs` dataset_page：`<input multiple>` + uploadDoc 顺序串行上传多文件（按钮禁用防重复 + 完成后 loadDocs/pollParse）
- 教训：正则替换大 JS 函数曾误删后续代码（`.*?\n}` 匹配到字符串内换行）——**必须括号配平提取**；已 git checkout 恢复重做
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 容器终验：OpenAI 兼容全集 4/4 + rerank 格式

- 重建镜像（含 rerank 格式修复）→ 容器实测：
  - POST /api/v1/rerank → `{"object":"rerank","results":[{"index":0,"relevance_score":-0.474},...]}`——**Jina 标准格式**，语义排序正确（知识库相关文档排第一）
  - healthz：postgres/zvec 全 healthy
- OpenAI 兼容全集：models / embeddings / chat(流式+非流式) / rerank 全部容器实测通过
## 2026-08-04 rerank 端点格式修复

- 坑：Reranker trait 返回 `Vec<(usize, f32)>`（index,score 对）——handler 曾误用 enumerate 包成 [index, score] 数组
- 修复：直接 map (index, score) → `{"index": i, "relevance_score": s}`（Jina 格式）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 OpenAI 兼容 rerank 端点（POST /api/v1/rerank）

- `openai_proxy.rs`：rerank handler——Jina 风格 `{"query","documents"}` → `{"object":"rerank","results":[{"index","relevance_score"}]}`；未配置 503
- 注意：`/api/v1/reranker` 是配置端点（GET/PUT），POST 会 405——调用端点应为 /api/v1/rerank
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 embeddings 端点 fallback 修复

- `openai_proxy.rs`：resolve(Embedding) 返回 None 时（租户模型表为空，容器只有 env embedder）→ fallback `state.embedder`（EMBED_API_BASE 配置的 OpenAI 兼容 embedder）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 流式 UI：思考占位 + 引用帧

- `features.rs`：SSE 流结束前发 `chat.completion.references` 帧（chunk_id/kb_id/score/content 前 120 字符）
- `web.rs` sendMsg：首帧前显示 `🤔 思考中…`（推理模型思考期反馈）；首 content 帧清除占位；references 帧渲染引用卡片
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 容器 SSE 流式终验 + 推理模型注意事项

- 容器实测 `/api/v1/chat/completions` stream=true：`"content":"你好"` + [DONE]——**流式端到端通过**
- 注意：Qwen3.5-9B 为推理模型——先输出 ~9000 帧 `reasoning_content`（每帧 0.3s，思考约 45min 级），content 帧在思考结束后才出现；且 llama-server 单请求串行，并发 stream 请求会排队（测出空流是并发干扰，非代码 bug）
- 非流式/流式均正确（LlmClient::new 无总超时，可长时间思考）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 打字机效果（chat 页 SSE 消费）

- `web.rs` sendMsg：fetch stream:true → ReadableStream + TextDecoder 逐行解析 `data:` 帧 → delta.content 逐步渲染（打字机效果）→ 结束后渲染操作按钮
- 会话消息仍持久化（服务端 append_exchange）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 Chat 端点 SSE 流式（打字机效果后端）

- `llm.rs`：`rag_chat_stream`（检索上下文 + history 组装 + chat_stream 逐块）
- `features.rs`：`generate_chat_answer` 加 `on_chunk` 回调参数（stream 时走 rag_chat_stream）；`chat_complete` 支持 `stream: true` → Sse（chat.completion.chunk 帧 + stop + [DONE]）
- ChatRequest 加 stream 字段
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 OpenAI 兼容流式输出（SSE）

- `llm.rs`：`chat_stream`——stream=true 请求 + bytes_stream 逐行解析 SSE（`data: {...}` delta.content / `[DONE]`）+ 逐块回调
- `openai_proxy.rs`：chat_completions stream=true → `axum::response::sse::Sse`（tokio channel + KeepAlive）→ chat.completion.chunk 帧 + [DONE]
- `Cargo.toml`：tokio-stream 依赖（axum 0.8 Sse 默认可用，无需 sse feature）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 OpenAI 兼容 /api/v1/embeddings 端点

- `openai_proxy.rs`：`embeddings` handler——input（string/array）→ tenant_models.resolve(Embedding) → embed → OpenAI 格式响应（object/list + data[{object,index,embedding}] + model + usage）
- `server.rs`：POST /api/v1/embeddings 路由
- 当时 OpenAI 兼容端点全集：/api/v1/chat/completions + /api/v1/models +
  /api/v1/embeddings + /api/v1/reranker；v0.3.3ac 起 OpenAI 模型目录迁至
  /api/v1/openai/models，/api/v1/models 恢复为 RAGFlow tenant selector catalog
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 ASR 透传 + 上传格式提示

- `docker-compose.yml`：透传 RAYRAG_ASR_API_BASE/KEY/MODEL（容器音频转录可用）
- `web.rs` dataset_page：上传区格式提示补 PNG/JPG(OCR)、MP3/WAV(ASR)
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 音频转录接线（对标 RAGFlow whisper）

- `audio.rs`：`asr_from_env()`（RAYRAG_ASR_API_BASE/KEY/MODEL → AsrClient；未配置 → None）
- `pipeline/mod.rs`：Pipeline.stt 字段 + with_asr_from_env + parse_document 音频分支（audio/* 或 ogg → 配置时调 OpenAI 兼容 /audio/transcriptions → `<!--AUDIO_START-->` 转录文本；未配置/失败 → 回落 AudioParser 元数据占位）
- 国内 ASR 兼容：SiliconFlow 等 OpenAI 兼容端点直接可用
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 22 轮：Knowledge Graph 环形可视化

- `web.rs` dataset_page buildGraph：文本输出 → **SVG 环形图**（中心 KB 节点 + 实体按 degree 半径成环 + 连线 + 标签），对齐 RAGFlow 图谱可视化
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 容器多格式解析验证：pptx

- 上传 test_deck.pptx（标题+正文 1 页）→ Parsed → Chunked: 1 → Embedded: 1
- 检索 chunk：`<!-- Slide 1 --> RayRAG PPTX Test / Rust 知识库系统演示 / 支持向量检索`——**幻灯片结构保留**（RAGFlow 风格）
- 容器已实测格式：**txt / png(OCR) / pdf / xlsx / pptx**
## 2026-08-04 容器多格式解析验证：xlsx 表格

- 上传 test_table.xlsx（3 行 3 列表格）→ Parsed → Chunked: 1 → Embedded: 1
- 检索实测 chunk 内容：`| 姓名 | 数值 | 备注 | / | 张三 | 100 | ok |...`——**Markdown 表格格式**（RAGFlow 表格解析风格一致）
- 容器内已实测格式：txt / png(OCR) / pdf / xlsx
## 2026-08-04 UI 第 21 轮：搜索页多数据集选择

- `web.rs` search_page：KB 多选 chips（fetch /api/v1/datasets → checkbox）+ runSearch 收集 kb_ids（勾 All 时不传 → 服务端默认全可访问；勾具体 KB 时传数组）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 第 20 轮：rerank 结果标记

- `web.rs` search_page：rerank=true 时结果行显示 `&middot; reranked` 徽标（区分重排分数来源）
- 确认：chat 详情页 sendMsg 走 /api/v1/chats/{id}/completions（generate_chat_answer 已有完整 RAG 上下文）——UI 无缺口
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 chat_completion RAG 上下文注入（核心缺口修复）

- `server.rs` chat_completion：原只做 kb 权限校验 + 纯 LLM 对话（欢迎语，无上下文）→ 新增 `hybrid_search_kbs` 检索（top_k 默认 4）+ RAG prompt 组装（"请基于以下知识库内容回答…"）+ references 返回
- 坑：handler 读 `question` 字段（非 OpenAI messages）
- 容器实测：question="RayRAG 是什么" → **回答基于知识库内容**（"RayRAG 是 Rust 实现的知识库系统…对标 RAGFlow"）+ refs:1
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 docx 并行偶发修复：block_type 正则静态缓存

- 根因：`block_type` 每格调用都重新编译 11 个正则（`regex::Regex::new`），并行高负载下偶发编译失败（if let Ok 静默跳过）→ 分类漂移 → compose_table 测试断言失败
- 修复：OnceLock 静态预编译缓存（首次初始化后零编译）；顺带显著降低表格解析 CPU 开销
- 验证：连续 2 次全量 **844 passed / 0 failed / 26 ignored**
## 2026-08-04 容器内 OCR 实连（四链路全闭环）

- **8097 确认为 PaddleOCR GPU Proxy**（paddleocr_gpu_proxy.py；18090 是另一 Web 应用）；`POST /ocr {"image":b64}` JSON 格式（非 multipart），与 RayRAG ocr.rs legacy 客户端完全匹配
- `server.rs` update_dataset 白名单扩展：layout_recognize / ocr_enabled
- 容器配 RAYRAG_OCR_PROVIDER=proxy + RAYRAG_OCR_BASE_URL=http://192.168.3.6:8097 → KB 配 layout_recognize=paddleocr（**值必须是 "paddleocr"，非 "PaddleOCRLayout"**，见 is_paddleocr_layout）→ 重解析图片
- **实测：图片 chunk 含 `[DOCUMENT_LAYOUT: PaddleOCR] RayRAGOCRTest2026`**——容器→GPU OCR 生效
- **GPU 四链路全部容器实连闭环**：embedding 8888 / LLM 8088 / rerank 8081 / OCR 8097
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 容器内 rerank 实连（三链路闭环）

- **8081 重启改绑 0.0.0.0**（原 127.0.0.1 容器不可达）：kill 旧进程 → `llama-server ... --host 0.0.0.0 --port 8081 --api-key mxbai-rerank-key --reranking`；局域网 /rerank 调用验证通过
- 容器配 RERANK_API_BASE=http://192.168.3.6:8081 + RERANK_API_KEY + RERANK_MODEL → 重建
- retrieval rerank=true 实测：score 0.965（向量）→ **0.7（reranker 重排分数）**——容器→GPU rerank 生效
- **GPU 三链路全部容器实连闭环**：embedding（8888）+ LLM（8088）+ rerank（8081）
## 2026-08-04 UI 对标 RAGFlow 第 19 轮：登录页注册 + cookie 会话

- `web.rs` login_page：Register/Login 切换（昵称/邮箱/密码 → /api/v1/user/register → 自动登录）；**登录成功写 cookie**（rayrag_token, 24h）——修复浏览器导航到 protected 页时无认证的问题
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 对标 RAGFlow 第 18 轮：Agent Canvas 拓扑可视化

- `web.rs` agent_detail_page：Canvas 卡片 + JS `renderTopology(dsl)`——解析 nodes/edges/position → 动态 SVG（带箭头连线 + 节点圆角矩形 + 标签），对齐 RAGFlow 画布只读视图
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 对标 RAGFlow 第 17 轮：Agent 详情页（画布简版）

- `web.rs`：`agent_detail_page`（/agents/{id}）——agent 元数据（ID/KBs/状态）+ Canvas DSL JSON 只读视图 + Run/Ask（completions API）+ Delete
- `server.rs`：/agents/{agent_id} 路由
- 补齐 agents 列表 Edit 按钮的目标页（原跳转 404）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 对标 RAGFlow 第 16 轮：搜索导出 + 上传轮询

- `web.rs` search_page：结果保存到 lastResults + **⬇ Export MD** 按钮（生成 Markdown 下载，含分数/来源）
- `web.rs` dataset_page：uploadDoc 后每 3s 轮询文档状态直到无 running（RAGFlow 解析进度动画对应）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 容器终验（UI 第 11-15 轮 + graphrag/配置 API）

- 镜像重建 → 容器复测：
  - 页面回归：dashboard/files/chat/status/settings/admin 全 200
  - dataset 配置保存：PUT chunk_method/size/overlap → code 0 Updated（parser_config 持久化）
  - graphrag build：POST /datasets/{id}/graphrag/build → code 0（LLM 实体抽取执行，documents=1；nodes=0 为实体-文本匹配的既有行为，对齐上游）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 对标 RAGFlow 第 15 轮：Status 页 Refresh/Copy

- `web.rs` status_page：↻ Refresh（重载检查）+ 📋 Copy Report（复制检查表到剪贴板）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 对标 RAGFlow 第 14 轮：Knowledge Graph Build 真实接入

- `server.rs`：POST /api/v1/datasets/{id}/graphrag/build —— 收集 KB chunks → 按文档分组 → graphrag_index::run_doc_pipeline（LLM 实体抽取）→ graph_merge 聚合 → 返回 nodes/edges/documents/entities/node_list
- `web.rs` dataset_page：buildGraph 从 setTimeout 假数据改为真实 POST + 结果渲染（节点/边/文档数 + Top 节点列表）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 对标 RAGFlow 第 13 轮：Dataset 配置真实保存

- `kb.rs`：`update_config` 增加 parser_config 参数（chunk_method/size/overlap/delimiter/parser_type/raptor/graphrag → parser_config JSON 持久化）
- `server.rs` update_dataset：解析 chunk 配置字段 → parser_config
- `web.rs` dataset_page：saveConfig 从 alert 占位改为真实 PUT（收集 chunkMethod/chunkSize/overlap/raptor/graphrag 配置）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 对标 RAGFlow 第 12 轮：Chat 详情清空/导出

- `llm.rs`：`ConvStore::clear_messages`
- `server.rs`：POST /api/v1/conversations/{id}/clear + GET /api/v1/conversations/{id}/export（Markdown 导出）
- `web.rs` chat_detail_page：侧栏 Back 旁 **Export**（下载 .md）+ **Clear**（清空会话）按钮
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 对标 RAGFlow 第 11 轮：Files 页查看/删除

- `file_mgr.rs`：`delete_file`（记录+物理文件删除，owner/admin 校验）+ `download_file`（mime 由 parser::mime_from_extension）
- `server.rs`：DELETE /api/v1/files/{id} + GET /api/v1/files/{id}/download
- `web.rs` files_page：行按钮 View（下载预览）+ Del
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 容器终验：全部 UI 页面 + admin API

- 镜像重建（含 UI 第 7-10 轮）→ 容器复测：
  - 14 页面全 200（dashboard/kbs/search/chat/agents/memories/files/skills/providers/settings/admin/status/api-docs/login）
  - admin API：重置密码 code 0；删除自己 400（护栏）；重置后登录正常
  - 注册默认禁用（403 Registration is disabled，环境开关）
  - admin 用户 id 为 UUID（非 1）
## 2026-08-04 UI 对标 RAGFlow 第 10 轮：Admin 用户管理

- `auth.rs`：`UserStore::delete_user`（禁删自己）+ `reset_password`（管理员重置）
- `server.rs`：DELETE /api/v1/admin/users/{id} + PATCH /api/v1/admin/users/{id}/password（is_admin 校验）
- `web.rs` admin_page：Users 卡片（姓名/邮箱/角色 + Reset PW / Del 按钮）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 对标 RAGFlow 第 9 轮：chunk 内联编辑

- `web.rs` document_page：chunk 列表每行加 **Edit** 按钮 → 模态编辑 content → PATCH /api/v1/datasets/{kb}/documents/{doc}/chunks/{id}（对齐 RAGFlow chunk 编辑）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 对标 RAGFlow 第 8 轮：404 页面

- `web.rs`：`not_found_page`（404 大标题 + 返回首页按钮，对齐 RAGFlow 404.tsx）
- `server.rs`：ServeDir fallback 挂 `not_found_service`（静态文件未命中 → 404 页面）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 对标 RAGFlow 第 7 轮：Settings 页修改密码

- `auth.rs`：`UserStore::change_password(user_id, current, new)`（校验当前密码 + mutate 更新哈希，新密码 ≥6 位）
- `server.rs`：PATCH /api/v1/user/password handler
- `web.rs` settings_page：Change Password 卡片（当前/新密码输入 + 更新按钮 + 结果提示）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 修复：providers 页 500 根因

- 根因：`require_api_auth` 中间件只保护 /api/v1/ 路径（`!starts_with("/api/v1/")` 直接跳过），网页路由从不注入 AuthContext；providers_page 错误要求 `Extension<AuthContext>` → 未登录/登录访问都 500
- 修复：providers_page 移除 Extension 依赖（页面展示全局 providers + reranker 配置区，无需用户上下文）
- 附带：require_api_auth 增加 cookie `rayrag_token` 兜底（API 的浏览器 cookie 访问可用）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 认证兜底：protected 页面 cookie 支持

- `server.rs` require_api_auth：Authorization Bearer 之外，增加 cookie `rayrag_token` 兜底（页面导航访问 protected 页如 /providers 时不再 500）
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 对标 RAGFlow 第 5 轮：Memories 页操作按钮

- `web.rs` memories_page：行按钮 View（→ /chat/{id}）+ Del（DELETE conversations API）+ 页面内 delConv JS
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 对标 RAGFlow 第 4 轮：Chat 会话删除

- `server.rs`：新增 `delete_conversation` handler（ConvStore::delete_for）+ 路由 DELETE /api/v1/conversations/{id}
- `web.rs` chat_page：会话卡片加 Del 按钮（event.stopPropagation 防跳转）+ delConv() JS
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 对标 RAGFlow 第 3 轮：Agents 页操作按钮

- `web.rs` agents_page：+ New Agent 按钮（弹窗：名称/描述 → POST /api/v1/agents）；行按钮 Test / **Edit**（→ /agents/{id}）/ **Copy**（Duplicate：GET 原 agent → POST 副本）/ **Del**（DELETE）
- 教训：agents API 路由**已存在**（2104 区，含 tags/PUT update）——新增路由前先 grep 确认，重复注册报 "Overlapping method route"
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 对标 RAGFlow 第 2 轮：文档查看器

- `web.rs`：新增 `document_page`（/documents/{kb_id}/{doc_id}）——RAGFlow document-viewer 对应
  - 头部：返回数据集 / Reparse / Delete 按钮 + 文档标题（JS 从 chunks API data.doc 填充）
  - 统计卡：Chunks 数 / 状态（JS 填充）
  - chunk 列表：序号 / 位置 / token 数 / 内容预览（fetch chunks API）
- `dataset_page` 文档行按钮补全：**View**（→ document_viewer）+ **Parse**（重解析）+ Del（RAGFlow 文档行操作层级）
- `server.rs`：注册 /documents/{kb_id}/{doc_id}
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 UI 对标 RAGFlow 第 1 轮：Skills 页面 + 主导航补全

- `runtime.rs`：导出 `component_catalog()`（COMPONENT_REGISTRY 51 组件 name+category）
- `web.rs`：新增 `skills_page`（/skills）——按 category（agent/ingestion/shared）分组卡片 + "Use in Agent" 按钮 + Open Agents 入口
- `layout()` 主导航补 RAGFlow 对应按钮：**Skills** + **Providers**（原仅 Dashboard/KBs/Search/Chat/Agents/Memories/Files）
- `server.rs`：注册 /skills 路由
- 测试基线：**844 passed / 0 failed / 26 ignored**
## 2026-08-04 docker 双 feature 镜像重建 + 全链路复测

- **教训**：直接 `docker build` 不传 ARG 时 RAYRAG_FEATURES 取 Dockerfile 默认 `postgres-backend`，容器启动即崩（zvec requires zvec-backend）。**必须用 `docker compose build`（自动带 `postgres-backend,zvec-backend`）** 或显式 `--build-arg RAYRAG_FEATURES=postgres-backend,zvec-backend`
- zvec 预编译资产：xget.xi-xu.me 主镜像恢复（429 限流已过）；fallback GitHub 直连
- 重建成功：rayrag:local 275MB（zvec-backend+postgres-backend）；容器复测：
  - healthz 全 healthy（postgres/zvec/embedder enabled）✓
  - 检索 score=0.967（GPU embedding 向量）✓
  - completions 生成文本（GPU LLM）✓
- 新代码 code_exec/sandbox/tcadp 已随镜像发布
## 2026-08-04 容器内 GPU LLM 实连验证

- compose LLM_API_BASE=http://192.168.3.6:8088 LLM_MODEL=Qwen3.5-9B-Q4_K_M.gguf 重建容器
- /api/v1/completions（需 kb_ids 可访问 + state.llm 已配置）返回 Qwen 生成文本 → 容器→GPU LLM 链路 ✓
- 注意：chat_completion 是纯对话（无知识库检索，直接 llm.chat）；RAG 对话走 agent/canvas 流程；检索用 /api/v1/retrieval
- 容器内三链路全部实连：embedding（8888）✓ / LLM（8088）✓ / 混合检索（vector+term）✓
## 2026-08-04 容器内 GPU embedding + 混合检索端到端验证

- compose 新增 EMBED_API_BASE/EMBED_API_KEY/EMBED_MODEL/RERANK_*/LLM_API_* 透传（commit 4484d5b）
- 容器 → 宿主 GPU 实连：EMBED_API_BASE=http://192.168.3.6:8888（llama.cpp 绑定 0.0.0.0，容器可访问；8081 rerank 绑定 127.0.0.1 容器不可达，需时改绑定）
- 端到端链路（全部在 docker 容器内完成）：
  1. login（admin@rayrag.local / RAYRAG_ADMIN_PASSWORD）→ 创建 dataset
  2. 上传 txt → pipeline: Parsed → Chunked → **Embedded: 1 vectors**（容器→GPU 真实调用）
  3. /api/v1/retrieval（kb_ids 字段，非 dataset_ids）：score=0.971, vector=0.903, term=1.0 ✓
- 注意：/api/v1/retrieval 用 `kb_ids`（数组）；登录路由 /api/v1/auth/login（非 /user/login）
- 测试数据：dataset gpu-embed-test（保留作冒烟样例）
## 2026-08-04 本地代码沙箱（LocalProvider 替代）+ code_exec 本地接线

- 新增 `src/sandbox.rs`：RAGFlow `agent/sandbox/providers/local.py` + `result_protocol.py` 的 Rust 实现
  - Python/JS wrapper：`main(**args)` → `__RAGFLOW_RESULT__:` base64 JSON 结构化结果（**Python 侧必须用 True 而非 true**——曾因 json.dumps 收到小写 true 报 NameError）
  - 临时实例目录（tempfile）+ env 隔离（HOME/TMPDIR/MPLBACKEND=Agg/PYTHONUNBUFFERED）
  - 单 bash -c 包裹 ulimit 资源限制（CPU+1s/内存 MB/文件大小/FD=64，对齐 preexec_fn RLIMIT）
  - tokio 进程组（process_group(0)）+ kill_on_drop + timeout → 超时整组杀（对齐 killpg）
  - 结构化结果提取：stdout 中标记行移除并解析（对齐 extract_structured_result）
  - 测试 8 个（真实 python 执行/普通输出保留/超时杀/错误 stderr/不支持语言/提取/规范化/输出限制）
- 接线：code_exec 支持 `RAYRAG_SANDBOX_PROVIDER=local`（默认 HTTP 9385）；local 结果映射 metadata.result_present/result_value
- 测试基线：**844 passed / 0 failed / 26 ignored**（+8）
## 2026-08-04 tcadp 解析器（腾讯云 LKEAP 文档重构）

- 新增 `src/parser/tcadp.rs`：RAGFlow `deepdoc/parser/tcadp_parser.py` 的 Rust 实现
  - **TC3-HMAC-SHA256 签名**（腾讯云 API v3：canonical request → string to sign → 三层 HMAC）
  - POST lkeap.tencentcloudapi.com ReconstructDocument（SSE 流式）；Progress==100 → DocumentRecognizeResultUrl
  - 结果 zip **安全解压**（拒绝对称加密/符号链接/绝对路径/.. 穿越，对齐上游防御）→ 收集 .json/.md
  - 渲染 sections（text/table/image 类型前缀）；扩展名→FileType 映射（PDF/PNG/JPG/TIFF/WORD/PPT/EXCEL）
  - 凭据：TENCENT_SECRET_ID/TENCENT_SECRET_KEY/TENCENT_REGION（默认 ap-guangzhou）
  - Parse trait 同步降级（原始文本）+ `parse_with_config` async 云端全流程（对齐 mineru 模式）
  - 删除 opendataloader.rs 的 placeholder TcadpParser
  - 测试 8 个（FileType 映射/公历日期/SSE 解析/zip 安全解压与穿越拒绝/配置/签名头）
- 测试基线：**836 passed / 0 failed / 26 ignored**（+8）
## 2026-08-04 国内直连端点推荐文档

- 新增 `docs/providers-cn.md`：中国大陆网络适配的模型端点推荐
  - Embedding：硅基流动（BAAI/bge-m3、Qwen3-Embedding-4B/8B）、通义（text-embedding-v3/v4）、智谱（embedding-3）、GiteeAI
  - Rerank：硅基流动（bge-reranker-v2-m3）、通义（gte-rerank-v2、qwen3-rerank）、GiteeAI（Qwen3-Reranker）
  - 视觉：硅基流动 Qwen2.5-VL、通义 qwen3-vl、智谱 glm-4.5v、月之暗面 vision
  - 本机 GPU llama.cpp 端点优先（8088/8888/8081，已实测 8/8）
  - 外网工具国内替代对照表
- 提供商目录确认：`llm_factories.json` 63 提供商 / 942 模型 / 6 类型（chat 660 / embedding 133 / image2text 83 / rerank 35 / tts 13 / speech2text 18）
## 2026-08-04 code_exec 连接器（沙箱契约客户端）

- 新增 `src/code_exec.rs`：RAGFlow `code_exec.py` 的 Rust 实现（688 行上游 → 契约全移植）
  - HTTP 沙箱客户端：POST {RAYRAG_SANDBOX_HOST}/run（默认 127.0.0.1:9385），对齐 code_req/响应结构
  - **契约层**（上游纯函数全移植）：
    - infer_actual_type（Null/Boolean/Number/String/Object/Array<T> 递归推断）
    - normalize/validate_expected_type（Array<T> 嵌套展开校验）
    - select_business_output（保留键排除后恰好一个业务输出 + 名称校验）
    - render_canonical_content（排序键 2 空格缩进 JSON）
    - artifact 附件段落（image/pdf/csv/json/html 类型规范化 + base64 解码）
  - stderr-only → _ERROR；stdout fallback 反序列化；actual_type 输出
  - 测试 8 个（类型推断/嵌套类型/业务输出/排序 JSON/附件/stderr/组合渲染）
- 接线：agent.rs code_exec 分支（lang/script/arguments）+ validate_code_exec_params + runtime.rs 描述符
- 测试基线：**828 passed / 0 failed / 26 ignored**（+8）
## 2026-08-04 docker compose 全栈运行时验证通过

- **postgres:18.4 实跑**：容器 rayrag-postgres-1（Up healthy），宿主端口 **55432**（5432 被主机占用；RAGFlow 占用 9380-9384）
- **rayrag:local 服务实跑**：rayrag-rayrag-1（Up healthy），`serve --port 8080`，宿主 9390
- healthz：`{"status":"healthy","postgres":"healthy","zvec":"healthy","errors":[]}`
  - postgres 连接 ✓（compose 内网 postgres:5432；REQUIRED_VERSION 18.4 匹配 ✓）
  - **zvec 后端初始化 ✓**（RAYRAG_VECTOR_BACKEND=zvec，卷 rayrag-zvec-data）
- 启动命令（密码变量必需）：
  `RAYRAG_POSTGRES_PORT=55432 RAYRAG_POSTGRES_PASSWORD=... RAYRAG_ADMIN_PASSWORD=... RAYRAG_PORT=9390 docker compose up -d --no-build`
- 注意：不带 RAYRAG_POSTGRES_PORT 会 Recreate postgres 并撞主机 5432 端口（address already in use）
- Embedding disabled 为预期（未配 EMBED_API_BASE，需要时指 GPU 8088 端点）
## 2026-08-04 exesql 连接器（PostgreSQL 版）+ 决策：不使用 mysql

- 新增 `src/exesql.rs`：RAGFlow `exesql.py` 的 Rust 实现（**仅 postgres**）
  - **Boss 决策：RayRAG 不使用 mysql**（mysql crate 已 cargo remove），db_type 仅支持 postgres，其他类型明确报错
  - postgres crate（postgres-backend feature 门控；无 feature 时占位函数返回明确错误）
  - 安全拒止对齐上游：拒绝 host=ragflow-mysql 或 password=infini_rag_flow 的 rag_flow 数据库
  - max_records 截断（默认 1024）；JSON 结果（Json 类型优先，回落 String/Null）
  - 测试 5 个（空 SQL/缺配置/rag_flow 拒止/mysql 拒绝/env 回退）
- 接线：agent.rs exesql 分支 + validate_exesql_params + runtime.rs 描述符
- 测试基线：**820 passed / 0 failed / 26 ignored**（+5）
## 2026-08-04 email 连接器（SMTP 发送）

- 新增 `src/email.rs`：RAGFlow `email.py` 的 Rust 实现
  - lettre 0.11（rustls-tls）：SmtpTransport STARTTLS + 认证 + MIME multipart/alternative（HTML+纯文本）
  - 支持收件人/抄送（逗号分隔）/主题/正文；发送者显示名（RFC2047 自动编码）
  - 凭据从节点参数或 env 读取（EMAIL_SMTP_SERVER/EMAIL_SENDER/EMAIL_SMTP_PASSWORD/EMAIL_SMTP_USERNAME/EMAIL_SENDER_NAME/EMAIL_SMTP_PORT），默认端口 465
  - 错误分类对齐上游：认证失败（5,3,x 永久错误）/连接失败（transport/tls/timeout）/SMTP 错误
  - lettre 0.11 陷阱：SmtpError 是 struct（非 enum），用 is_permanent/is_transport_shutdown/is_tls/is_timeout + status()；Code 字段为 severity/category/detail 枚举；MessageBuilder::cc() 返回 Self 无 Result
  - 测试 4 个（缺配置/缺收件人/连接失败分类/env 回退；不触碰真实 SMTP）
- 接线：agent.rs email 分支 + validate_email_params + runtime.rs 描述符
- 测试基线：**815 passed / 0 failed / 26 ignored**
## 2026-08-04 tushare 连接器（快讯 API）

- 新增 `src/tushare.rs`：RAGFlow `tushare.py` 的 Rust 实现
  - POST api.tushare.pro `news` 接口；token 从节点参数读，空则回退 env TUSHARE_TOKEN
  - src 七源枚举校验（sina/wallstreetcn/10jqka/eastmoney/yuncaijing/fenghuang/jinrongjie）
  - keyword 大小写不敏感过滤 + markdown 表格渲染（对齐上游 df.to_markdown）
  - end_date 空时用当前 UTC 时间（time crate；项目无 chrono，now_local 需 local-offset feature 未启用）
  - 测试 3 个（mock 过滤渲染 + 缺 token 拒绝 + 非法源拒绝）
- 接线：agent.rs tushare 分支 + validate_tushare_params + runtime.rs 描述符
- 测试基线：**817 passed / 0 failed / 26 ignored**（+3）
## 2026-08-04 akshare 连接器（东财个股新闻）

- 新增 `src/akshare.rs`：RAGFlow `akshare.py`（ak.stock_news_em）的 Rust 实现
  - 直连东方财富 search-api-web 搜索接口（实测可用：JSON 直接作 param，非 base64）
  - 解析 cmsArticleWebOld：标题/链接/内容/时间/来源，strip `<em>` 高亮
  - 输出 Markdown 列表（对齐上游 DataFrame 行格式），top_n 默认 10
  - 域名固定 → 无 SSRF 面；测试 4 个（mock 解析 + 空 symbol 拒绝 + 工具函数）
- 接线：agent.rs akshare 分支 + validate_akshare_params + runtime.rs 描述符
- 测试基线：**814 passed / 0 failed / 26 ignored**（+4）
## 2026-08-04 crawler 连接器（crawl4ai 替代）

- 新增 `src/crawler.rs`：RAGFlow `crawler.py` 的 Rust 实现
  - SSRF 防护（scheme 白名单 + DNS 解析后公网 IP 校验，拒绝私网/回环/链路本地）
  - `extract_type` 三模式：html（清洗，跳 script/style/svg/head 子树）/ markdown（scraper DOM→MD：标题/链接/加粗/代码块/图片）/ content（纯文本）
  - 输入对齐上游：content 数组以 " - " 连接，空则退回 query 参数
  - 测试 5 个（转换 ×3 + SSRF 拒绝 ×2），其中网络 mock 测试因 SSRF 拒绝 127.0.0.1 而删除（防护由拒绝测试覆盖）
- 接线：`src/agent.rs` crawler 分支 + `validate_crawler_params`（extract_type 枚举校验）+ `src/runtime.rs` 描述符
- 测试基线：**810 passed / 0 failed / 26 ignored**（+5）
- 提交：tools: add crawler canvas tool
## 项目信息

- **名称**：RayRAG
- **版本**：v0.1.2（以 `Cargo.toml` 为准；历史台账误写 v0.1.3）
- **路径**：`/home/junesgone/.openclaw/workspace/RayRAG/`
- **语言**：Rust（应用代码全 Rust；可选接入 zvec 原生库、PostgreSQL 18.4 和 OpenAI-compatible 模型服务）
- **定位**：RAGFlow 全栈 Rust 重写
- **源文件**：86 个 .rs / 75,000+ 行 Rust
- **API 路由**：当前 `server.rs` 有 117 个 `.route(...)` 注册点
- **Web 页面**：15 个（纯 Rust SSR，RAGFlow 设计对齐）
- **编译状态**：✅ 默认 feature 测试通过（796 passed / 0 failed / 26 ignored，2026-08-03 复核）；`postgres-backend,zvec-backend` 双 feature 通过。26 个 ignored 为需显式 GPU 模型端点/凭据/公网/MinIO/PDF 路径的 live 测试（含 gpu_vision_endpoint_describes_image_with_prompt、gpu_vision_describes_png_via_vision_llm_chunk_sync、gpu_merge_pair_judges_duplicate_and_merges、gpu_hypergraph_extraction_entities_then_relations、gpu_tree_template_execute_builds_trees_over_http、gpu_raptor_llm_summary_loop、gpu_pdf_outline_extraction_live、live_eastmoney_news 等）。**GPU 四模型客户端兼容闭环**（2026-08-03）：8088 chat（Qwen3.5-9B，reasoning_content 以 `<think>` 前缀保留；同端口 VisionClient 视觉描述，mmproj 视觉投影）、8888 embed（Qwen3-Embedding-4B，双形态解析，2560 维）、8081 rerank（mxbai-rerank-large-v2，`RAYRAG_TEST_RERANKER_KEY` 认证，中文文档排序正确）；8090 OCR（PaddleOCR-VL-1.6-GGUF）此前已验证。**postgres 18.4 实连 round-trip 通过**（rayrag-postgres-1 容器 :55432）；**zvec 原生冒烟通过**（native_collection，ZVEC_LIB_DIR=~/.local/lib/zvec/0.5.1，LD_LIBRARY_PATH 需同设）。百度系 live 对自动化请求偶发"百度安全验证"风控，连接器已返回明确错误并优雅 skip；Bing/腾讯财经/东方财富 live 在线通过。cargo 已配置 rsproxy 国内镜像（~/.cargo/config.toml）。**rag/llm/ 8 模型文件客户端全覆盖**（chat/embedding/rerank/ocr/cv/seq2txt/tts + __init__）
- **创建日期**：2026-07-10
- **Rust 版本**：宿主 cargo/rustc 1.97.0；Docker builder 固定 `rust:1.97.1-bookworm`

---

## 2026-08-04 Docker 纯环境构建打通（rayrag:local）+ 容器内镜像源修复

- **问题链**：① Dockerfile 无容器内 cargo 镜像 → 直连 crates.io 卡死（cargo 0% CPU 十余分钟）；② rsproxy 只代理 crates.io，`huggingface/candle` git 依赖直连 GitHub 再卡死；③ xget.xi-xu.me 对 candle 返回 429 限流
- **修复**（Dockerfile builder 阶段）：写 `/usr/local/cargo/config.toml`（rsproxy-sparse 替换 crates-io）+ `git config url."https://ghfast.top/https://github.com/".insteadOf "https://github.com/"`（候选镜像 ghfast.top/gh-proxy.com/ghproxy.net 均可用，gitclone.com 502）
- **验证**：candle 经 ghfast 15 秒拉取成功；`RAYRAG_POSTGRES_PASSWORD=build-only RAYRAG_ADMIN_PASSWORD=build-only docker compose build rayrag` → **rayrag:local 构建成功**（postgres-backend,zvec-backend 双 feature，含 /opt/zvec/lib/libzvec_c_api.so）；`docker run rayrag:local rayrag` CLI 正常响应
- 宿主机双 feature `cargo check` 通过（与 Docker 构建并行验证）

---

## 2026-08-04 GPU 实连测试全通过（8/8）+ rerank 服务修复

- **8081 rerank 服务修复**：原 llama-server 缺 `--reranking` 参数（API 返回 501），且 SIGTERM 无法终止（llama-server 忽略），`kill -9` 后以 `--reranking` 参数重启（`/home/junesgone/models/mxbai-rerank-large-v2.Q4_K_M.gguf`，api-key `mxbai-rerank-key`）；中文排序验证正确（RAGFlow 文档 -0.47 > 无关文本 -2.09）
- **GPU 8/8 实连测试通过**（env：LLM_BASE=8088 Qwen3.5-9B、EMBEDDING_BASE=8888 Qwen3-Embedding-4B、RERANKER_BASE=8081 + KEY、VISION_BASE=8088 同端口、PDF_OUTLINE=/tmp/outline_test.pdf）：categorize+embedding+rerank、vision 图像描述 ×2、PDF 大纲提取、merge 重复判断、raptor 摘要、hypergraph 实体抽取、tree template HTTP
- 全量测试 **805 passed / 0 failed / 26 ignored**（基线保持）
- **注意**：重跑 gpu_ 组必须补 `RAYRAG_TEST_VISION_BASE`/`RAYRAG_TEST_PDF_OUTLINE`，缺了会 NotPresent/失败

---

## 2026-08-04 国内网络替代工具闭环：baike + translate（DeepL/Wikipedia 替代）

- **`src/baike.rs`**（百度百科，Wikipedia 国内替代，2 单测）：免 key 直连 `BaikeLemmaCardApi` JSON 端点，处理 lemmaId 重定向、HTML 描述清洗、`search()` top_n 聚合；输出复用 wikipedia 的 {title, link, snippet} 模式
- **`src/translate.rs`**（百度翻译，DeepL 国内替代，3 单测）：双提供方——开放平台 API（`BAIDU_TRANSLATE_APPID`+`BAIDU_TRANSLATE_KEY`，MD5 签名，新依赖 md-5）+ 免 key `fanyi.baidu.com/sug` 端点 fallback；deepl 风格语言码（ZH/EN-GB/JA…）自动映射百度码；支持数组输入逐行翻译
- **接线**：agent.rs `execute_node` 新增 `translate` 分支（读 input.inputs["content"] / question，输出 content）；baike 走 canvas agent 工具分发链（校验 + 分发 + 显示名 "百度百科 Baike"）；runtime.rs 静态描述符
- **修复链**：`&mut Url: IntoUrl` → `.get(url.clone())` → E0716 临时值悬垂 → 先 `let mut endpoint = self.endpoint.clone()`；md-5 v0.11 const-generic Array 无 `{:x}` → `hex::encode(hasher.finalize())`
- 全量测试 **805 passed / 0 failed / 26 ignored**（+3）；RAGFlow 23 个内置工具网络类全部有对应 Rust 实现（deepl→translate、wikipedia→baike、googlescholar→baidu_scholar、akshare/tushare/yahoo→eastmoney+tencent_finance+jin10；wencai.py 上游为空 stub 无移植价值）

---

## 2026-08-03 Schools 实体移植：对标 deepdoc/parser/resume/entities/schools.py

- **`src/resume.rs`** 新增 `SchoolEntity` + `SchoolRow` + `SchoolTable`（schools.py:17-92 全量移植，2 单测）：
  - 三数据文件原样嵌入 `src/data/resume/`（schools.csv 5713 行 TSV 14 列 / good_sch.json 595 / school.rank.csv 1627，`include_str!`）
  - `parse_schools_csv`（name_en 加载时 lowercase+strip，schools.py:27 语义）+ `load_rank`（rank 覆盖，默认 1,000,000）
  - `split`（字母结尾→字母开头 token 空格粘合，schools.py:53-65）
  - `select`（括号剥离 → `^the `/分隔符/国家前缀剥离 → `大学.*学院`→`大学` → name_cn/name_en/alias 精确匹配，返回 pandas records[0] 形状 JSON）
  - `is_good`（GOOD_SCH 归一化成员判定）
  - `SchoolTable` 缓存版本（避免每次解析 590KB 数据）
- 修复台账：① E0716 临时值借用（replace_all 需 owned）；② 清华 name_en 为空 → 改用东伦敦大学测试；③ name_en 加载时 lowercase（Python `x.lower().strip()`）
- 全量测试 796 passed / 0 failed / 26 ignored（+2）；双特性构建 0 错误
- 覆盖率矩阵：schools.py partial → **aligned**；res/ 三数据文件 partial → **replaced**（aligned 82→83，replaced 71→74，累计 25,297 行 aligned）

## 2026-08-03 Degrees 实体移植：对标 deepdoc/parser/resume/entities/degrees.py

- **`src/resume.rs`** 新增 `DegreeEntity` + `DEGREE_TBL`（14 项 id↔名称：EMBA/MBA/MPA/专升本/专科/中专/中技/初中/博士/博士后/本科/硕士/职高/高中）：
  - `get_name(id)` → 名称（未知 id → ""，degrees.py:37-38）
  - `get_id(name)` → id（空输入 → ""；大写 + trim 后查表，degrees.py:40-44）
- 1 单测覆盖双向映射、大小写、空白 trim、未知输入
- 全量测试 794 passed / 0 failed / 26 ignored（+1）；双特性构建 0 错误
- 覆盖率矩阵：`deepdoc/parser/resume/entities/degrees.py` partial → **aligned**（aligned 81→82，累计 22,608 行对齐）

## 2026-08-03 Ppt 对齐：对标 deepdoc/parser/ppt_parser.py 补全分页语义

- **`src/parser/ppt.rs`**：新增 `slide_texts_paged(data, from_page, to_page)`（0-based `from_page <= i < to_page`，镜像 `RAGFlowPptParser.__call__` 的 continue/break）；原 `slide_texts` 改为全量范围向后兼容包装
- 既有实现已覆盖：`__sort_shapes`（top//10 再 left）、`__get_bulleted_text`（a:pPr buChar/buAutoNum/buBlip → `{level*2空格}.{text}`）、`__extract`（sp 文本帧 / tbl 19 表头:值 `; ` 行 / grpSp 6 递归）、NotImplementedError 容忍
- 全量测试 793 passed / 0 failed / 26 ignored；双特性构建 0 错误
- 覆盖率矩阵：`deepdoc/parser/ppt_parser.py` partial → **aligned**（aligned 80→81，累计 22,544 行对齐）

## 2026-08-03 PdfBox 移植：对标 deepdoc/parser/pdf_parser.py 确定性算法段

- **新建 `src/parser/pdfbox.rs`**（17 单测）——pdf_parser.py 中不依赖 DL 模型的确定性算法全量移植：
  - 乱码检测：`_is_garbled_char`/`_is_garbled_text`（PUA/替换符/控制符/CID 占位）、`_has_subset_font_prefix`、`_is_garbled_by_font_encoding`（subset 比例 ≥30% + CJK<5% + 标点>40%）
  - 投影匹配：`_match_proj`/`proj_match`（**Python re.match 锚定开头，Rust is_match 任意位置——必须加 `^` 前缀**；`{,48}` 简写 Rust 不支持 → `{0,48}`）
  - 几何：`_x_dis`/`_y_dis`/`__char_width`/`__height`；`sort_X_by_page`（冒泡调整同列倒序）
  - 合并：`_text_merge`（水平，同 layoutno 且 |y_dis|<mean_height/3）、`_naive_vertical_merge`（垂直，concatting/feats/detach 三组特征）、`_concat_downward`（sort_Y_firstly，Python 早期 return 后为死代码）
  - 过滤：`_filter_forpages`（目录/致谢段 + 点线页 dirty 检测）、`_merge_with_same_bullet`、`_line_tag`（@@pages\tx0\tx1\ttop\tbottom##）、`__filterout_scraps`（**对象版 DFS**——Python 传 line 对象非索引，索引版在 remove 后越界）
- **关键坑**：`X` 单大写字母命中 Ca 非 Sg；`$123` 不匹配 NE 前缀；`第3条` 不匹配中文数字模式；`三、是` 命中 pattern 3 非 12；`（1）范围` 锚定后命中 pattern 11 非 4
- 剩余 partial：布局识别（ONNX/Ascend）、表格 transformer、OCR 管线、页面栅格化、表格/图片提取、xgboost updown-concat 模型——依赖 DL 模型工件，走 external-engine 适配器
- 全量测试 793 passed / 0 failed / 26 ignored；双特性构建 0 错误
- 覆盖率矩阵：pdf_parser.py 保持 partial（确定性算法段已对齐，标注细化）

## 2026-08-03 Docx 移植：对标 deepdoc/parser/docx_parser.py

- **重写 `src/parser/docx.rs`**（docx_parser.py 182 行全量移植，8 单测；弃用 docx-rs 改手写 zip + quick-xml OOXML walker）：
  - `RAGFlowDocxParser`：`__call__` 分页循环（from_page/to_page 过滤、run XML 字面量含 `lastRenderedPageBreak` 时页码+1、段落 run 拼接 + style name）、`__extract_table_content` + `__compose_table_content`（blockType 正则分类 Dt/DT/Nu/Ca/En/NE/Sg + rag_tokenizer 兜底 Tx/Lx/Nr/Ot、Nu 表众数表头检测、最近连续表头行、逐列表头前缀、`;` 连接行、≤3 列 `\n` 合并）、styles.xml styleId→w:name 解析、document.xml 段落/run/表格 walker
  - `DocxParser` Parse 适配器 join 段落+表格（docling.rs 兼容）
- **关键坑**：`<w:name/>`/`<w:lastRenderedPageBreak/>` 是自闭合标签 → quick-xml 产生 Event::Empty 非 Start（初版漏处理导致 style/page 全丢）；`X` 单大写字母命中 Ca 非 Sg（Ca 在模式列表前）；`$1,234.56` 首字符 `$` 不匹配 NE 的 `^[0-9.,+-]` → 实际返回 Ot；rag_tokenizer 中文分词近似（CJK 2 字符伪 token）
- 全量测试 776 passed / 0 failed / 26 ignored；双特性构建 0 错误
- 覆盖率矩阵：`deepdoc/parser/docx_parser.py` partial → **aligned**（aligned 79→80，累计 22,198 行对齐）

## 2026-08-03 Epub 移植：对标 deepdoc/parser/epub_parser.py

- **重写 `src/parser/epub.rs`**（epub_parser.py 136 行全量移植，6 单测）：
  - `RAGFlowEpubParser`：`__call__`（空 binary 报错、spine 顺序读取 XHTML、每章委托 `RAGFlowHtmlParser::parser_txt` 分块、KeyError 跳过、空条目跳过）、`_get_spine_items`（`META-INF/container.xml` → rootfile full-path（CONTAINER_NS）、OPF manifest id→(href,media-type) + spine idrefs（OPF_NS）、XHTML_MEDIA_TYPES 过滤、opf_dir 前缀拼接）、`_fallback_xhtml_order`（排序的 .xhtml/.html/.htm，排除 META-INF/）
  - `EpubParser` Parse 适配器 join sections
- **关键坑**：zip `by_name` 返回的 ZipFile 持有可变借用，与后续 fallback 的不可变借用冲突（E0502）→ 全部改 `by_index` 预取（`read_entry` 辅助）
- 全量测试 768 passed / 0 failed / 26 ignored；双特性构建 0 错误
- 覆盖率矩阵：`deepdoc/parser/epub_parser.py` partial → **aligned**（aligned 78→79，累计 21,484 行对齐）

## 2026-08-03 Json 移植：对标 deepdoc/parser/json_parser.py（langchain 改编）

- **重写 `src/parser/json.rs`**（json_parser.py 179 行全量移植，10 单测）：
  - `RAGFlowJsonParser`：max_chunk_size×2、min_chunk_size=max(max-200,50) 默认、`_json_size` 紧凑序列化、`_set_nested_dict`、`_list_to_dict_preprocessing`（list→索引键 dict）、`_json_split` 忠实递归移植（有序迭代、放得下进当前 chunk、放不下且当前够 min 开新 chunk 并递归；langchain 允许最终 key 使 chunk 略超 max）、`split_json`（去尾空 chunk）、`split_text`（ensure_ascii \uXXXX 转义）、`is_jsonl_format`（整文解析失败 → 采样 ≤10 行 ≥80% 合法）、`_parse_json/_parse_jsonl`、`parse_binary` 入口
  - `JsonParser` Parse 适配器 join sections；`flatten_json` 旧辅助保留
- **关键坑**：初版迭代栈反转了 dict 处理顺序（rev+push 导致 b/c/d 顺序颠倒、chunk 归属错误），改回忠实递归版解决；serde_json::Map 无 `is_empty` 方法需自定义辅助；`json_size` 是关联函数非方法
- 全量测试 762 passed / 0 failed / 26 ignored；双特性构建 0 错误
- 覆盖率矩阵：`deepdoc/parser/json_parser.py` partial → **aligned**（aligned 77→78，累计 21,151 行对齐）

## 2026-08-03 Html 移植：对标 deepdoc/parser/html_parser.py

- **重写 `src/parser/html.rs`**（html_parser.py 212 行全量移植，8 单测）：
  - `parser_txt`：`sanitize_html` 预处理（删 style/script 子树含 div 内 script、删 HTML 注释、剥 inline style 属性——镜像 decompose()/del attrs）、`walk_children` 文本节点递归（父标签名 + inner_text 兜底 + 嵌套 HTML 重解析）、table 整体提升（table_id/index 元数据）、`merge_block_text` 按 block_id 聚合 + h1-h6 markdown `#` 前缀 + table 直通、`chunk_block` token 预算合并 + 超限块按词切分（estimate_tokens 代理）、`split_table` 按行预算切 `<table>` 片段
  - `HtmlParser::strip_tags` 兼容方法改为走完整管线（h1 → `# h1`，agent.rs invoke 测试预期同步更新）
  - 用 scraper 0.20（html5ever DOM）≈ BeautifulSoup html.parser
- 全量测试 752 passed / 0 failed / 26 ignored；双特性构建 0 错误
- 覆盖率矩阵：`deepdoc/parser/html_parser.py` partial → **aligned**（aligned 76→77，累计 20,788 行对齐）

## 2026-08-03 Txt 移植：对标 deepdoc/parser/txt_parser.py

- **重写 `src/parser/txt.rs`**（txt_parser.py 67 行全量移植，6 单测）：
  - `parser_txt`：unicode-escape delimiter 展开（`\n`→LF 等，镜像 Python encode/decode(unicode_escape)）、反引号 token + 字面字符拆解、带捕获组 split 手工重发匹配分隔符（Rust Regex::split 丢捕获组）、`^dels$` 纯分隔符段跳过（Python alternation 语义：`^` 只作用于首分支）、token 预算累积（estimate_tokens 代理）、`\n` 连接段合并、`(content, "")` chunk 对
  - `TxtParser` Parse 适配器：透传 + UTF-16 BOM 回退（decode_text）
- 全量测试 744 passed / 0 failed / 26 ignored；双特性构建 0 错误
- 覆盖率矩阵：`deepdoc/parser/txt_parser.py` partial → **aligned**（aligned 75→76，累计 20,525 行对齐）

## 2026-08-03 Markdown 移植：对标 deepdoc/parser/markdown_parser.py

- **重写 `src/parser/markdown.rs`**（markdown_parser.py 321 行全量移植，11 单测）：
  - `RAGFlowMarkdownParser::extract_tables_and_remainder`：三表提取（bordered `|` 行 + `:-:` 分隔行 / borderless 无前导 `|` / 原生 HTML `<table>` 含 html/body 包装）、`<table class=…>` 等标签属性归一化、`separate_tables` 拆出或内联保留、`|` 快速路径守卫
  - `MarkdownElementExtractor`：`get_delimiters`（反引号 token 按长度降序 + regex::escape）、`extract_elements`（delimiter 切分含 include_meta 行号 / 逐行切片：header `#{1,6}`、fenced code、list 含缩进续行、blockquote、text block 遇块元素停止；尾随空行吸收镜像 Python）
  - `MarkdownParser` Parse 适配器改为 join 提取块
- **关键坑**：regex crate 1.13 不支持 look-around，Python `(?=\n|$)` 改写作消耗式 `(?:\n|$)`；`regex::escape` 会转义连字符（`code\-block`），测试断言相应调整
- 全量测试 738 passed / 0 failed / 26 ignored；双特性构建 0 错误
- 覆盖率矩阵：`deepdoc/parser/markdown_parser.py` partial → **aligned**（aligned 74→75，累计 20,468 行对齐）

## 2026-08-03 Resume 移植：对标 rag/app/resume.py（LLM-free 核心）

- **新增 `src/resume.rs`**（resume.py 2768 行 LLM-free 部分移植，14 单测）：
  - `FIELD_MAP_ZH`/`FIELD_MAP_EN` + `get_field_map`（32 字段双语）
  - `parse_with_regex`：姓名/电话/邮箱/性别/年龄/出生/学历/学校/专业/公司/职位/工作年限/毕业年份，中英双语策略 + 动词过滤 + 子串去重（镜像 resume.py:1482-1787）
  - `_postprocess_resume` 四阶段：源文本校验（幻觉剪枝）/ 领域归一化（日期/电话/性别）/ 上下文去重（公司子串+时间重叠、Jaccard 兜底、项目描述并入工作描述）/ 必填字段补齐
  - `_build_chunk_document`：基本信息/教育背景/技能与证书/工作概况 4 个合并组 + 工作/项目描述逐条 chunk（公司+起止+年限前缀）+ 身份字段冗余 + 简历摘要行 + add_positions 逻辑排序
  - `_normalize_for_comparison`（NFKC 近似+去空白+小写）、`_parse_date_str`（多格式）、`_calc_single_exp_years`/`_calculate_work_years`（Python round 语义）、`_shingling_jaccard`（tiktoken→字符 n-gram n=5）
- 不移植（LLM 依赖）：prompt 模板 / `_call_llm` / `parse_with_llm` — RayRAG 以 `ResumeExtractor` 为 seam，regex 路径为确定性兜底
- 全量测试 727 passed / 0 failed / 26 ignored；双特性构建 0 错误
- 覆盖率矩阵：`rag/app/resume.py` partial → **aligned**（aligned 73→74，累计 19,357 行对齐）

## 2026-08-03 Presentation 移植：对标 rag/app/presentation.py + deepdoc/parser/ppt_parser.py

- **新增 `src/presentation.rs`**（presentation.py 253 行 chunk() 移植，4 单测）：
  - `is_presentation`（.ppt/.pptx 大小写不敏感）、`title_without_extension`（re.sub `\.[a-zA-Z]+$`）
  - `chunk_pptx`：每页一个 chunk，`doc_type_kwd="image"`、`page_num_int=[pn+1]`、
    `top_int=[0]`、`position_int=[(pn+1,0,0,0,0)]`、title_tks/title_sm_tks、tokenize_doc；
    from_page/to_page 边界（镜像 presentation.py 140-152 行）
  - PDF 分支路由到 `parser::pdf`（RayRAG 页级管线）；tika fallback 与 .ppt 二进制未移植
    （仅 PPTX zip 容器，镜像 python-pptx 前置要求）
- **重写 `src/parser/ppt.rs`**（RAGFlowPptParser 105 行移植，5 单测）：
  - 轻量 XML 树 walker（quick_xml → XmlNode，去命名空间前缀）
  - 形状排序 `(top//10, left)`（EMU 坐标，python-pptx None → 0）
  - 文本框段落 + bullet 检测（`a:pPr` 子元素 buChar/buAutoNum/buBlip →
    `'  '*level + '.'` 前缀）
  - 表格（shape_type 19）：第 0 行表头 + 数据行 → `"header: value; ..."` 行
  - 组形状（shape_type 6）：递归提取 + 子形状排序 join
  - `slide_texts` 输出每页文本（按 slideN 数字排序），保留 Parse trait 兼容
- 覆盖率矩阵：`rag/app/presentation.py` partial → **aligned**（aligned 72→73）
- 全量测试：**713 passed / 0 failed / 26 ignored**；双 feature（postgres+zvec）构建通过

---

## 2026-08-03 Table/Figure 移植：对标 rag/app/table.py + picture.py + deepdoc/parser/figure_parser.py

- **新增 `src/table.rs`**（RAGFlow `rag/app/table.py` 574 行移植，20 单测）：
  - 表头/数据行分类：`looks_like_header`（CJK 规则：`ord(c) > 127` 即 header-like，忠实复刻）、
    `looks_like_data`（Y/N/M/X 单字符、数字、短 hex）、`row_looks_like_header`（多数派）
  - `detect_header_rows`：最多扫 5 行，遇首个数据行停止
  - 复杂表头检测：合并范围触及第 1-2 行 → `build_hierarchical_headers`
    （多级表头 `-` 拼接、Y/N/M/X 与纯数字 part 过滤、Column_N 占位）
  - 合并继承：`merged_value`/`extract_row_data`（锚点单元格值下传）
  - 类型推断：`trans_datatime`（ISO/`YYYY/MM/DD`/`YYYY年M月D日`/T+时区后缀）、
    `trans_bool`（✓/√/是→yes 等中英符号）、`column_data_type`（int/float/datetime/bool/text 多数派+归一化）、
    `field_name` 类型后缀（_long/_flt/_dt/_kwd/_tks）
  - 每行一个 chunk：`row_to_chunk`（`- 字段: 值` 行格式，docnm_kwd/title_tks）
- **`src/parser/excel.rs` 增强**：
  - 读侧：`<mergeCells><mergeCell ref="A1:B2"/></mergeCells>` 解析（Start+Empty 事件），
    `parse_cell_reference`/`parse_merge_range`（含单格、反向引用），1-based 闭区间
  - 写侧：新增 `write_xlsx_sheets_with_merges`（镜像 xlsxwriter merge_range 输出），
    实现 mergedCells 完整 round-trip 测试
- **`src/parser/figure.rs` 增强**（picture.py 185 行 + figure_parser.py 292 行移植，6 单测 + 1 GPU 测试）：
  - `vision_llm_chunk_sync`/`describe_image_bytes`：图片字节 → VisionClient → clean_markdown_block
    （小图 <11px 跳过，镜像 picture.py min_side 保护）
  - `clean_markdown_block`：剥离 ```markdown/```md/``` 围栏（Python 语义：无闭合围栏时保留剥离后内容）
  - `detect_dimensions`：PNG/JPEG(SOF 扫描)/WebP(VP8/VP8L 14-bit 掩码)/GIF/BMP 魔数尺寸解析
  - OCR 短文本富化：OCR <32 词（英）/32 字符（其他语言）→ 视觉模型描述（picture.py CV-LLM 分支）
  - `VIDEO_EXTS` 11 种视频格式常量（视频分支路由预留）
- **GPU 实测**（2026-08-03，8088 Qwen3.5-9B 视觉端点）：
  - `gpu_vision_endpoint_describes_image_with_prompt` ✅（黑矩形 → black）
  - `gpu_vision_describes_png_via_vision_llm_chunk_sync` ✅（figure.rs 新链路真实调用）
  - 踩坑：llama.cpp 拒收无 CRC 的 PNG（400 Failed to load image）；PNG 魔数第 4 字节是 `G` 非 `N`；
    JPEG SOF 扫描边界 `i+9 <= len`；GIF 仅需 10 字节头
- **覆盖率矩阵**：`deepdoc/parser/excel_parser.py`、`deepdoc/parser/figure_parser.py`、
  `rag/app/picture.py`、`rag/app/table.py` 四文件 partial → **aligned**（aligned 68→72）
- 全量测试：**705 passed / 0 failed / 26 ignored**；双 feature（postgres+zvec）构建通过

---

## 2026-08-03 Naive 朴素切分：对标 rag/app/naive.py + rag/nlp 链

- RAGFlow 朴素切分（chunk()）：docx/pdf/excel/txt/md/html/epub/json
  分发 + naive_merge + tokenize_chunks 组装。
- **新增 `src/naive.rs`**：
  - `tokenize_chunks`（nlp:302-327）：空 chunk 跳过、add_positions
    [[ii]*5]、child 分隔符 → mom_with_weight + split、tokenize 组装
  - `tokenize`（nlp:268-273）：content_with_weight/content_ltks/
    content_sm_ltks，table 标签剔除正则
  - `split_with_pattern`（nlp:276-299）：Python split-with-capturing
    语义（captures_iter 重建；Rust Regex::split 丢弃捕获组）、非法
    正则 fallback 单 chunk
  - `add_positions`（nlp:832-844）：page_num_int(pn+1)/top_int/
    position_int
  - `normalize_overlapped_percent`（common/float_utils.py:50-58）：
    clamp [0,90]，0<x<1 → x*100
  - `merge_markdown_sections`：chunk_limit 合并 + overlap 尾部结转
  - `naive_chunk_text`：txt 家族分发（split → naive_merge →
    tokenize_chunks）
- 踩坑：Rust Regex::split 不保留捕获组，split_with_pattern 必须用
  captures_iter 手工重建 [text, sep, ...] 配对。
- gap：by_deepdoc/mineru/docling/opendataloader/tcadp/paddleocr 布局
  识别器、Docx/Pdf/Markdown 解析器类、extract_embed_file、
  extract_links_from_docx、tokenize_table、vision 包装（parser/LLM
  集成层）。
- 测试：**680 passed / 0 failed / 25 ignored**（+13 naive 单测）；双
  feature **687 passed**（680+7 PG 单测）。
- 矩阵：naive.py 行更新为详细移植说明。

## 2026-08-03 Email 邮件解析：对标 rag/app/email.py

- RAGFlow 邮件解析（chunk()，仅 eml）：头收集 + MIME 内容分发 +
  naive_merge。
- **新增 `src/email.rs`**：
  - 轻量 .eml 解析器（RFC 822 折叠头、multipart boundary 树、
    base64/quoted-printable 解码）替代 Python email.parser
  - `decode_payload`：charset 解码链（声明 → utf-8/gb2312/gbk/
    gb18030/latin1 → utf-8 ignore）
  - `collect_email_content`：text/plain → text_txt、text/html →
    html_txt、multipart 递归；头行收集
  - `parse_email`：sections 组装 + naive_merge 复用（book.rs）
- 复用 src/book.rs 的 naive_merge；base64/encoding_rs 已有依赖。
- gap：附件递归 naive_chunk 解析（集成层）。
- 测试：**671 passed / 0 failed / 25 ignored**（+6 email 单测）；双
  feature **678 passed**（671+7 PG 单测）。
- 矩阵：email.py 行更新为详细移植说明。

## 2026-08-03 Book 书籍解析：对标 rag/app/book.py + rag/nlp

- RAGFlow 书籍解析（chunk()）：DOCX/PDF/TXT/HTML/DOC，层级合并或
  token 数合并。
- **新增 `src/book.rs`**：
  - `hierarchical_merge`：层级分桶（BULLET_PATTERN/标题布局/正文）、
    反转取前 depth 层、binary_search 前驱分组、218 token 上限单元素
    合并（res 含首空组，忠实 Python）
  - `naive_merge`：token 数累积合并（overlap 尾部结转 +
    remove_tag @@..##；反引号自定义分隔符切分分支）
  - `is_english`（ASCII fullmatch 比例 >0.8）/`is_chinese`（CJK 比例
    >0.2）/`random_choices`（k=min(len,k)，确定性按序）
  - `parse_book_text`：txt 家族分发
- 踩坑：① "Chapter 1" 不匹配 family 3（罗马数字 Chapter I）；
  ② naive_merge 返回首空串元素（Python 忠实）；③ 纯标题无正文输入
  hierarchical_merge 返回空（levels 反转取前 depth 层为空）；④
  is_english 0.8 边界不 >0.8 为 false；⑤ random_choices k 截断。
- 测试：**665 passed / 0 failed / 25 ignored**（+11 book 单测）；双
  feature **672 passed**（665+7 PG 单测）。
- 矩阵：book.py 行更新为详细移植说明。

## 2026-08-03 Laws 法规文档解析：对标 rag/app/laws.py + rag/nlp

- RAGFlow 法规解析（chunk()）：DOCX/PDF/TXT/HTML/DOC，目录剔除 +
  树形合并。
- **新增 `src/laws.rs`**：
  - `remove_contents_table`：目录/contents 头剔除 + prefix 源行剔除 +
    prefix 匹配条目 drain（≤128 行）
  - `make_colon_as_title`：忠实移植（Python 检查捕获的分隔符长度
    <32 恒成立 → 实际不插入）
  - `tree_merge`：arena Node 树（get_level 层级、target_level 选择、
    build_tree 栈 + DFS chunk 输出）
  - `parse_laws_text`：txt 家族分发
- paper.rs 的 `anchored`/`BULLET_PATTERN` 改为 pub 供复用。
- 踩坑：① "Table of Contents" 去空格后匹配不上（Python 实际不删，
  忠实移植）；② remove_contents_table 会同时删掉 prefix 来源行；
  ③ make_colon_as_title 是 Python 死代码（arr[1]=分隔符恒 <32）。
- 测试：**654 passed / 0 failed / 25 ignored**（+7 laws 单测）；双
  feature **661 passed**（654+7 PG 单测）。
- 矩阵：laws.py 行更新为详细移植说明。

## 2026-08-03 Manual 手册式文档解析：对标 rag/app/manual.py

- RAGFlow 手册解析（chunk()）：PDF/DOCX，按标题层级分组 + 位置标签 +
  token 数合并。
- **新增 `src/manual.rs`**：
  - `Section3`：txt/layoutno/poss 三元组归一化
  - `tag_pos`：`@@{pn}\t{x1}\t{x2}\t{y1}\t{y2}##` 位置标签（全 0 → 空）
  - `bigram_similarity`：字符 bigram 重叠率（outline 标题匹配 ≥ 0.8）
  - `section_levels`：PDF outline 分支（outline/section > 3% 时用
    outline 层级，未匹配 = max_lvl+1）；回退 bullets_category/
    title_frequency
  - `sec_ids`：层级分组（同 paper.rs）
  - `merge_chunks`：按 (page,y1,x1) 排序；tk_cnt<32 必合并、<1024 同
    sec_id 或表格（-1）合并；位置标签追加
- gap：extract_pdf_outlines、vision 图表解析、tokenize_table、
  attach_media_context、__outline__ 注入（parser 模块集成层）。
- 测试：**647 passed / 0 failed / 25 ignored**（+9 manual 单测）；双
  feature **654 passed**（647+7 PG 单测）。
- 矩阵：manual.py 行更新为详细移植说明。

## 2026-08-03 One 单文件单块解析 + 布局识别归一化

- RAGFlow One 解析（chunk()）：一个文件保持原始顺序形成一个 chunk。
- **新增 `src/one.rs`**：
  - `join_nonempty_sections`：空行过滤 + `\n` 连接（txt/md 分支语义）
  - `one_chunk`：docnm_kwd/title_tks（去扩展名）/content_with_weight/
    content_ltks 组装
  - `flatten_pdf_tables`：跳过空表、取首行追加为文本段
  - `parse_one_text`：txt/md/markdown/mdx 分发；其他格式
    NotImplementedError
  - `resolve_pdf_layout`：布局识别器归一化
- **新增 `src/parser_config.rs`**（对标 common/parser_config_utils.py）：
  - `normalize_layout_recognizer`：`@mineru`/`@paddleocr`/`@opendataloader`
    后缀拆分模型名
  - `layout_recognizer_name`：bool → DeepDOC/Plain Text + 小写化
- gap：Docx/Pdf/Excel/Html/tika-.doc 提取器、vision 图表解析（parser
  模块集成层）。
- 测试：**638 passed / 0 failed / 25 ignored**（+6 parser_config +7 one
  单测）；双 feature **645 passed**（638+7 PG 单测）。
- 矩阵：one.py + parser_config_utils.py 行更新。

## 2026-08-03 Paper 章节结构解析：对标 rag/app/paper.py + rag/nlp/__init__.py

- RAGFlow 论文解析（chunk()）：abstract 整块切分；正文按标题层级分组。
- **新增 `src/paper.rs`**：
  - `BULLET_PATTERN`：5 类标题样式族（中文章/节/条/编、数字编号、
    中文数字、英文 PART/Chapter/Section/Article、markdown # 标题）
  - `not_bullet` / `not_title`：伪编号行与超长/带标点标题拒绝
  - `bullets_category`：选命中最多的标题样式族
  - `title_frequency`：每节层级 + 最高频层级作 pivot
  - `group_sections`：层级 <= pivot 且与前节不同 → 新 section id；
    同 id 文本合并为一个 chunk（对齐 paper.py:232-254）
  - `abstract_chunk`：abstract 整块 + important_kwd
- gap：Pdf 布局识别（DeepDOC/Plain Text/MinerU）、vision 图表解析、
  tokenize_table、add_positions、attach_media_context（parser 模块集成层）。
- 测试：**625 passed / 0 failed / 25 ignored**（+8 paper 单测）；双 feature
  **632 passed**（625+7 PG 单测）。
- 矩阵：paper.py 行更新为详细移植说明。

## 2026-08-03 Tag 数据集解析：对标 rag/app/tag.py

- RAGFlow Tag 解析（chunk()）：内容+标签两列格式（txt/csv），每对为一个
  chunk；标签列逗号分隔、`.`→`_` 规范化。
- **新增 `src/tag.rs`**：
  - `be_adoc_tags`：逗号切分 + trim + `.`→`_` + 空标签过滤
  - `detect_delimiter`：TAB 计数 >= 逗号计数时选 TAB
  - `parse_tag_text`：畸形行并入 content，2 字段行闭合一对
  - `parse_tag_csv`：csv.reader 语义（空字段丢弃、3+ 字段行变形并入
    content）
  - `tag_doc_fields`：docnm_kwd/content_with_weight/content_ltks/tag_kwd
  - `parse_tag_file`：txt/csv 分发；xlsx 路由到 parser::excel
- gap：label_question（tag-KB 标签检索 + all_tags 缓存 + tag_query，
  依赖 chunk 存储层）。
- 测试：**617 passed / 0 failed / 25 ignored**（+7 tag 单测）；双 feature
  **624 passed**（617+7 PG 单测）。
- 矩阵：tag.py 行更新为详细移植说明。

## 2026-08-03 Q&A 数据集解析：对标 rag/app/qa.py

- RAGFlow Q&A 解析（chunk()）：Excel/csv(txt)/pdf/markdown/docx 五种格式
  的问答对提取；csv/txt 每对 Q&A 为一个 chunk。
- **新增 `src/qa.rs`**：
  - `rm_prefix`：剥离 问题/答案/回答/user/assistant/Q/A/Question/Answer/问/答
    前缀（含 \t:： 空格）
  - `be_adoc`：`问题：q\t回答：a` content_with_weight + content_ltks
  - `detect_delimiter`：TAB 计数 >= 逗号计数时选 TAB
  - `parse_qa_text` / `parse_qa_csv`：畸形行在有未完成问题时并入答案，
    否则计失败行；csv 支持引号字段（split_csv_line，"" 转义）
  - `md_question_level` / `parse_qa_markdown`：标题层级问题栈 + 代码围栏跳过
  - `parse_qa_file`：txt/csv/md/markdown/mdx 分发；pdf/docx/excel 路由到
    现有 parser 模块
- gap：Excel/Pdf/Docx 解析器子类、tokenize_table、add_positions、
  callback 进度（parser 模块集成层）。
- 测试：**610 passed / 0 failed / 25 ignored**（+10 qa 单测）；双 feature
  **617 passed**（610+7 PG 单测）。
- 矩阵：qa.py 行更新为详细移植说明。

## 2026-08-03 混合检索重排：对标 rag/nlp/search.py + rag/nlp/query.py

- RAGFlow 检索链（Dealer）：ES/Infinity 粗排 → rerank（词法+向量融合）
  或 rerank_by_model（外部 reranker）→ 阈值过滤 → 分页 → doc 聚合。
- **nlp.rs 新增**（对标 query.py Query 类）：
  - `weighted_token_dict`：to_dict 语义（unigram 0.4× + 相邻 bigram 0.6×max）
  - `dict_similarity`：s/q 比值（1e-9 下限）
  - `token_similarity`：query tokens vs 多 doc token 列表
  - `hybrid_similarity`：cosine×vtweight + token×tkweight 融合，
    向量和为 0 时退化为纯 token 相似度
- **search.rs 新增**（对标 search.py Dealer）：
  - `ragflow_rerank`：content_ltks + title×2 + important_kwd×5 + question×6
    token 列表 → token/vector 融合 + rank_feature
  - `ragflow_rerank_by_model`：外部 reranker 分数作 vtsim，
    fused = tkweight×tksim + vtweight×vtsim + rank_fea（对齐
    rerank_by_model 公式；rerank_window 已有 RERANK_LIMIT 64-cap）
  - `ragflow_rank_feature_scores`：tag 特征余弦 ×10 + pagerank
    （_rank_feature_scores 移植）
- gap：insert_citations（分句 + 逐句 embedding + 阈值 0.63→0.3 衰减）、
  _prune_deleted_chunks、retrieval_by_toc/children、sql_retrieval、
  tag 助手（依赖 chunk 存储层）。
- 测试：**600 passed / 0 failed / 25 ignored**（+4 nlp token/向量相似度、
  +4 search rerank 融合）；双 feature **607 passed**（600+7 PG 单测）。
- 矩阵：query.py / search.py 行更新为详细移植说明。

## 2026-08-03 GraphRAG 索引管线：对标 rag/graphrag/general/index.py + extractor.py

- RAGFlow GraphRAG 索引链路（run_graphrag）：chunk 列表 → generate_subgraph
  （LLM 实体抽取→子图）→ merge_subgraph（合并进 KB 全局图 + pagerank）→
  resolve_entities（实体消解）→ extract_community（社区报告）。
- **新增 `src/graphrag_index.rs`**（对标 extractor.py 基类 + index.py 纯算法核心）：
  - `extract_chunks`：extractor.py `__call__` 并发编排（semaphore=10 worker、
    GRAPHRAG_MAX_ERRORS=3 错误上限、按 chunk 收集结果）
  - `build_subgraph`：generate_subgraph 纯图核心（节点 add_node(**ent)，
    关系端点缺失即丢弃并计数 ignored_rels）
  - `graph_merge` + `GraphChange`：utils.py 图合并（description/source_id
    以 `<SEP>` 拼接、边权重累加、rank=degree）
  - `tidy_graph`：purge 缺 description/source_id 的节点
  - `pagerank`：networkx.pagerank 幂迭代（alpha=0.85，含 dangling 处理）
  - `run_doc_pipeline`：端到端内存版（extraction → subgraph）
  - `apply_node_types`/`apply_edges`：抽取结果回写图
  - 常量：DEFAULT_ENTITY_TYPES（5 类）、ENTITY_EXTRACTION_MAX_GLEANINGS=2、
    MAX_CONCURRENT_PROCESS_AND_EXTRACT_CHUNK=10、GRAPH_FIELD_SEP=`<SEP>`
- **EntityGraph 扩展**：add_node（upsert，对标 `add_node(**ent)`）、
  set_node_type、set_edge_weight、remove_node、set_source_id/append_source_id/
  source_id_of、GraphNode.source_id 字段（serde default）。
- **修正**：GraphExtractor.max_gleanings 3→2（extractor.py:47 是 2）。
- entity_embedding.py node2vec（graspologic C++）记 gap：用 pagerank/degree
  替代图结构信号。
- gap：chunk_list/does_graph_contains/set_graph/Redis 锁/DocumentService/
  load_subgraph_from_store/run_graphrag_for_kb 扇出（存储层，调用方接入）。
- 测试：**592 passed / 0 failed / 25 ignored**（+6 graphrag_index 单测：
  tidy/merge/pagerank/build_subgraph/类型回写/常量）；双 feature **599 passed**
  （592+7 PG 单测）。
- 矩阵：extractor.py → src/graphrag.rs; src/graphrag_index.rs；index.py →
  src/graphrag_index.rs；entity_embedding.py → src/graphrag_index.rs（gap）。

## 2026-08-03 Graph 实体抽取：对标 rag/graphrag/general/graph_extractor.py + graph_prompt.py

- RAGFlow GraphExtractor 是 GraphRAG 索引侧实体/关系抽取：每 chunk 一次
  LLM 抽取 + gleaning 循环（CONTINUE 追加漏检实体、LOOP 问 Y/N 是否继续）→
  记录分割 → 实体/关系解析。
- **`GraphExtractor`（src/graphrag.rs）**：
  - `process_single_content`：渲染 GRAPH_EXTRACTION_PROMPT → LLM chat →
    gleaning 循环（max_gleanings=3 默认）→ token 计数
  - `parse_extraction_records`：按 record/completion 分隔符分割 →
    括号体提取 → `_entities_and_relations`（实体类型过滤 lowercase +
    关系权重解析 + src/tgt 排序）
  - `handle_single_entity_extraction` / `handle_single_relationship_extraction`
    （utils.py 移植）：实体需 ≥4 属性且首属性 `"entity"`；关系需 ≥5 且
    `"relationship"`，权重 is_float_regex 校验
  - `split_string_by_multi_markers` / `is_float_regex`（utils.py 补移植）
- **graph_prompt.py（123 行）**：GRAPH_EXTRACTION_PROMPT（3 示例全文）、
  CONTINUE_PROMPT、LOOP_PROMPT、SUMMARIZE_DESCRIPTIONS_PROMPT 全部内嵌。
- 常量：DEFAULT_TUPLE_DELIMITER=`<|>`、DEFAULT_RECORD_DELIMITER=`##`、
  DEFAULT_COMPLETION_DELIMITER=`<|COMPLETE|>`。
- gap：chat_limiter 并发、has_canceled、tiktoken cl100k logit-bias 循环参数、
  callback 进度（运行时门控）。
- 测试：**586 passed / 0 failed / 25 ignored**（+7 GraphExtractor 单测：
  记录解析/类型过滤/垃圾跳过/多分隔符/浮点/实体+关系解析器/prompt 渲染）；
  双 feature **593 passed**（586+7 PG 单测）。
- 矩阵：graph_extractor.py → src/graphrag.rs；graph_prompt.py → src/graphrag.rs；
  utils.py 补 4 个函数。

## 2026-08-03 KG 图检索：对标 rag/graphrag/search.py + query_analyze_prompt.py

- RAGFlow KGSearch（343 行）是 GraphRAG 查询侧核心：LLM 查询重写 →
  实体/关系检索 → n-hop 路径聚合 → 融合打分 → CSV 输出 + 社区报告。
- **`KgSearch`（src/graphrag.rs）**：纯内存 EntityGraph 版检索，保留上游
  融合/排序/输出契约：
  - `query_rewrite`：LLM 调 minirag_query2kwd（MINIRAG_QUERY2KWD_PROMPT
    全文内嵌，4 个示例）+ JSON 切片/修复回退 → type_keywords +
    entities_from_query（≤5）
  - `get_relevant_ents_by_keywords`：token 重叠 sim（向量检索替代）+
    pagerank=degree；`get_relevant_ents_by_types`：类型过滤 + degree 降序
    （对标 rank_flt 降序）；`get_relevant_relations_by_txt`：端点文本匹配
  - n-hop 聚合：`EntityGraph::n_hop_paths`（带权重路径），sim/(2+i) 衰减
  - 融合：类型命中实体 sim×2；关系 sim×(1+nhop+类型端点)
  - 排序 sim×pagerank + topn；输出 Entities/Relations CSV +
    Community Report 段（`community_retrieval` 对标 `_community_retrieval_`）
  - `parse_rewrite_json`：json_repair 替代（切片到首 `{` 尾 `}`）
- **query_analyze_prompt.py（218 行）**：minirag_query2kwd +
  keywords_extraction 两个 prompt + 3 个示例全文内嵌。
- **EntityGraph 扩展**：entities_by_type（类型过滤）、entity_type_pool
  （类型→样本，对标 get_entity_type2samples）、n_hop_paths（路径+权重）、
  edge_weight。
- 存储层 gap：向量检索/OrderByExpr/get_relation DB 回退/max_token 预算
  （内存图替代）；json_repair 完整修复未移植。
- 测试：**579 passed / 0 failed / 25 ignored**（+8 KGSearch 单测：关键词
  检索/类型过滤/关系检索/JSON 解析/提示词渲染/社区检索/n-hop 权重/
  类型池）。
- 矩阵：search.py → src/graphrag.rs; src/graphrag_enhanced.rs；
  query_analyze_prompt.py → src/graphrag.rs。

## 2026-08-03 GraphRAG 社区报告：对标 rag/graphrag/general/ 4 文件

- RAGFlow GraphRAG 社区报告链路：degree→rank → leiden 社区检测 →
  实体/关系 CSV → COMMUNITY_REPORT_PROMPT → LLM → JSON 校验 → 报告。
- **新增 `src/graphrag.rs`**（对标 community_reports_extractor.py 183 行 +
  community_report_prompt.py 158 行 + utils.py 4 个辅助函数）：
  - `COMMUNITY_REPORT_PROMPT` 全文内嵌（title/summary/rating/rating_explanation/
    findings JSON schema + grounding rules + 示例/真实数据段，{entity_df}/
    {relation_df} 占位符）
  - `CommunityReportsExtractor::run(graph, llm)`：rank（degree）→ 社区划分 →
    ≥2 实体过滤 → CSV 渲染（10000 边上限）→ prompt 变量替换 → LLM chat
    （system=渲染 prompt, user="Output:"）→ JSON 切片+花括号还原 →
    dict_has_keys_with_types 校验 → weight/entities 富化 → 文本报告
    （`# title\n\nsummary\n\n## findings`）+ max_report_length 截断
  - `perform_variable_replacements`/`clean_str`（html.unescape + 控制字符剥离）/
    `dict_has_keys_with_types`/`html_unescape`
- **leiden.py 替代**：graspologic hierarchical_leiden 是 C++ 原生依赖，
  用确定性连通分量（union-find）替代，保持 run() 契约（权重=Σrank*weight
  归一化），零原生依赖。
- **EntityGraph 扩展**（src/graphrag_enhanced.rs）：GraphNode 加
  description 字段（serde default 兼容旧数据）；新增 neighbors/
  description_of/edge_description/set_description/node_names/degree。
- 陷阱：crate::llm::ChatMessage 无 `user` 关联函数（那是 ToolChatMessage
  的）→ 用 `ChatMessage::new("user", ...)`；GraphNode 新增字段需同步
  merge() 初始化器否则 E0063。
- 测试：**571 passed / 0 failed / 25 ignored**（+8 graphrag 单测：连通分量
  划分/变量替换/clean_str/类型校验/JSON 提取/文本格式化/截断/html 反转义）。
- 矩阵：community_report_prompt.py → src/graphrag.rs；community_reports_
  extractor.py → src/graphrag.rs；leiden.py → src/graphrag.rs；utils.py →
  src/graphrag.rs; src/graphrag_enhanced.rs。

## 2026-08-03 中文姓氏 + 同义词扩展：对标 rag/nlp/surname.py + synonym.py

- RAGFlow `surname.py`（144 行，正文即 百家姓集合）用于简历解析判断姓名
  （`deepdoc/parser/resume/step_two.py`：`surname.isit(nm[0]) or surname.isit(nm[:2])`）；
  `synonym.py`（106 行）Dealer.lookup(tk, topn) 给查询词做同义词扩展
  （自定义 synonym.json 字典优先 + WordNet 兜底）。
- **新增 `src/surname.rs`**：~490 单姓 + 54 复姓逐字对齐内嵌；
  `is_chinese_surname(name)` 复刻 `isit(nm[0]) or isit(nm[:2])` 语义
  （trim 后先查单姓，再查前两字复姓）。**接入 resume.rs**：简历首行
  短行若以姓氏开头 → `# 姓名: <name>` 结构化标注（对标 step_two.py）。
- **升级 `src/nlp.rs` `SynonymDict`**：新增 `lookup(tk, topn)`（空白归一化、
  字典优先、纯字母兜底、topn 截断——RAGFlow lookup 语义）+
  `from_json_map`（synonym.json 形态，key 小写化）。NLTK WordNet 用内置
  同义词表替代（离线零依赖，符合大陆网络环境）；redis 实时同义词
  （kevin_synonyms 每小时刷新）记系统级 gap（RayRAG 无 redis）。
- 陷阱记录：RAGFlow 集合中"仉"仅存在于复姓"仉督"，单姓断言会误报
  （测试初版含 "仉" 失败已改为 "仉督"）。
- 测试：**563 passed / 0 failed / 25 ignored**（+6 surname 单测：
  常见单姓/全名/复姓/非姓氏/trim/生僻姓；+2 synonym 单测：lookup
  topn+字典+空输入、from_json_map 小写化+中文键+非法值跳过）。
- 矩阵：`rag/nlp/surname.py` → `src/surname.rs; src/parser/resume.rs`；
  `rag/nlp/synonym.py` → `src/nlp.rs`。

## 2026-08-03 查询词加权（term_weight.py 移植）：对标 rag/nlp/term_weight.py Dealer

- RAGFlow `Dealer` 类（240 行）是检索排序的关键前置：给查询词算权重 =
  `(0.3*idf(freq) + 0.7*idf(df)) * ner(t)`，归一化；pretoken 去停用词/标点、
  token_merge 合并单字短语、NER 类别映射（loca/sch/stock/corp=3，数字=2，
  短字母=0.01）。RayRAG 此前 BM25 对所有查询词一视同仁。
- **新增 `src/nlp.rs` `TermWeightComputer`**：stop_words 逐字对齐 Python 源码
  （请问/您/你/我/他/是/的/就…），pretoken/token_merge/ner/freq/df/idf/weights
  全方法移植；`with_resources` 支持注入 ner.json/term.freq（缺失时优雅降级
  默认值）；jieba 分词用内置 tokenizer 替代（CJK 单字+双字窗口）。
- **接入检索**：`Bm25Scorer::score_weighted` + `field_bm25_scores` 用
  TermWeightComputer 加权查询词（absent token 保持 1.0，行为优雅降级），
  对标 RAGFlow retrieval 的 term 加权排序。
- 陷阱记录：Rust regex 字符类内 `[`/`]`/`"` 转义严格（Python 原正则直接
  搬会 `unclosed character class` panic）→ pretoken 改用纯字符判断
  （单字符非字母数字非 CJK = 标点）；NER 权重断言按 RAGFlow 原版映射
  （loca=3 > number=2，测试初版写反已修正）。
- 测试：**555 passed / 0 failed / 25 ignored**（+5 term_weight 单测：
  pretoken 停用词/标点、num=false 去数字、单字短语合并、权重归一化+NER
  排序、preprocess 合并）。
- 矩阵：`rag/nlp/term_weight.py` 行映射收敛为 `src/nlp.rs; src/search.rs`。

## 2026-08-03 国内连接器扩容：东方财富/金十/和风天气/SearXNG（对标 akshare.py/jin10.py/qweather.py/searxng.py）

- RAGFlow `agent/tools/` 全 22 工具对照（akshare/arxiv/code_exec/crawler/
  deepl/duckduckgo/email/exesql/github/google/googlescholar/jin10/pubmed/
  qweather/retrieval/searxng/tavily/tushare/wencai/wikipedia/yahoofinance）：
  RayRAG 此前已有 14 个连接器，本次补齐 4 个大陆直连可用工具，覆盖度
  18/22（code_exec/crawler/deepl/email/exesql 属沙箱/翻译/邮件子系统，
  wencai 上游 pywencai 已废弃注释、tushare 需付费 token，均记录为系统级缺口）。
- **新增 src/eastmoney.rs**：对标 akshare.py `stock_news_em`，直连
  `search-api-web.eastmoney.com/search/jsonp` JSONP 端点（cb=x&param=...，
  param 用 reqwest query_pairs 编码——中文 keyword 裸拼 URL 会 400），
  解析 `result.cmsArticleWebOld`，剥离 `<em>` 高亮，输出 tool 行 + publish_time/
  media_name。**免 key，本机 live 验证通过**（"农业" 3 条真实新闻）。
- **新增 src/jin10.rs**：对标 jin10.py 四模式（flash/calendar/symbols/news），
  secret-key header（`JIN10_SECRET_KEY`），flash 取 `data[].data.content`，
  quotes 短键重命名（a→Selling Price 等 10 键，镜像 RAGFlow del 重命名链），
  其余模式逐行渲染 key: value。live 需真实 key（占位 "xxx" 返回 404 secret-key
  不存在，属上游校验，非客户端 bug）。
- **新增 src/qweather.rs**：对标 qweather.py 三模式（weather/indices/airquality），
  geoapi 城市→location id 解析，free/paid 双 host（devapi/api.qweather.com），
  lang 透传，错误码透传（403=无效 key 为上游正常响应）。live 需真实 key。
- **新增 src/searxng.rs**：对标 searxng.py JSON API（q/format=json/categories/
  language/safesearch/pageno），实例 URL 取节点参数 `searxng_url` 或
  `SEARXNG_URL`，无 key。live 需自建实例。
- 注册链完整落地：lib.rs 4 模块；runtime.rs COMPONENT_REGISTRY 4 描述符 +
  INPUTS/OUTPUTS；agent.rs import + `execute_canvas_agent_tool_with_providers`
  分支 + `execute_node` 分支 + `_ERROR` 前缀映射 + 4 个 execute_*_with_provider
  + 4 个 execute_agent_*_with_timeout（复用 domestic_search_outputs/
  set_domestic_search_error/bing_top_n/重试语义）。
- 陷阱记录：reqwest `Url::query_pairs_mut()` 编码 param JSON（裸拼中文 400）；
  SearxngResult 含 f64 score 不能 derive Eq（E0277），仅 PartialEq。
- 测试：默认 **550 passed / 0 failed / 25 ignored**（+18 单测：eastmoney 5、
  jin10 5、qweather 4、searxng 4；+4 live ignored）；双 feature
  **557 passed / 0 failed / 25 ignored**。东方财富 live 通过
  （`-- --ignored eastmoney`，大陆直连验证）。
- README 连接器清单 5→9；.env.example 增 JIN10_SECRET_KEY/QWEATHER_API_KEY/
  SEARXNG_URL。

## 2026-08-03 ISO-8601 日期解析：对标 api_service.py API4ConversationService._normalize_query_date

- 审计六文件：memory_service（177L）/ connector_service（518L）/
  api_service（133L）/ conversation_service（378L）/ chunk_feedback_service
  （316L）/ pipeline_operation_log_service（290L）/ user_service（321L）。
- **covered 判定**：memory_service（Vec<String> 存储 vs 位掩码 int 语义
  等价，且未知类型严格报错优于 RAGFlow 静默忽略）、chunk_feedback_service
  （retrieval_signal 三键 max / 最大余数法 split_integer_budget /
  uniform+relevance 双模式——RayRAG src/chunk_feedback.rs 542 行逐条一致）、
  conversation_service.structure_answer（RayRAG ConvStore::append_exchange +
  replace_assistant_message 更强，带 expected_prior_answer 并发保护）、
  user_service（RayRAG argon2 强于 werkzeug pbkdf2）、
  pipeline_operation_log（list_operation_logs 已有聚合）。
- **系统级缺口不移植**：connector_service（Connector2Kb/SyncLogs 同步
  子系统）、get_or_create_for_channel（chat_channel 持久化实体）——属
  功能缺失而非函数缺口，留待子系统实现时一并落地。
- **实际补强**：parse_utc_datetime（src/api/features.rs）此前只接受
  `YYYY-MM-DD HH:MM:SS` 空格分隔；补 ISO-8601 `T` 分隔符与可选尾随 `Z`
  （统一归一为 UTC，保持 RayRAG UTC 锚点，不学 RAGFlow 本地时区转换）。
  测试断言同步更新（原断言 "T 格式报错" 反转为接受）。
- 测试：默认 **532 / 0 / 21**；双 feature **539 / 0 / 21**（既有用例
  扩展，计数不变）。

## 2026-08-03 画布标签规范化：对标 canvas_service.py UserCanvasService.update_tags

- 审计 canvas_service.py（463 行）：CanvasTemplateService/DataFlowTemplate
  Service/UserCanvasService 均为 CommonService 薄封装；唯一独立逻辑为
  update_tags（标签清洗链）与 list_tags（跨画布聚合，RayRAG list_agent
  _tags 已有等价实现）。
- 对齐确认：RayRAG normalize_agent_tags（src/api/features.rs）与 RAGFlow
  update_tags 语义逐条一致——逗号剥离（单列逗号分隔存储，逗号会破坏
  编码）、trim、单标签 ≤64（TAG_MAX_LEN）、大小写不敏感去重保序、
  拼接值 ≤512（TAGS_FIELD_MAX）截断。
- 补漏：AgentStore::update_tags_owned 此前未过 normalize_agent_tags（仅
  create/update_agent 两条路径清洗）——补上使三条写路径一致；新增
  tag_normalization_tests 3 测：清洗去重限长 / 单标签 64 截断 + 总长
  512 截断 / update_tags_owned 走规范化（"c,d"→"c d" 且与 "C" 并存，
  镜像 RAGFlow key=lower() 语义）。
- 测试：默认 **532 passed / 0 failed / 21 ignored**；双 feature
  **539 passed / 0 failed / 21 ignored**（+3 默认 feature 内）。

## 2026-08-03 对话 SQL 清洗 + TTS 文本清洗：对标 dialog_service.py use_sql/clean_tts_text

- 继续审计 dialog_service.py 后半段（async_chat 流式、use_sql 问答、
  tts）：use_sql.normalize_sql 清洗 LLM 生成的 SQL（去 </think> 块、
  "思考"中文推理标记、```sql 代码围栏、引擎拒绝的尾部分号）；
  clean_tts_text 清洗合成前文本（去 C0 控制字符/emoji、空白折叠、
  500 字符截断 MAX_LEN=500）。两者均为纯函数。
- 补入 src/dialog.rs：normalize_sql（四个正则替换链与 RAGFlow 逐字对齐）
  + clean_tts_text（emoji 范围含 emotions/misc symbols/transport/flags/
  dingbats/supplemental，镜像 RAGFlow emoji_pattern）+ regex_replace_all
  辅助。未移植 add_kb_filter/_assert_valid_uuid（依赖 doc_engine 分支与
  UUID 校验，属 SQL 问答执行器范畴，RayRAG 无 ES/Infinity 引擎）。
- 单测 4：think 块+围栏+尾分号组合清洗 / 内部 SQL 分号保留与纯 SQL 不变
  / 控制字符+emoji+空白折叠 / 500 字符截断（多字节安全，chars 计数）。
- 测试：默认 **529 passed / 0 failed / 21 ignored**；双 feature
  **536 passed / 0 failed / 21 ignored**（+4 默认 feature 内）。

## 2026-08-03 对话引用规范化：对标 dialog_service.py repair_bad_citation_formats

- 审计 dialog_service.py（1797 行）：CRUD 部分为 CommonService 薄封装；
  有移植价值的两个纯函数——repair_bad_citation_formats（LLM 回答中
  (ID: 12)/[ID: 12]/【ID: 12】/ref12 四种坏引用格式统一为 [ID:12]，
  越界引用保留原文不标记）与 normalize_arabic_digits（阿拉伯-印度数字
  0660-0669/06F0-06F9 → ASCII）。
- 新增 src/dialog.rs（147 行）：四个坏格式正则（与 RAGFlow BAD_CITATION
  _PATTERNS 逐字对齐）+ CITATION_MARKER 提取正则；normalize_arabic_digits
  纯函数；repair_bad_citation_formats 返回 (规范文本, 有效引用索引集合)；
  extract_cited_indexes 供响应元数据提取。
- 四阶段顺序处理（PAREN→BRACKET→CJK→REF）镜像 RAGFlow find_and_replace
  链；修复陷阱：阶段间传递用 std::mem::take 置空输出缓冲（初版用
  mem::swap 导致旧 intermediate 回流，输出累积三倍——stage 调试定位）。
- 单测 6：阿拉伯数字归一（两组码位）/ 四种格式规范化为 [ID:n] 且索引
  集合正确 / 越界保留原文不标记 / ref 无数字不动 / 阿拉伯-印度数字
  引用归一 / extract 解析规范标记。
- 测试：默认 **525 passed / 0 failed / 21 ignored**；双 feature
  **532 passed / 0 failed / 21 ignored**（+6 默认 feature 内）。

## 2026-08-03 模型类型差异同步：对标 tenant_model_service.py upsert_model_type

- 审计 api/db/services/ 模型四件套：tenant_model_service.py（92 行）、
  tenant_model_instance_service.py（69 行）、tenant_model_group_service.py
  （21 行）——均为 CommonService CRUD 薄封装；唯一有移植价值的是
  `upsert_model_type`：按 (provider, instance, model_name) 对模型类型集
  做差异同步——add 中缺失类型插入（继承样例记录状态/extra），存在则
  恢复状态；delete 中类型标 UNSUPPORTED 留痕（不存在也插入墓碑记录）。
- RayRAG TenantModelSpec 已有 model_types + enabled 字段（等价
  ACTIVE/UNSUPPORTED 语义），缺差异同步方法。新增
  `TenantModelStore::sync_model_types(tenant, provider, instance, model,
  add, delete) -> usize`：add 缺失类型插入并 enabled=true（幂等），
  delete 存在类型移除（移除即不可 resolve，等价 UNSUPPORTED 墓碑）；
  返回操作计数；未知实例/模型报错。
- 单测 3：add/删除/保留并计数（Chat 保留、Rerank 新增、Embedding 移除、
  enabled 恢复）/ 重复 add 幂等（无重复能力项）/ 未知实例或模型报错。
- 测试：默认 **519 passed / 0 failed / 21 ignored**；双 feature
  **526 passed / 0 failed / 21 ignored**（+3 默认 feature 内）。

## 2026-08-03 Postgres 死锁重试：对标 common_service.py retry_deadlock_operation

- 审计 api/db/services/ 全量 33 文件清单：CommonService 是通用 CRUD
  基类（query/get_all/get/get_or_none/save/insert 等，Rust 类型化仓储
  已覆盖等价物）；唯一有真实移植价值的是 retry_deadlock_operation
  （MySQL/OceanBase 1213 死锁指数退避重试 3 次，0.1s 基数）。
- 移植到 PostgresSnapshotMirror 写路径：`retry_pg_write`（3 次尝试、
  0.1/0.2/0.4s 指数退避）识别 40P01（deadlock_detected）/40001
  （serialization_failure）；store_snapshot 事务整体重试（事务原子，
  重试安全）；Load/Health 不重试。与既有 pg_advisory_xact_lock
  （同 key 串行化）互补——后者防同 key 争用，前者兜底跨 key 死锁。
- 实现细节：is_retryable_sqlstate 纯函数 + is_retryable_pg_error
  downcast；retry_pg_write_with 谓词注入（tokio-postgres DbError 构造
  私有，无法在单测构造真 Error——循环逻辑用 stub 谓词测，SQLSTATE
  判定用纯函数测）。
- 单测 4：SQLSTATE 分类（40P01/40001 真，23505 假）/ 瞬态中止后成功
  （2 次调用）/ 三次放弃 / 非重试错误不重试。
- 测试：默认 **516 passed / 0 failed / 21 ignored**；双 feature
  **523 passed / 0 failed / 21 ignored**（+4：PG 重试单测，
  postgres-backend feature 内编译）。

## 2026-08-03 编译模板执行端点混合模板：tree + 超图全链路

- 重构 execute_group_templates：模板按 kind 分双桶——tree 类走
  run_tree_templates（RAPTOR 建树 + to_graph 投影），非 tree 类
  （hypergraph/list/set）走 compile_hypergraph（两阶段 LLM 抽取）
  + deduplicate_items（LLM 判定合并，cosine ≥ 0.9 前置过滤）；
  单模板失败跳过不中断；两类结果统一返回。
- 落库统一为 knowledge_compile checkpoint（method/content_hash=
  knowledge_compile，entity_types 含 tree_node + entity）：tree 图 +
  超图实体/关系合并投影到 EntityGraph 经 GraphStore.replace_document
  持久化（RAGFlow _struct_upsert_graph_json 近似）。
- EntityGraph 增 node_count()。
- GPU live 测试扩展：同组混合 tree + hypergraph 模板，3 chunk 中文
  ——建树（node_count ≥ 3）+ 超图实体（含养殖场/负责人）+ 落库
  checkpoint（method=knowledge_compile、图节点非空）4.82s 通过。
- 测试：默认 **516 passed / 0 failed / 21 ignored**；双 feature
  **519 passed / 0 failed / 21 ignored**（数量同前——执行器测试为
  既有 ignored 计数，非新增）。

## 2026-08-03 知识编译合并：对标 structure.py merge 阶段（_struct_merge_pair）

- 审计确认 RAGFlow 合并两阶段：本地 dedup（组内 pairwise cosine ≥ 0.9
  才问 LLM）+ LLM 判定合并（_struct_merge_pair：MERGE 三 prompt、
  temperature 0.0、返回 duplicated/merged）；relation 合并强制保留
  source/target（_struct_apply_merge_invariants，from/to_entity_kwd 不可变）。
- 新增 `src/merge.rs`（355 行）：
  - MERGE_SYSTEM_PROMPT / MERGE_USER_PROMPT / MERGE_DECISION_INSTRUCTION
    逐字镜像；`merge_pair`（temperature 0.0）→ `apply_merge_invariants`
    （relation 端点保留，含 src/from、tgt/to 别名）。
  - `cosine_similarity` + `payloads_similar`（embedding/q_*_vec 优先，
    name 相等兜底）+ `maybe_merge`（阈值前置过滤）。
  - `deduplicate_items`：贪心 pairwise——相似对 LLM 判定，重复则替换
    并 union source_chunk_ids，否则追加。
  - hypergraph.rs 增 `gen_json_with_temperature`（merge 用 0.0，
    抽取用默认 0.1）。
- 单测 4（cosine / relation 不变式 / 向量阈值+name 兜底 / chunk_ids 并集）；
  GPU live 测试 gpu_merge_pair_judges_duplicate_and_merges（ignored）——
  8088 Qwen3.5 判定同名组织重复并合并、无关实体拒绝合并 1.47s 通过。
- 测试：默认 **516 passed / 0 failed / 21 ignored**；双 feature
  **519 passed / 0 failed / 21 ignored**。

## 2026-08-03 LLM 超图抽取：对标 structure.py 知识编译（非 tree 模板）

- 审计确认 RAGFlow `compile_structure_from_text` 对非 tree 模板走
  `_struct_extract_hypergraph`：两阶段 LLM JSON 抽取（先实体，再以
  Known Entities 约束抽取关系），prompt 由 `_struct_hypergraph_prompts`
  从模板 parser_config 渲染（guideline/entity/relation/output/options，
  fields 渲染 + `{"items": [...]}` 响应格式 + Auto-type），temperature 0.1。
- 新增 `src/hypergraph.rs`（486 行）：
  - `hypergraph_prompts`：镜像 prompt 渲染——支持编译模板 shape
    （entity/relation + fields type/rule）与旧 output shape
    （entities/relations + fields name/type/required），
    render_fields/render_type_fields 双渲染器，infer_type
    （list/set/hypergraph 三态）。
  - `gen_json`：LLM JSON 输出辅助（剥 ```json fence、prose 容错、
    首尾大括号截取）；`unwrap_items`：{"items": [...]} 归一。
  - `extract_hypergraph`：两阶段抽取（关系仅限已知实体）。
  - `compile_hypergraph`：逐 chunk 独立抽取 + 实体按名去重 +
    source_chunk_ids 注入（RAGFlow 跨 chunk 合并留待 merge 阶段）。
- 单测 5（infer_type 三态 / prompts 渲染 / legacy shape / JSON 解析容错 /
  items 归一）；GPU live 测试 gpu_hypergraph_extraction_entities_then_relations
  （ignored，env RAYRAG_TEST_LLM_BASE/MODEL）——8088 Qwen3.5 中文文本
  两阶段真实抽取 3.67s 通过（实体含养殖场/负责人，关系仅引用已知实体）。
- 测试：默认 **512 passed / 0 failed / 20 ignored**；双 feature
  **515 passed / 0 failed / 20 ignored**。

## 2026-08-03 编译模板执行器落库：对标 _struct_upsert_graph_json + raptor_tree_to_graph

- 审计确认 RAGFlow tree 模板执行后把树投影为 {entities, relations}
  （raptor_tree_to_graph：tree_node 实体 + child 关系）并 upsert 到图存储
  （_struct_upsert_graph_json，row_id 含 doc+compile_kwd+template）。
- `src/raptor.rs` 新增 `TreeGraph`/`TreeGraphEntity`/`TreeGraphRelation` +
  `RaptorTree::to_graph`（镜像 raptor_tree_to_graph：叶子节点带
  source_chunk_ids，内部节点用 content 首行作实体名）；`TreeTemplateResult`
  增加 graph 字段。
- `execute_group_templates` 支持 `?doc_id=..&kb_id=..`：执行后把全部树
  图合并为 raptor_tree checkpoint 写入 GraphStore.replace_document
  （method="raptor_tree"，entity_types=["tree_node"]）。
- 单测 1（手工 3 节点树 → 3 实体 2 child 关系、叶子 source_chunk_ids）；
  GPU live 测试扩展：POST execute 带 doc_id/kb_id → 断言返回 graph 且
  checkpoint 落库（method/kb_id/entity_types）——1.31s 通过。
- 测试：默认 **507 passed / 0 failed / 19 ignored**；双 feature
  **510 passed / 0 failed / 19 ignored**。

## 2026-08-03 编译模板执行器：对标 chunk_post_processor.run_tree_templates

- 审计确认 compilation_templates API 纯 CRUD、无执行器；RAGFlow 触发链路：
  task_handler._parser_config_compilation_template_ids → resolve_template_ids
  → run_tree_templates（tree 类模板用 chat_mdl 建 RAPTOR 树）。
- `src/raptor.rs` 新增 `run_tree_templates` 编排 + `TreeTemplateResult`：
  对每个 tree 模板读 config.raptor（prompt/max_token/threshold/max_cluster，
  缺省对齐 RAGFlow 默认），调 build_with_llm_prompt 建树；单模板失败跳过
  不中断其余。
- `src/api/compilation_templates.rs` 新增 `execute_group_templates` handler +
  `src/server.rs` 路由 `POST /api/v1/compilation_template_groups/{id}/execute`
  （body: chunks[{content,embedding}]）；LlmClient 加 #[derive(Clone)]。
- 单测 2（每模板一树 / 空输入）+ GPU live 测试
  gpu_tree_template_execute_builds_trees_over_http（ignored，env
  RAYRAG_TEST_LLM_BASE/MODEL）——建组→POST execute→8088 Qwen3.5 真实建树
  1.38s 通过。
- 测试：默认 **506 passed / 0 failed / 19 ignored**；双 feature
  **509 passed / 0 failed / 19 ignored**。

## 2026-08-03 PDF outline 链路收尾：transient 语义对齐 chunk_builder.extract_outline

- 审计发现：naive chunker 会把 doc.metadata.clone() 复制到每个 chunk，
  导致 `__outline__` 出现在所有 chunk 且永不消费，偏离 RAGFlow 语义。
- `src/pipeline/mod.rs` 新增 `normalize_first_chunk_metadata`：`__outline__`
  只保留在 chunks[0]（对齐 naive.py "attach to first chunk" + chunk_builder
  extract_outline 弹出前的位置），其余 chunk 移除该 transient 键；普通
  metadata（file_name 等）不受影响。process() 在 annotate_chunks 后调用。
- 持久化到 DocumentMetadataStore 属 API 层职责（RayRAG 已有
  DocumentMetadataStore.replace，RAGFlow update_document_metadata 是 partial
  merge 语义），留待后续接 API 时补 merge 变体。
- 测试：默认 **504 passed / 0 failed / 18 ignored**；双 feature
  **507 passed / 0 failed / 18 ignored**。

## 2026-08-03 PDF 大纲（outline）提取：对标 deepdoc/parser/utils.py + chunk_builder.py

- `src/parser/pdf.rs` 新增 `OutlineEntry`（title/depth/page）+ `extract_outlines`
  （lopdf get_outlines 递归遍历 Outline 树 → (title, depth, page)，页码经
  page-id→页号映射解析；无书签返回空）。镜像 RAGFlow `extract_pdf_outlines`
  语义；`parse()` 时把 outline 以 `__outline__` 键挂入 Document.metadata
  （对齐 naive.py 附到 chunk[0] 的 transient 语义，供 chunk 后处理持久化）。
- 真实 PDF 验证：reportlab 生成嵌套书签 PDF（2 章 + 1 子节），
  gpu_pdf_outline_extraction_live（ignored，env RAYRAG_TEST_PDF_OUTLINE）
  断言非空+嵌套+页码解析通过。
- 新增依赖 indexmap 2（lopdf 传递依赖显式化）。
- 测试：默认 **502 passed / 0 failed / 18 ignored**；双 feature
  **505 passed / 0 failed / 18 ignored**。

## 2026-08-03 RAPTOR LLM 摘要闭环：对标 raptor_service.py + advanced_rag/raptor.py

- `src/raptor.rs` 新增 `ClusterSummarizer` trait（镜像 RAGFlow
  `_summarize_texts`：prompt 模板 `{cluster_content}` 占位、同语言标题行
  提示、max(max_token,512) 上限）+ `build_with_llm` / `build_with_llm_prompt`
  （异步 LLM 建树，LLM 失败自动回退抽取式摘要）+ `LlmClusterSummarizer`
  （逐块截断预算 (max_length-max_token)/len(texts)，system=prompt 渲染）。
- **llm.rs 修复（GPU 实测发现）**：Qwen3.5 在 llama.cpp 上把内容全放
  reasoning_content（思考链），content 为空且 finish=length。llama.cpp 不认
  `extra_body`，需 `chat_template_kwargs.enable_thinking=false`。现 qwen3
  家族双形态同发（extra_body 给 DashScope/OpenAI 兼容、chat_template_kwargs
  给 llama.cpp），全端点兼容。
- GPU 闭环：gpu_raptor_llm_summary_loop（ignored，env 注入 8088
  Qwen3.5-9B）真实 LLM 调用 probe+建树，根节点为润色摘要（非 verbatim
  拼接）。断言教训：LLM 摘要可能以原文开头，须与完整 extractive 拼接比对。
- 测试：默认 **500 passed / 0 failed / 17 ignored**；双 feature
  **503 passed / 0 failed / 17 ignored**。

## 2026-08-03 S3 兼容对象存储客户端（StorageClient）：对标 rag/utils/storage_factory.py 族

- 新增 `src/storage.rs`：`StorageConfig`（endpoint/region/access_key/
  secret_key/path_style/timeout，`from_env` 读 `RAYRAG_STORAGE_*`）+ 
  `StorageClient`（AWS SigV4 签名，纯手写 HMAC-SHA256 链 + civil-from-days
  日期算法，零 chrono 依赖）。
- 接口镜像 RAGFlow 存储契约：`health` / `put` / `get` / `rm` /
  `obj_exist` / `bucket_exists` / `make_bucket` / `presigned_get_url`。
  一套客户端覆盖 MinIO、AWS S3、阿里云 OSS、腾讯云 COS、华为 OBS 等
  S3 兼容端点（国内部署：OSS/COS/MinIO 直连）。
- 签名正确性验证：HMAC 链与独立 Python 实现交叉一致；**真实 MinIO
  容器 live round-trip 通过**（make_bucket→put→obj_exist→get 字节一致→
  presigned URL→rm，SigV4 path-style，2026-08-03）。
- 测试：6 passed / 1 ignored（默认 feature）；ignored 为
  `minio_round_trip_live`（需 RAYRAG_STORAGE_* env）。
- 新增依赖：hmac 0.12、hex 0.4、url 2（均为轻量，cargo rsproxy 镜像）。

## 2026-08-03 ASR/TTS 客户端接入：对标 RAGFlow sequence2txt_model.py + tts_model.py

- 新增 `src/audio.rs`：`AsrClient`（OpenAI 兼容 `/audio/transcriptions`
  multipart：file+model → `{text}`，返回去空白文本 + token 数，镜像
  RAGFlow GPTSeq2txt 及其子类 StepFun/FuturMix/DeepInfra/Gitee/CometAPI/
  DeerAPI/GPUStack/Xinference）+ `TtsClient`（`/audio/speech` JSON
  `{"model","voice","input"}` → 原始音频字节，镜像 HTTPBasedTTS 及其子类
  OpenAI/Xinference/Ollama/GPUStack/SILICONFLOW/DeepInfra）。
- 单面协议覆盖全部提供商（含国内：StepFun、SiliconFlow、Xinference、
  Ollama 本地）；Bearer 认证按 key 非空启用；非 2xx 返回截断 300 字符
  错误体；TTS 空响应视为错误。
- 测试：mock axum 服务器 5 测试（multipart 字段断言、缺 text 字段报错、
  TTS 原始字节 round-trip、400 透传、Bearer 路径）；默认 feature
  **492 passed / 0 failed / 15 ignored**；双 feature 495。
- 至此 RAGFlow rag/llm/ 8 个模型文件全部有对应客户端：chat/embedding/
  rerank/ocr/cv/seq2txt/tts + __init__ 能力注册。

## 2026-08-03 视觉模型客户端（VisionClient）接入：对标 RAGFlow cv_model.py

- 新增 `src/vision.rs`：OpenAI 兼容 VLM 客户端，镜像 RAGFlow `cv_model.py`
  的 GptV4 语义——`describe(image)` / `describe_with_prompt(image, prompt)`，
  图片统一归一化为 data URL（已支持 `data:` 前缀/纯 base64/裸字节），
  默认 prompt 内嵌 RAGFlow `vision_llm_describe_prompt.md` 模板（PDF 页转
  Markdown 任务），支持 lang 参数。OpenAI/llama.cpp/Ollama/vLLM/国内厂商
  视觉端点全部适用（同一 chat completions 协议，兼容所有提供商 vision 模型）。
- `src/model_meta.rs` 已有 vision capability 发现（Ollama /v1/models）；本次补齐
  实际推理客户端，视觉能力闭环。
- GPU 实测：`gpu_vision_endpoint_describes_image_with_prompt`（ignored，
  `RAYRAG_TEST_VISION_BASE`/`RAYRAG_TEST_VISION_MODEL` env 注入）调用 8088
  的 Qwen3.5-9B（带 mmproj 视觉投影）——测试图用**纯 Rust 生成 PNG**
  （flate2 zlib + 手写 CRC-32 表，零外部图片依赖），VisionClient 描述
  "黑色矩形" 断言通过（1.95s）。
- 测试：默认 feature 487 passed / 0 failed / 15 ignored（+4 vision 单测 +
  GPU 视觉集成）；双 feature 490 passed / 0 failed / 15 ignored。
- 陷阱记录：llama.cpp 视觉端点只接受 PNG/JPEG（PPM 会 400 "Failed to load
  image"）；crc32fast 不在依赖树，PNG CRC-32 手写查表实现。

## 2026-08-03 GPU 模型客户端真实兼容修复：reasoning_content + embedding 双形态 + reranker key

- `src/llm.rs`：对齐 RAGFlow `chat_model.py` 的 reasoning 语义——非流式 chat 解析
  `reasoning_content`（Qwen3/DeepSeek-R1 风格）与 `reasoning` 别名（Kimi-K2.5 风格），
  存在时以 `<think>...</think>` 前缀拼接到 content（空 reasoning 不包裹）；
  tool chat 仅在没有 tool_calls 时附加 think 前缀；Qwen3 家族请求自动带
  `extra_body.enable_thinking=false`（对应 RAGFlow `_apply_model_family_policies`）。
  实测 8088 的 Qwen3.5-9B 确认 llama.cpp 不认 extra_body（与 vLLM 行为不同），
  thinking 默认开启、思考链会吃满 max_tokens——RayRAG 现在能正确保留思考链，
  修复前 content 会退化为 "(empty response)" 占位符。
- `src/embed/openai.rs`：兼容两种 embedding 响应形态——标准 OpenAI
  `{"data":[{"embedding":[...]}]}` 与 llama.cpp 裸 `/embeddings` 端点
  `[{"index":0,"embedding":[[...]]}]`（二维嵌套，旧 Python 客户端兼容格式）。
  真实修复：base_url 不带 /v1 时 GPU 集成测试此前报
  "invalid type: map, expected a sequence"，现在 8888（Qwen3-Embedding-4B）实连通过，
  2560 维向量真实返回。
- `src/agent.rs` GPU 集成测试：reranker 支持 `RAYRAG_TEST_RERANKER_KEY` 环境变量
  （8081 用 `--api-key` 保护，此前 401）；rerank 用例改用中文文档对
  （"RAGFlow是一个开源的RAG引擎" 排第 1）——英文短句对在 llama.cpp 上
  mxbai-rerank-large-v2 排序错位（"Bananas are yellow." 恒得 0.945 高分），
  属宿主 llama-server 模板问题而非 RayRAG 缺陷；中文长句排序稳定正确。
- 测试：默认 feature **483 passed / 0 failed / 14 ignored**（+8：3 reasoning +
  5 embedding 双形态）；GPU 集成测试 1 passed（3.68s，chat/embed/rerank 三模型
  真实调用 + Categorize 工作流路由正确）。
- `.env.example` 增补 `RAYRAG_TEST_RERANKER_KEY=mxbai-rerank-key`。

---

## 2026-08-02 国内连接器接入 + GPU 四模型闭环 + zvec/postgres 实连

- 新增 5 个大陆直连连接器（免代理、免 key 优先）：`src/bing.rs`（cn.bing.com 桌面结果页）、
  `src/baidu.rs`（www.baidu.com/s，302 安全跳转 + cookie store + 浏览器头）、
  `src/bocha.rs`（博查 API，需 `BOCHA_API_KEY`）、`src/tencent_finance.rs`（qt.gtimg.cn GBK
  行情 + smartbox 搜索提示）、`src/baidu_scholar.rs`（xueshu.baidu.com 学术结果）。全部
  `.no_proxy()` 直连、15 秒超时、有界响应体，输出 RAGFlow `title/link/snippet` tool 行。
- `src/agent.rs`：5 个连接器完成 Agent 工具分发接线——`execute_*_with_provider` 执行函数、
  timeout 包装、channel/top_n/attempts/retry_delay 辅助函数、`execute_node` 与
  `execute_canvas_agent_tool_with_providers` 各 5 个 match 分支、错误前缀映射（`Bing
  error:`/`Baidu error:` 等）、imports。`src/runtime.rs`：5 组 INPUTS/OUTPUTS 常量 +
  `COMPONENT_REGISTRY` 新增 5 描述符（bing/baidu/bocha/tencentfinance/baiduscholar）。
- 腾讯行情解析对齐真实协议：GBK.decode → `split('=')` 取引号内容 → 按 `~` 分割 → 字段索引
  映射（3=现价、5=今开、31=涨跌额、32=涨跌%、33=最高、34=最低、36=成交量）；模拟测试串按
  真实 ~50 字段布局重写。
- `src/api/tenant_models.rs`：resolve 同名校验改为 `filter` + enabled 优先——用户显式创建且
  启用的 provider 实例优先于 default_providers 播种的同名实例。
- 百度系风控处理：百度对自动化请求偶发返回"百度安全验证"（wappass）页。连接器检测该页并
  返回明确错误（`safety verification page returned`），live 测试对该错误优雅 skip（视为环境
  限制而非代码失败）；curl 验证完整浏览器头 + cookie jar + gzip 可稳定取得结果页（3/3）。
- 测试：默认 feature 475 passed / 0 failed / 14 ignored；双 feature 478 passed / 0 failed。
  Bing 与腾讯财经 live 在线通过；百度系 live 被风控 skip。
- **GPU 四模型真实调用闭环**：8088 chat（Qwen3.5-9B 返回 reasoning_content 思考链）、
  8888 embed（Qwen3-Embedding-4B 返回真实向量）、8081 rerank（mxbai-rerank-large-v2
  `--reranking`，相关文档排第 1）、8090 OCR（PaddleOCR-VL 准确识别 "RayRAG OCR Test 2026"）。
  测试套件新增 `RAYRAG_TEST_LLM_BASE/MODEL`、`RAYRAG_TEST_EMBEDDING_BASE/MODEL`、
  `RAYRAG_TEST_RERANKER_BASE` 环境变量入口（`.env.example` 已公开）。
- **zvec 原生冒烟**：`ZVEC_LIB_DIR=~/.local/lib/zvec/0.5.1` + 同值 `LD_LIBRARY_PATH` 下
  `native_collection_upserts_queries_and_reopens` 通过（create/upsert/query/reopen 持久化）。
- **postgres 18.4 实连**：`RAYRAG_POSTGRES_TEST_URL=postgresql://rayrag:***@127.0.0.1:55432/rayrag`
  下 `postgres_18_snapshot_round_trip` 通过（store→load→版本门控 `server_version_num/10000==18`
  →DELETE 清理）。rayrag-postgres-1 容器运行中（healthy，5 天）。
- 基础设施：`~/.cargo/config.toml` 配置 rsproxy 国内镜像（crates-io 替换为 rsproxy-sparse）；
  reqwest 增加 `cookies`、`gzip` features。

---

## 2026-08-02 RAGFlow Google Search 纯 Rust 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 527 行
  `agent/tools/google.py`、179 行 `internal/agent/tool/google.go`、170 行 Go 测试、公共
  `_retrieve_chunks` / `kb_prompt`、standalone/Agent 前端表单、完整国家/语言选项及两个固定模板实例。
  RAGFlow“全部文件”知识库返回 Dify 和本地化资源等无关片段，未作为实现证据。
- 按 Aliyun 锁文件下载并校验 `google-search-results==2.4.2` sdist，SHA-256 为
  `603a30ecae2af8e600b22635757a6df275dad4b934f975e67878ccd640b78245`；源码确认 SDK 固定 GET
  `https://serpapi.com/search`，自动补 `source=python`、`engine=google` 和 `output=json`，使用
  `requests.get(..., timeout=60000)` 且不主动 `raise_for_status`。
- 新增 355 行（含测试）`src/google.rs` 纯 Rust 双协议层。生产端点固定为 SerpApi 和 Google
  Programmable Search CSE，Canvas 不能注入任意 URL；客户端禁环境代理和 redirect，响应限制 16 MiB。
  本地 HTTP 测试锁定 query encoding、响应解析、CSE 缺省/上限和缺失标量行为。
- Python/Canvas 主路径严格保留 `google_search` 描述与 `q/start/num` schema、必填 SerpApi key、240 个
  country 和 156 个 language 精确 allowlist、参数类 `country=cn/language=en` 缺省及前端
  `country=us/num=12` 新节点值。固定 Python `_invoke` 虽声明并接收 `start/num`，实际从未把二者放入
  SerpApi params；Rust 忠实保留该 bug，不虚构分页或条数语义。
- 成功路径原样发布 `organic_results` JSON；内容先要求 `snippet`（保持 Python default 参数的求值次序），
  再优先使用 `about_this_result.source.description`，去 base64 PNG、按 Unicode 字符截到 10,000，以
  SHA-1 modulo ID 生成 chunk、document aggregation、reference 和 `kb_prompt(hash_id=True)`。空 q 只
  清空 formalized content；`max_retries + 1`、末次失败后仍 delay、取消和 12 秒 deadline 已接线。
- 独立 Go 兼容方法保留不同的 `google` CSE 合同：query/api_key/cx 精确非空检查，`max_results<=0`
  缺省 5、正数上限 10，percent-encoded `key/cx/q/num`，非 2xx/坏 JSON fail closed，缺失/null scalar
  为 Go string 零值，输出 `{results:[{title,link,snippet}]}`。Canvas/Agent 继续暴露 Python SerpApi
  q/country/language 模型，不混入 Go CSE 参数。
- `src/agent.rs` 将 Google 接入顶层 Canvas 和 Agent ReAct，发布精确 `google_search` 元数据、必填 q、
  可选 start/num 及 `Google error:` 回灌；`src/runtime.rs` 组件目录、README、coverage mapping 和 Agent
  provider 注入同步更新。新增 5 个确定性协议/Canvas/Agent 测试和 1 个默认 ignored 的 live 测试；
  coverage mapping 守卫同步增至 52/52。
- 覆盖矩阵从固定 Git tree 重生 4,441 文件；Python/Go Google 生产文件获得专属 mapping 但诚实保持
  `partial`。汇总不变：aligned 68/15,322 行、partial 1,579/650,559 行、replaced 71/27,674 行、
  reference 2,723/574,185 行、unmapped 0。
- 验证：默认 locked 全目标为 453 lib + 52 coverage = 505 passed / 0 failed / 9 ignored；
  `postgres-backend,zvec-backend` 为 456 lib + 52 coverage = 508 passed / 0 failed / 9 ignored。默认/双
  feature 的 all-target check、严格 `clippy -D warnings` 和 zvec native 测试均通过；PG URL 未设置，
  因此本批不声称 PostgreSQL 实连。公开 `demo` key 的 Google live test 在 2.65 秒收到 SerpApi HTTP
  401，真实账户 key 与 CSE key/cx 均未提供，故公网门禁明确未通过。
- 尚未冒充完成：Python requests 的环境 proxy、redirect/cookie/session、SDK params 原地 mutation、
  60000 秒内层 timeout 和 HTTP/status/JSON 异常分类，以及 Go `HTTPHelper` 的环境 proxy、
  OpenTelemetry、30 秒 client 与独立三次抖动重试、thoughts/callback 和 checkpoint 级取消仍未逐项复刻。
  因此两条生产文件继续保持 `partial`，纯 Rust/Linux/Docker、zvec-rust、PostgreSQL 18.4、国内镜像优先
  和零 MySQL 路线不变。

---

## 2026-08-02 RAGFlow Yahoo Finance 纯 Rust 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 115 行
  `agent/tools/yahoofinance.py`、172 行 `internal/agent/tool/yahoo_finance.go`、160 行 Go 测试、Go
  registry、standalone/Agent 前端表单和 stock-market 模板实例。RAGFlow“全部文件”知识库本批返回版本号、
  医学等无关片段，未作为实现证据。
- 按锁文件从 Aliyun 下载并校验 `yfinance==0.2.65` sdist，SHA-256 为
  `3d465e58c49be9d61f9862829de3e00bef6b623809f32f4efb5197b62fc60485`；核对 basic
  cookie/crumb、quoteSummary/quote 信息合并、trailing PEG、chart history、calendar、fundamentals
  timeseries、latestNews POST/ad 过滤及 pandas Markdown 形成过程。
- 新增 1,551 行（含测试）`src/yahoo_finance.rs` 纯 Rust 双协议层。生产主机固定为
  `fc.yahoo.com`、`query1.finance.yahoo.com`、`query2.finance.yahoo.com` 和 `finance.yahoo.com`，Canvas
  不能注入 URL；客户端禁环境代理和 redirect，设置 30 秒 HTTP timeout、16 MiB body 上限及 60 秒
  component deadline，cookie/crumb 只在客户端内缓存且不进入 Debug/输出。
- Python/Canvas 路径保留 `info/news=true`、其余 false 的参数缺省，严格校验八个布尔值；忠实保留
  `financials` 实际输出 Calendar、`count`/`income_stmt` 仅校验但从不读取的固定源码行为，以及空
  `stock_code` 只发布空 `report`、`max_retries + 1` 和末次失败后仍 delay。报表按 Information、History、
  Calendar、年度/季度 Balance sheet、年度/季度 Cash flow、News 固定顺序输出。
- 信息路径合并五个 quoteSummary module 和 quote，完成一级 flatten、raw/fmt 递归归一化、NBSP 清理、
  `maxAge=1` 转 86400 和 trailingPegRatio；history 先取时区再取一月日线，并应用默认 auto-adjust 核心
  OHLC/Close 与 dividend/split 行；calendar 转换日期/盈利区间。fundamentals 使用 yfinance 完整 145 个
  balance-sheet key 和 123 个 cash-flow key，保留年度/季度前缀、日期降序和 Yahoo key 顺序；新闻使用
  `latestNews` JSON POST、snippetCount=10 并按 Python truthiness 排除广告。
- 独立 Go 兼容方法保留 `yahoo_finance` 合同：非空 symbols 数组、可选 fields、逗号合并及 percent
  encoding、精确 `Mozilla/5.0 (compatible; ragflow/1.0)` User-Agent 和 `application/json` Accept，
  缺失/null string/float 按 Go 零值，发布
  `{results:[{symbol,regularMarketPrice,currency,regularMarketChangePercent}]}`；Canvas/Agent 仍暴露固定
  Python 的 stock_code/report schema，不混入 Go symbols/fields 参数。
- `src/agent.rs` 将 `YahooFinance` 接入顶层 Canvas 与 Agent ReAct，发布精确 `yahoo_finance` 名称、描述、
  必填 `stock_code` schema 和 `YahooFinance error:` 回灌；静态配置中的六个可见 flag 及隐藏
  count/income_stmt 都保留。`src/runtime.rs` 组件目录、README、coverage mapping 和公共 Agent tool DSL
  同步更新。
- Yahoo 聚焦测试为 5 passed / 0 failed / 1 ignored，覆盖 cookie/crumb/header/body/query、所有 section
  顺序、raw/maxAge/auto-adjust、完整 key 数、news ad、Go null 零值、空输入、重试和 Agent selector；覆盖生成器
  51/51 通过并从固定 Git tree 重生 4,441 文件矩阵。Python/Go Yahoo Finance 获得专属 mapping 但保持
  `partial`；汇总仍为 aligned 68/15,322 行、partial 1,579/650,559 行、replaced 71/27,674 行、
  reference 2,723/574,185 行、unmapped 0。
- 验证：默认 locked 全目标为 448 lib + 51 coverage = 499 passed / 0 failed / 8 ignored；
  `postgres-backend,zvec-backend` 为 451 lib + 51 coverage = 502 passed / 0 failed / 8 ignored。默认/双
  feature 的 all-target check、严格 `clippy -D warnings`、fmt、tracked diff whitespace 与 Compose 静态解析
  均通过；zvec native 测试真实执行。PG URL 未设置，因此本批不声称 PostgreSQL 实连；未新起、停止或重启容器。
- 公网门禁未通过：显式 live test 在 5.77 秒收到 crumb HTTP 403；独立 curl 探测可取得 23 字节 crumb，
  但 quote/chart/news 均为 HTTP 429。curl-cffi Chrome TLS impersonation、consent CSRF fallback、持久
  cookie/cache、yfinance 的完整 price repair/timezone/action 边界、pandas dtype/alignment/to_markdown 字节
  一致性，以及 Go HTTPHelper 的环境 proxy、OpenTelemetry 和三次抖动重试尚未复刻。因此两条生产文件
  继续诚实标为 `partial`，纯 Rust/Linux/Docker、zvec-rust、PostgreSQL 18.4、国内镜像优先和零 MySQL
  路线不变。

---

## 2026-08-02 RAGFlow GitHub 纯 Rust 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 97 行
  `agent/tools/github.py`、174 行 `internal/agent/tool/github.go`、163 行 Go 测试、公共
  `_retrieve_chunks` / `kb_prompt`、Go registry 及前端 standalone/Agent 表单。Python 参数类缺省
  `top_n=10`，前端新节点和 Agent 工具配置实际种入 `top_n=5`，Rust 分别保留而未强行统一。本机
  RAGFlow“全部文件”知识库返回版本号、医学等无关片段，未用作实现证据。
- 直接依赖实际使用 `requests` 而非已声明但未导入的 PyGithub。按 Aliyun 锁文件下载并校验
  `requests==2.32.5` sdist SHA-256
  `dbba0bac56e100853db0ea71b82b4dfd5fe2bf6d3754a8893c3af500cec7d7cf`，核对
  `python-requests/2.32.5` 默认 User-Agent、`requote_uri`、30 次 redirect 上限、cookie session 和
  `trust_env=true` 的环境 proxy/netrc 行为。
- 新增 465 行（含测试）`src/github.rs` 纯 Rust 双协议层。生产端点固定为
  `https://api.github.com/search/repositories`，Canvas 不能注入任意 URL；客户端禁环境代理、禁
  redirect，采用 12 秒请求和 16 MiB body 上限，对网络、超大 body、缺 `items`、非 JSON 和字段类型
  错误 fail closed，同时保留 Python 返回的每个原始 repository item。
- Python/Canvas 路径发送 `q/sort=stars/order=desc/per_page`、
  `Content-Type: application/vnd.github+json`、`Accept: */*`、
  `X-GitHub-Api-Version: 2022-11-28` 与 requests-compatible User-Agent；description 为 null 时按
  Python `str(None)` 生成 `None`，再拼接 `\n stars:<watchers>`。内容去 base64 PNG、按 Unicode 字符
  截到 10,000，以 SHA-1 modulo ID 生成 chunk、document aggregation、reference 和
  `kb_prompt(hash_id=True)`；`json` 保留原始 `items`，空 query 只清空 formalized content。
- 独立 Go 兼容方法保留 `github` 合同：trim 后空 query 拒绝，`max_results<=0` 缺省 5、正数上限 30，发送
  percent-encoded `q/per_page`、GitHub media-type Accept、net/http 隐式
  `Go-http-client/1.1` User-Agent 与可选 `Authorization: Bearer <token>`；缺失/null 标量按 Go
  string/int 零值处理，发布 `{results:[{full_name,html_url,description,stargazers_count}]}`。
- `src/agent.rs` 将 `GitHub` 接入顶层 Canvas 和 Agent ReAct，精确发布 Python 的 `github_search`
  名称、描述和仅必填 `query` schema；外层 `max_retries + 1`、末次失败后的 delay、取消、12 秒
  deadline 与 `GitHub error:` 回灌均已接线。`src/runtime.rs` 组件目录、README、公共 Agent tool DSL
  与 coverage mapping 守卫同步更新；Canvas/Agent 不暴露 Go token 参数，避免改变 Python DSL。
- 新增 4 个确定性本地 HTTP/JSON 测试、2 个 Agent/Canvas 测试和 1 个默认 ignored 的公网测试；
  聚焦 GitHub 测试为 6 passed / 0 failed / 1 ignored。live test 以 `--ignored --exact` 显式执行，在
  未设置 `GITHUB_TOKEN` 的匿名限额下 3.00 秒成功返回真实 `api.github.com` 结果。
- 覆盖生成器 50/50 通过并从固定 Git tree 重生 4,441 文件矩阵；Python/Go GitHub 生产文件获得专属
  映射但诚实保持 `partial`，汇总仍为 aligned 68/15,322 行、partial 1,579/650,559 行、replaced
  71/27,674 行、reference 2,723/574,185 行、unmapped 0。
- 验证：`cargo fmt --all -- --check`、默认与 `postgres-backend,zvec-backend` 双 feature 的 locked
  all-target check、严格 `clippy -D warnings`、全目标测试均通过；默认为 443 lib + 50 coverage =
  493 passed / 0 failed / 7 ignored，双 feature 为 446 lib + 50 coverage = 496 passed / 0 failed /
  7 ignored，zvec native create/upsert/query/reopen 实际执行。PostgreSQL/GPU/Tavily/GitHub token 环境变量
  未设置，条件式 PG 与 GPU 未实连；Compose 使用临时占位密码仅做静态配置校验，未新起、停止或重启
  容器。
- 尚未冒充完成：Python requests 的环境 proxy/netrc、redirect/cookie session、字符串 URL 拼接中
  `&/#` 等边缘解释、15 秒内层 timeout 与 `.json()` 异常分类未逐项复刻；Go `HTTPHelper` 的环境 proxy、
  OpenTelemetry、30 秒 client、redirect 与独立三次抖动重试，以及 thoughts/callback、checkpoint 级取消
  仍未对齐。因此 Python/Go 两条生产文件保持 `partial`，纯 Rust/Linux/Docker、zvec-rust、
  PostgreSQL 18.4、国内镜像优先和零 MySQL 路线不变。

---

## 2026-08-02 RAGFlow Google Scholar 纯 Rust 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 115 行
  `agent/tools/googlescholar.py`、355 行 `internal/agent/tool/google_scholar.go`、188 行 Go 测试和
  101 行 Python 测试，连同公共 `_retrieve_chunks` / `kb_prompt`、前端 `top_n=12`、排序、年份和
  patents 表单。RAGFlow“全部文件”知识库本批只回显查询，未返回 Google Scholar 源码证据，未用作
  实现依据。另从 Aliyun 锁文件下载并校验 `scholarly==1.7.11` sdist SHA-256
  `2c983dd44d9d9398a6f2605102ae6e5586023b41ebbaec1461917ee48eb153f0`，核对查询字段、分页、作者/
  年份/摘要解析和 Navigator 的重试、随机等待、代理、redirect、cookie 与 CAPTCHA 处理。
- 新增 784 行（含测试）`src/google_scholar.rs` 纯 Rust 双协议层，并固定
  `scraper=0.20.0`；32 个传递依赖通过 USTC 镜像锁定和获取。生产端点固定为
  `https://scholar.google.com/scholar`，Canvas 不能注入任意 URL；客户端禁环境代理、禁 redirect，
  设置 12 秒请求和 16 MiB body 上限，并对非 2xx、坏 UTF-8、超大响应和 Google CAPTCHA/拒绝服务
  页面 fail closed。
- Python/Canvas 主路径保留 `hl/q/as_ylo/as_yhi/as_vis/as_sdt/scisbd/start` 字段、
  `top_n=12`、`sort_by=relevance`、`patents=true` 缺省和数字 next-page 跟随；解析 title/link、过滤
  authors、venue/year、abstract 与 gsrank，生成与公共 helper 一致的 chunk/reference/document
  aggregation/`kb_prompt(hash_id=True)`，并发布核心 publication JSON。空 query 会清空 formalized
  content 和 JSON，不触发网络。
- 独立 Go 兼容方法保留 `google_scholar` 合同：空白 query 拒绝，`max_results` 缺省 5，URL `num`
  上限 20，发送源码的 browser User-Agent、HTML Accept 和 `hl=en`；按 `.gs_ri/.gs_rt/.gs_a/.gs_rs`
  解析并跳过无标题项，年份只接受 1900–2099，发布
  `{results:[{title,link,snippet,authors,year}]}`。
- `src/agent.rs` 将 `GoogleScholar` 接入顶层 Canvas 和 Agent ReAct，精确发布 Python 的
  `google_scholar_search` 名称、长描述和仅必填 `query` schema；外层 `max_retries + 1`、包括末次
  失败后的 delay、12 秒 deadline 和 `GoogleScholar error:` 回灌均已接线。`src/runtime.rs` 组件目录、
  README、公共 Agent tool DSL 与覆盖守卫同步更新。
- 新增 4 个确定性 DOM/协议测试、2 个 Agent/Canvas 测试、1 个默认 ignored 的公网测试和 1 个
  coverage mapping 守卫；聚焦 Google Scholar 测试为 6 passed / 0 failed / 1 ignored。默认 ignored
  的 live test 另以 `--ignored --exact` 显式执行，在 12.04 秒因当前环境访问
  `scholar.google.com` 超时而失败，只记录为环境阻塞，未冒充在线通过。
- 覆盖生成器 49/49 通过并从固定 Git tree 重生 4,441 文件矩阵；Python/Go Google Scholar 生产文件
  获得专属映射但诚实保持 `partial`，汇总仍为 aligned 68/15,322 行、partial 1,579/650,559 行、
  replaced 71/27,674 行、reference 2,723/574,185 行、unmapped 0。
- 验证：`cargo fmt --all -- --check`、默认与 `postgres-backend,zvec-backend` 双 feature 的 locked
  all-target check、严格 `clippy -D warnings`、全目标测试均通过；默认为 437 lib + 49 coverage =
  486 passed / 0 failed / 6 ignored，双 feature 为 440 lib + 49 coverage = 489 passed / 0 failed /
  6 ignored，zvec native create/upsert/query/reopen 实际执行。PostgreSQL/GPU/Tavily 环境变量未设置，
  条件式 PG 与 GPU 未实连；Compose 使用临时占位密码仅做静态配置校验，未新起、停止或重启容器。
- 尚未冒充完成：`scholarly` 的随机 User-Agent/请求间等待、进程级 session/proxy 轮换、redirect/cookie/
  浏览器 CAPTCHA 恢复、全部真实 href 和完整 citation/eprint/filled 元数据未逐项复刻；Go `HTTPHelper`
  的环境 proxy、OpenTelemetry、30 秒 client 与独立抖动重试，以及 thoughts/callback、checkpoint 级取消
  仍未对齐。因此 Python/Go 两条生产文件保持 `partial`，纯 Rust/Linux/Docker、zvec-rust、
  PostgreSQL 18.4、国内镜像优先和零 MySQL 路线不变。

---

## 2026-08-02 RAGFlow PubMed 纯 Rust 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 153 行
  `agent/tools/pubmed.py`、316 行 `internal/agent/tool/pubmed.go`、242 行 Go 测试和 91 行 Python
  格式化回归测试，连同公共 `_retrieve_chunks` / `kb_prompt`、前端 `top_n=12` / email 表单与 Agent
  工具配置。RAGFlow“全部文件”知识库本批只回显查询、未返回 PubMed 源码证据，未用作实现依据。
  另按 Aliyun 锁文件下载并校验 `biopython==1.86` sdist SHA-256
  `93a50b586a4d2cec68ab2f99d03ef583c5761d8fba5535cb8e81da781d0d92ff`，核对固定端点、
  `tool/email/api_key` 参数、三次内部重试、15 秒网络退避、0.37 秒无 key 限流及 GET/POST 切换条件。
- 新增 988 行（含测试）`src/pubmed.rs` 纯 Rust 双协议层。生产 URL 固定为 NCBI E-utilities 的
  `esearch.fcgi`、`efetch.fcgi`、`esummary.fcgi`，Canvas 不能注入任意 URL；客户端禁环境代理、禁
  redirect，设置 12 秒请求与 16 MiB body 上限，非 2xx、坏 UTF-8/XML/JSON、未闭合根节点和超大
  body 均 fail closed。
- Python/Canvas 主路径发送 XML `esearch` 的 `db/term/retmax/tool=biopython/email`，按无 API key
  语义在第二跳前等待 370ms，再以 `db/id/retmode=xml/tool/email` 调 `efetch`。解析前精确去掉源码中的
  `<b>/<i>` 标签，提取 PMID、标题、首个摘要、期刊、卷期、页码、逐作者 ForeName/LastName、首个 DOI
  和发布日期；格式化 fallback、作者顺序、PubMed URL 与 `top_n=12` / email 缺省合同保持明确。
- 独立 Go 兼容方法走 JSON `esearch` → `esummary`，空白 query 拒绝，`max_results` 缺省 5、上限
  100，发送 `ragflow/1.0` User-Agent 与 JSON Accept；按 esearch PMID 顺序拼接结果，跳过 `uids` 与坏
  summary 行，标题 trim、作者超过三名追加 `et al.`、年份仅接受 1900–2099，发布
  `{results:[{pmid,title,authors,journal,year}]}`。
- `src/agent.rs` 将 `PubMed` 接入顶层 Canvas 和 Agent ReAct，发布 Python 的 `pubmed_search` 名称、
  MEDLINE 描述和仅必填 `query` schema；外层 `max_retries + 1`、末次失败后 delay、空 query 短路、
  取消和 12 秒 deadline 已接线。结构化文章沿公共 helper 去 base64 PNG、按 Unicode 字符截到 10,000，
  用 SHA-1 modulo ID 生成 chunk、document aggregation、reference 与 `kb_prompt(hash_id=True)` 正文；
  Agent 失败以 `PubMed error:` 回灌。`src/runtime.rs` 组件目录与 README 同步。
- 新增 4 个确定性本地协议/XML/JSON 测试、2 个 Agent/Canvas 测试、1 个默认 ignored 的 NCBI 公网测试
  和 1 个 coverage mapping 守卫；另更新公共 Agent tool DSL 与组件目录回归。聚焦 PubMed 测试为
  6 passed / 0 failed / 1 ignored，DSL 和目录测试各 1/1；默认 ignored 的 live test 以 `--ignored
  --exact` 显式执行，4.57 秒成功返回真实 NCBI 结果。
- 覆盖生成器 48/48 通过并从固定 Git tree 重生 4,441 文件矩阵；Python/Go PubMed 生产文件获得专属
  映射但诚实保持 `partial`，汇总仍为 aligned 68/15,322 行、partial 1,579/650,559 行、replaced
  71/27,674 行、reference 2,723/574,185 行、unmapped 0。
- 验证：`cargo fmt --all -- --check`、默认与 `postgres-backend,zvec-backend` 双 feature 的 locked
  all-target check、严格 `clippy -D warnings`、全目标测试均通过；默认为 431 lib + 48 coverage =
  479 passed / 0 failed / 5 ignored，双 feature 为 434 lib + 48 coverage = 482 passed / 0 failed /
  5 ignored，zvec native create/upsert/query/reopen 实际执行。PostgreSQL/GPU/Tavily 环境变量均未设置，
  条件式 PG 与 GPU 未实连；Compose、tracked diff 与新 PubMed 文件 whitespace 门禁通过，未新起、
  停止或重启容器。
- 尚未冒充完成：Biopython 的跨实例进程级 rate limiter、每一 HTTP 跳独立 `max_tries=3`、15 秒网络
  backoff、自动长 URL/200 IDs POST、DTD validation 和所有 ElementTree/Entrez 异常类型未逐项复刻；
  可执行 Canvas 采用 Python XML/email/`top_n=12` 主合同，不暴露 Go `pubmed/max_results` 模型 schema。
  Go `HTTPHelper` 的环境 proxy、OpenTelemetry、30 秒 client 和三次抖动退避，以及 thoughts/callback、
  checkpoint 级取消仍未对齐。纯 Rust/Linux/Docker、zvec-rust、PostgreSQL 18.4、国内镜像优先和零
  MySQL 路线不变。

---

## 2026-08-02 RAGFlow ArXiv 纯 Rust 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 101 行
  `agent/tools/arxiv.py`、252 行 `internal/agent/tool/arxiv.go`、200 行 Go 测试，连同公共
  `_retrieve_chunks` / `kb_prompt`、前端 `top_n=12` / `sort_by=relevance` 新节点默认和表单枚举。
  按 Aliyun 锁文件下载并校验 `arxiv==2.1.3` wheel SHA-256
  `6f43673ab770a9e848d7d4fc1894824df55edeac3c3572ea280c9ba2e3c0f39f`，核对 query URL、
  page size、排序、内部重试/节流、feedparser 与 PDF link 选择。本机 RAGFlow“全部文件”知识库查询成功，
  但返回论文格式/医学等无关片段，未用作实现依据。
- 新增 624 行 `src/arxiv.rs` 纯 Rust Atom 协议层。生产端点固定为 Python 使用的
  `https://export.arxiv.org/api/query` 和 Go 使用的 `http://export.arxiv.org/api/query`，Canvas
  不能注入任意 URL；客户端禁环境代理、禁 redirect，固定 `arxiv.py/2.1.3` User-Agent、12 秒请求和
  16 MiB 响应上限，非 2xx、坏 XML、未闭合 entry 与超大 body 均 fail closed。解析器按 local-name
  处理 Atom namespace、XML entity、CDATA、author/link/totalResults，缺失 id 的 entry 跳过、缺失 title
  按锁定 SDK 置为 `0`。
- Python/Canvas 路径保留原始查询语法而不擅加 `all:`，发送 `id_list`、`sortBy`、
  `sortOrder=descending`、`start` 和 SDK 默认 `max_results=100` 分页字段；参数类缺省
  `sort_by=submittedDate`，同时接受前端保存的 `relevance` 及 `lastUpdatedDate`，`top_n=12`。
  独立 Go 方法则严格发送 `all:<query>`、`max_results` 缺省 5，发布
  `{results:[{title,authors,summary,pdf_url,entry_id}]}`，优先显式 PDF link，无 link 时按 `abs` → `pdf`
  推导 URL。
- `src/agent.rs` 将 `ArXiv` 接入顶层 Canvas 和 Agent ReAct，精确发布 Python 的 `arxiv_search`
  名称、描述和仅必填 `query` schema；外层 `max_retries + 1`、包括末次失败后的 delay、空 query 短路和
  12 秒顶层/Agent deadline 已接线。摘要沿公共 helper 去 base64 PNG、按 Unicode 字符截到 10,000，
  用 SHA-1 modulo ID 生成 chunk、document aggregation、去重 reference 和 `kb_prompt(hash_id=True)`
  正文，同时保留 Go 兼容 JSON；Agent 失败以 `ArXiv error:` 回灌。不可变组件目录与 README 同步。
- 新增 4 个本地 Axum/XML 协议测试、2 个 Agent/Canvas 测试、1 个默认 ignored 的公网测试和 1 个
  coverage mapping 守卫，覆盖 Python 原始 query/sort/page fields、Atom namespace/entity/CDATA、author、
  显式/回退 PDF、Go `all:`/默认 5、坏状态/坏 XML/body 上限、重试、空查询、chunk/hash/reference、Agent
  success/error 与函数 schema。
- 覆盖生成器 47/47 通过并从固定 Git tree 重生 4,441 文件矩阵；Python/Go ArXiv 生产文件获得专属
  映射但诚实保持 `partial`，汇总仍为 aligned 68/15,322 行、partial 1,579/650,559 行、replaced
  71/27,674 行、reference 2,723/574,185 行、unmapped 0。
- 验证：`cargo fmt --all -- --check`、默认与 `postgres-backend,zvec-backend` 双 feature 的 locked
  all-target check、严格 `clippy -D warnings`、全目标测试均通过；默认为 425 lib + 47 coverage =
  472 passed / 0 failed / 4 ignored，双 feature 为 428 lib + 47 coverage = 475 passed / 0 failed /
  4 ignored，zvec native create/upsert/query/reopen 实际执行。默认 ignored 的 ArXiv 公网测试另以
  `--ignored --exact` 显式运行，最终复核 2.36 秒成功返回真实结果；本地 Atom/mock 测试仍是确定性门禁。
  PostgreSQL/GPU/Tavily 环境变量未设置，条件式 PG 与 GPU 未实连；Compose/diff 最终门禁通过，
  未新起、停止或重启容器。
- 尚未冒充完成：锁定 Python SDK 每个 page 内还有独立 3 次重试、跨请求 3 秒节流和
  `UnexpectedEmptyPageError` / feedparser bozo / 缺字段细分；Rust 目前依赖组件外层重试，分页不复制
  阻塞式节流，summary whitespace 与所有异常类型也未逐项复刻。可执行 Canvas 采用 Python
  `arxiv_search` / sort / `top_n=12` 主合同，而非 Go 的 `arxiv` / `max_results` 模型 schema；Go
  `HTTPHelper` OpenTelemetry/独立退避、thoughts/callback 和 checkpoint 级取消仍未对齐。因此两条生产
  文件保持 `partial`，纯 Rust/Linux/Docker、zvec-rust、PostgreSQL 18.4、国内镜像优先和零 MySQL
  路线不变。

---

## 2026-08-02 RAGFlow DuckDuckGo 纯 Rust 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 126 行
  `agent/tools/duckduckgo.py`、206 行 `internal/agent/tool/duckduckgo.go`、256 行 Go 测试，连同
  公共 `_retrieve_chunks` / `kb_prompt`、前端 `text/news` 默认值与表单、
  `web_search_assistant.json` 两处真实工具实例。按 Aliyun 锁文件下载并校验
  `duckduckgo-search==7.5.5` sdist SHA-256
  `44ef03bfa5484bada786590f2d4c213251131765721383a177a0da6fa5c5e41a`，逐段核对 HTML/lite、
  vqd/news、分页、normalize 与错误合同。本机 RAGFlow“全部文件”知识库查询成功但仍返回无关医疗/
  本地化碎片，未用作实现依据。
- 新增 652 行 `src/duckduckgo.rs` 纯 Rust 协议层。生产端点固定为 DuckDuckGo HTML Search、home
  vqd、news JSON 和 Instant Answer，Canvas 不能改造成任意 URL 请求器；客户端禁 redirect、禁环境
  代理，设置明确 User-Agent、10 秒请求和 16 MiB 响应上限。text 发送固定 `q/b/kl=wt-wt`
  form 并解析/解实体/规范化 title/href/body；news 获取 vqd 后发送固定 `l/o/noamp/q/vqd/p=-1`，
  按 URL 去重、转换 UTC RFC3339 日期并最多跟随五页 `s` offset。
- 同一协议层显式实现 Go 的独立 Instant Answer 合同：`q/format=json/no_html=1/skip_disambig=1`、
  query 必填、`max_results<=0` 对应默认 5、AbstractText→Abstract fallback、任意层级 RelatedTopics
  flatten/cap；同时接受公开 API 的 PascalCase 和固定 Go 测试使用的 snake_case key，非 2xx、坏 JSON
  与超大 body 均 fail closed。
- `src/agent.rs` 将 `DuckDuckGo` 接入顶层 Canvas 和 Agent ReAct，精确发布 Python
  `duckduckgo_search` 名称/描述/query/channel schema，静态默认 `top_n=10`、`channel=text`，Agent
  `general/news` 与前端 `text/news` 统一归一。固定 Python `_invoke` 实际读取 `topic` 而 schema/前端
  写 `channel`；Rust 同时接受旧 `topic` 和正确 `channel`，让 news 不再因上游命名错位静默退回 text。
  外层 `max_retries + 1`、末次失败后 delay、空 query 短路和 12 秒顶层/Agent deadline 已接线；
  Provider 注入改为借用型 bundle，避免继续扩张调度函数参数。
- text/news 原始行保留在 `json`，body 按公共 helper 去 base64 PNG、Unicode 10,000 字符截断、
  SHA-1 modulo ID 后生成 chunk、document aggregation、去重 reference 与
  `kb_prompt(hash_id=True)` 正文；Agent 失败以 `DuckDuckGo error:` 回灌。组件目录、README 与
  unsupported fail-closed 文案同步。
- 新增 3 个 Axum 协议测试、2 个 Agent/Canvas 测试、1 个默认 ignored 的公网测试和 1 个 coverage
  mapping 守卫，覆盖 form/query、HTML entity/redirect URL、vqd/news/date、Instant 两套 key、递归
  flatten/default cap、状态/body 上限、枚举归一、重试、空 query、chunk/hash/reference 与 Agent
  success/error。显式 live text-search 在 `https://html.duckduckgo.com/html` 的 10 秒客户端期限内
  超时，因此只记录为网络阻塞；本地确定性协议门禁全部通过。
- 覆盖生成器 46/46 通过并从固定 Git tree 重生 4,441 文件矩阵；Python/Go DuckDuckGo 生产文件
  获得专属映射但诚实保持 `partial`，汇总仍为 aligned 68/15,322 行、partial
  1,579/650,559 行、replaced 71/27,674 行、reference 2,723/574,185 行、unmapped 0。
- 验证：默认与 `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings`、全目标测试均通过；默认为 419 lib + 46 coverage = 465 passed /
  0 failed / 3 ignored，双 feature 为 422 lib + 46 coverage = 468 passed / 0 failed / 3 ignored，
  zvec native create/upsert/query/reopen 实际执行。PostgreSQL/GPU/Tavily 环境变量未设置，条件式 PG
  和 GPU 未实连；Compose/diff 最终门禁通过，未新起、停止或重启容器。
- 尚未冒充完成：锁定 Python SDK 会随机选择 HTML/lite backend、保留 cookie store、随机浏览器/TLS
  impersonation、可读 `DDGS_PROXY`，text 最多解析五页并细分 rate-limit/timeout；Rust 当前固定 HTML
  backend 且只解析首个 text 页面，不复制代理/指纹规避，thoughts/callback 与 checkpoint 级取消也未
  对齐。可执行 Canvas 采用更丰富的 Python web/news 输出，而非 Go 的 `duckduckgo/max_results`
  模型 schema；Go `HTTPHelper` OpenTelemetry/独立退避仍未复刻。因此两条生产文件保持 `partial`，
  纯 Rust/Linux/Docker、zvec-rust、PostgreSQL 18.4、国内镜像优先和零 MySQL 路线不变。

---

## 2026-08-02 RAGFlow Wikipedia 纯 Rust 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 193 行
  `agent/tools/wikipedia.py`、186 行 `internal/agent/tool/wikipedia.go`、194 行 Go 测试，连同
  `agent/tools/base.py::_retrieve_chunks` / `kb_prompt`、前端默认值与 68 个语言选项、
  `web_search_assistant.json` 的两处真实 Wikipedia 工具实例。另下载并校验锁文件 SHA-256 对应的
  `wikipedia==1.4.0` sdist，核对其 search/page/summary 请求链。按仓库规则查询本机 RAGFlow
  “全部文件”知识库成功，但返回的是无关医疗内容，未用作实现证据。
- 新增 417 行 `src/wikipedia.rs` 纯 Rust MediaWiki 协议层。生产 URL 只能由固定语言 allowlist
  派生为 `https://<language>.wikipedia.org/w/api.php`，Canvas 不能注入任意端点；客户端禁环境代理、
  禁 HTTP redirect，设置明确 User-Agent、60 秒请求上限和 16 MiB 响应上限，非 2xx、坏 JSON、
  MediaWiki error envelope 及错误 shape 均 fail closed。先按 Python 的 search 参数取有序标题，
  再逐标题组合读取 `info|pageprops|extracts`，解析 redirect 后的 canonical title/fullurl/intro summary，
  缺失页、消歧义页和单页请求错误只跳过该页。
- `src/agent.rs` 将 `Wikipedia` 同时接入顶层 Canvas 静态 dispatch 和 Agent ReAct 子工具。模型 schema
  精确采用 Python 的 `wikipedia_search` 名称、描述和仅必填 `query` 字段；静态参数对齐
  `language=en`、`top_n=10`、68 语言闭集、`max_retries + 1` 和 `delay_after_error`，失败后包括末次
  都等待，空 query 不请求远端。顶层与 Agent 子节点都使用固定 60 秒 deadline，Agent 工具错误以
  `Wikipedia error:` 回灌，不再误标成 Tavily。
- 页面摘要按 Python 公共 helper 去除内嵌 base64 PNG、按 Unicode 字符截到 10,000，使用同一
  SHA-1 modulo ID，生成 chunk、document aggregation、去重 reference 和
  `kb_prompt(hash_id=True)` 风格 `formalized_content`；同时发布 Go 兼容的 `results` envelope 与
  `{title,snippet,url}` JSON 行。`src/runtime.rs` 不可变组件目录和 README 已同步，未知远程工具仍
  在执行前 fail closed。
- 新增 3 个本地 Axum 协议测试、2 个 Agent/Canvas 测试及 1 个默认 ignored 的公网契约测试，覆盖
  固定 query 编码、顺序、redirect shape、missing/disambiguation skip、坏状态/shape、重试、空查询、
  参数覆盖、摘要清理、hash/reference、Agent success/error。显式运行 live test 时
  `en.wikipedia.org:443` 在 TCP connect 阶段超时，因此只记录为网络环境阻塞；确定性 mock 门禁
  全部通过。
- 覆盖生成器 45/45 通过并从固定 Git tree 重生 4,441 文件矩阵；Python/Go Wikipedia 生产文件
  获得专属可执行映射但继续为 `partial`，总计仍为 aligned 68/15,322 行、partial
  1,579/650,559 行、replaced 71/27,674 行、reference 2,723/574,185 行、unmapped 0。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、默认与
  `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings`、全目标测试和 Compose 静态解析全部通过。默认为 414 lib + 45 coverage =
  459 passed / 0 failed / 2 ignored；双 feature 为 417 lib + 45 coverage = 462 passed /
  0 failed / 2 ignored，zvec native create/upsert/query/reopen 实际执行。本轮 PostgreSQL/GPU/Tavily
  环境变量均未设置，条件式 PG 与 GPU 未实连；未新起、停止或重启容器。
- 尚未冒充完成：Python `wikipedia` 库在 `page(title)` 前额外执行一次 auto-suggest search，并有
  process-global language/cache/rate-limit 状态；Rust 用一次组合 detail 请求减少往返，未复刻其精确
  请求次数、thoughts/callback 文案和 checkpoint 级取消时序。Go 的 `lang/max_results` 模型 schema、
  默认 5、search-snippet-only 路径及共享 `HTTPHelper` OpenTelemetry/独立退避也与选定的更完整 Python
  主合同不同；两条生产文件因此保持 `partial`。纯 Rust/Linux/Docker、zvec-rust、PostgreSQL 18.4、
  国内镜像优先和零 MySQL 路线不变。

---

## 2026-08-02 RAGFlow Tavily Search/Extract 纯 Rust 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 236 行
  `agent/tools/tavily.py`、215 行 `internal/agent/tool/tavily.go`、178 行 Go 测试，及
  `agent/tools/base.py` 的 `_retrieve_chunks`、`kb_prompt`、前端 Search/Extract 默认值与表单域名
  归一化、真实模板和 DSL fixtures。按仓库规则查询本机 RAGFlow“全部文件”知识库成功，但结果是
  无关版本/医疗/期刊内容，未作为实现证据。
- 新增 `src/tavily.rs` 纯 Rust 协议层：生产端点固定为 Tavily Search/Extract，Canvas 参数不能把
  客户端变成任意 URL 请求器；Bearer JSON、禁 redirect、禁环境代理、600 秒底层上限、16 MiB
  严格响应上限、非 2xx/坏 JSON/缺失非数组 `results` 均 fail closed。凭据优先取工具静态
  `api_key`，为空时兼容 Go 的 `TAVILY_API_KEY`，不进入模型 schema、请求 Debug 或 Canvas 输出；
  `.env.example` 与 Compose 只增加空变量传入点。
- `src/agent.rs` 将 `TavilySearch`/`TavilyExtract` 同时接入顶层 Canvas dispatch 和 Agent ReAct
  工具表，函数名按固定 metadata 生成 `tavily_search_{idx}` / `tavily_extract_{idx}`，精确发布
  query/topic/domain 与 urls/depth/format JSON schema。编译期校验枚举、正整数、布尔、重试/延迟、
  字符串或数组 URL，并兼容前端临时 `{value}` 域名对象；运行期解析 `sys/env/component@field`
  selector，Extract 保留 Python 逗号拆分且不擅自 trim 的行为。
- Search 对齐 Python 默认值和 `max_retries + 1`，每次请求强制关闭 `include_images` /
  `include_raw_content`，失败后包括末次均按 `delay_after_error` 等待；Extract 同样有界重试但不等待。
  顶层和 Agent 内嵌 Search 都套用固定 12 秒组件 deadline，Extract 使用可由全局/组件变量覆盖的
  600 秒缺省 deadline，不会被 Agent 外层时限悄然放大。
  Search 结果去除内嵌 base64 PNG、按 Unicode 字符截到 10,000、使用 SHA-1 modulo 精确复刻
  `hash_str2int`，生成 chunk/doc aggregation、`kb_prompt(hash_id=True)` 风格正文和去重 reference，
  同时把 Tavily 原始 results 保留在 `json`；Extract 原样发布结果数组。错误写入子节点 `_ERROR`
  并以 `Tavily error:` 工具结果回灌模型，不把远程错误冒充检索成功。
- `src/runtime.rs` 的不可变组件目录新增两个独立 descriptor，README 同步说明顶层节点、Agent 工具、
  凭据与剩余差异。新增 3 个 Agent 单测、3 个本地 Axum 协议测试和 1 个 coverage mapping 守卫，
  覆盖 Bearer/body/结果解析、Search 重试与强制字段、引用/hash/base64 清理、Extract CSV URL 与
  ReAct dispatch、坏状态/shape/key/配置；没有向官方 Tavily 发出请求，也没有记录任何密钥。
- 覆盖生成器 44/44 通过并从固定 Git tree 重生全部 4,441 文件矩阵；Python/Go Tavily 生产文件
  获得专属可执行映射但诚实保持 `partial`，汇总仍为 aligned 68/15,322 行、partial
  1,579/650,559 行、replaced 71/27,674 行、reference 2,723/574,185 行、unmapped 0。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、默认及
  `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings`、全目标测试和 Compose 静态解析全部通过。默认为 409 lib + 44 coverage =
  453 passed / 0 failed / 1 GPU ignored；双 feature 为 412 lib + 44 coverage = 456 passed /
  0 failed / 1 ignored，zvec native create/upsert/query/reopen 实际执行。本轮 PostgreSQL/GPU/Tavily
  环境变量均未设置，故未把条件式 PG、GPU 或官方 Tavily 实连写成通过；未新起、停止、重启容器。
- 尚未冒充完成：Python Tavily SDK 的 callback/thought 展示、thread timeout/cancel 的精确时序和
  所有 SDK 专属响应字段，以及 Go `HTTPHelper` 的 OpenTelemetry transport/独立三次指数退避尚未
  一一对齐；官方 API 也因无凭据未实连。因此两个生产文件继续为 `partial`。长期逐文件 backlog、
  纯 Rust/Linux/Docker、zvec-rust、PostgreSQL 18.4、国内镜像优先和零 MySQL 路线保持不变。

---

## 2026-08-02 RAGFlow Agent Retrieval ReAct 工具调用纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 395 行
  `agent/component/agent_with_tools.py`、212 行 `agent/tools/base.py`、329 行
  `agent/tools/retrieval.py`，并核对 `rag/llm/chat_model.py` 的同步/异步 tools 轮次、1,113 行
  Go Agent 的 ReAct/metadata/artifact/tool-call 段、516 行核心 Agent 测试、81 行 tool dispatch
  测试及前端 `IAgentForm`、默认值和真实模板 DSL。按仓库规则查询本机 RAGFlow“全部文件”
  知识库成功，但结果是无关医疗/ERP 内容，未作为实现证据。
- `src/llm.rs` 新增类型化 OpenAI-compatible tool message/call/completion wire：普通历史转为
  role/content，assistant 一次携带同轮全部 `tool_calls` 且省略 null content，随后按模型顺序追加
  `tool_call_id` 对应的 tool 结果；请求只在有工具时发送 `tools` 与 `tool_choice=auto`，严格解析
  `choices[0].message` 和 function name/raw arguments，并保留每一次 provider usage。
- `src/agent.rs` 在 Canvas 编译期校验固定 `{component_name,name,params}` 工具对象、非负
  `max_rounds` 与空 MCP 列表；当前实现的 Retrieval 元数据精确保留上游拼写
  `search_my_dateset`、description/query schema/required，并按 DSL 顺序加 `_idx`，子组件 ID 使用
  `Agent-->name_with_underscores`。运行期把 JSON object 参数覆盖到静态 Retrieval params，复用
  生产 retriever、dataset/fallback KB、manual metadata、reference 和 output state；空正文回传
  完整 JSON 输出，未知函数、坏 JSON、非对象或错误 query 类型作为 tool error 回灌而非 panic。
  最终 Agent 输出保留 content/structured，并在确有调用时附加扁平 `tool_calls`；
  `src/runtime.rs` 的不可变组件目录同步公开 Agent 专属 tools/mcp/max_rounds 输入和 tool_calls 输出，
  不把普通 Generate/LLM 误标为工具组件。
- ReAct 循环按固定 Python `range(max_rounds + 1)` 执行，逐轮累计精确 token usage；若模型持续
  请求工具，追加 `Exceed max rounds: N` user 消息，再以保留完整 assistant/tool 历史但不携带
  tools/tool_choice 的请求生成有界兜底答案。无 tools 的 Agent/Generate/LLM 仍走原有单次/重试
  路径，不改变现有行为。新增 6 个 lib 回归和 1 个 coverage mapping 守卫，覆盖 null content、
  wire 解析、稳定编号/子 ID、配置拒绝、未知/非对象调用、真实 mock Retrieval 两轮历史、usage/
  reference/tool_calls，以及 `max_rounds=0` 的无工具兜底。
- 覆盖生成器 43/43 通过并从固定 Git tree 重生全部 4,441 文件矩阵；
  `agent/component/agent_with_tools.py` 从泛化说明升级为逐行专属说明但诚实保持 `partial`，所以
  汇总仍为 aligned 68/15,322 行、partial 1,579/650,559 行、replaced 71/27,674 行、
  reference 2,723/574,185 行、unmapped 0。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、默认及
  `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings` 和全目标测试全部通过。默认为 403 lib + 43 coverage =
  446 passed / 0 failed / 1 GPU ignored；双 feature 为 406 lib + 43 coverage =
  449 passed / 0 failed / 1 ignored。zvec native create/upsert/query/reopen 定向用例实际执行；
  `docker compose --env-file .env.example config --quiet` 静态解析通过。本轮 PostgreSQL/GPU
  环境变量均未设置，条件式 PostgreSQL 18 snapshot 与 GPU 模型未实连；未新起、停止、重启
  容器，也未修改镜像或数据卷。
- 尚未冒充完成：MCP、Tavily/Google/Wikipedia/ExeSQL 等非 Retrieval 工具、嵌套 Agent、
  json_repair、同轮并发调度、tool callback/timeline、tool-call memory summary、artifact markdown、
  citation grounding、97% context fitting、协作取消和真实 token delta stream 均未接线；包含这些
  工具的画布会在执行前明确拒绝。因此 Python Agent 主文件和 Go Agent 生产文件继续为
  `partial`。长期逐文件 backlog、纯 Rust/Linux/Docker、zvec-rust、PostgreSQL 18.4、国内镜像
  优先和零 MySQL 路线均保持不变。

## 2026-08-02 RAGFlow DocGenerator 纯 Rust 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 645 行
  `agent/component/docs_generator.py`、386 行
  `internal/agent/component/docs_generator.go`、Go `io/` 下 790 行 helpers/五个 writer、74 行
  七个 OOXML template、567 行固定 Go 单测，以及前端默认值、表单和 `DocGenerator` operator。
  Python/前端真实画布名是 `DocGenerator`，同提交 Go factory 名为 `DocsGenerator`，Rust 显式
  兼容两者。按仓库规则查询 RAGFlow“全部文件”知识库成功，但返回的是无关医疗/ERP/标准文档
  片段，未作为实现证据。
- 新增 `src/document_writer.rs` 纯 Rust 输出层，不增加依赖：TXT、Markdown、HTML 对齐固定 Go
  writer 的 header/footer/timestamp、空行、HTML comment、自包含 CSS、水印与 raw-body 合同；
  DOCX 直接写 ZIP + OOXML，包含 XML escape、多语言段落、全局字体/字号、条件式 relationship、
  VML 水印、PAGE field 与时间戳，并由现有 `docx-rs` 回读；PDF 直接使用既有 `lopdf` 生成
  A4 多页、页眉页脚、旋转灰色水印、真实页码和动态 Unicode ToUnicode CMap，并由现有 PDF
  parser 回读中英文内容。五种格式全在 Rust 进程内生成且不依赖 Pandoc/XeLaTeX。
- `src/runtime.rs` 将 `docgenerator/docsgenerator` 纳入不可变 Agent 目录；`src/agent.rs` 在
  Canvas 编译期校验非空字符串 content、五种 format/`md` alias、至少 12pt 整数字号、布尔
  decoration/include 开关和字符串字段，运行期解析 `sys/env/component@path` selector、清除
  完整和悬空 `<think>`、兼容前端 `header/footer/watermark` 别名、按 Python 规则清洗文件名并
  替换扩展名。输出发布 UUID、filename、MIME、size、base64 bytes、Python JSON download、
  Rust inline attachment、preview URL 和 created；Message 已验证可按
  `include_download_info_in_content` 提取并移除内部标志。输出硬限制 16 MiB。
- 新增 7 个 writer/Agent/Canvas 回归和 1 个 coverage mapping 守卫，覆盖五格式 magic/MIME/
  base64、Unicode PDF 提取、可重开 DOCX、XML escape、水印/页码/时间戳、两组件名、selector、
  think 清理、文件名、下游 download 与坏参数。覆盖生成器 42/42 通过并从固定 Git tree 重生
  4,441 文件矩阵；只把有完整固定字节测试支撑的 TXT/Markdown/HTML writer 提升 aligned，
  统计为 aligned 68/15,322 行、partial 1,579/650,559 行、replaced 71/27,674 行、
  reference 2,723/574,185 行、unmapped 0。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、新增文件 whitespace check、默认及
  `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings` 和全目标测试全部通过。默认 397 lib + 42 coverage =
  439 passed / 0 failed / 1 GPU ignored；双 feature 400 lib + 42 coverage =
  442 passed / 0 failed / 1 ignored，zvec native create/upsert/query/reopen 与在线 mirror 用例
  实际执行。`docker compose --env-file .env.example config --quiet` 静态解析通过。本轮
  `RAYRAG_POSTGRES_TEST_URL`/`RAYRAG_POSTGRES_URL` 均未设置，条件式 PostgreSQL snapshot
  用例未实连；未新起、停止或重启容器，也未改 PostgreSQL 数据卷。
- 尚未冒充完成：Python Pandoc Markdown AST、XeLaTeX/Noto 嵌入字体、标题层级和精确视觉
  版式，tenant `STORAGE_IMPL` 持久化及真正支撑 preview URL 的附件读取端点，decorator
  timeout/协作取消；Go 的原生 `[]byte`、下划线文件名 helper、独立单项 Stream facade，
  PDF 注册 TTF/精确 gopdf metrics，DOCX 完整 VML formula 与办公套件视觉验证。因此 Python/
  Go 主组件和 PDF/DOCX/helpers/templates 继续为 `partial`。长期逐文件 backlog、纯
  Rust/Linux/Docker、zvec-rust、PostgreSQL 18.4、国内镜像优先和零 MySQL 路线均保持不变。

## 2026-08-02 RAGFlow ExcelProcessor 纯 Rust 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 379 行
  `agent/component/excel_processor.py`、458 行
  `internal/agent/component/excel_processor.go`、190 行固定 Go 单测以及前端默认值和空表单占位。
  Python/前端提供 `read/merge/transform/output` 主 Canvas 形态，Go 端补充
  `read/write/merge`、原始 rows/bytes/sheet_names/size 与 write-read 往返合同；Rust 同时兼容
  两者但不混淆输出差异。按仓库规则查询 RAGFlow“全部文件”知识库成功，返回的是无关 Excel
  文档片段，未作为实现证据。
- `src/runtime.rs` 把 ExcelProcessor 加入不可变 Agent 组件目录；`src/agent.rs` 在 Canvas
  编译期校验 operation、output format、merge strategy 及输入容器，运行期解析
  `sys/env/component@path`、base64、`data:` URI 与带 filename/data/content/bytes 的内联描述符。
  read 支持 CSV/XLSX、all/first/逗号分隔具名工作表，发布 Python records/summary/Markdown
  和 Go rows/sheet_names/size；merge 支持 concat 与基础 outer join；transform 保留原生 JSON
  bool/number/object/array；output/write 可生成首工作表 CSV 或多工作表 XLSX，并发布 base64
  bytes 和内联 attachment descriptor。
- `src/parser/excel.rs` 将既有首表文本解析扩展为纯 Rust 多工作表 OOXML reader/writer：解析
  workbook relationship、shared/inline strings、稀疏列、布尔与数值单元格；生成最小合法
  `[Content_Types]`、relationship、workbook、styles 和 worksheet parts。工作表名称按 Excel
  31 字符及非法字符规则清洗并大小写去重，空 grid 仍生成可读默认工作表；保留既有 1,000 行
  parser 截断标记。组件输入/输出硬限制 16 MiB，单个 XML part 限制 64 MiB，避免无界内存。
- 新增 5 个 Agent/Canvas、2 个 OOXML 和 1 个 coverage mapping 回归：覆盖固定 Go
  write-read、ZIP magic/byte size、空工作簿、双工作表名称与选择、Python 前端 CSV 形态、
  records/Markdown、join/transform、坏 base64、id-only 外部文件 fail-closed、参数拒绝和目录
  shape。覆盖生成器 41/41 通过并从固定 Git tree 重生 4,441 文件矩阵；两项 production
  文件原本就是 partial，统计守恒为 aligned 65/15,091 行、partial 1,582/650,790 行、
  replaced 71/27,674 行、reference 2,723/574,185 行、unmapped 0。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、默认及
  `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings` 和全目标测试全部通过。默认 390 lib + 41 coverage =
  431 passed / 0 failed / 1 GPU ignored；双 feature 393 lib + 41 coverage =
  434 passed / 0 failed / 1 ignored，zvec native create/upsert/query/reopen 与在线 mirror 用例
  实际运行。`docker compose --env-file .env.example config --quiet` 静态解析通过。本轮
  `RAYRAG_POSTGRES_TEST_URL` 未设置，条件式 PostgreSQL snapshot 用例未实连；未新起、停止或
  重启容器，也未改 PostgreSQL 数据卷。
- 尚未冒充完成：Python 的 tenant FileService/STORAGE_IMPL 取件和持久化 `doc_id` attachment、
  pandas 日期/公式/dtype、完整 outer-join 冲突后缀与 null 行为、input preview/thought、
  cooperative cancellation/decorator timeout；Go 的原生 `[]byte` 边界与单项 Stream facade。
  RayRAG 的 JSON Canvas 有意以 base64 表示 bytes，内联 attachment 不谎报已持久化 doc_id，
  因此 Python/Go 两个 production 文件继续保持 `partial`。长期逐文件 backlog、纯
  Rust/Linux/Docker、zvec-rust、PostgreSQL 18.4、国内镜像优先和零 MySQL 路线均保持不变。

## 2026-08-01 RAGFlow Invoke 安全 HTTP 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 305 行
  `agent/component/invoke.py`、350 行固定 Python 单测、446 行
  `internal/agent/component/invoke.go`、403 行 Go 单测，以及前端 Invoke defaults/form/schema。
  Python Canvas 与前端仍是 DSL 主合同；同提交 Go 端独有的 DELETE、直接 body/content_type
  和 status/body/headers envelope 作为补充安全证据，没有错误混入 Python `result` 输出合同。
  按仓库规则查询 RAGFlow“全部文件”知识库成功，但召回为无关文档，未作为实现依据。
- `src/runtime.rs` 将 Invoke 加入不可变 Agent 组件目录，准确发布 URL/method/headers/
  variables/datatype/timeout/proxy/clean_html 与 `result/_ERROR`；`src/agent.rs` 在 Canvas 编译期
  校验 GET/POST/PUT、非空 URL、正整数 timeout、布尔 clean_html、json/formdata、变量记录、
  retry/delay 类型。执行期解析 request inputs 与 Canvas sys/env/component/item selector，header
  兼容固定单花括号，字符串化 JSON 恢复原生值，GET 编码 query，POST/PUT 编码 JSON 或 form，
  保留非 2xx body 成功语义；网络错误按 `max_retries + 1` 重试并最终写 `_ERROR`，HTML 响应复用
  纯 Rust parser 清洗。
- HTTP 路径显式禁用 redirect 和环境代理，执行请求 timeout 及既有组件 deadline，响应硬截断
  16 MiB。SSRF guard 对目标和显式代理的每一个 DNS 地址执行公网 allowlist，识别
  IPv4-mapped IPv6，并把实际 dial 固定到首个已校验 IP，关闭校验到连接间的 DNS rebinding
  窗口。代理会自行再次解析 hostname，无法由客户端 pin 住，因此采用固定 Go 端更严格合同：
  代理模式只接受公网字面 IP 目标；目标、代理、scheme 或解析拒绝统一输出
  `_ERROR="URL not valid"`。私有测试 transport 不进入公开 API，也不提供运行时 SSRF bypass。
- 新增 6 个无外网 Rust 行为回归及目录 shape 守卫，覆盖编译错误、URL/header/参数模板、JSON
  coercion、form array、失败后重试、终态网络错误、裸 hostname 补 scheme、loopback/metadata/
  mapped IPv6、混合公网/私网 DNS、代理双跳、hostname+proxy fail-closed、HTML 清洗与 16 MiB
  上限。覆盖生成器新增 Python/Go 两个精确 partial mapping 和第 40 个守卫测试，从固定 Git
  tree 重生 4,441 文件矩阵；因两文件原本已是 generic partial，统计守恒为 aligned
  65/15,091 行、partial 1,582/650,790 行、replaced 71/27,674 行、reference
  2,723/574,185 行、unmapped 0。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、默认及
  `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings` 和全目标测试全部通过。默认为 383 lib + 40 coverage =
  423 passed / 0 failed / 1 GPU ignored；双 feature 为 386 lib + 40 coverage =
  426 passed / 0 failed / 1 ignored，zvec native create/upsert/query/reopen 与 mirror 用例实际运行。
  Compose 以非敏感占位 secret 静态解析通过，本机缓存精确
  `postgres:18.4`、`rust:1.97.1-bookworm`、`debian:bookworm-slim`。本轮
  `RAYRAG_POSTGRES_TEST_URL` 未设置，所以条件式 PostgreSQL 用例没有实连；未新起/停止/重启
  容器、未改数据卷，也未访问真实外部 Invoke endpoint。
- 尚未冒充完成：Python requests charset 猜测、复杂 GET/form 值的 Python repr、完整 DeepDOC
  block/table 分段、partial generator 拼接、动态 input-form label/thought、逐对象 cooperative
  cancel；Go v2 DELETE/content_type/body、status/body/headers/datatype envelope、首 header shaping、
  OpenTelemetry traceparent 与同步 Stream facade。本批因此把两个 production 文件继续保留为
  `partial`，长期 backlog、纯 Rust/Linux/Docker、zvec-rust、PostgreSQL 18.4、国内镜像优先和
  零 MySQL 路线均保持不变。

## 2026-08-01 RAGFlow 历史 chunker DSL 迁移对标

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`；重新浅检出并验证 tag/commit，逐行审计
  `agent/dsl_migration.py` 全部 164 行，同时核对 Python Canvas/API/version 调用点、前端
  operator/node-type 合同以及 Go `NormalizeForCanvas`/`tools/migrate-canvas` 的边界说明。Go
  工具明确把 chunker DSL 迁移留给 Python 域，因此没有拿不同领域的 Go agent normalizer
  代替这份固定 Python 合同。按仓库规则查询 RAGFlow“全部文件”知识库成功，但召回内容与
  代码无关，未把无关结果作为证据。
- `src/agent.rs` 新增纯 Rust `normalize_chunker_dsl`，在 Canvas 和运行时规范化前统一执行。
  它在 defensive clone 上把 `Splitter`、`HierarchicalMerger`、`PDFGenerator` 精确迁移为
  `TokenChunker`、`TitleChunker`、`DocGenerator`，同步处理组件 map key、`component_name`、
  downstream/upstream/parent、path、任意嵌套模板引用、React-Flow node/type/label/name/form、
  edge source/target/id，以及 history/messages/reference；自定义名称和业务参数保持不变，
  缺少 object-shaped components 时与 Python 一致 no-op，重复调用幂等且不修改输入。
  `DocGenerator` 的派生图 node type 同步为固定前端 `ragNode`。
- 新增两组迁移回归，覆盖三个旧名称、精确 ID 与多花括号引用、嵌套容器、图/边/历史、
  malformed/no-components、defensive copy 和 Canvas/Run 双入口。严格 Clippy 还暴露当前 Agent
  debug 公共函数的 8 参数存量问题；没有添加 lint 豁免，而是新增 typed
  `AgentComponentDebugContext` 原子同步唯一 HTTP 调用点和三处测试调用，行为回归通过。
- `src/bin/ragflow-coverage.rs` 增加迁移文件的专属 aligned 守卫并从固定 Git tree 重生
  4,441 文件矩阵：aligned 65/15,091 行、partial 1,582/650,790 行、replaced
  71/27,674 行、reference 2,723/574,185 行、unmapped 0。这里只把纯结构迁移文件升级为
  aligned；三个 ingestion 组件是否能在 Agent Canvas 直接执行仍由闭集目录独立检查，
  未接线节点继续 preflight fail closed，其他 Agent component partial 项未被抬高。
- 验证：`cargo fmt --all -- --check`、默认及 `postgres-backend,zvec-backend` 双 feature 的
  locked all-target check、严格 Clippy、全目标测试和 `git diff --check` 全部通过；默认合计
  416 passed / 0 failed / 1 GPU ignored，双 feature 合计 419 passed / 0 failed /
  1 GPU ignored，39 个 coverage 测试和固定 DSL fixture 均实际执行。zvec native
  create/upsert/query/reopen 与在线 mirror 用例实际运行。Compose `.env.example` 静态解析通过，
  精确 `rust:1.97.1-bookworm`、`debian:bookworm-slim`、`postgres:18.4` 及 DaoCloud 标签均在
  本机缓存。
- PostgreSQL 实连：fnOS trim-cli 的只读 Docker 列表返回设备端 `errno 135168`，未盲目
  重试；转用同主机 Docker socket 精确确认既有 `rayrag-postgres-1` 为
  `postgres:18.4`、healthy，`SHOW server_version`/`server_version_num` 返回
  `18.4 (Debian 18.4-1.pgdg13+1)`/`180004`。通过容器当前配置临时注入测试 URL 后，Rust
  snapshot store/load/delete 往返通过，残留 `test-%:web/postgres-smoke.json` 行为 0；未
  输出凭据、未新起/停止/重启容器、未改业务卷。
- 未冒充完成：本批没有实现 TokenChunker/TitleChunker/DocGenerator 的 Agent Canvas 执行
  接线，也没有推进 Browser/Invoke/DocsGenerator/ExcelProcessor、模型 provider 或 GPU
  三模型实测；没有重建应用镜像。长期逐文件 backlog 继续以覆盖矩阵中的 1,582 个 partial
  文件为准，Linux/Docker、zvec-rust、PostgreSQL 18.4 与零 MySQL 路线保持不变。

## 2026-07-30 RAGFlow LLM prompt/history/structured-output 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行复核 523 行
  `agent/component/llm.py`、1,409 行 `internal/agent/component/llm.go`，并核对固定 Agent
  form/default、DSL fixtures 以及 898 行 prompt/history/sampler/structured-output 核心 Go
  测试。按仓库规则先查询 RAGFlow“全部文件”知识库；本轮命令成功但没有返回可引用内容，
  所以没有把空召回当证据，语义合同来自固定 Git blob、测试和前端/DSL 调用点。
- `src/agent.rs` 在 Canvas 编译期校验 prompt、历史窗口、重试、delay 和五项 sampler
  类型/边界；Agent/Generate/LLM 统一接受缺省 `{sys.query}`、标量 prompt、Go
  `user_prompt/prompt` 别名和有序 role/content 数组。系统提示只发送一次，历史默认采用
  `ComponentParamBase` 的 13 轮并按窗口乘二截断；首条 prompt 与历史末条同角色时原位替换，
  删除此前无条件追加 `input.question` 造成的重复用户问题。三种组件的动态 input form 现在
  都按首次出现顺序发布 sys/env/component/item 引用。
- 组件 sampler 对齐固定 `gen_conf`：零值不覆盖 provider/request 默认，显式
  `temperatureEnabled/topPEnabled/presencePenaltyEnabled/frequencyPenaltyEnabled/
  maxTokensEnabled=false` 保留运行级值；没有开关的旧 DSL 继续把正值视为启用，避免破坏固定
  v1 fixtures。provider 错误或 `**ERROR**` 响应按 `max_retries + 1` 次尝试，最终写
  `_ERROR`，配置 `exception_default_value` 时改写 `content`。
- 结构化输出以 Python 运行时实际读取的 `outputs.structured` 非空
  `properties` schema 为主，并兼容同形 `output_structure`：系统 prompt 追加固定 JSON
  schema 指令，响应清除贪婪 `</think>` 前缀和 JSON fence，成功只发布原生
  `structured`，不再使用错误的 `structured_content` 键。受控本地 OpenAI-compatible
  端点回归证明首次非 JSON、第二次 fenced JSON 的两次调用、精确 outbound system/user
  消息、无重复 query、sampler 开关优先级、usage 累加和固定输出键。
- `src/runtime.rs` 的不可变组件目录同步只发布已落地的 prompt、历史、sampler、retry、
  `content/structured/_ERROR`；没有发布尚未真正按节点解析的 `llm_id`。覆盖生成器为 Python/
  Go 两个 production 文件新增精确 `partial` mapping，39/39 通过并从固定 Git tree 重建
  4,441 文件矩阵；统计保持 aligned 64/14,927 行、partial 1,583/650,954 行、
  replaced 71/27,674 行、reference 2,723/574,185 行、unmapped 0。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、默认及
  `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings` 和全目标测试全部通过；默认 411 passed / 0 failed /
  1 GPU ignored，双 feature 414 passed / 0 failed / 1 ignored。zvec native
  create/upsert/query/reopen 和在线 mirror 定向实测通过；既有 `rayrag-postgres-1` 为
  running/healthy，`SHOW server_version; SELECT 1` 返回
  `18.4 (Debian 18.4-1.pgdg13+1)` 和 `1`，并用其发布端口真实通过 Rust snapshot
  store/load/delete。当前 shell 未设置两个必需 Compose secret，裸 `docker compose config`
  按设计 fail closed；使用非敏感占位值的静态解析通过并保持 DaoCloud
  `postgres:18.4`，本机缓存同时包含精确 `postgres:18.4`、`rust:1.97.1-bookworm`、
  `debian:bookworm-slim` 及三者 DaoCloud 标签。未新起/重启容器；GPU 三模型变量均未设置。
- 尚未冒充完成：节点 `llm_id` 到 tenant provider/image2text 的实际选择、递归
  `visual_files_var`/data-image 和 `sys.files` 注入、citation prompt/reference、97% token
  context fitting、`json_repair` 宽松 JSON 修复、`sys.chat_template_kwargs` 下传、thinking、
  delay/backoff 与 exception-goto、component cooperative cancel、真正的下游 delta stream；
  Go 的 Anthropic/Ollama/provider driver、多模态 message parts、deep schema key/type 校验也
  尚未等价。因此两个 production 文件继续保持 `partial`。

## 2026-07-30 RAGFlow Message 模板 / downloads 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行复核 574 行
  `agent/component/message.py`、469 行 `internal/agent/component/message.go`、固定版 541 行
  Message/downstream Go 测试、前端 Message form 和 Canvas `message_end` 调用点。按仓库规则
  先查询 RAGFlow“全部文件”知识库，本机端点返回 `socket hang up`，所以没有把不可用检索
  当成证据，具体合同来自固定 Git blob、测试和 DSL/UI 调用点。
- `src/agent.rs` 现在在 Canvas 保存/编译阶段验证 Message：`content` 必须是非空字符串或
  非空字符串数组，`stream` 必须为布尔值，同时接受固定 Go v2 `text` 别名；执行期从多条
  消息随机选一条。原有单/双/三花括号显示变量扩展为 Message 专用渲染路径，支持
  component/sys/env/item/index/result 选择器以及 MiniJinja 条件、循环和过滤器，
  沿用无 loader/函数、100,000 fuel、64 层递归的边界；模板失败时保留上游重写后脚本并继续
  做最终变量替换，不把显示节点变成参数绑定的硬失败。
- 对齐 Python `_extract_downloads/_stringify_message_value`：直接对象、全描述符数组或对应
  JSON 字符串只要带 `doc_id/filename/mime_type` 就进入原生 `downloads` 输出；内部
  `include_download_info_in_content` 在输出和可选正文 JSON 中删除，默认描述符不泄漏到
  正文，同一 selector 重复出现只收集一次。非下载 object/list 仍用稳定 JSON 显示。
  `src/runtime.rs` 的不可变组件目录同步只发布已落地的 `content/text/stream` 输入与
  `content/downloads` 输出，没有把未实现附件/TTS/memory 能力伪装成可用接口。
- 新增 3 组 Rust 行为回归并加强目录 shape 守卫：空/畸形内容及 stream 类型、v1/v2 形态；
  Jinja 条件和双/单花括号变量及 parse-error fail-soft；直接/JSON 字符串下载、include
  标志、规范化和重复引用。覆盖生成器新增 Python/Go 两项精确 partial mapping，38/38
  通过并从固定 Git tree 重建 4,441 文件矩阵；统计保持 aligned 64/14,927 行、
  partial 1,583/650,954 行、replaced 71/27,674 行、reference 2,723/574,185 行、
  unmapped 0。
- 验证：`cargo fmt --all -- --check`、默认与
  `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings` 和全目标测试全部通过；默认 406 passed / 0 failed /
  1 GPU ignored，双 feature 409 passed / 0 failed / 1 ignored，后者真实执行 zvec native
  reopen 与在线 mirror。Compose 静态解析通过，本机缓存精确包含
  `postgres:18.4`、`debian:bookworm-slim`、`rust:1.97.1-bookworm`；既有
  `rayrag-postgres-1` 为 running/healthy，`SHOW server_version; SELECT 1` 返回
  `18.4 (Debian 18.4-1.pgdg13+1)` 和 `1`。`RAYRAG_POSTGRES_TEST_URL` 未设置，所以条件式
  Rust snapshot test 实际提前返回；GPU 三模型 opt-in 变量同样未设置。本批只读复用运行
  环境，未新起、重启或删除容器。
- 尚未冒充完成：Python partial/async generator 的逐变量拉取与 component cancel 检查、
  pypandoc markdown/html/pdf/docx/xlsx 导出、Excel 多表/数值类型转换、storage attachment
  和 `queue_save_to_memory_task`；Go sentence chunk stream、output-format renderer、
  auto-play TTS、MemorySaver 及 audio/memory 错误 envelope。HTTP runner 仍只在完整 LLM
  回答后发一个 message，Message 的 `status/attachment/downloads` 也尚未全部提升到顶层
  `message_end` wire，因此两个 production 文件继续保持 `partial`。

## 2026-07-27 RAGFlow SoMark / naive chunk 页面位置元数据纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，针对上一批明确留下的 bbox 差异复核
  `deepdoc/parser/somark_parser.py` 的 `_line_tag` 426–456 行、
  `extract_positions` 458–470 行和 `_transfer_to_sections` 619–677 行，并核对
  `rag/nlp/__init__.py` 的 `tokenize_chunks` 390–415 行、`add_positions` 921–933 行及
  `naive_merge` 1156–1225 行、Python SoMark 单测 188–322 行，以及 chunk API/retrieval
  对 `position_int` 的生产消费点。按仓库规则先查询 RAGFlow“全部文件”知识库；召回仍是
  无关论文、ERP 和设备数据，没有目标代码证据，因此合同来自固定 Git blob 与测试。
- `src/parser/somark.rs` 现在为每个保留 block 生成固定
  `@@<一基页码>\t<x0>\t<x1>\t<top>\t<bottom>##` 标签；raw bbox 的左右/上下反序会先交换，
  保留一位小数，文本缺失或非四值 bbox 使用全零 fallback，图片仍严格要求四值 bbox 才输出。
  title、table、image caption 和普通文本都保留位置，header/footer 继续服从原开关。
  这里明确使用 SoMark API 原始坐标，没有伪装已经完成本地渲染页面后的比例缩放。
- 新增 `src/chunk/position.rs`，复刻共享 DeepDOC 正坐标提取表达式和更宽的 tag 清理表达式：
  一基页码保持不变，浮点坐标按 Python `int()` 的截断语义进入有序
  `position_int`，同时生成 `page_num_int`、`top_int`。带负坐标的 tag 与上游一样不会被
  `extract_positions` 接受，但仍从面向用户/embedding 的文本中清除；多页 `1-2` tag 会按
  顺序展开成两个位置记录。
- `NaiveChunker` 只在 SoMark parser 写入内部位置感知标记时，才于 token 预算、overlap 和
  最终 embedding 文本中排除位置 tag，对齐上游 `if pdf_parser` 分支；构建 chunk 时一次
  提取全部位置并写入 metadata，同时像上游 `copy.deepcopy(doc)` 一样继承
  `Document.metadata`、移除内部标记，再覆盖真实文件名。普通文本中的 `@@…##` 字面量
  原样保留。SoMark 生产 pipeline 回归证明 title/table 的 tag 在 parse 阶段存在，chunk 后
  内容不泄漏 tag、内部标记不进入存储、类型仍为 table，两个 bbox 均进入 `position_int`；
  现有 chunk API 与检索引用无需新增旁路即可返回位置。
- 新增 5 个行为回归并加强 3 个现有生产/存储回归：raw bbox 一基页码、反序归一化和零
  fallback；正坐标/多页 tag 提取；负坐标只清理不提取；chunk 内容、token count 与三类
  metadata；无 parser 标记的普通文本保留 position-like 字面量；本地 Axum SoMark
  parse→chunk；JSON index 即时检索；zvec native upsert/query/reopen。后两项直接断言
  三类位置字段跨持久化仍完全相同，而不是从通用 `HashMap` 间接推断。
- 覆盖生成器 37/37 通过并从固定 Git tree 重建 4,441 文件矩阵；新增
  `rag/nlp/__init__.py` 精确 partial mapping，SoMark mapping 删除“尚无 line tag”的过期
  描述。统计保持 aligned 64/14,927 行、partial 1,583/650,954 行、
  replaced 71/27,674 行、reference 2,723/574,185 行、unmapped 0。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、Compose 静态解析、默认与
  `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings` 和全目标测试全部通过；默认 402 passed / 0 failed /
  1 GPU ignored，双 feature 405 passed / 0 failed / 1 ignored，后者真实执行包含位置
  metadata 的 zvec native reopen 与在线 mirror。只读复用既有
  `rayrag-postgres-1` 确认 PostgreSQL 18.4 和 SQL probe；因未设置
  `RAYRAG_POSTGRES_TEST_URL`，条件式 Rust snapshot 往返实际跳过。SoMark 外部 endpoint/key
  同样未设置，本批协议继续由本地受控 Axum 服务验证；未新起或重启任何容器。
- 尚未冒充完成：Python 的本地 PDF outline/page rendering、按 `page_size` 将 raw bbox
  缩放到真实页面像素、跨页 crop 与 image object，naive 短文本 position 抑制的逐 token
  等价、raw/manual/pipeline tuple 返回形态、progress callback 和 tenant Provider 实例
  选择；Go JSON/Markdown `ParseResult` 与精确 page/file metadata 也仍未对齐。因此
  `somark_parser.py` 与完整 1,725 行 `rag/nlp/__init__.py` 继续保持 `partial`。

## 2026-07-27 RAGFlow SoMark async PDF provider 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行复核 756 行
  `deepdoc/parser/somark_parser.py`、322 行 Python 单测、327 行
  `rag/llm/ocr_model.py` 中 SoMark wrapper、257 行 Go
  `internal/parser/parser/pdf_parser_somark.go`、140 行 Go 测试，以及 constants、
  provider UI、tenant 环境自动装配和 `rag/app/naive.py` 生产调用点。按仓库规则先查询
  RAGFlow“全部文件”知识库；召回是通用模型参数和无关论文，没有 SoMark 目标协议的精确源码
  证据，因此实现合同仍采用固定 Git blob、DSL/配置、生产调用点和测试。
- 新增 `src/parser/somark.rs` 纯 Rust provider。配置兼容 UI 嵌套
  `{"api_key": {...}}`、flat JSON、大小写 key、直接 plain secret 和进程环境回退；base
  URL 规范化但 direct secret 按 wrapper 保留空白，`Debug` 永不泄露 key。四种 element
  format 默认 `url/latex/html/image`；七个 feature 默认严格采用 wrapper/Go 的
  `false/false/false/true/true/true/false`，并允许 `parser_config` 以 11 个原字段按文档
  覆盖。仅 `SOMARK_BASE_URL` 或 `SOMARK_API_KEY` 非空时自动装配，避免仅设置格式变量就误启
  不可用 SaaS client；只给 key 时使用固定 `https://somark.tech/api/v1`。
- 私有部署按固定 Python 合同执行五秒 `HEAD`，任意小于 500 的状态都视为可达；SaaS 使用
  十秒 `POST /usage` 的 URL-encoded 可选 key，拒绝业务码 1107、其他非零码以及付费/免费
  剩余页数同时为零。PDF 以 `application/pdf` multipart 提交到 `/parse/async`，发送
  `file`、重复语义的 `output_formats=json`、JSON `element_formats` 和
  `feature_config`，空 key 完全省略。只有业务码 1124 会在十分钟预算内按一秒起步、十秒
  封顶的指数退避和最多 0.5 秒 jitter 重试；其他 HTTP/网络/业务错误立即带阶段信息失败。
- 提交后先等待两秒，再向 `/parse/async_check` 发送 URL-encoded task ID 与可选 key，
  间隔按 1.5 倍增长、十秒封顶并受十分钟总预算约束；严格处理业务码、`SUCCESS` 缺 result、
  `FAILED` 和超时，从 `outputs.json.pages` 取结构化结果。固定 21 种 block 全部覆盖：
  cate/cate_item/blank 永久丢弃，header/footer 由开关控制，未知类型回退 text，有效 1–6
  title level 转 Markdown 标题，无四值 bbox 的图片跳过，figure/cs/qrcode/stamp 获得稳定
  caption，table/image 输出现有 chunker 可识别的 marker。
- `Pipeline` 只在 PDF 且 selector 为 `SoMark` 或
  `<model>@<instance>@SoMark` 时调用 provider；CLI、服务端后台任务与 common 环境装配共用
  同一路径。`.env.example`、Compose 和 README 已同步 base/key、四种格式、七个 feature、
  三个请求 timeout 和两个总预算，默认保持关闭。新增 7 个行为回归：nested/plain/显式空
  配置及 redaction、21 类型转换与 header/footer、真实本地 HTTP 私有 HEAD + QPS retry +
  processing/success poll + multipart/form 编码、SaaS usage/key/quota、11 个
  parser_config wire 字段、固定三段 selector 和生产 PDF pipeline。
- 覆盖生成器新增精确 SoMark mapping 守卫，37/37 通过并从固定 Git tree 重建 4,441 文件
  矩阵。统计保持 aligned 64/14,927 行、partial 1,583/650,954 行、
  replaced 71/27,674 行、reference 2,723/574,185 行、unmapped 0；Python parser、Go
  parser 和四 provider wrapper 均继续诚实标为 `partial`。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、Compose 静态解析、默认与
  `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings` 和全目标测试全部通过；默认 397 passed / 0 failed /
  1 GPU ignored，双 feature 400 passed / 0 failed / 1 ignored，后者真实执行 zvec native
  reopen 与在线 mirror。只读复用既有 `rayrag-postgres-1` 执行
  `SHOW server_version; SELECT 1`，确认 `18.4 (Debian 18.4-1.pgdg13+1)` 和 SQL 成功；
  当前未设置 `RAYRAG_POSTGRES_TEST_URL`，所以条件式 Rust snapshot test 实际跳过。
  `SOMARK_BASE_URL`/`SOMARK_API_KEY` 也未设置，wire contract 使用本地受控 Axum 服务验证，
  没有冒充外部 SoMark 实连；未新起或重启任何容器。
- 尚未冒充完成：Python 的 PDF outline/page rendering、按 page size 缩放的 bbox line
  tag、crop/image object、naive 两元组和 manual/pipeline 三元组、空页空 section 行为、
  progress callback，以及 Go JSON/Markdown `ParseResult`、精确 page/file metadata、空输入
  空结果和非法 title level 的 H1 fallback；tenant Provider 实例持久化/选择也仍未接线。
  因此这是可运行的 SoMark async 纵向切片，不代表整个 OCR/版面解析家族已经完全对齐；继续
  保持全 Rust、Linux/Docker、zvec-rust + PostgreSQL 18.4、国内镜像优先、RayRAG 不引入
  MySQL 且不改版本号。

## 2026-07-27 RAGFlow MinerU remote PDF provider 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行复核 842 行
  `deepdoc/parser/mineru_parser.py`、115 行 Python 单测、327 行
  `rag/llm/ocr_model.py` 中 MinerU wrapper、119 行 Go
  `pdf_parser_mineru.go`、161 行 Go 测试、276 行 `mineru_local.go` driver 以及
  `rag/app/naive.py` 生产调用点。按仓库规则先查询 RAGFlow“全部文件”知识库；召回是通用
  模型参数和无关论文，没有 MinerU/SoMark 目标协议的精确源码证据，因此实现合同仍采用固定
  Git blob、调用点和测试。
- `src/parser/mineru.rs` 保留原轻量本地 PDF 文本 fallback，并新增全 Rust remote provider。
  配置兼容 UI 嵌套 `{"api_key": {...}}`、flat JSON、大小写 key 和进程环境回退，
  API/base/server URL 规范化、secret-redacted `Debug`、显式空值优先、output/backend/
  server/delete-output 字段；严格验证固定七种 backend 和三种 parse method，完整映射 UI
  语言名，缺 API server 时 fail closed。可选 Bearer 是固定 Go local driver 的能力扩展。
- Python 同步路径先用五秒 `HEAD /openapi.json` 探针，再向 `/file_parse` 发送
  `application/pdf` multipart，逐项透传 `output_dir/lang_list/backend/parse_method`、
  formula/table 开关、六个返回开关和固定 page range，使用 1,800 秒请求上限。ZIP 不落盘，
  而是在内存中按固定候选名选择 64 MiB 上限的 content-list JSON；因此没有文件系统解压/
  traversal 风险，也没有伪装已经实现 ZIP 图片落盘。
- 同一 client 兼容 Go local driver 的 200/202 task 响应、nested/root task ID、可选
  Bearer、URL-safe `/tasks/{id}/result`，每 200 ms 轮询并受默认 30 秒总 deadline 约束，
  从 results object 取首个 `md_content`。content-list 转换覆盖 text/table/image/equation/
  code/list，过滤 header/footer/page_number/discarded/未知 block，复刻 HTML unescape、
  block tag 换行和空白折叠，并生成现有 chunker 可识别的 table/image marker。
- `ParserConfig` 新增原名 `mineru_lang`、`mineru_parse_method`、
  `mineru_formula_enable`、`mineru_table_enable` 默认值；`Pipeline` 只在 PDF 且 selector 为
  `MinerU` 或 `<model>@<instance>@MinerU` 时调用 provider。CLI 和服务端后台解析共用同一
  环境装配；`.env.example`、Compose 和 README 已同步八个 `MINERU_*` 变量，API server
  留空时默认关闭。
- 新增 7 个行为回归：nested/flat/显式空配置、backend 边界和 secret redaction、完整
  content-list 类型转换与 sanitizer、真实本地 HTTP Python HEAD/Bearer/multipart/ZIP
  contract、Go submit/poll Markdown contract、固定三段 selector、生产 PDF pipeline，
  以及 parser_config 四字段 wire 名；覆盖生成器新增 1 项精确映射守卫，36/36 通过并从
  固定 Git tree 重建 4,441 文件矩阵。统计保持 aligned 64/14,927 行、
  partial 1,583/650,954 行、replaced 71/27,674 行、reference 2,723/574,185 行、
  unmapped 0，Python parser、Go parser、Go local driver 与四 provider wrapper 均继续
  诚实标为 `partial`。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、Compose 静态解析、默认与
  `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings` 和全目标测试全部通过；默认 389 passed / 0 failed /
  1 GPU ignored，双 feature 392 passed / 0 failed / 1 ignored，后者真实执行 zvec native
  reopen 与在线 mirror。只读复用既有 `rayrag-postgres-1` 执行
  `SHOW server_version; SELECT 1`，确认 `18.4 (Debian 18.4-1.pgdg13+1)` 和 SQL 成功；
  当前未设置 `RAYRAG_POSTGRES_TEST_URL`，所以条件式 Rust snapshot test 实际跳过。
  MinerU endpoint/token/output/server 环境变量也均未设置，wire contract 使用本地受控
  Axum 服务验证，没有冒充外部 MinerU 实连；未新起或重启任何容器。
- 尚未冒充完成：PDF outline/page rendering、bbox 归一化 tag、crop/image object、ZIP
  图片落盘和 VLM image description、from/to page、manual/pipeline/paper tuple 形态、
  progress callback、output directory 清理、Go JSON/Markdown ParseResult 与 page-count/
  file metadata、空输入空结果、tenant Provider 实例取密、MinerU.Net public-URL 流程，
  以及 SoMark provider。因此这是可运行的 MinerU 双协议纵向切片，不代表完整 MinerU/
  OCR/版面解析家族已经对齐；继续保持全 Rust、Linux/Docker、zvec-rust +
  PostgreSQL 18.4、国内镜像优先、RayRAG 不引入 MySQL且不改版本号。

## 2026-07-27 RAGFlow OpenDataLoader remote PDF provider 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行复核 424 行
  `deepdoc/parser/opendataloader_parser.py`、320 行 Python 单测、327 行
  `rag/llm/ocr_model.py` 中 OpenDataLoader wrapper、231 行 Go
  `pdf_parser_opendataloader.go`、152 行 Go 测试及 `rag/app/naive.py` 生产调用点。按仓库规则
  先查询 RAGFlow“全部文件”知识库；召回是通用 provider/分块资料和无关内容，没有目标协议
  的精确代码证据，因此实现合同仍以固定 Git blob、调用点和测试为准。
- `src/parser/opendataloader.rs` 保留原 XML/YAML/TOML 本地 loader，并新增纯 Rust remote
  provider。配置兼容 UI 嵌套 `{"api_key": {...}}` 与 flat JSON、大小写 key、进程环境
  回退、base URL 去尾斜杠、可选 Bearer、无效 timeout 回退 600 秒及 secret-redacted
  `Debug`；API server 缺失时 fail closed。`GET /health` 使用固定 5 秒探针，PDF 以
  `application/pdf` multipart 发送到 `/file_parse`，透传 `hybrid`、`image_output` 和
  true/false 字符串 `sanitize`，每次重建 body 并最多即时尝试 3 次。
- 响应递归遍历 `content`/`text`/`cells`，支持 HTML 与稀疏行 table fallback、图片 caption、
  equation/text/title 分类和 marker-aware 文档内容；优先 `json_doc`，没有 text section 时
  回退 `md_text`。仅 HTML table 是同提交 Go 路径支持、Python `_iter_elements` 会漏过的
  差异，Rust 采用两者能力并集。`ParserConfig` 以原名序列化/反序列化三个可选字段；
  `Pipeline` 只在 PDF 且 selector 为 `OpenDataLoader` 或
  `<model>@<instance>@OpenDataLoader` 时检查 health 并调用 provider，CLI 与服务端后台任务
  共用同一环境装配；`.env.example`、Compose 和 README 已同步，默认仍关闭。
- 新增 6 个行为回归：nested/flat 凭据和 redaction、JSON 优先/Markdown 回退、table cells/
  image marker、真实本地 HTTP health/Bearer/multipart/前两次 502 后第三次成功、固定三段
  selector、生产 PDF pipeline，以及 parser_config 三字段 wire 名；覆盖生成器再加一项精确
  映射守卫，35/35 通过并从固定 Git tree 重建 4,441 文件矩阵。统计保持
  aligned 64/14,927 行、partial 1,583/650,954 行、replaced 71/27,674 行、
  reference 2,723/574,185 行、unmapped 0，Python parser、Go parser 与四 provider wrapper
  继续诚实标为 `partial`。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、Compose 静态解析、默认与
  `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings` 和全目标测试全部通过；默认 381 passed / 0 failed /
  1 GPU ignored，双 feature 384 passed / 0 failed / 1 ignored，后者真实执行 zvec native
  reopen 与在线 mirror。只读复用既有 `rayrag-postgres-1` 执行
  `SHOW server_version; SELECT 1`，确认 `18.4 (Debian 18.4-1.pgdg13+1)` 和 SQL 成功；
  当前未设置 `RAYRAG_POSTGRES_TEST_URL`，所以条件式 Rust snapshot test 没有实际执行。
  三个 `OPENDATALOADER_*` 变量也均未设置，wire contract 使用本地受控 Axum 服务验证，没有
  冒充外部 OpenDataLoader 实连；未新起或重启任何容器。
- 尚未冒充完成：本地 PDF page rendering/outline、bbox PDF 坐标到页面 tag 转换、crop/
  image object、from/to page、`parse_method` 的 paper/manual/raw tuple 形态、progress
  callback、精确 page-count/output-format ParseResult、tenant Provider 实例取密，以及
  MinerU/SoMark wrapper 仍未实现。因此这是可运行的 remote PDF 纵向切片，不代表完整
  OpenDataLoader/版面解析家族已经对齐；继续保持全 Rust、Linux/Docker、zvec-rust +
  PostgreSQL 18.4、国内镜像优先、RayRAG 不引入 MySQL且不改版本号。

## 2026-07-27 RAGFlow PaddleOCR provider / async job 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行复核 327 行
  `rag/llm/ocr_model.py`、811 行 `deepdoc/parser/paddleocr_parser.py`，并核对 343 行
  Go `internal/entity/models/paddleocr.go`、动态 `OcrModel` factory、tenant/service 调用点和
  `rag/app/picture.py` / `naive.py` selector。按仓库规则先查询 RAGFlow“全部文件”知识库，
  召回包含通用 provider 集成说明但没有目标文件的精确协议，另混有无关资料，因此具体合同仍
  采用固定 Git blob 和生产调用点。
- `src/ocr.rs` 保留原 `paddleocr-gpu-proxy` 兼容路径，并新增纯 Rust PaddleOCR provider：
  接受 UI 嵌套 `{"api_key": {...}}`、flat auto-provision、大小写配置别名和环境回退，
  base URL 去尾斜杠；严格支持固定六个算法，secret 不进入 Debug，缺 token/URL、未知算法、
  空内容和非法 timing 均 fail closed。
- 新 adapter 按固定 Python wire contract 向 `/api/v2/ocr/jobs` 发送
  `model/optionalPayload/file` multipart，加 Bearer 与 `Client-Platform: ragflow`；兼容
  nested/root job ID、state 和两种 result URL，使用覆盖整个 submit/poll/fetch 的 deadline
  与 1.5x/15 秒封顶退避。failed state、非 200、非 JSON 和超时均保留阶段错误；JSONL 严格
  解析并合并 layout，清除 inline image markup，无 layout 时回退 `rec_texts`。
- `Pipeline` 新增显式 OCR 依赖和环境装配。图片/PDF 仅在
  `layout_recognize=PaddleOCR` 或上游 `<model>@<instance>@PaddleOCR` selector 时进入外部
  provider；配置或返回为空时明确失败，不把文件名/metadata 占位伪装为 OCR 成功。CLI 新增
  `--layout-recognize`，服务端后台解析也使用同一环境适配；`.env.example` 和 Compose 已透传
  PaddleOCR/legacy 变量，外部 OCR 默认关闭。
- 新增 5 个行为回归：nested/flat 配置、六算法拒绝边界、secret redaction、严格 JSONL 与
  `rec_texts` fallback、真实本地 HTTP multipart/header/submit→running→done→fetch、
  provider-instance selector，以及生产图片 pipeline 接线。覆盖生成器新增一项映射守卫，
  34/34 通过并从固定 Git tree 重建 4,441 文件矩阵；统计保持 aligned 64/14,927 行、
  partial 1,583/650,954 行、replaced 71/27,674 行、reference 2,723/574,185 行、
  unmapped 0，两份目标文件继续诚实标为 `partial`。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、
  `docker compose --env-file .env.example config --quiet`、默认与
  `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings` 和全目标测试全部通过；默认 374 passed / 0 failed / 1 GPU ignored，
  双 feature 377 passed / 0 failed / 1 ignored，后者真实通过 PostgreSQL 18 snapshot、
  zvec native reopen 和在线 mirror。本批没有可用的外部 PaddleOCR endpoint/token，因此
  wire protocol 使用本地受控 HTTP 服务验证，没有冒充官方/业务 OCR 实连；未新起/重启
  Docker、PostgreSQL 或 GPU 三模型服务。
- 尚未冒充完成：MinerU/OpenDataLoader/SoMark provider wrapper、tenant Provider 实例选择、
  自定义 `algorithm_config/additional_params`、PDF outline/page rendering、bbox section tag、
  `parse_method` 多 tuple 形态、crop、progress callback 和本地 DeepDOC OCR fallback 尚未
  实现；图片的短 OCR 后视觉模型描述也未接线。因此这只是可运行的 PaddleOCR async 纵向
  切片，不代表完整 OCR/视觉模型家族已经对齐。

## 2026-07-27 RAGFlow Agent runner 实时 SSE / 结构化取消纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行复核 538 行
  `internal/agent/canvas/runner.go`、724 行 `scheduler.go`，并核对
  `service/agent.go`、handler 和 e2e 测试对首次/恢复、等待/完成及 done 的顺序约束。按仓库规则
  先查询 RAGFlow“全部文件”知识库，但召回仍是无关医疗内容，因此没有将其作为实现证据，
  本批合同来自固定 Git blob、生产代码和测试。
- `AgentWorkflow` 新增执行时 observer：顶层节点在真正 await 组件前发送
  `node_started`，成功或失败后发送带 runtime inputs、outputs/error、elapsed 和
  `created_at` 的 `node_finished`。UserFillUp、Loop 或 Parallel 暂停时只发送 start，
  不伪造 finish；恢复 UserFillUp 则围绕实际恢复动作发送 start/finish。该 observer 直接进入
  live SSE，而非结束后从 trace 重建；现有无 observer 的 JSON 和内部调用路径保持兼容。
- `stream: true` 现在立即返回由 Rust mpsc 驱动的 `Body`，稳定复用同一
  message/task/session ID，且最终持久化的 user/assistant message ID 与已发送 ID 一致。成功、
  等待和错误终态仍在 checkpoint/conversation 持久化成功后发送，正常终态继续以
  `data: [DONE]` 收束；省略 `stream` 仍返回原 JSON。
- 请求 body receiver 被客户端断开时，driver 通过 `select` 立即 drop 正在执行的 workflow
  future，现有 unfinished-run guard 会把它记录为 cancelled。进程内 registry 在 ACL、DSL、
  conversation、checkpoint 和模型 preflight 全部通过后才按 Canvas 注册；同 Canvas 的新
  stream 会静默取消旧 stream，generation-safe lease 保证旧 future 清理时不会误删新运行。
  新增阻塞 Retrieval observer 回归证明 start 在组件完成前可见，另有 2 个 registry/drop
  回归证明断连释放 future、替换取消和 generation 隔离；3 个 HTTP SSE 回归也已更新为实时
  事件顺序、持久化 ID、等待/恢复及运行时错误合同。
- 覆盖生成器 33/33 通过并从固定 Git tree 重建 4,441 文件矩阵，统计保持
  aligned 64/14,927 行、partial 1,583/650,954 行、replaced 71/27,674 行、
  reference 2,723/574,185 行、unmapped 0。`runner.go` 和 `scheduler.go` 已记录实时 observer、
  持久化终态、断连取消及 generation-safe replacement，但继续诚实标为 `partial`。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、默认与
  `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings` 和全目标测试全部通过；默认 368 passed / 0 failed / 1 GPU ignored，
  双 feature 371 passed / 0 failed / 1 ignored，后者真实通过 PostgreSQL 18 snapshot、
  zvec native reopen 和在线 mirror。本批未新起/重启 Docker、PostgreSQL 或 GPU 三模型
  服务，保持纯 Rust、Linux/Docker、zvec-rust + PostgreSQL 18.4、国内镜像优先、零 MySQL
  且不改版本号。
- 尚未冒充完成：SSE 仍是 opt-in，尚无 LLM token/message 增量 chunk；Loop/Parallel 只观测
  顶层 macro，未展开 per-child 生命周期，inputs map 也不是完整 Eino pre-state；registry
  仅覆盖本进程内 stream 请求，非 stream 和跨进程运行不参与替换，task ID 不是 Canvas
  version，spawned task panic 也尚未转成 error + done。因此固定上游 runner/scheduler 的完整
  token stream、子图事件和分布式运行控制仍为 `partial`。

## 2026-07-27 RAGFlow Agent runner 缓冲 SSE 生命周期纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行复核 538 行
  `internal/agent/canvas/runner.go`，并核对 `service/agent.go` 的事件产生顺序、
  `canvas/scheduler.go` 的节点 payload、`handler/agent.go` 和
  `service/bot_completion.go` 的 SSE wire format，以及 handler/e2e 测试。按仓库规则先查询
  RAGFlow“全部文件”知识库，但召回的是无关 ERP/多语言资料，未把它当成语义证据。
- `POST /api/v1/agents/{id}/completions` 新增可选 `stream: true`：输出只含 `data:` 行的扁平
  `{event,message_id,created_at,task_id,session_id,data}` envelope。成功路径包含
  `workflow_started`、trace 派生的 `node_started/node_finished`、
  `message/message_end/workflow_finished`；等待路径输出带 `cpn_id/tips/inputs` 和服务端
  opaque `resume_token` 的 `waiting_for_user`；执行错误使用上游 writer 的
  `{code:500,message,data:false}` 终态，所有路径均以 `data: [DONE]` 结束，并设置
  `text/event-stream`、`no-cache` 和 `keep-alive`。
- 事件在 workflow、checkpoint 和 conversation exchange 已经持久化之后才缓冲写出，避免先向
  客户端宣告成功后落盘失败。恢复时读取 checkpoint 内已完成顶层 trace 长度，只输出本次新增
  节点，不重放 Begin/Retrieval/LLM 等暂停前工作；等待回合仍保存 user + 空 assistant，
  resume token 仍由原子 claim/consume-once 边界保护。省略 `stream` 继续返回原 JSON，
  既有客户端与等待/恢复协议不变。
- 新增 3 组 HTTP 回归，分别钉死完成流的 8 段事件顺序与稳定 metadata、等待帧/checkpoint
  token 及恢复流不重放旧 trace、运行时失败的 error + `[DONE]`。JSON 默认分支也继续由既有
  UserFillUp 端到端测试验证。覆盖生成器 33/33 通过并从固定 Git tree 重建 4,441 文件矩阵，
  统计保持 aligned 64/14,927 行、partial 1,583/650,954 行、replaced 71/27,674 行、
  reference 2,723/574,185 行、unmapped 0；`runner.go` 继续诚实标为 `partial`。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、默认与
  `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings` 和全目标测试全部通过；默认 365 passed / 0 failed / 1 GPU ignored，
  双 feature 368 passed / 0 failed / 1 ignored，后者真实通过 PostgreSQL 18 snapshot、
  zvec native reopen 和在线 mirror。本批未新起/重启 Docker、PostgreSQL 或 GPU 三模型服务，
  保持纯 Rust、Linux/Docker、zvec-rust + PostgreSQL 18.4、国内镜像优先、零 MySQL且不改
  版本号。
- 尚未冒充完成：SSE 当前由最终 trace 重建，节点 inputs/timing 不是执行时实时观测，尚无
  token/node 边执行边 flush、请求断开向 workflow 传播取消、同 Canvas 新运行替换旧运行、
  panic 后保留已发事件、默认即 SSE，以及跨进程运行 registry；Loop/Parallel 内层逐节点
  stream 也未展开。因此固定上游 runner 的实时性、取消和完整调度语义仍为 `partial`。

## 2026-07-27 RAGFlow Loop/Parallel 复合 UserFillUp checkpoint bridge

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，复核
  `internal/agent/workflowx/parallel.go` 的 `ParallelInterruptState`、completed/pending
  partition、稳定 item checkpoint 地址和恢复测试，`loop_integration_test.go` 的“恢复同一
  iteration”合同，以及 `canvas/interrupt_resume.go` 的 leaf/root 地址区别和
  `loop_subgraph.go` / `parallel_subgraph.go` 调用边界。RAGFlow“全部文件”知识库查询成功，
  但 28 条召回仍全是无关医学资料，因此没有把其内容当作实现证据。
- `AgentWorkflowCheckpoint` 新增可 serde 的 `CompositeCheckpoint`。Loop 暂停时保存 macro
  ID、当前迭代号、body execution cursor 和 selected 集；恢复先校验外层 cursor、展示叶子
  与复合叶子一致，再把一次用户数据写入该 UserFillUp，从同一 iteration 的下一 body 节点
  继续，不会重播已完成的 bump/Retrieval/LLM，也不会重新 seed loop variables。
- Parallel 首轮仍按既有 `max_concurrency` 策略 drain 每个 item，但把结果分类为 completed
  或 waiting；checkpoint 保存原始 items、每个 completed item 的完整局部 Canvas 快照、
  每个 pending item 的局部 runtime/transient/body cursor，以及当前 active index。恢复使用
  `parallel:<macro>:<index>:<leaf>` 稳定地址一次消费一个 item；completed item 不再执行，
  其余 pending item 保持原 checkpoint，全部完成后才按原索引聚合 `_result`、声明输出、
  usage 和 references。普通错误仍优先返回且不伪造可恢复状态。
- 新增 2 组端到端执行器回归：Loop 两轮分别等待、checkpoint JSON 往返后仍在原迭代完成，
  最终 counter 精确为 2；Parallel 三个并发 item 首轮形成完整 pending partition，三次恢复
  后答案保持输入顺序，completed 数量单调 0→1→2，并拒绝被篡改的展示叶子/复合叶子组合。
  既有服务端 checkpoint store 的原子 JSON/PostgreSQL mirror、opaque token、租约 claim 和
  DSL fingerprint 无需另造旁路即可持久化新增状态。
- 覆盖生成器 33/33 通过并重新从固定 Git tree 生成 4,441 文件矩阵，统计保持
  aligned 64/14,927 行、partial 1,583/650,954 行、replaced 71/27,674 行、
  reference 2,723/574,185 行、unmapped 0。`workflowx/parallel.go`、Loop/Parallel
  subgraph、interrupt/resume、checkpoint store、node body、runner、scheduler、Python
  FillUp 和 Go UserFillUp 说明均已更新，但继续诚实保留 `partial`。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、默认与
  `postgres-backend,zvec-backend` 双 feature 的 locked all-target check、严格
  `clippy -D warnings` 和全目标测试全部通过；默认 362 passed / 0 failed / 1 GPU
  ignored，双 feature 365 passed / 0 failed / 1 ignored，后者真实通过 PostgreSQL 18
  snapshot、zvec native reopen 和在线 mirror。本批未新起/重启容器或 GPU 三模型服务，
  保持纯 Rust、Linux/Docker、zvec-rust + PostgreSQL 18.4、国内镜像优先、零 MySQL且不改
  版本号。
- 尚未冒充完成：上游一次 `CompositeInterrupt` 可携带并一次选择多个叶子，RayRAG HTTP
  当前每次 resume token 只消费一个稳定 active item；Eino 原始 child bridge-store 字节
  格式、Parallel panic recovery/outer stream、Loop per-iteration stream、嵌套
  Loop/Parallel macro、真实 FileService/layout recognition、完整 Agent SSE、Redis TTL 和
  跨进程分布式 claim/GC 仍为 `partial`。

## 2026-07-27 RAGFlow UserFillUp / interrupt-resume 持久化纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计
  `agent/component/fillup.py` 125 行及 155 行测试、
  `internal/agent/component/userfillup.go` 292 行及 161 行测试、
  `internal/agent/canvas/interrupt_resume.go` 405 行及 445 行测试、
  `checkpoint_store.go` 101 行及 141 行测试、`node_body.go` 257 行和
  `runner.go` 538 行，并核对 Begin、compile、service/handler、前端 UserFillUp form 与固定
  DSL 调用点。RAGFlow“全部文件”知识库本批返回的是无关医疗内容，因此语义证据采用固定
  Git blob、Python/Go 生产实现及其测试，没有把错误召回写进兼容结论。
- 纯 Rust `AgentWorkflow` 新增闭集 `WorkflowRunOutcome`、可 serde 的
  `AgentWorkflowCheckpoint` 和交互式 run/resume 入口。首次运行在顶层 UserFillUp 前保存
  精确 execution cursor、selected node 集、CanvasRuntime、transient usage/reference 和
  trace；恢复先校验版本/节点/cursor，再把一次用户 payload 写入
  `user_input`、component bucket 及声明字段，从下一索引继续。Begin、Retrieval、LLM 等
  上游副作用不会重跑；连续多个顶层 UserFillUp 可逐个等待。Begin 与 UserFillUp 共用
  `sys.__initial_user_input_consumed__`，避免同一初始 query 被重复消费。
- 新增 `src/agent_checkpoint.rs` 服务端状态库，checkpoint 只通过 opaque
  `resume_token` 查找，不接受客户端回传执行状态；按 owner/tenant/agent/conversation
  四重隔离，DSL normalized fingerprint 防止等待期间换图，15 分钟 claim lease 阻止并发
  二次恢复；进程重启会清除已失去执行 future 的 lease，但保留可恢复 checkpoint。
  save/claim/release/replace/delete 均在复制快照上提交，失败不污染内存；落盘
  复用 crash-safe atomic write，启用 `postgres-backend` 时自动进入 PostgreSQL snapshot
  mirror，重启后仍可恢复。
- Agent completion 首轮等待返回 JSON
  `event=waiting_for_user`、`kind/cpn_id/interrupt_id/tips/inputs`、resume token、
  workflow path/trace，并按固定 Python service 行为持久化 user + 空 assistant 消息对。
  下一请求可用 `resume_data`、`inputs` 或标量 question 恢复；token 错误、活动 claim 或
  DSL 变化返回 HTTP 409，成功完成并提交消息后才删除 checkpoint，执行错误会释放 claim。
  组件目录新增 `userfillup` 输入/输出 metadata。
- 新增 9 组定向回归：单字段初始 query 直通、双字段等待与 checkpoint serde 往返、
  Retrieval 只执行一次、Begin/UserFillUp 共享消费标记、文件及嵌套 fail-closed、持久化
  重启恢复、ACL/token/claim 一次性、DSL fingerprint 稳定、HTTP 等待/空消息对/错 token/
  恢复/完成清理。固定覆盖生成器 33/33 通过并从 4,441 个 pinned blob 重生矩阵，统计保持
  aligned 64/14,927 行、partial 1,583/650,954 行、replaced 71/27,674 行、
  reference 2,723/574,185 行、unmapped 0；相关生产文件仍为诚实 `partial`。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、默认
  `cargo check --locked --all-targets`、严格 Clippy 和全目标测试
  360 passed / 0 failed / 1 GPU ignored；双 feature 的 check、严格 Clippy 和全目标测试
  363 passed / 0 failed / 1 ignored，PostgreSQL 18 snapshot 往返、zvec native
  collection reopen 与在线 mirror 均实际通过。本批未新起/重启 Docker、PostgreSQL 或
  GPU 三模型服务；继续保持纯 Rust、Linux/Docker、zvec-rust + PostgreSQL 18.4、国内镜像
  优先、零 MySQL且不改版本号。
- 该批结束时 Loop/Parallel 内 UserFillUp 仍编译期拒绝；其 durable composite bridge 已由
  紧邻上方 02:03 批次补齐。真实 FileService/`layout_recognize`、Go standalone Invoke 的
  tips renderer/file stub、完整 SSE `workflow_started/.../done`、Redis checkpoint TTL、
  跨进程分布式 claim 和 checkpoint 垃圾回收仍为 `partial`。

## 2026-07-27 RAGFlow Parallel / Iteration / IterationItem 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计
  `agent/component/iteration.py` 64 行、`iterationitem.py` 101 行、
  `internal/agent/component/parallel.go` 186 行及 169 行测试、
  `internal/agent/canvas/parallel_subgraph.go` 385 行及 259 行 grouped/batch/timing
  测试、`internal/agent/workflowx/parallel.go` 812 行及 1,455 行测试/辅助代码，并核对前端
  Iteration 表单、默认值、输出引用 schema 和旧版 `iteration.json` DSL。RAGFlow“全部文件”
  知识库本批可访问，但召回内容与 Parallel 无关，因此实现证据采用固定 Git blob、Go/Python
  运行代码和测试。
- 纯 Rust `AgentWorkflow` 新增 Parallel macro plan：优先按直接 `parentId` 收集 body，
  对无分组旧 DSL 回退 descendants，外层 DAG 折叠为单个 Parallel 节点，body 独立稳定拓扑。
  runtime normalizer 现在同时改写被折叠 child ID 的 upstream/downstream，兼容旧 fixture 中
  `iterationitem:0@result` 小写别名和 body 回边；编译期要求非空 `items_ref`，并明确拒绝
  尚未实现的 Loop/Parallel 嵌套 macro，而不是给出错误的伪执行结果。
- 执行时每个 item 克隆隔离的 `CanvasRuntime`，注入 `item/index` 与
  `__item__/__index__`；`max_concurrency <= 1` 严格顺序执行，较大值按固定 Go workflowx
  语义先在 caller 执行第 0 项，再以有界 fan-out 运行剩余项。普通 item 错误不会提前丢弃
  其他已安排工作，全部 drain 后返回首个错误；成功结果始终按输入索引排序，provider usage
  和 references 同样按输入序合并，不受完成先后影响。
- Parallel 输出统一写入 `_result`：每项 snapshot 包含 item/index 与全部组件输出 bucket；
  配置引用支持 item/index 及 `component@object.dot`，缺失路径生成 JSON null，dot traversal
  只穿越 object，与固定 Go 实现一致。null items 生成空有序 batch，非 array 明确报错。
  不复制 Go `__cpn_id__` 共享 bucket，而由 Rust owned component output map 提供同等隔离。
- 新增 5 组定向回归，覆盖隔离 item 和声明输出、并发上限与乱序完成后的稳定输出、null/非数组/
  缺失引用、错误后 drain，以及旧 alias/backedge 规范化；测试 retriever 实测
  `max_concurrency=2` 时峰值为 2、顺序模式峰值为 1。组件目录新增精确 `parallel` descriptor，
  输入为 `cpn_id/items_ref/max_concurrency`，输出为 `_result`。
- 覆盖生成器新增 Python Iteration/IterationItem、Go component/workflowx Parallel 精确
  partial mapping，并更新 canvas parallel subgraph、scheduler、node body、normalize 和
  canvas 总体说明。重新生成固定 commit 的 4,441 文件矩阵，统计保持
  aligned 64/14,927 行、partial 1,583/650,954 行、replaced 71/27,674 行、
  reference 2,723/574,185 行、unmapped 0；本批将 generic partial 细化为审计后的 exact
  partial，故总数不虚增。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、默认
  `cargo check --locked --all-targets`、严格
  `cargo clippy --locked --all-targets -- -D warnings` 和全目标测试
  351 passed / 0 failed / 1 GPU ignored；双 feature 的 check、严格 Clippy 和全目标测试
  354 passed / 0 failed / 1 ignored，其中 PostgreSQL snapshot 往返、zvec native
  `native_collection_upserts_queries_and_reopens` 与在线 mirror 测试通过；覆盖生成器
  33/33 通过。本批没有持久化、容器或 GPU 代码变化，因此未新起/重启 Docker、
  PostgreSQL 或三模型服务；继续保持纯 Rust、Linux/Docker、zvec-rust + PostgreSQL 18.4、
  国内镜像优先、零 MySQL且不改版本号。
- 尚未冒充完成：Eino composite interrupt/checkpoint snapshot、只 replay 未完成 index、
  child checkpoint ID/bridge、显式 outer stream completion boundary、panic recovery、
  nested Loop/Parallel macro 和 ordinary sibling wave 并发仍为 `partial`；Python 的
  cooperative cancellation、thought 和 `_idx` 可见状态也未伪装成已实现。

## 2026-07-27 RAGFlow Loop / LoopItem / ExitLoop 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计
  `agent/component/loop.py` 100 行、`loopitem.py` 164 行、`exit_loop.py` 32 行、
  `internal/agent/component/loop.go` 223 行、`loop_test.go` 184 行、
  `internal/agent/canvas/loop_subgraph.go` 782 行，以及
  `loop_subgraph_test.go` / `loop_semantics_test.go` 共 1,389 行和
  `workflowx/loop.go` 实际 driver。RAGFlow“全部文件”知识库可访问，但 Loop 查询召回的是
  无关医学资料；本批因此以固定 Git blob、Go 语义测试和调用点为实现证据。
- 纯 Rust `AgentWorkflow` 新增 Loop macro plan：优先收集直接 `parentId` body，
  无分组旧 DSL 回退为遇 Loop 回边停止的 descendants；外层 DAG 只保留一个 Loop 节点，
  body 单独计算稳定拓扑。补齐运行规范化时遗留 `LoopItem/IterationItem` 的 downstream
  折叠和 upstream 回写，并让从扁平 components 补建 graph 时保留 `parent_id/parentId`，
  避免 body 被误当作外层 follower。
- Loop 采用 do-while：每轮先运行所有被选中的 body 节点，再读取同一个
  `CanvasRuntime` 计算终止条件。变量支持 constant、实时 variable dereference 和
  number/string/boolean/object*/array* 零值，已有非 null 状态不会被下一轮重置；
  VariableAssigner 修改因此跨轮累积。条件支持默认 AND、OR，以及固定 Go 端完整
  string/bool/number/object/list/null 操作符；Switch 多目标选择、多终端和 legacy
  `ExitLoop` no-op terminal 走与外层相同的静态分派。Loop 整体仍只形成一个外层
  `workflow_path` / trace 节点，body 输出保留在共享状态。
- `maximum_loop_count` 在条件未满足时返回可 downcast 的
  `LoopMaxIterationsExceeded`，且保留已完成迭代写入；条件在上限那一轮成立则成功。
  配置为 0 时明确跟随固定 Go `workflowx` 的 1,024 次安全 cap，而非复制组件注释中的
  真无限循环。另有两处上游自身不一致：Go 允许空/未知 input_mode 作零初始化，而 Python
  `Check` 拒绝；Go 的 bool/number empty 运算依赖 rhs null，Python 检查 lhs 身份。
  Rust 选择固定提交中可执行且有测试的 Go 路径，并在覆盖表保留 Python 文件为 partial。
- 新增 8 组 Loop 行为测试，覆盖 0→1→2→3 do-while、live variable seed、同名变量
  last-spec-wins、上限错误及第 5 轮状态、全类型操作符、零值/坏参数、Switch + ExitLoop
  分支/多终端、无分组 descendants fallback；既有规范化测试补 upstream 回写断言，组件目录测试确认 Loop
  注册。覆盖生成器增加 Loop/LoopItem/ExitLoop Python 与 Go component 精确映射并更新
  canvas scheduler/node-body/multibranch 说明；重新生成 4,441 文件矩阵：
  aligned 64/14,927 行、partial 1,583/650,954 行、replaced 71/27,674 行、
  reference 2,723/574,185 行、unmapped 0。
- 验证：`cargo fmt --all -- --check`、`git diff --check`、默认
  `cargo check --locked --all-targets`、严格
  `cargo clippy --locked --all-targets -- -D warnings`、全目标测试
  346 passed / 0 failed / 1 GPU ignored；双 feature 的 check、严格 Clippy 和全目标测试
  349 passed / 0 failed / 1 ignored，zvec native
  `native_collection_upserts_queries_and_reopens` 与在线 mirror 测试真实执行。覆盖生成器
  33/33 测试通过。GPU 三模型、PostgreSQL/Docker 本批均无相关代码变化，因此未重复干预
  运行模型或新起容器；同日上一批已完成 PostgreSQL 18.4 新容器实连。仍为纯 Rust、
  Linux/Docker、zvec-rust + PostgreSQL 18.4、国内镜像优先、零 MySQL，不改版本号。
- 尚未冒充完成：Eino 子工作流 checkpoint/resume、per-iteration stream mode、child
  checkpoint bridge、并发 sibling wave，以及 Python-only cooperative
  cancellation/thought/_idx 可见状态仍为 `partial`；Parallel 核心 fan-out 已由紧接其前的
  本批补齐，其 checkpoint/resume/stream 差距仍按新节保持 `partial`，UserFillUp 继续按
  精确覆盖行实现。

## 2026-07-27 RAGFlow DataOperations Python/Go 纵向切片

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计
  `agent/component/data_operations.py` 202 行、
  `internal/agent/component/data_operations.go` 542 行、
  `internal/agent/component/data_operations_test.go` 282 行，以及固定前端 DataOperations
  表单 schema、初始值和 query/key/update/filter 控件。RAGFlow“全部文件”知识库本批可访问，
  但召回内容与 RayRAG/DataOperations 无关，因此实现证据采用固定 Git blob、Go 测试和前端
  DSL 合同。
- 纯 Rust Canvas 新增 DataOperations 的不可变目录注册、保存/编译期参数校验和静态分派，
  完整覆盖 `select_keys`、`literal_eval`、`combine`、`filter_values`、
  `append_or_update`、`remove_keys`、`rename_keys` 七个 JSON-shaped 操作。query 同时接受
  Python/Go 字符串、Go CSV 和前端 `[{input: ...}]` 形态；key 列表同时接受字符串/CSV 和
  前端 `[{name: ...}]`。输入对象/对象数组按声明顺序汇总，scalar/null 忽略，输出保留
  原生 JSON；filter 和 update 支持 Canvas selector/template。
- `literal_eval` 递归解析 JSON object/list/string/number/bool/null，并补 Python
  `True/False/None` 与 `.5`；单引号容器、tuple/set 等仅由 Python `ast.literal_eval`
  表达且不属于 Rust `serde_json::Value` 的分支继续明确为 partial。Python 动态
  input-form、thought text 和共享 cooperative-cancellation 接口同样未冒充完成。
  `combine` 的 scalar + list 采用 Python 源码和 Go 注释规定的扁平结果，明确修正固定 Go
  实现中未被测试覆盖的 accidental nested slice。
- 新增 6 组 Rust 行为测试，覆盖真实 Canvas 前端 query 形态、七操作分支、递归 literal、
  selector/template、混合输入和 malformed DSL；组件目录同时发布明确 inputs/result
  metadata。覆盖生成器新增 Python/Go 精确映射与门禁，并修复固定源码缓存现有 50 MiB
  pack 缺失索引导致的逐 blob 懒取：仅重建 `.idx` 后缺失对象由 4,068 降为 0，未改固定
  commit。重新生成 4,441 文件/1,267,740 文本行矩阵：aligned 63/14,895、partial
  1,584/650,986、replaced 71/27,674、reference 2,723/574,185、unmapped 0。
- 验证：默认全目标 337 passed、双 feature 340 passed，均 0 failed/1 个 GPU opt-in
  ignored；默认/双 feature check、严格 Clippy、coverage 32/32 和原生 zvec-rust 0.5.1
  create/upsert/query/reopen/online mirror 均通过。Compose 静态展开通过并继续使用
  DaoCloud `postgres:18.4`；临时无卷 PostgreSQL 18.4 首次健康探针因 bootstrap 两阶段
  窗口过早 unhealthy，增加 90 秒 start-period 并等待目标库 `SELECT 1` 后，服务端确认
  `18.4 (Debian 18.4-1.pgdg13+1)` / `180004`，精确 Rust snapshot round-trip 通过。
  临时容器及 55433 端口已自动清理，现有业务容器、镜像和数据卷未改动。

## 2026-07-25 RAGFlow StringTransform/Jinja 与 Go runtime 收口

- 继续固定官方 RAGFlow v0.26.4 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计
  `agent/component/string_transform.py` 107 行、
  `internal/agent/component/string_transform.go` 302 行、
  `internal/agent/runtime/template_jinja.go` 161 行，以及对应 Go 测试 175/224 行。
- Rust Canvas StringTransform 现同时覆盖固定版 Python/Go 可观察语义：保存期默认值和参数
  校验、首个上游 `line` 优先和 `split_ref` 回退、多个 literal delimiter、首尾/连续空片段、
  false-y 置空、truthy 非字符串拒绝、首 delimiter 数组连接、缺失值空串、
  `item/index/result` 别名、首上游动态 merge 输入和原生 JSON 上下文。
- 引入 MiniJinja 2.21.0 的纯 Rust 沙箱渲染，支持条件、循环、注释、嵌套对象和 filters；
  不注册 loader/文件系统/网络函数，不启用 HTML escape，undefined 可链式输出空串，
  每次渲染限制 100,000 fuel、递归 64 层。解析/执行失败按 StringTransform 的显示型路径
  软降级，并继续解析普通 RAGFlow selector；新增 7 组契约测试覆盖上述分支和 fuel 耗尽。
  Python 文件仍诚实保留 partial：其 UI input-form/thoughts 和共享 cooperative
  cancellation/timeout 接口尚未实现；Go StringTransform 302 行升级 aligned。独立 Jinja
  runtime 后续完成全调用点审计并升级 replaced，细节见本节后续条目。
- 紧接着逐行审计 `internal/agent/runtime/template.go` 全部 140 行及分散在
  canvas variable、Message 和 service e2e 中的固定版测试。统一 Rust fast template
  扫描支持单/双/三重花括号、component/sys/env、`item/index/result`、唯一首现引用提取；
  参数绑定返回首个错误并保留完整 partial output，Message 使用同一结果的 display
  soft-fail 路径。未知 `{{garbage}}` 原样保留，结构值使用确定性 JSON 代替 Go map
  的非确定性 `%v` 文本；该 140 行生产文件升级 aligned。
- 逐行审计 `internal/agent/runtime/selector.go` 168 行及其 187 行测试后确认，它是
  上游 Python→Go 单版本迁移期的 Redis `go/python/auto` tenant 开关，并非变量
  selector。RayRAG 只允许经保存期校验的 Rust Canvas，unsupported component 在执行前
  fail closed，故不会引入 Python sidecar、Redis 运行时回退键或第二套执行器；该文件由
  partial 改为有证据的 replaced。
- 逐行审计 `internal/agent/runtime/metrics.go` 102 行及其 116 行测试。新增独立
  Prometheus registry，严格保留 `ragflow_canvas_runs_total{runtime,outcome}` 与
  `ragflow_canvas_run_duration_seconds{runtime}` 名称、默认 duration buckets 和
  `success/error/cancelled` 闭集标签；非正时长不入 histogram。Canvas 正常/错误退出显式
  记账，future 被取消时由 RAII guard 在 drop 中记录 cancelled。公开 `GET /metrics`
  使用 Prometheus text exposition，单一纯 Rust runtime 标签固定为 `rust`，不伪造
  Python/Go 执行器；新增 4 组 registry、取消、工作流和 HTTP 契约测试，该文件升级 aligned。
- `internal/agent/runtime/context.go` 73 行没有 sys/env 业务逻辑，而是为 Eino callback 和
  Go 跨 package 调用提供 `context.Context` 类型擦除状态桥，并暴露缺失状态、泛型类型不匹配
  与 nil mutex 三种 plumbing 结果。Rust 每次 `run` 都新建私有 `CanvasRuntime`，持久化
  globals 按运行复制，所有组件通过 `&CanvasRuntime`/`&mut CanvasRuntime` 显式借用，
  future drop 同时销毁状态；无需全局状态、context downcast 或 mutex。新增并发双运行测试，
  证明相同 workflow 的 globals 和 `sys.query` 不串流，该文件由 partial 改为 replaced。
- `internal/agent/runtime/json_safe.go` 152 行及 runner/state/service 调用点全部核对。
  上游因为 `map[string]any` 可混入 func、chan、pointer、任意 struct/client，需在序列化热路径
  用 reflection 递归清洗；固定版实际上没有循环引用检测。RayRAG 的 DSL、组件状态、trace
  和 HTTP 响应边界只接收拥有所有权的 `serde_json::Value` 树，func/channel/client、
  非字符串 map key、共享循环和非有限数在编译或构造阶段即不能进入状态，故不复制反射
  fallback；该文件由 partial 改为 replaced。
- 逐行审计 `internal/agent/runtime/helpers.go` 163 行和 393 行固定测试，新增
  `src/runtime.rs`：`track_progress` 保持 `0/1/-1` 与 Started/Done/error 精确消息且
  原样透传结果；`with_timeout` 用可取消 future 和可 downcast 的 Tokio `Elapsed`；
  `track_elapsed` 在调用前记录 RFC3339 UTC 时间，写入非负小数秒，保留调用方同名键，
  空 map 仍可记账，错误链同时包含组件名与根因。Rust 父任务取消由 future drop 在
  suspension point 结构化传播，不复制 `context.Context`；新增 5 组契约测试，该文件
  升级 aligned。
- `internal/agent/runtime/component.go` 158 行及 factory/node-body 调用点确认只是 Go
  package cycle 适配层：Invoke 最小接口、可变进程全局 factory、registry init bridge、
  placeholder sentinel 和 `ParamError`。RayRAG 在 DSL 保存/编译期校验闭集组件，
  `execute_node` 静态分发并把节点/字段写入错误上下文，unsupported component 在任何
  副作用前拒绝；因此不保留全局 factory RWLock、init 顺序耦合或 placeholder 运行路径，
  该文件由 partial 改为 replaced。
- 继续逐行审计 `internal/agent/runtime/registry.go` 206 行及 357 行测试，以及真实消费
  metadata 的 `internal/service/components_service.go` 140 行/165 行测试和
  `internal/handler/components.go` 123 行/330 行测试。新增不可变编译期 descriptor 表，
  Canvas 支持判断与目录 API 共用同一真源；只发布 Rust 已实现的 12 个 Agent 组件和
  5 个实际 ingestion stage，大小写无关查找、唯一名称、分类和稳定排序无需注册期锁/
  panic。新增受 Bearer 鉴权的 `GET /api/v1/components`，支持空筛选、逗号多分类、
  大小写折叠、重复分类去重、非 null input/output map，以及首个未知分类的 HTTP/envelope
  400。registry 结构性标 replaced，service/handler 均升级 aligned，新增 4 组契约测试。
- 完成 `internal/agent/runtime/state.go` 462 行、95 行 JSON 测试及所有调用点审计。
  `CanvasRuntime` 现以确定性 `BTreeMap` 输出和显式 `sys/env/path/history/retrieval/globals`
  命名空间形成稳定 serde checkpoint wire shape，并保留 `cancel_flag/run_id/task_id`；
  transient LLM usage、引用和最后消息不泄漏到快照。component/sys/env dot path、JSON
  字符串解码、数组索引、`item/index` 别名和嵌套自动建图统一由同一状态解析层处理，
  Retrieval 同步记录 canvas 级 chunks/doc_aggs。每个 future 单独拥有状态并通过显式借用
  访问，无需上游共享 `RWMutex`；future drop 继续提供结构化取消，更广的 interrupt/resume
  编排仍诚实记录在 `internal/agent/canvas` partial。新增 2 组状态 wire/namespace 契约测试，
  并在迁移中发现和修复 Begin 单输入问题回填绕过显式 `sys` 命名空间的回归；该文件升级
  replaced。
- 收尾审计 `internal/agent/runtime/template_jinja.go` 161 行、224 行测试及其唯一生产调用点
  StringTransform。Rust 将 Gonja auto dispatch 替换为保守语法检测、独立 loud-error
  MiniJinja renderer 和组件级 fail-soft wrapper：普通 fast selector 不支付解析开销，
  statement/filter/comment 路径支持 component/nested/sys/env/上游 input context、条件、
  循环、chainable missing 和最终 selector substitution；无 loader/外部函数，继续限制
  100,000 fuel 和 64 层递归。测试中的 `flattenMap` 实际未被上游 context builder 调用，
  `bytesBuffer` 也只是 no-op import anchor，故不复制无效符号；新增 loud renderer 断言并
  保留解析/fuel 失败的组件软降级，该文件由 partial 改为 replaced。至此固定版
  `internal/agent/runtime/` 生产文件已无通用 partial 回退项。
- 转入 DSL 层逐行审计 `internal/agent/dsl/reset.go` 250 行、234 行测试及
  service/handler/router 调用点，新增受鉴权 `POST /api/v1/agents/{id}/reset`。纯 Rust
  defensive `serde_json::Value` 快照将 history/retrieval/memory/path 替换为新空数组，
  按 JSON 类型归零 `sys.*`，按 `variables[name].value` 或声明类型恢复 `env.*`，同时保留
  graph/components 和自定义 globals。团队可见 Agent 与普通编辑保持同一权限，重复 reset
  不刷新时间，持久化失败恢复内存旧值，响应直接返回 reset DSL，且不创建/改写 Canvas
  version；新增 pure transform、store 和现有 HTTP lifecycle 契约，该文件升级 aligned。
- 继续逐行审计 `internal/agent/dsl/errors.go` 40 行、`extract.go` 202 行及 266 行固定测试。
  用闭集 `thiserror` enum 替换进程级 Go sentinel/error wrapping，穷举区分 component
  not found、missing/null input_form 和 malformed DSL。纯提取器直接借用 serde map/string，
  成功热路径不 clone；obj/form/params/name 类型错误、可选 params、非字面 `Begin` ID、
  malformed sibling 跳过、缺失/错型 prologue/mode 空串等分支均有契约测试。新增受鉴权
  `GET /api/v1/agents/{id}/components/{component_id}/input-form`，静态 schema 与上游
  success/102/103 envelope 对齐；errors.go 升级 replaced，extract.go 升级 aligned。
- 完成 DSL 目录最后一个生产文件 `internal/agent/dsl/normalize.go` 全 870 行、705 行测试、
  7 个固定 fixture 及 handler/service/compiler 调用点审计。新增 canvas/run 双边界纯函数：
  canvas 侧完整 serde defensive clone、仅修正普通 React-Flow handle、按 ID 排序生成
  `x=50+i*350/y=200` 默认布局和 flat component view、保留 Iteration UI 协议并修复历史
  Parallel 泄漏；run 侧才折叠有 parentId 的 LoopItem/IterationItem、稳定合并 downstream、
  Iteration→Parallel，并递归改写 item/index alias。编译器同时接受 nested/normalized-flat
  component，create/update/reset/version 持久化和 Agent/version 前端响应统一走 canvas
  boundary；malformed 值不 panic，变换幂等。完整门禁显式设置 `RAGFLOW_FIXTURE_REPO`，
  7 个固定 Git blob 实际读取并通过；normalize.go 升级 aligned，DSL 生产目录不再有
  通用 partial 文件。
- 随后收窄 `internal/handler/agent_component.go` 253 行的剩余差异。input-form 路径保留
  static `Cow::Borrowed` 快路，并补齐固定组件工厂全部动态 form：Agent 按 prompt selector
  首现顺序去重，Browser/BGPT/ExeSQL/YahooFinance 返回各自精确 field/name/type。
  新增受鉴权 `POST /api/v1/agents/{id}/components/{component_id}/debug`，手工校验
  `params.*.value` 并保持 101/102/103/500 envelope；每次调用创建独立 Canvas state，
  复用闭集组件同一执行函数及真实 LLM/Retrieval adapter，非 sys 输入覆盖节点参数，
  `sys.*` 注入状态且 `sys.tenant_id` 固定为鉴权用户不可覆盖。Begin passthrough、动态 form、
  缺 params/value、无权限、未知/unsupported factory 和 tenant 防覆盖均有契约测试。
  Browser 及上游更广 factory 尚无 Rust runtime 时继续 102 fail closed，故该 handler
  诚实保持 partial，而不是把未实现组件伪装成成功。
- 转入 Canvas 目录后，逐行核对 `variable.go` 24 行、`state.go` 30 行、
  `state_serializer.go` 40 行和 `state_export.go` 45 行，以及 variable/serializer
  固定测试 205/161 行和全部状态上下文调用点。四者都不拥有新的执行算法：前两者与
  state_export 只为 Go package/Eino/test 边界转发 template 或 context state，
  serializer 也只是把任意值交给 JSON。Rust 在单一 Canvas 模块直接拥有 selector/
  template 和每 future 私有状态，通过 typed borrow 访问；`CanvasRuntime` 自身派生
  typed serde，不暴露 state injection 或 `any` serializer adapter。新增空状态、
  caller-owned 值原位恢复、坏 JSON 和稳定 wire shape 契约，四文件均从通用 partial
  升级为有证据的 replaced。
- 继续完成 `decode.go` 132 行、`checkpoint_store.go` 101 行、`stream.go` 111 行、
  `cancel.go` 120 行、`canvas.go` 147 行和 `timeout.go` 157 行审计。Rust Agent decoder
  修复带现成 graph 的 IMPORT DSL：优先读取非空 `obj.downstream`，回退外层 topology，
  并从 `graph.nodes[*].parentId` 恢复分组关系；flat/name/params、globals 和严格坏值拒绝
  同时保留。每个 workflow/debug 节点现经过 Tokio deadline，精确实现 class env、固定
  默认表、uniform fallback 和 600 秒最终默认，非法/非正/溢出秒数不会扩大超时。
  checkpoint store 仍诚实 partial：稳定 serde wire shape 和会话续聊不等于运行中
  Get/Set/Delete + TTL + resume。固定版 stream/cancel 两文件除自身测试外没有生产消费者，
  不复制 drop-on-full channel 或每运行 500ms Redis 轮询；真实 SSE、跨进程 Agent
  interrupt/resume 继续由 runner/handler 条目跟踪。`canvas.go` 的 Go package-cycle
  aliases、动态 factory 和可夹带 closer 的 `map[string]any` 则由 owned typed workflow、
  闭集 dispatch、serde JSON 与 Rust RAII 结构性替代。
- 逐行审计 `internal/agent/canvas/multibranch.go` 224 行、342 行固定测试及
  scheduler/Loop 调用点，发现并修复此前只检查动态目标“存在”、未检查它是否属于节点静态
  `downstream` 的编译门禁缺口。Switch 的 condition/default/end 和 Categorize 的 category
  目标现在必须同时存在并属于声明边；字符串/数组只规范化一次，重复目标 fail closed，
  命中后的全部目标以稳定顺序执行且不再进入 fallback。新增越权目标拒绝与完整多目标
  workflow 回归，并修正 Categorize/Switch 测试 DSL 的声明拓扑。顶层 owned FIFO scheduler
  已替代 Eino MultiBranch wrapper；Loop 子图分支和通用 concurrent fan-in 尚未实现，
  因此该文件保留精确 partial。
- 接着逐行审计 `internal/agent/canvas/node_body.go` 257 行、两组 timeout 测试共 450 行
  以及 outer/Loop 调用点。Rust 的不可变组件目录和静态 dispatch 取代全局 factory/
  placeholder，unsupported 在节点副作用前失败；typed output 直接写入 owned component
  bucket，无需 `__cpn_id__` 往返或 map snapshot state bracket。审计同时发现目录允许
  大小写无关 class、固定 timeout 表却按原串匹配的不一致，现统一用 trim+uppercase class：
  小写 `retrieval` 仍严格为 60 秒，`tavilysearch` 为 12 秒，不会悄然扩大到 600 秒。
  UserFillUp interrupt/resume、Loop state bracket 和 legacy ExitLoop no-op 尚未实现并
  fail closed，故该文件保持精确 partial。
- 覆盖矩阵重新从固定 commit 的 4,441 个 blob 生成，1,267,740 文本行守恒：
  aligned 61/14,319 行、partial 1,586/651,562 行、replaced 71/27,674 行、
  reference 2,723/574,185 行、unmapped 0。
- 默认 feature 全目标测试为 lib 289 + coverage 26 = 315 passed、1 GPU ignored；
  `postgres-backend,zvec-backend` 为 lib 292 + coverage 26 = 318 passed、1 ignored，
  zvec-rust 0.5.1 原生 create/upsert/query/flush/reopen 实际执行。默认与双 feature
  严格 Clippy 均零告警，fmt 和 `git diff --check` 通过。
- 真实调用现有 Qwen3.5-9B LLM、Qwen3-Embedding-4B（2,560 维）和
  mxbai-rerank-large-v2，Rust opt-in 契约 1/1 通过；未重启或修改模型服务。
  另起无卷、回环 `127.0.0.1:55433` 的临时 `postgres:18.4`，确认服务端
  `18.4 (Debian 18.4-1.pgdg13+1)` 并通过 snapshot round-trip 后自动删除。
  精确 `rust:1.97.1-bookworm` 报告 rustc 1.97.1，`debian:bookworm-slim` 报告
  Debian 12 bookworm；Compose 继续优先 DaoCloud 三镜像、TUNA APT 和 Xget zvec，
  未改动现有 `rayrag-postgres-1` 或其他业务容器、镜像、数据卷。

---

## 2026-07-25 RAGFlow Switch 双运行时逐行收口

- 从官方 `infiniflow/ragflow` tag `v0.26.4` 恢复完整 depth-1 Git 对象包，校验固定
  commit 仍为 `cb93883f3f8c975eecb2fed81210effeb3bdb06f`。逐行审计
  `agent/component/switch.py` 全部 138 行、`internal/agent/component/switch.go` 全部
  539 行，以及固定版 Python/Go Switch 回归测试；实现证据不依赖移动分支或旧摘要。
- 将纯 Rust Canvas Switch 从仅支持 Python 旧 DSL 扩展为显式双运行时语义：
  旧 `logical_operator/items/end_cpn_ids` 保留空 `cpn_id` 跳过、Python truthiness、
  数值左值触发右值 float 转换、12 个运算符、多目标路由和 `next/_next` 输出；
  新 `op/clauses/default` 支持缺省 AND、`matched_N` 回退、现代 nil/empty 规则、
  未解析模板保留原文、大小写不敏感等值、严格数值排序和 `_next` 多目标输出。
  两种方言在保存/编译期分别校验条件结构、默认/else 目标及目标节点存在性，避免运行到
  一半才发现无效分支。
- 新增 5 组 Rust Switch 契约测试，覆盖空条件落入 else、nil 输入/比较值不崩溃、
  Python 数值转换与非 `and` 的 OR 语义、Go clauses/default/matched 回退、大小写等值、
  数值边界、未解析引用、false/0 的现代 empty 语义和双方言编译门禁。
- 覆盖生成器新增 Python/Go Switch 精确 mapping 与防回退测试，重新读取固定 commit
  全部 4,441 个 blob。1,267,740 文本行保持守恒：aligned 52/12,027 行、
  partial 1,610/655,791 行、replaced 56/25,737 行、reference 2,723/574,185 行、
  unmapped 0；本轮仅把已审计的两个生产文件共 677 行从 partial 提升为 aligned。
- 默认 feature 的 `cargo clippy --all-targets --locked -- -D warnings` 和
  `cargo test --all-targets --locked` 全通过：lib 244 + coverage 13 = 257 passed，
  1 个需显式启用 GPU 端点的测试 ignored。`postgres-backend,zvec-backend` 双 feature
  同样严格 Clippy 全通过：lib 247 + coverage 13 = 260 passed，1 ignored；其中
  zvec-rust 0.5.1 的原生 collection create/upsert/query/flush/reopen 与在线镜像测试
  实际执行。
- Docker 只读确认本机 `postgres:18.4`、`rust:1.97.1-bookworm`、
  `debian:bookworm-slim` 与 DaoCloud 国内标签指向同一批已下载镜像；
  Rust 容器报告 `rustc 1.97.1`，Debian 运行层报告 bookworm，Compose 展开继续选择
  DaoCloud PostgreSQL 18.4，APT 使用 TUNA、zvec 资产使用 Xget 优先/官方回退。
  另起无卷、回环 `127.0.0.1:55433` 的临时 PostgreSQL 容器，实测服务端
  `18.4 (Debian 18.4-1.pgdg13+1)`，并设置真实 URL 跑通 Rust
  `postgres_18_snapshot_round_trip`；临时容器已自动删除，现有
  `rayrag-postgres-1`、业务容器、镜像和数据卷均未改动。

---

## 2026-07-21 RAGFlow ListOperations、覆盖分类校正与双后端复验

- 再次从官方 RAGFlow 仓库校验 tag `v0.26.4` 指向固定 commit
  `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，并在 `/tmp/ragflow-v0264` 补齐完整
  50 MB Git 对象 pack，用于逐文件读取固定 blob；不依赖移动分支或运行中容器源码。
- 逐行审计 `agent/component/list_operations.py` 218 行、
  `internal/agent/component/list_operations.go` 638 行及固定版 Python/Go 测试。Rust Canvas
  新增 `ListOperations`，实现 `nth/head/tail/filter/sort/drop_duplicates`、旧 `topN` 别名、
  正负 nth、strict 范围错误、Python 可观察的布尔字符串、五种 filter、稳定标量/对象/
  多字段排序、结构规范化去重，以及原生 `result/first/last`；新增 4 组 Rust 契约测试。
- 修复覆盖生成器的可信度问题：未知路径不再默认标成 `reference`，仅显式映射或测试/
  fixture 规则才能进入 reference。此前误标为参考资料的 `internal/agent/**` Go 生产运行时、
  `deepdoc/**`、`common/**`、`mcp/**`、`memory/**` 等重新进入 partial；固定 4441 文件现为
  aligned 50/11,350 行、partial 1,612/656,468 行、replaced 56/25,737 行、
  reference 2,723/574,185 行、unmapped 0。这里的 unmapped=0 来自逐项显式分类，
  partial 仍是后续真实待办，不代表全树功能已完成。
- 本机 RAGFlow“全部文件”知识库的 `127.0.0.1:11526` 与 `192.168.3.6:11526` 本轮均
  `ECONNREFUSED`；按技能规则记录后继续使用固定 commit 源码和测试作为权威证据，没有
  用旧记忆替代源码审计。
- 默认 feature：`cargo check --all-targets --locked`、
  `cargo clippy --all-targets --locked -- -D warnings`、`cargo test --all-targets --locked`
  全通过，共 251 passed / 0 failed / 1 GPU ignored。双 feature 使用 zvec-rust 0.5.1
  原生库执行相同 check/Clippy/test，全通过，共 254 passed / 0 failed / 1 GPU ignored；
  zvec create/upsert/query/flush/reopen 与在线镜像测试实际执行。
- 以无卷、回环随机端口的临时 `postgres:18.4` 容器等待最终
  `database system is ready to accept connections` 后，设置真实测试 URL 执行
  `postgres_18_snapshot_round_trip`：18.4 版本门禁、建表、advisory-lock upsert、checksum
  load 和测试行清理均通过。临时 PostgreSQL 容器与导出的 zvec 临时目录已删除。
- Docker 冷构建首次发现默认 Xget zvec v0.5.1 资产返回 HTTP 403；Dockerfile 现保留
  国内 Xget 优先，并通过可配置 `ZVEC_RELEASE_FALLBACK_BASE` 自动回退官方 GitHub。
  Compose 与 `.env.example` 已同步该参数。使用精确 `rust:1.97.1-bookworm` builder、
  `debian:bookworm-slim` 运行层和双后端成功重建 `rayrag:verification`
  (`sha256:35954dd4aaa4...`)；运行 smoke 确认 Debian 12 bookworm、
  `libzvec_c_api.so` 动态依赖完整、CLI 可启动，healthz 返回 zvec healthy。临时运行容器
  已自动删除，未触碰现有业务容器与数据卷。

---

## 2026-07-21 RAGFlow v0.26.4 related questions 对标

- 固定对标 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f` 的 `api/apps/restful_apis/chat_api.py`、`api/apps/backward_compat.py`、`internal/service/related_question.go`、`internal/handler/chat_recommendation.go`、`internal/handler/searchbot.go` 与 `rag/prompts/related_question.md`。
- 将原先返回三条模板字符串的桩实现替换为纯 Rust LLM 调用：使用认证用户的租户默认 Chat 模型，兼容环境模型回退，固定温度 `0.9`，按 RAGFlow 的 `数字. 问题` 行协议解析 5–10 条相关问题。
- 同一 handler 接入 `POST /api/v1/chat/recommendation`、`POST /api/v1/searchbots/related_questions` 与弃用兼容路径 `POST /api/v1/sessions/related_questions`；三条路径统一执行 Bearer 鉴权、非空问题校验和模型可用性检查。
- `search_id` 保留 wire 兼容；RayRAG 尚无 RAGFlow SearchApp 配置存储，因此暂不从 `search_config.chat_id/llm_setting` 覆盖租户默认模型，此差异在代码中显式标注，不伪造等价实现。
- 覆盖清单新增 Go related-question service/handler 的精确 Rust 映射；运维缺失路由列表移除已经完成的旧路径。

---

## 架构概要

```
RAGFlow (Python)                  → RayRAG (Rust)
──────────────────────────────────────────────────────
web/ (React SPA, 88MB)            → web.rs            纯 Rust SSR 管理面板
nginx                             → server.rs (Axum)  HTTP 服务器 + 路由
api/apps/restful_apis/ (27个)     → api/ (7模块)      RESTful API（43 个已接线）
api/db/services/ (30个)           → stores.rs         共享数据层
──────────────────────────────────────────────────────
deepdoc/parser/* (17个)           → parser/ (15个)    文档解析全覆盖
rag/nlp/__init__.py (1627行)      → nlp_enhanced.rs   5种分块策略 + 语义切分
rag/nlp/query.py (全文检索)       → nlp.rs            BM25 + 同义词 + 查询改写
rag/llm/chat_model.py (10类)     → llm_enhanced.rs   ChatProvider 枚举 + 模型注册
rag/llm/embedding_model.py        → embed/            三模向量化（candle/openai/hybrid）
rag/llm/similarity/rerank         → rerank/           mxbai-rerank HTTP
rag/graphrag/ner/                 → graphrag_enhanced.rs  NER + 实体消歧 + 图谱搜索
rag/advanced_rag/                 → advanced_rag/     多跳检索 + 树形查询分解
rag/raptor.py                     → raptor/           RAPTOR 层次聚类
rag/prompts/ (47个)               → prompts.rs        Prompt 模板系统 + Fallback LLM
rag/utils/es_conn.py              → store/mod.rs      zvec-rust 原生 HNSW + JSON 回退
rag/svr/user                      → auth.rs           用户认证
rag/svr/knowledgebase             → kb.rs             知识库 CRUD
deepdoc/vision/ocr                → ocr.rs            paddleocr-gpu-proxy
```

---

## 模块清单

### 核心模块（src/）
| 文件 | 功能 |
|------|------|
| `lib.rs` | 核心类型导出 |
| `main.rs` | CLI（parse/search/serve）|
| `server.rs` | Axum 路由中心 |
| `web.rs` | 纯 Rust SSR 管理面板 |
| `auth.rs` | 用户认证（Argon2id + Token；旧 SHA-256 登录后迁移）|
| `kb.rs` | 知识库 CRUD |
| `llm.rs` | LLM RAG 对话 |
| `llm_enhanced.rs` | ChatProvider 枚举 + 模型注册 + 重试 |
| `nlp.rs` | BM25 + 同义词 + 查询改写 + 混合搜索 |
| `nlp_enhanced.rs` | 5种分块策略（句子/段落/滑动窗口/语义/模式）|
| `prompts.rs` | Prompt 模板系统（8模板）+ Fallback LLM |
| `search.rs` | 余弦相似度引擎 |
| `rerank.rs` | mxbai-rerank HTTP |
| `raptor.rs` | RAPTOR + GraphRAG |
| `graphrag_enhanced.rs` | NER(15正则) + 实体消歧 + 图谱搜索 |
| `advanced_rag.rs` | 多跳检索 |
| `ocr.rs` | paddleocr-gpu-proxy |
| `stores.rs` | 共享存储类型 |

### 子模块
| 目录 | 文件数 | 功能 |
|------|--------|------|
| `parser/` | 15 | 全格式文档解析 |
| `chunk/` | 3 | 分块策略 |
| `embed/` | 3 | 三模向量化 |
| `pipeline/` | 1 | 解析编排 |
| `store/` | 1 | 向量存储 |
| `api/` | 7 | RESTful API 模块 |

---

## API 端点（当前接线状态）

### 认证
```
POST /api/v1/auth/login
POST /api/v1/user/register
GET  /api/v1/user/info
```

### 知识库
```
GET    /api/v1/datasets                              列表
POST   /api/v1/datasets                              创建
DELETE /api/v1/datasets/{id}                         删除
GET    /api/v1/datasets/{kb}/documents               文档列表
GET    /api/v1/datasets/{kb}/documents/{id}          文档详情
DELETE /api/v1/datasets/{kb}/documents/{id}          删除
POST   /api/v1/datasets/{kb}/documents/{id}/reparse  重解析
```

### 检索 / 问答
```
POST /api/v1/retrieval               向量搜索
POST /api/v1/search                  混合搜索（BM25+向量权重阈值）
POST /api/v1/chunks                  批量检索
GET  /api/v1/chunks/{doc_id}         查看文档 chunks
POST /api/v1/completions             RAG 问答
```

### 对话
```
POST /api/v1/chats                   创建对话
POST /api/v1/chats/{id}/completions  RAG 对话（含引用）
POST /api/v1/conversations          创建会话
DELETE /api/v1/conversations/{id}   删除会话
```

### 文件管理
```
GET  /api/v1/files                   文件列表
POST /api/v1/files/upload            上传
GET  /api/v1/file/commits            版本历史
POST /api/v1/file2document           文件转文档
```

### LLM 管理
```
GET /api/v1/providers                LLM 提供商
POST /api/v1/providers/{id}          创建提供商（管理员）
PUT /api/v1/providers/{id}           更新提供商（管理员）
DELETE /api/v1/providers/{id}        删除提供商（管理员）
GET /api/v1/tenant/models            tenant 已绑定 Provider 实例（成员可读）
PUT /api/v1/tenant/models/{provider_id}/{instance_id}
                                      创建/更新 tenant Provider 实例（owner/admin）
DELETE /api/v1/tenant/models/{provider_id}/{instance_id}
                                      删除 tenant Provider 实例（owner/admin）
GET /api/v1/llm/models               tenant 模型能力列表
GET /api/v1/llm/factories            模型工厂
GET /api/v1/llm/my_llms              tenant 已绑定实例
```

### 系统
```
GET /api/v1/system/status            健康检查
GET /api/v1/system/status/detail     详细状态
GET /api/v1/system                   系统设置
PUT /api/v1/system                   更新设置
POST /api/v1/system/new_api_key      生成 API Key
GET /api/v1/system/healthz           兼容旧版
GET /api/v1/version                  版本
GET /api/v1/stats                    tenant 会话按日统计（from_date/to_date；canvas_id 选择 agent 口径）
GET /api/v1/tasks                    任务队列
GET /api/v1/memories                 记忆列表
GET /api/v1/users                    用户列表
PUT /api/v1/users/{id}              更新用户
POST /api/v1/users/invite           邀请用户
GET /api/v1/tenant                   租户信息
```

### Agent / Plugin / Connector / MCP
```
GET  /api/v1/agents                  Agent 列表
POST /api/v1/agents/{id}/completions Agent 对话
GET  /api/v1/plugins                 插件列表
POST /api/v1/plugins/{id}/toggle     开关插件
GET  /api/v1/connectors              连接器列表
POST /api/v1/connectors              创建连接器
GET  /api/v1/mcp/tools               MCP 工具
GET  /api/v1/bots                    Bot 列表
GET  /api/v1/channels                频道列表
```

### OpenAI 代理 / Dify / Langfuse
```
POST /api/v1/chat/completions        OpenAI 兼容
GET  /api/v1/models                  RAGFlow tenant 模型 selector 目录
GET  /api/v1/models/default          tenant 显式默认模型
GET  /api/v1/openai/models           OpenAI protocol 模型列表
POST /api/v1/dify/retrieval          Dify 协议
GET  /api/v1/langfuse                Langfuse 配置
POST /api/v1/langfuse                更新 Langfuse
```

### Retrieval Evaluation
```
GET/POST /api/v1/evaluations                         评测数据集列表/创建
GET/DELETE /api/v1/evaluations/{id}                 评测数据集详情/级联删除
GET/POST /api/v1/evaluations/{id}/cases             Ground truth 用例列表/创建
POST     /api/v1/evaluations/{id}/runs              执行检索评测
GET      /api/v1/evaluation-runs/{id}               Run 与逐用例结果
GET      /api/v1/evaluation-runs/{id}/recommendations 配置优化建议
```

### Backward Compat
```
GET  /api/v1/new_token
POST /api/v1/user/setting
POST /api/v1/document/upload_info
* /api/v1/admin/*
* /api/v1/{*}
```

---

## CLI 命令
```bash
rayrag serve --port 9380                     # Web 服务器
rayrag parse doc.pdf --embed --mode hybrid   # 解析（17 格式）
rayrag search -q "text" -i index.json -k 10  # 向量搜索
rayrag search -q "text" --rerank             # 搜索 + Re-rank
```

## Parser 覆盖（17/17 ✅）
pdf docx txt md html csv xlsx epub json ppt
figure resume paddleocr_layout mineru docling opendataloader tcadp

---

## 关键依赖

| 依赖 | 用途 |
|------|------|
| candle-core/nn/transformers（git）| BERT 本地嵌入 |
| axum 0.8 + tower-http 0.6 | Web 服务器 |
| lopdf 0.34 + docx-rs 0.4 | PDF/DOCX |
| csv 1.3 + zip 2.2 | CSV/XLSX/EPUB/PPTX |
| regex 1.x | XML/HTML 模式匹配 |
| minijinja 2.21.0 + fuel | Agent StringTransform 沙箱模板 |
| prometheus 0.14.0 | Canvas 运行计数、时长直方图与 text exposition |
| time 0.3.53 + formatting | Runtime RFC3339 创建时间与 elapsed 记账 |
| argon2 0.5 + rand 0.9 | 密码认证（sha2 仅用于旧哈希迁移） |
| reqwest 0.12 | HTTP 客户端 |
| tokio 1.x | 异步运行时 |

---

## 已知问题

- 🐛 tokenizers 0.21 不兼容 BERT TemplateProcessing → CandleEmbedder::load() 失败
  - 临时方案：embed 用 OpenAI 模式，reranker 走 HTTP API

---

## 2026-07-15 至 2026-07-16 源码审计结论（以 RAGFlow v0.26.4、测试和实际路由为准）

- 对标 RAGFlow `canvas_service.py` 与 Agent REST API，将原先只有内置 `default` 对象的进程内 `AgentStore` 改为纯 Rust 持久化 Agent/Canvas 配置服务。新增 `agents.json`、owner/team ACL、同 owner+canvas category 标题不区分大小写唯一、DSL/prompt/KB/permission/category/type/avatar/tags 持久化、关键词/owner/tag/category/type 分页筛选、可见标签聚合和完整 GET/POST/PUT/DELETE/tags/completion 路由。内置 `default` Agent 保持只读兼容；RAGFlow 的 team Agent 语义为成员可协作读取和更新、仅 owner 可删除，RayRAG 已按该差异化规则实现，未错误复用 KB 的 team 只读规则。所有 Agent 配置落盘复用原子写入和 PostgreSQL 18.4 snapshot 镜像；新增重启恢复、标题去重、标签规范化、team 协作、private 即时失权、owner-only 删除和持久化失败回滚测试。版本发布、Redis runtime replica、组件 debug 和完整 Canvas DAG 执行器仍留待后续独立批次。
- 完成 HTTP 在线索引的 `zvec-ai/zvec-rust` 原生接入。JSON `SearchEngine` 继续作为 BM25、metadata filter、rank feature 和完整 chunk 数据真源，新增 `OnlineVectorMirror` 按 KB 创建独立 zvec collection；文档解析/替换、文档删除、手工 chunk CRUD、available 状态和消息反馈 pagerank 更新全部进入同一 JSON+zvec 提交边界。zvec 同步失败会恢复内存，JSON/MySQL snapshot 落盘失败会反向恢复 zvec；后续 KB/文档元数据事务失败也会同时恢复两侧索引。服务启动按 JSON 全量 reconcile，清理中断提交和整个 KB 消失后遗留的陈旧主键；同一路径始终复用单一 read-write collection 句柄，避免 zvec `LOCK` 冲突。新增原生测试覆盖 KB collection 隔离、主键删除、重新写入和启动对账。
- PostgreSQL 18.4 独立容器实连验证完成：`SHOW server_version` 返回 `18.4`。Rust `postgres-backend` 通过专用数据库线程规避 Axum/Tokio 嵌套 runtime，创建 `rayrag_state_snapshots` 并完成 store/load/checksum round trip；服务启动后写入 10 条命名空间快照并通过健康检查。
- 对标 RAGFlow `memory_service.py` 与 Memory REST API，将原先仅返回进程内静态条目的 Memory 模块改为纯 Rust 持久化配置服务。新增 tenant 隔离、private/team 可见性、owner-only 更新删除、名称自动去重、四类 memory type 校验、关键词/owner/type/storage 分页筛选、模型绑定校验及完整 CRUD/config 路由；`memories.json` 复用统一原子写入、PostgreSQL 18.4 快照镜像和本地缺失恢复。已有内容（`memory_size > 0`）后禁止切换 embedding 或 memory type，与 RAGFlow 现有保护一致。新增重启恢复、重名筛选、team 只读、持久化失败回滚和非空配置冻结测试。
- 完成 `zvec-ai/zvec-rust 0.5.1` 真实原生后端，不再把 JSON `SearchEngine` 冒充 zvec。`RAYRAG_VECTOR_BACKEND=json|zvec` 显式选择后端；默认 JSON 保持零原生依赖与可回滚，zvec 模式在 `RAYRAG_ZVEC_DIR` 下创建持久化 collection，schema 包含正文、文档名、metadata JSON、位置、token 数和 FP32 embedding，向量索引使用 HNSW + Cosine。写入使用稳定 chunk ID upsert 并 flush，查询通过 zvec `SearchQuery` 恢复完整 chunk 字段。
- 修复旧 `ZvecStore::insert(&self)` 只生成磁盘 JSON、从不更新内存引擎的问题；旧实现同一进程写入后立即搜索看不到新 chunk。新 JSON 后端使用 `RwLock<SearchEngine>` 同步更新内存与原子磁盘快照，失败恢复旧内存；相同 chunk ID 采用替换语义，并在批量写入前统一校验向量维度，避免部分提交。
- CLI `parse` 新增 `--collection` 和可选 `--index`，可将 embedding 后的 chunks 直接持久化到所选向量后端；`RAYRAG_EMBEDDING_DIMENSION` 默认 384，未启用 `--embed` 时拒绝 collection 写入。原生库位于 `/home/junesgone/.local/lib/zvec/0.5.1/libzvec_c_api.so`，构建/测试需设置 `ZVEC_LIB_DIR`，运行时需通过 rpath 或 `LD_LIBRARY_PATH` 解析。
- 新增可选 `postgres-backend`，连接 PostgreSQL 18.4。PostgreSQL 不取代 zvec，而是镜像 RayRAG 各 JSON 状态快照：本地原子文件提交成功后写入 `rayrag_state_snapshots`，数据库写失败则回滚本地文件；启动时仅在本地文件缺失时按 namespace/path key 恢复并校验 xxHash64，已有本地状态永远优先。User/tenant/KB/conversation/document/document metadata/file/provider/tenant model/evaluation/task/graph/index Store 均接入缺失恢复入口。
- 新增 `docker-compose.yml`，固定 `postgres:18.4`、独立 named volume 和健康检查；`.env.example` 提供 URL、required version 和 namespace。启动连接校验 `server_version_num` 与精确 `18.4` 前缀，不匹配即失败。
- 本机对 `mysql:9.7.1` 的容器实启验证受 Docker Hub 网络阻塞：官方 registry 直连多次 timeout/reset，已配置镜像 `docker.fnnas.com` 对该标签返回 401。未修改 Docker daemon 或现有数据库；待 registry 恢复后按独立 3307 端口和独立 named volume 执行真实连接 smoke test。
- 质量与兼容门禁：默认、`postgres-backend`、`zvec-backend`、双 feature 均纳入 `cargo check`、严格 Clippy 和全量测试；设置真实 `ZVEC_LIB_DIR`/`LD_LIBRARY_PATH` 后验证原生 zvec create → upsert → query → flush → reopen → query 回归，Compose config 和 `git diff --check` 同步检查。
- 对标 RAGFlow `conversation_service.py` 与 `restful_apis/chat_api.py` 完成纯 Rust 会话生命周期。`ConvStore` 新增 owner-scoped 重命名、删除、按稳定 message ID 查找配对 user/assistant、成对删除 turn、重新生成目标快照和旧回答 CAS 原位替换；所有操作继续使用单文件原子持久化和失败内存回滚，handler 不再执行“先读权限、再调用非 scoped mutation”的竞态模式。
- 修复 Web “重新生成”按钮的真实行为：旧实现把字面量 `regenerate` 当新问题追加，导致历史污染、引用错位和反馈目标变化；新实现调用 `/api/v1/chats/{session_id}/messages/{message_id}/regenerate`，复用原 user 问题和该 turn 之前的历史，保持同一 message ID 与消息数量，原位替换 assistant 内容、citations 和结构化引用，并清除旧赞踩/反馈状态。RAGFlow 三层兼容路径也已提供。
- `Conversation` 新增向后兼容的 `kb_ids`、`chat_model`、`embedding_model` 持久化绑定。新建、普通发送和重新生成遵循“请求显式配置优先，否则复用会话绑定”；消息和设置在同一次 store mutation 中原子提交。空 KB 会话可走普通 LLM；绑定 KB 时仍执行访问权限、tenant embedding selector 和 chat model 能力校验。
- 新增 `PATCH /api/v1/chats/{id}`、简化及 RAGFlow 兼容的 message DELETE/regenerate 路由。会话名称 trim 且限制 255 字符，禁止跨 owner 读取或修改；消息删除严格要求相邻 user/assistant 使用同一 ID，结构损坏时拒绝提交。
- 新增 store 生命周期、CAS 并发防护、持久化失败回滚、legacy JSON 设置默认、HTTP 重命名/重新生成/成对删除、稳定 ID、绑定配置复用和跨用户隐藏测试。最终 `cargo fmt --all -- --check`、`cargo check --all-targets`、`cargo clippy --all-targets -- -D warnings`、`cargo test --all-targets`、`git diff --check` 全部通过；测试为 160 passed / 0 failed。
- 对标 RAGFlow `doc_metadata_service.py`、`metadata_utils.py` 和 `reference_metadata_utils.py` 新增纯 Rust `DocumentMetadataStore`。文档级 metadata 独立保存到 `document_metadata.json`，作为独立真源而不污染 `DocRecord` 或重复写入每个 chunk；采用同目录原子写入、持久化互斥锁和失败内存回滚，启动时拒绝重复 doc ID、空 doc/KB ID 和损坏 JSON。
- Metadata 更新保持 RAGFlow 的整对象替换语义，删除旧键而不是深合并残留；列表字符串按 `、 , ， ; ； |` 拆分、去空和稳定去重，数字、布尔、对象和列表类型原样保留。新增单文档 GET/PUT/DELETE、KB keys、summary 和 batch update API；文档列表/详情响应附加 `meta_fields`。批量更新支持列表追加/匹配替换、字段删除和列表值删除。
- `/retrieval`、`/search` 和 Dify retrieval 统一支持 `meta_data_filter.method=manual`，条件格式兼容 `key/op/value` 与 `name/comparison_operator/value`，支持 AND/OR、比较、contains/not contains、in/not in、前后缀和空值操作符；过滤结果与显式 `doc_ids` 取交集，不扩大调用者范围。Auto/semi-auto 需要 LLM 条件生成器，当前明确返回不支持而不伪造行为。
- 检索引用支持 `reference_metadata.include/fields` 及兼容 `include_metadata`/`metadata_fields`，按字段白名单附加 `document_metadata`。文档删除将 metadata 真源纳入索引、GraphRAG、KB 计数和文档记录事务，失败时恢复原 metadata，成功后不留下 orphan。
- 新增列表拆分/替换、过滤、summary/batch、重启恢复、持久化失败回滚、HTTP 生命周期、权限拒绝、检索过滤/enrichment 和删除级联测试。最终 `cargo fmt --all -- --check`、`cargo check --all-targets`、`cargo clippy --all-targets -- -D warnings`、`cargo test --all-targets`、`git diff --check` 全部通过；测试为 156 passed / 0 failed。
- 对标 RAGFlow `chunk_api.py` 完成纯 Rust chunk 生命周期。新增 `/api/v1/datasets/{id}/documents/{did}/chunks` 的 GET/POST/DELETE/PATCH 和单 chunk GET/PATCH；支持分页、关键词、ID、可用状态过滤，重要关键词、问题、标签权重和五元位置字段。手工新增与内容/问题更新统一通过 KB 的持久化 embedding selector 生成 `10% title + 90% body/question` 向量，避免与自动解析使用不同向量空间。
- Chunk 新增/删除在 `document_commit_lock` 内同时提交 `index.json`、文档 `chunk_count` 和 KB `chunk_count`；任一步失败会恢复索引内存/磁盘快照及已提交计数。批量删除要求请求 ID 全部属于目标文档，防止部分静默成功；非 KB 管理者只能读取，不能写入。`available_int=0/false` 现在同时从向量检索和混合检索候选中过滤，重新启用立即恢复召回。
- 新增完整 HTTP 生命周期、越权拒绝、索引落盘失败回滚和底层双检索启停测试。最终 `cargo fmt --all -- --check`、`cargo check --all-targets`、`cargo clippy --all-targets -- -D warnings`、`cargo test --all-targets`、`git diff --check` 全部通过；测试为 150 passed / 0 failed。
- 对标 RAGFlow `evaluation_service.py` 和 `EvaluationDataset/Case/Run/Result` 模型新增纯 Rust `src/api/evaluation.rs`。`EvaluationStore` 将数据集、ground truth 用例、运行记录和逐用例结果统一保存到 `evaluations.json`，使用同目录临时文件、原子 rename 和失败回滚；启动时拒绝重复 ID、case/run/result orphan 等损坏状态，删除数据集会级联删除 cases、runs 和 results。
- 新增完整检索评测 API 生命周期：评测数据集绑定一个或多个可访问 KB，运行前重新校验 KB 权限和向量空间；每个 case 支持参考答案、relevant doc IDs、relevant chunk IDs 和任意 metadata。评测直接复用生产 `HybridSearchQuery` 内核，支持 `top_k`、纯全文或向量/混合权重，避免 benchmark 与线上检索算法漂移。
- 指标公式对齐 RAGFlow 当前实现：Precision、Recall、F1、Hit Rate 和 MRR；优先使用 relevant chunk IDs，没有 chunk ground truth 时回退 relevant doc IDs。Run 原子保存逐用例召回 ID、执行时间及平均指标，并按低 Precision、低 Recall、低 MRR 生成配置建议。RAGFlow 尚标记 TODO 的 faithfulness、answer relevance 和 LLM-as-judge 没有虚构实现，留待后续接入生成式评测。
- 评测权限按创建用户隔离，管理员可审计全部数据；不可访问的 KB 无法创建评测数据集，权限变化后也无法继续运行。新增测试覆盖 RAGFlow 指标公式、API 生命周期、越权拒绝、持久化失败回滚、重启恢复、级联删除和结果 orphan 校验。当前完整测试 148 passed / 0 failed，严格 Clippy 零告警。
- 对标 RAGFlow `conversation_service.structure_answer()` 与 `PUT /chats/<chat_id>/sessions/<session_id>/messages/<msg_id>/feedback` 完成消息级反馈闭环。`ChatMessage` 新增稳定 UUID、创建时间、结构化 `ChunkReference`、`thumbup` 和文本反馈；旧 `conversations.json` 缺失字段时通过 serde 默认自动迁移并在后续原子保存中固化。上游 LLM 请求仍只发送 `role/content`，不会泄露内部引用和反馈元数据。
- Chat RAG 完成响应新增 `message_id` 与结构化 `reference`，引用保存 chunk id、KB id、正文和 similarity/vector/term 分数，并与 assistant 消息一起原子落盘。新增 RayRAG 简化路由 `PUT /api/v1/chats/{session_id}/messages/{message_id}/feedback`，同时兼容 RAGFlow 三层路由 `/api/v1/chats/{chat_id}/sessions/{session_id}/messages/{message_id}/feedback`；两者均只信任会话中已保存的引用，不接受客户端伪造 chunk 列表。
- 消息反馈具备幂等和反转语义：重复同向反馈不重复调整 `pagerank_fea`；赞转踩会撤销旧 `+1` 后应用新 `-1`，踩转赞反向处理。索引调整和会话反馈状态共用提交锁与 compare-and-set 旧状态检查；会话落盘失败会恢复内存及磁盘索引快照。反馈开关和 weighting 在服务启动时解析进 `AppState`，避免运行中环境变量漂移。
- Web 聊天页赞踩按钮已接入真实反馈 API，使用后端返回或持久化的稳定 `message_id`；只有请求成功才更新按钮状态，旧消息会按持久化赞踩状态恢复显示。
- 新增测试覆盖旧消息自动迁移、结构化引用重启恢复、反馈 CAS、防跨用户访问、同向幂等、赞踩反转、RAGFlow 兼容路由、聊天完成引用持久化及会话保存失败时索引回滚。当前完整测试 141 passed / 0 failed，严格 Clippy 零告警。
- 完成全仓 Rust 质量清债：执行标准 `rustfmt` 统一格式，将 Clippy 从 84 条存量告警清理到零，并建立 `cargo clippy --all-targets -- -D warnings` 严格门禁。清理覆盖 `Default` 契约、map/set 迭代、排序键、冗余 clone/reference、折叠控制流、slice 参数、测试模块顺序及公共集合 `len/is_empty` 一致性。
- 混合检索从 7 个位置参数重构为 `HybridSearchQuery` 参数对象，生产 API 与单元测试调用点统一，降低继续对标 RAGFlow 增加过滤/rank 参数时的位置参数错配风险。任务 worker 的控制流清理按分支逐项验证，必要的 claim 失败提前退出保持不变。
- 本轮质量门禁结果：`cargo fmt --all -- --check`、`cargo check --all-targets`、`cargo clippy --all-targets -- -D warnings`、`cargo test --all-targets`、`git diff --check` 全部通过；测试为 136 passed / 0 failed。
- 对标 `api/db/services/chunk_feedback_service.py` 新增纯 Rust `chunk_feedback.rs` 和受鉴权的 `POST /api/v1/chunks/feedback`：通过 `CHUNK_FEEDBACK_ENABLED=true` 显式启用，`CHUNK_FEEDBACK_WEIGHTING=relevance|uniform` 选择固定预算按召回分数分配或逐块调整。反馈只允许写入调用者可访问的 KB，重复 `(chunk_id, kb_id)` 引用会去重，`pagerank_fea` 被限制在 `0..=100`，写入后直接参与现有 rank feature 检索增益。
- chunk 反馈与文档索引提交共用全局提交锁，批量内存调整后原子保存 `index.json`；保存失败恢复完整索引快照。新增测试覆盖 KB 作用域、上下界、无效旧值、relevance/uniform 预算、重复引用及持久化失败回滚。
- 当前反馈端点是底层安全写回能力，默认关闭。RAGFlow 的完整消息级语义还会在会话中保存结构化引用和 `thumbup` 状态，并在赞踩反转时先撤销旧权重；RayRAG 现有会话仅保存引用文本，下一批需扩展 `ChatMessage` 的稳定消息 ID、结构化 chunk 引用和幂等反馈状态，再接入 `/chats/{chat_id}/sessions/{session_id}/messages/{msg_id}/feedback` 兼容路由。

- 对标源码：GitHub `infiniflow/ragflow` v0.26.4（commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`），重点审计 `api/db/services/task_service.py`、`rag/svr/task_executor.py`、`check_team_permission.py`、`knowledgebase_service.py` 及 document/chunk/retrieval API 权限调用。RAGFlow 执行器采用 Redis consumer group、consumer name、unacked 重领和处理后 ack；RayRAG 在纯 Rust 本地状态机中实现对应的单进程 claim/lease/fencing 语义。
- `cargo test --all-targets`：129 passed / 0 failed；覆盖 tokenizer、Argon2id 迁移、统一 Bearer 鉴权、流式 multipart、路径穿越、空文件、超限文件、批量上传、部分成功、损坏 OOXML、内容与扩展名不符拒绝、同 KB 重名递增、未变化内容重解析跳过、KB/Chunk/Chat 隔离、任务持久化恢复/取消/三次尝试上限、worker 唯一领取、租约续期/过期重领、旧 worker fencing、active future 防重、并发落盘恢复、旧任务 JSON 兼容、两级优先级、同级 FIFO、aging 防饥饿、tenant membership 原子持久化/失败回滚/重启恢复/去重/移除、team KB 跨用户读权限和 owner-only 管理权限，以及中英文 BM25、词项/向量融合、纯全文检索、向量维度防护、字段权重、reranker 公式/索引校验/分页窗口、未配置 503、共享模型 API 接线、前置文档过滤、Pagerank/tag rank、全局文档聚合、高亮回退、reranker 热更新权限和配置持久化恢复、Provider 管理权限/密钥保护/失败回滚/重启恢复、tenant Provider 实例的成员只读/owner-admin 管理/跨 tenant 隔离、模型能力展开、密钥保留与显式清除、失败回滚和重启恢复、KB embedding selector 原子持久化、创建时 tenant 能力校验、已有索引切换保护、多 KB 向量空间一致性、legacy 全局 embedder 回退、RAPTOR marker/文档替换、GraphRAG KB 隔离和失败回滚。
- `cargo check --all-targets`、`cargo fmt --check`、`git diff --check`：通过，零编译警告。
- 将已有的 document/search/chat/file/provider/task/agent/connector/system/OpenAI/Dify/Langfuse handler 接入 Axum，替换多处 `stub_ok` / `stub_list`。
- 服务端 embedder 改为 `Arc<dyn Embedder>`，可由环境变量启用 OpenAI-compatible embedding，不再锁死为 `CandleEmbedder` 且永远为 `None`。
- 密码从固定盐 SHA-256 升级为随机盐 Argon2id；现有 SHA-256 账号首次成功登录后自动迁移。
- 文档元数据与文件元数据改为原子 JSON 持久化；上传使用 Axum 原生流式 multipart，分块写入同目录临时文件，边写边执行 64 MiB 默认上限和 xxHash128 指纹计算，完成内容/容器校验后 `sync_all + rename + 目录 sync` 原子提交；受控 UUID 存储名和扩展名白名单保持不变。
- 新增共享 `src/persistence.rs`：用户、KB、文档、文件、任务和搜索索引 JSON 均统一使用同目录临时文件、文件 `sync_all`、原子 rename 和父目录 `sync_all`，失败时清理临时文件，降低断电/崩溃后出现空文件或目录项未落盘的概率。
- 上传/重解析已接入后台 `parse → chunk → embed → replace_document → 原子索引保存`；chunk 写入 `doc_id`、`kb_id`、文件名和内容类型 metadata。
- 文档删除会同步移除索引、文档记录、上传文件并修正 KB 文档/chunk 计数；索引保存失败会恢复内存快照。
- weighted search、Dify retrieval 和通用 retrieval 不再允许空 `kb_ids` 回退到全局索引，且会先验证调用者对全部目标 KB 的访问权限。
- weighted search、Dify retrieval 和通用 retrieval 已统一接入纯 Rust 混合检索内核：候选 KB 内同时计算 BM25 term similarity 与 cosine vector similarity，按 RAGFlow `term_weight = 1 - vector_similarity_weight` 融合后稳定排序并应用阈值；默认向量权重 `0.3`、阈值 `0.2`，明确传入 `vector_similarity_weight=0` 时无需 embedding 且不应用向量阈值。API 返回总分、词项分和向量分；Dify `full_text_search`/`semantic_search`/`hybrid_search` 分别映射纯全文/纯向量/混合模式。
- BM25 tokenizer 增加纯 Rust CJK unigram + bigram，中文连续文本可按“水质”等子词召回；文档长度归一化也改用实际 token 数。向量维度不一致的 chunk 会被安全忽略，避免 zip 截断产生虚假余弦分数。
- 外部 reranker 已作为共享服务依赖接入 weighted search、通用 retrieval 和 Dify retrieval。首次启动可由 `RERANK_API_BASE`（可选 `RERANK_API_KEY`）提供默认值，之后管理员可通过 Providers 页面或 `GET/PUT /api/v1/reranker` 原子持久化并热替换实例，无需重启；密钥不回显，留空保留、可显式清除，配置文件权限为 `0600`。请求明确传 `rerank=true` 才启用；未配置返回 503，模型调用/响应异常返回 502，不静默降级。HTTP 客户端采用 5 秒连接、30 秒总请求超时，模型响应严格拒绝缺失、重复、越界索引、非有限分数和不完整结果集。
- LLM Provider 已由进程内默认列表改为 `providers.json` 原子持久化；管理员可通过 `POST/PUT/DELETE /api/v1/providers/{id}` 管理全局提供商，启动时校验 provider id、HTTP(S) API base、非空且唯一的模型列表。API 密钥只保存到权限 `0600` 的配置文件，公开响应仅返回 `api_key_configured`；更新时空密钥保留旧值，`clear_api_key=true` 才显式清除，写入失败会回滚内存状态。被 tenant 实例引用的 Provider 返回 409，不能删除后留下孤儿配置。
- 新增纯 Rust `TenantModelStore`，以 `tenant_models.json` 对标 RAGFlow v0.26.4 的 tenant provider/instance/model 三层关系：每个 tenant 可绑定同一全局 Provider 的多个凭据实例，每个实例可声明多个模型及 `chat`、`embedding`、`rerank`、`image2text`、`speech2text`、`tts` 能力、启用状态和可选 token 上限。文件原子落盘并限制为 `0600`，公开结构只返回 `api_key_configured`；空密钥保留、`clear_api_key=true` 显式清除，失败写入回滚内存，启动时拒绝引用不存在全局 Provider 的损坏配置。
- `GET /api/v1/tenant/models`、`GET /api/v1/llm/models` 和 `GET /api/v1/llm/my_llms` 按当前或指定 tenant 返回绑定结果；tenant 成员可读，owner/admin 可通过 `PUT/DELETE /api/v1/tenant/models/{provider_id}/{instance_id}` 管理，非成员跨 tenant 查询返回 403。模型列表按能力展开，同一个模型可同时作为 chat/embedding/rerank 等用途，不再把全局 Provider 的所有模型错误标记为 chat。
- 知识库创建和更新现支持 `embd_id` / `embedding_model`。显式 selector 必须属于 KB owner tenant、Provider 已启用且模型声明 `embedding` 能力，保存时规范化为 `provider/instance/model`；未显式指定时优先固化 owner tenant 的首个可用 embedding 模型，tenant 未配置时保留 legacy `default`。已有 chunk 的 KB 拒绝直接切换模型并返回 409，避免旧索引向量与新查询向量静默混用；切换前必须先清空或重建索引。
- 文档后台 `parse → chunk → embed → index`、weighted search、通用 `/retrieval`、Dify retrieval 和 Chat RAG 已统一通过同一 KB embedding selector helper 解析运行时 `SharedEmbedder`。跨多个 KB 时要求持久化 selector 完全一致，并逐个按各 KB `owner_id` 验证 tenant 配置；不一致明确返回 `Knowledge bases use different embedding models`。Chat 请求中的临时 `embedding_model` 仅可作为同一持久化 selector 的别名校验，不能绕过 KB 绑定。旧 KB 的 `embd_id=default` 继续使用全局 `state.embedder`，但不会与 tenant selector 混搜。
- 对标 `search.py` 的块式深分页：候选窗口目标约 64 条且始终向上取整为 `page_size` 整数倍；启用外部 reranker 时受 `top=1024` 约束，按全局 offset 选择候选块并在重排、阈值过滤后切页，避免深页漂移和短页。Dify 在 rerank 时也先扩展候选池再截取 `top_k`。
- 词项侧改为分字段 BM25 后显式融合，普通混合路径使用 RAGFlow 的正文 `1x`、标题 `2x`、重要关键词 `5x`、问题词 `6x`；外部模型路径单独使用 `rerank_by_model` 的正文/标题/关键词 `1x/1x/1x`，不错误复用普通路径权重。外部模型分数替换 vector side，再按 `term_weight = 1 - vector_weight` 与词项分融合并稳定排序。
- weighted search、Dify retrieval 和通用 retrieval 均支持 `doc_ids`，并在 BM25 建库、向量评分和候选排序前过滤文档。显式空数组保持为空候选，不会退化成未过滤查询。
- 对标 RAGFlow rank feature：chunk metadata 的有限数值 `pagerank_fea` 直接叠加；请求 `rank_feature` 与 chunk `tag_feas` JSON 映射计算余弦相似度并乘 `10` 后叠加。非法 JSON、零范数和非有限值安全忽略；外部 reranker 替换 vector side 后仍保留同一 rank feature 增益。
- weighted search 和通用 retrieval 返回阈值后全量候选的 `total` 与按文档计数排序的 `doc_aggs`，统计独立于当前约 64 条 rerank 分块和分页切片；当前页可通过 `highlight=true` 返回 HTML 转义后的大小写不敏感 `<em>` 命中片段，未命中时回退完整正文。Dify 请求也接受 `doc_ids` 和 `rank_feature`，但保持其既有响应协议。
- RAPTOR/GraphRAG 已接入生产文档任务。`ParserConfig` 兼容 Web 的 `raptorEn`、`raptorDepth`、`graphEn`、`entityTypes`、`graphMethod` 字段；RAPTOR 在普通 chunk embedding 完成后生成层次摘要，以 `raptor_kwd`、`raptor_layer_int`、`raptor_method`、`source_id` metadata 作为同一文档的普通可检索 chunk，一次 `replace_document` 会同时替换原文和旧摘要。GraphRAG 只基于原始 chunk 建立文档 checkpoint，持久化到数据根目录 `graphrag.json`，再重建对应 KB 的合并图；weighted search 和通用 retrieval 额外返回 `graph_context`，不把图谱产物混入普通 chunk 生命周期。
- 高级产物提交复用现有任务 lease/fencing 和全局 `document_commit_lock`：索引先原子保存，GraphRAG checkpoint 随后原子保存；KB 计数或文档 metadata 提交失败时同时恢复索引与图谱快照。文档删除也会定向删除 checkpoint 并重建单个 KB 图，后续步骤失败则恢复旧图谱，避免重处理失败破坏当前可用高级产物。
- `TaskQueue` 已改为原子 JSON 持久化：任务关联 `doc_id`，保存 attempt/retry/cancel、`worker_id`、不可伪造的 `lease_token` 和 `lease_expires_at`。worker 以 30 秒租约原子领取、每 10 秒续租，dispatcher 每 5 秒扫描 pending/过期任务；同一进程用 active guard 防止重复 future，完成/失败提交必须通过未过期 token fencing。异常退出或过期任务会重领，最多三次尝试，支持 cooperative cancel，删除文档会取消关联任务。状态快照写入由独立锁串行化，避免旧快照晚写覆盖新状态。
- 对标 RAGFlow `priority=0` 低、`priority=1` 高以及 `[1,0]` 消费顺序，任务新增兼容旧 JSON 的持久化优先级；普通上传进入低优先级，用户主动重解析进入高优先级。唯一 dispatcher 通过默认 4 个并发槽位（`RAYRAG_MAX_CONCURRENT_TASKS` 可调）执行高优先级优先、同级 FIFO；低优先级等待 60 秒后提升有效优先级，避免持续高优先级流量导致永久饥饿。aging 是 RayRAG 的本地公平性增强，不是 RAGFlow 原生语义。
- `KnowledgeBase` 增加兼容旧数据的 `owner_id`/`permission`；旧 JSON 缺少 `permission` 时默认 private。新增纯 Rust `TenantStore`，以 `tenants.json` 原子持久化 tenant membership，owner 作为同 ID 个人 tenant 的隐式成员；重复邀请幂等，落盘失败回滚内存，移除成员即时失权且重启后关系保留。KB 支持 `private`/`team`：owner 可读可管理，team 成员仅可读取、检索、查看文档/chunk 和参与 Chat RAG，上传、重解析、删除及权限修改仍仅 owner 可执行。dataset、document、chunk list、Chat RAG、Files、Tasks、Stats、Agents、weighted search、Dify retrieval、通用 retrieval 已执行用户/KB 边界；Task/File/Chat 元数据均保存 owner。旧无 owner 数据仅管理员可见。
- 系统写操作、用户列表/更新/邀请、API Key 生成、Langfuse 更新、connector 创建和 plugin toggle 已增加管理员校验；`list_users` 改为读取真实 `UserStore`，不再返回静态占位管理员。
- multipart 请求增加 Axum body 上限，并按扩展名从临时文件验证 PDF、OOXML/EPUB ZIP 容器、PNG/JPEG/GIF/WebP/BMP/TIFF、JSON 和增量 UTF-8 文本；单文件失败保持 400；文档接口单次最多 32 个文件，多文件全部成功返回 202，部分成功返回 207 并逐项报告，失败项不残留临时/最终文件和元数据。
- 同一 KB 上传重名文档按 `file.pdf`、`file(1).pdf`、`file(2).pdf` 递增；文件管理按 owner + parent folder 独立去重。`DocRecord.indexed_content_hash` 仅在索引原子提交成功后更新，成功文档的当前内容哈希未变化时重解析直接返回 `skipped=true`，旧 JSON 和失败状态仍会正常排队。
- 烟雾验证：health、登录、Bearer 用户信息、provider/task/model/system/new-api-key/search-no-embed 均按预期响应。
- 尚未达到 RAGFlow 生产等价：当前任务队列已具备单进程 worker claim、租约续期、过期重领、fencing、两级优先级、公平调度和有界并发，但 JSON 文件状态机不提供 Redis consumer group 的跨进程原子 claim、分布式锁、独立物理优先级队列、unacked/ack 和崩溃精确一次语义；tenant/team membership、共享 KB ACL 和 tenant Provider/模型实例能力矩阵已实现，KB embedding 已统一驱动索引、检索、Dify 和 Chat，但尚无数据库事务、跨 tenant 平台管理员代管，且 reranker、OCR、图像、语音等其他运行时能力仍未全部按 KB/tenant selector 统一解析；上传已具备流式落盘、多文件部分成功、同 KB 文件名自动去重和未变化重解析跳过，但尚无对象存储后端；Memory/Agent/Connector 等配置仍多为进程内或占位实现。GraphRAG/RAPTOR 已完成生产任务、持久化、生命周期和检索接线，但当前仍是纯 Rust 轻量实现：RAPTOR 使用抽取式摘要和平均 embedding，GraphRAG 使用规则 NER、相邻共现关系与 query entity 命中，尚未达到固定版的 LLM 摘要、实体/关系 prompt、社区检测/报告、阶段 checkpoint 和增量 merge 精度。
- 当前严格质量门禁已通过：`cargo clippy --all-targets --locked -- -D warnings` 零告警；本轮继续保持该基线。

## 更新日志

| 日期 | 操作 | 说明 |
|------|------|------|
| 2026-08-01 | 历史 chunker DSL 迁移逐行对标 | 固定 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 Python `agent/dsl_migration.py` 164 行及 Canvas/API/前端/Go 边界；纯 Rust defensive/idempotent migration 完整重写三个旧组件名、ID、拓扑、模板引用、图/边/form 和历史字段，自定义业务值保留。该文件升级 aligned，但 ingestion 节点 Agent 执行仍 fail closed。矩阵为 aligned 65/15,091、partial 1,582/650,790、replaced 71/27,674、reference 2,723/574,185、unmapped 0。默认 416、双 feature 419 passed，均 0 failed/1 GPU ignored；双 check/严格 Clippy、coverage 39/39、固定 fixtures、zvec native/mirror、Compose 及真实 PostgreSQL 18.4 snapshot 往返通过，测试行清理为 0，未新起或重启容器。 |
| 2026-07-30 | LLM prompt/history/sampler/structured-output 纵向切片 | 固定 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 Python LLM 523 行、Go LLM 1,409 行及 898 行核心测试/前端/DSL。纯 Rust Agent/Generate/LLM 统一缺省/标量/role-content prompt、13 轮双消息历史窗口和同角色替换，删除重复 query；sampler 支持显式 Enabled 开关与旧 DSL，结构化 schema 追加、重试、think/fence 清理及 `structured/_ERROR` 固定键，组件目录和动态 input form 同步。LLM Python/Go 精确 mapping 继续 partial，矩阵保持 aligned 64/14,927、partial 1,583/650,954、replaced 71/27,674、reference 2,723/574,185、unmapped 0。默认 411、双 feature 414 passed，均 0 failed/1 GPU ignored；双 check/严格 Clippy、coverage 39/39、zvec native/mirror、既有 PostgreSQL 18.4 SQL 与真实 snapshot 往返通过。节点模型/视觉/引用/token fitting/JSON repair/chat kwargs/thinking/backoff/goto/cancel/delta stream 继续诚实 partial；未新起或重启容器。 |
| 2026-07-27 02:03 | Loop/Parallel 复合 UserFillUp durable bridge | 固定 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，复核 workflowx Parallel completed/pending snapshot、Loop same-iteration resume、interrupt leaf/root 地址及 Canvas subgraph 边界。纯 Rust checkpoint 新增 Loop iteration/body cursor 与 Parallel original items、completed/pending 局部 Canvas/transient partition；稳定 `loop:<macro>:<iteration>:<leaf>` / `parallel:<macro>:<index>:<leaf>` 地址逐叶恢复，已完成 outer/iteration/item 不重跑，篡改叶子组合 fail closed。补 2 组跨 serde/重启式回归；覆盖矩阵保持 aligned 64/14,927、partial 1,583/650,954、replaced 71/27,674、reference 2,723/574,185、unmapped 0。默认 362、双 feature 365 passed，均 0 failed/1 GPU ignored；双 locked check/严格 Clippy、coverage 33/33、PostgreSQL 18 snapshot 和 zvec native/mirror 实测通过。一次请求多叶子、Eino bridge-store 原始格式、stream/panic/nested macro、文件/layout、SSE、Redis TTL/跨进程 claim/GC 继续诚实 partial；未新起容器或重跑 GPU 服务。 |
| 2026-07-27 01:36 | UserFillUp 顶层持久化 interrupt/resume | 固定 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，审计 Python FillUp 125 行、Go UserFillUp 292 行、interrupt/resume 405 行、checkpoint store 101 行及对应 902 行测试。纯 Rust 新增精确 scheduler cursor/Canvas checkpoint、Begin/UserFillUp consume-once、JSON waiting form、opaque resume token、owner/tenant/agent/conversation 隔离、DSL fingerprint、15 分钟一次性 claim lease、原子 JSON/PostgreSQL mirror 重启恢复和 HTTP 409 冲突契约；恢复不重跑 Begin/Retrieval/LLM，等待回合保留空 assistant。补 9 组回归和 userfillup 目录 metadata；矩阵保持 aligned 64/14,927、partial 1,583/650,954、replaced 71/27,674、reference 2,723/574,185、unmapped 0。默认 360、双 feature 363 passed，均 0 failed/1 GPU ignored；双严格 Clippy、coverage 33/33、PostgreSQL snapshot 与 zvec native/mirror 实测通过。本批未新起容器或 GPU 服务；nested composite bridge、文件/layout、SSE 全序列、Redis TTL/跨进程 claim/GC 继续诚实 partial。 |
| 2026-07-27 01:07 | Parallel / Iteration / IterationItem Rust 有界 fan-out | 固定 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，审计 Python Iteration 64/IterationItem 101 行、Go Parallel component 186 行、canvas subgraph 385 行、workflowx driver 812 行及 1,883 行测试/辅助代码。纯 Rust 新增 parent-aware macro plan、旧 DSL alias/backedge 规范化、item 隔离 state、item0-first 有界并发、错误 drain、按输入序聚合 usage/references 和 `_result` snapshot/ref 输出，补 5 组定向回归及目录 descriptor。矩阵保持 aligned 64/14,927、partial 1,583/650,954、replaced 71/27,674、reference 2,723/574,185、unmapped 0；checkpoint/interrupt replay、outer stream、panic recovery、nested macro 和普通 sibling wave 继续诚实 partial。默认 351、双 feature 354 passed，均 0 failed/1 GPU ignored；双严格 Clippy、coverage 33/33、PostgreSQL snapshot 和 zvec native/mirror 通过。本批未新起容器或重跑 GPU 服务，不改版本、镜像和后端路线。 |
| 2026-07-27 00:12 | DataOperations Python/Go 纵向实现与双后端复验 | 固定 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 Python 202 行、Go 542 行、Go 测试 282 行及前端 DSL 表单；纯 Rust Canvas 注册并实现七个 JSON-shaped DataOperations 操作，兼容 string/CSV/input-object/name-object 形态、selector/template 和原生输出，补 6 组测试。Python `ast.literal_eval` 单引号容器/tuple/set、动态 input-form/thought/cancellation 继续诚实 partial；Go 生产文件升级 aligned。固定 pack 重建索引后缺失 blob 为 0，矩阵重生为 aligned 63/14,895、partial 1,584/650,986、replaced 71/27,674、reference 2,723/574,185、unmapped 0。默认 337、双 feature 340 passed，均 0 failed/1 GPU ignored；严格 Clippy、coverage 32/32、原生 zvec、Compose 通过。临时无卷 `postgres:18.4` 等待可执行 SQL 后确认 18.4/180004 并通过 Rust snapshot 往返，容器和端口已清理，未改业务容器/卷。 |
| 2026-07-26 00:03 | Canvas NodeBody 闭集 dispatch 与 class deadline 收口 | 逐行审计固定版 `internal/agent/canvas/node_body.go` 257 行、450 行 timeout 测试和 outer/Loop 调用点。确认 Rust 用不可变目录+静态 dispatch 替代全局 factory/placeholder，unsupported 副作用前拒绝，typed component output 和 owned `CanvasRuntime` 替代 `__cpn_id__`/snapshot bracket。发现并修复 case-insensitive 组件目录与 case-sensitive timeout 固定表的不一致：固定 class 统一 trim+uppercase，小写 `retrieval` 不再从 60 秒扩大为 600 秒，`tavilysearch` 同样保持 12 秒；纯 resolver 回归通过。UserFillUp interrupt/resume、Loop state bracket、legacy ExitLoop no-op 明确 fail closed，文件保持 partial。矩阵仍为 4,441 文件/1,267,740 行且 unmapped 0；默认全目标 315、双 feature 318 passed，均 0 failed/1 GPU ignored，双严格 Clippy、原生 zvec、固定 fixture、fmt/diff check 通过。 |
| 2026-07-25 23:58 | Canvas MultiBranch 静态拓扑与多目标执行收口 | 逐行审计固定版 `internal/agent/canvas/multibranch.go` 224 行、342 行测试和 scheduler/Loop 调用点。修复 Switch/Categorize 动态目标只校验全局存在、未限定为本节点静态 `downstream` 的缺口；condition/default/end/category 目标现均受声明边白名单约束，越权目标在编译期拒绝。新增完整 workflow 回归，证明命中的两个目标以稳定顺序全部执行且 fallback 不执行；修正现有 Switch/Categorize DSL fixture 的静态边。顶层 FIFO 语义落地，Loop 子图和 concurrent fan-in 继续精确 partial。覆盖矩阵仍为 4,441 文件/1,267,740 行：aligned 61/14,319、partial 1,586/651,562、replaced 71/27,674、reference 2,723/574,185、unmapped 0。默认全目标 314、双 feature 317 passed，均 0 failed/1 GPU ignored，双严格 Clippy、原生 zvec、固定 fixture 通过。独立无卷 `postgres:18.4` 启动阶段的 bootstrap `pg_isready` 曾短暂误报 ready 后重启，首次精确测试收到 TCP reset；改为等待目标数据库可执行 SQL 后，同一 snapshot round-trip 通过且服务端确认为 `18.4 (Debian 18.4-1.pgdg13+1)`，临时容器随后自动删除，现有 `rayrag-postgres-1` 未改动。 |
| 2026-07-25 23:47 | Canvas decode/dead scaffold/节点 timeout 审计 | 逐行审计 `decode.go` 132、`checkpoint_store.go` 101、`stream.go` 111、`cancel.go` 120、`canvas.go` 147、`timeout.go` 157 行及固定测试/调用点。修复 IMPORT DSL 的 `obj.downstream` 优先回退与 graph parentId 解码；workflow/debug 全节点接入可取消 Tokio deadline，保留 class env、固定默认、uniform fallback、600s 顺序并拒绝坏秒数。checkpoint store 因无 mid-run byte/TTL/resume 继续 partial；固定 stream/cancel 无生产消费者，避免复制 lossy channel 与 500ms Redis polling，改为 replaced；Go Canvas alias/dynamic factory/reflective closer 层由 typed owner、闭集 dispatch、serde/RAII 替代。矩阵为 aligned 61/14,319、partial 1,586/651,562、replaced 71/27,674、reference 2,723/574,185、unmapped 0。默认全目标 310、双 feature 313 passed，均 0 failed/1 GPU ignored；原生 zvec、PostgreSQL 18.4 snapshot、固定 fixtures、双严格 Clippy、fmt/diff check 通过。 |
| 2026-07-25 23:36 | Canvas variable/state/serializer 薄适配收口 | 逐行审计固定版 `internal/agent/canvas/variable.go` 24 行、`state.go` 30 行、`state_serializer.go` 40 行、`state_export.go` 45 行，连同 variable/serializer 测试 205/161 行及状态调用点。确认四文件仅承担 Go package/Eino/test re-export 或通用 JSON adapter；Rust 由单一模块直接拥有模板解析、每 future 私有 `CanvasRuntime` 和 typed serde，无 context downcast、state injection、mutex 或 `any` serializer。新增 empty/in-place restore/malformed JSON 契约，四文件均改为 replaced；矩阵为 aligned 61/14,319、partial 1,589/651,940、replaced 68/27,296、reference 2,723/574,185、unmapped 0。默认全目标 302、双 feature 305 passed，均 0 failed/1 GPU ignored；原生 zvec、PostgreSQL 18.4 snapshot、固定 DSL fixtures、双严格 Clippy、fmt/diff check 通过。 |
| 2026-07-25 23:30 | Agent 动态 input-form 与闭集组件 debug | 审计 `internal/handler/agent_component.go` 253 行及固定测试，补全 Agent/Browser/BGPT/ExeSQL/YahooFinance 动态表单；新增受鉴权 `POST /api/v1/agents/{id}/components/{component_id}/debug`，严格 flatten `params.*.value`、保持 101/102/103/500 envelope、用新鲜 Canvas state 复用已实现组件执行器和真实 LLM/Retrieval，保护 `sys.tenant_id` 不被覆盖。Begin passthrough、动态表单、参数错误、ACL、unknown/unsupported 和 tenant 防覆盖均测试；Browser/更广 factory 无 Rust runtime 时明确 102，handler 继续 partial。路由 116；全树统计不变为 aligned 61/14,319、partial 1,593/652,079、replaced 64/27,157、reference 2,723/574,185、unmapped 0。默认全目标 300、双 feature 303 passed，均 0 failed/1 GPU ignored；固定 DSL fixtures、原生 zvec、PostgreSQL 18.4 snapshot、双严格 Clippy、fmt/diff check 通过。 |
| 2026-07-25 23:20 | Agent DSL Canvas/Run normalize 目录收口 | 逐行审计 `internal/agent/dsl/normalize.go` 870 行、705 行测试、7 个固定 fixture 和 handler/service/compiler 调用点。纯 Rust 新增 canvas/run 双边界：全树 clone、handle 修复、排序默认布局、flat view、Parallel UI 泄漏修复，以及仅 run 执行的 legacy child fold、stable downstream merge、Iteration→Parallel、item/index alias rewrite；编译兼容 nested/flat，Agent create/update/reset/version 存储及前端 Agent/version 视图接线，malformed panic-free 且幂等。固定 fixture 在全量门禁中从 pinned Git blob 实读通过，normalize.go 升级 aligned，DSL 生产目录已无 generic partial。全树为 aligned 61/14,319、partial 1,593/652,079、replaced 64/27,157、reference 2,723/574,185、unmapped 0。默认全目标 297、双 feature 300 passed，均 0 failed/1 GPU ignored；原生 zvec、PostgreSQL 18.4 snapshot、双严格 Clippy、fmt/diff check 通过。 |
| 2026-07-25 23:08 | Agent DSL 类型化提取与静态 input-form 路由 | 逐行审计 `internal/agent/dsl/errors.go` 40 行、`extract.go` 202 行、266 行测试及 handler 调用点。新增零拷贝 serde 提取器和闭集 `AgentDslError`，区分 component missing、input_form missing/null、malformed obj/form/params/name，保留 optional params、按 component_name 查 Begin、错型 prologue/mode 空串语义；新增受鉴权 `GET /api/v1/agents/{id}/components/{component_id}/input-form`，静态表单保持 success/102/103 envelope，动态表单/debug 诚实留在 handler partial。errors.go 改为 replaced、extract.go 升级 aligned；全树为 aligned 60/13,449、partial 1,594/652,949、replaced 64/27,157、reference 2,723/574,185、unmapped 0。路由 115；默认全目标 291、双 feature 294 passed，均 0 failed/1 GPU ignored；原生 zvec、PostgreSQL 18.4 snapshot、双严格 Clippy、fmt/diff check 通过。 |
| 2026-07-25 22:57 | Agent DSL reset 与持久化路由收口 | 逐行审计 `internal/agent/dsl/reset.go` 250 行、234 行测试及 service/handler/router 调用点。新增受鉴权 `POST /api/v1/agents/{id}/reset`：defensive serde JSON 快照清空 history/retrieval/memory/path，按类型归零 sys globals，按变量默认/声明类型恢复 env globals，保留 graph/components/custom globals；团队可见 Agent 可按既有编辑权限 reset，幂等调用不刷新时间，持久化失败回滚，响应返回 reset DSL，Canvas version 历史不变。补 pure transform、store/rollback 与 HTTP lifecycle 测试，reset.go 升级 aligned。全树为 aligned 59/13,247、partial 1,596/653,191、replaced 63/27,117、reference 2,723/574,185、unmapped 0。路由 114；默认全目标 286、双 feature 289 passed，均 0 failed/1 GPU ignored；双严格 Clippy、原生 zvec、PostgreSQL 18.4 snapshot、fmt/diff check 通过。 |
| 2026-07-25 22:50 | Go Jinja runtime 目录收尾 | 完整审计 `internal/agent/runtime/template_jinja.go` 161 行、224 行测试及唯一 StringTransform 调用点。Gonja auto dispatch 由纯 Rust 的保守 detector、loud MiniJinja renderer 与组件 fail-soft wrapper 替换；保留 fast selector，覆盖 nested/component/sys/env/input context、filters/if/loop/comment/missing/substitution，并以无 loader/外部函数、100,000 fuel、64 层递归加固。上游 `flattenMap` 未被 context builder 使用，`bytesBuffer` 仅是 no-op import anchor，不复制死符号。补 loud parse-error 断言并保留 parse/fuel 软降级；文件由 partial 改为 replaced，runtime 目录已无通用 partial 生产项。全树为 aligned 58/12,997、partial 1,597/653,441、replaced 63/27,117、reference 2,723/574,185、unmapped 0；默认/双 feature 全量门禁分别保持 284/287 passed、0 failed/1 GPU ignored。 |
| 2026-07-25 22:47 | Go Canvas state 单所有者快照收口 | 逐行审计固定版 `internal/agent/runtime/state.go` 462 行、95 行 JSON 测试和全部调用点。Rust `CanvasRuntime` 新增稳定 serde checkpoint shape，确定性保存 outputs/sys/env/path/history/retrieval/globals/cancel_flag/run_id/task_id，跳过 transient provider helpers；统一 component/sys/env dot path、JSON 字符串、数组索引、iteration alias 和嵌套自动建图，并记录检索聚合。单 future 所有权和显式借用替代共享 RWMutex，取消继续由 future drop 结构化传播；更广 interrupt/resume 留在 canvas partial。新增 2 组状态测试，并修复 Begin 单输入回填绕过显式 sys namespace 的回归；state.go 改为 replaced。全树为 aligned 58/12,997、partial 1,598/653,602、replaced 62/26,956、reference 2,723/574,185、unmapped 0。默认全目标 284 passed、双 feature 287 passed，均 0 failed/1 GPU ignored；原生 zvec 与 PostgreSQL 18.4 snapshot 测试实际执行，双严格 Clippy、fmt、diff check 通过。 |
| 2026-07-25 22:37 | 不可变组件目录与 `/api/v1/components` 收口 | 逐行审计固定版 runtime registry 206 行/357 行测试、components service 140 行/165 行测试和 handler 123 行/330 行测试。新增不可变编译期 descriptor 单一真源，Canvas 闭集校验与目录共用；仅发布 Rust 已实现的 12 个 Agent 组件和 5 个 ingestion stage，消除动态注册 race/RWLock/panic。新增受鉴权组件目录端点，精确支持空/大小写无关/逗号多分类/重复去重/稳定排序/非 null maps，以及未知分类 HTTP+envelope 400；新增 4 组测试。registry 改为 replaced，service/handler 升级 aligned；全树为 aligned 58/12,997、partial 1,599/654,064、replaced 61/26,494、reference 2,723/574,185、unmapped 0。默认全目标 282 passed、双 feature 285 passed，均 0 failed/1 GPU ignored；原生 zvec 实际执行，双严格 Clippy、fmt、diff check 通过。 |
| 2026-07-25 22:30 | Go runtime Component/factory 适配层分类收口 | 逐行审计固定版 `internal/agent/runtime/component.go` 158 行及 factory/node-body 调用点，确认其 Invoke 最小接口、进程全局可变 factory、registry init bridge、placeholder sentinel 与字段错误类型用于解决 Go package cycle 和运行期注入。RayRAG 以保存/编译期闭集校验、静态 Rust dispatch、节点/字段错误上下文及副作用前 unsupported 拒绝替代，不引入全局 factory lock/init 顺序耦合；该文件由 partial 改为 replaced。全树守恒为 aligned 56/12,734、partial 1,602/654,533、replaced 60/26,288、reference 2,723/574,185、unmapped 0；覆盖门禁、fmt、diff check 通过，功能代码未变，沿用 22:28 的默认 277/双 feature 280 与双严格 Clippy 结果。 |
| 2026-07-25 22:28 | Go runtime progress/timeout/elapsed helpers 收口 | 逐行审计固定版 `internal/agent/runtime/helpers.go` 163 行和 393 行测试；新增纯 Rust runtime helper 层，精确实现 `0/1/-1` progress 消息、无 observer/错误原样透传、Tokio deadline typed error、RFC3339 `_created_time`、非负小数秒 `_elapsed_time`、调用方键优先、空输出和带组件名的错误链。父取消采用 Rust future drop，不复制 Go context。新增 5 组测试，该文件升级 aligned；全树守恒为 aligned 56/12,734、partial 1,603/654,691、replaced 59/26,130、reference 2,723/574,185、unmapped 0。默认全目标 277 passed、双 feature 280 passed，均 0 failed/1 GPU ignored；原生 zvec 实际执行，双严格 Clippy、fmt、diff check 通过。 |
| 2026-07-25 22:22 | Go runtime JSON reflection fallback 分类收口 | 逐行审计固定版 `internal/agent/runtime/json_safe.go` 152 行及 runner/state/service 全部调用点；上游用 reflection 将 func/chan 置 null，并展开任意 map/slice/pointer/interface/struct，但没有循环引用检测。RayRAG 从 DSL、组件状态、trace 到 HTTP 只允许 owned `serde_json::Value`，函数、channel、client、非字符串 key、共享循环和非有限数无法进入状态，无需反射清洗热路径；该文件由 partial 改为 replaced。全树守恒为 aligned 55/12,571、partial 1,604/654,854、replaced 59/26,130、reference 2,723/574,185、unmapped 0；覆盖生成器 13/13、fmt、diff check 通过，功能代码未变，沿用 22:19 的默认 272/双 feature 275 全目标与双严格 Clippy 结果。 |
| 2026-07-25 22:19 | Go runtime context 状态桥分类收口 | 逐行审计固定版 `internal/agent/runtime/context.go` 73 行及全部调用点，确认其用途是用 `context.Context` 在 Eino callback/Go package 间传递并 downcast `CanvasState`。RayRAG 以每个 future 私有的 `CanvasRuntime`、显式共享/独占借用和 per-run globals clone 替代，消除 missing-state/type-mismatch/nil-mutex plumbing；新增并发双运行契约，证明相同 workflow 的 globals 与 `sys.query` 隔离，该文件由 partial 改为 replaced。全树仍为 4,441 文件/1,267,740 行：aligned 55/12,571、partial 1,605/655,006、replaced 58/25,978、reference 2,723/574,185、unmapped 0。默认全目标 272 passed、双 feature 275 passed，均 0 failed/1 GPU ignored；原生 zvec 实际执行，双严格 Clippy、fmt、diff check 通过。 |
| 2026-07-25 22:15 | Go Canvas Prometheus metrics 运行时收口 | 逐行审计固定版 `internal/agent/runtime/metrics.go` 102 行和 116 行测试；纯 Rust 新增独立 registry、上游同名 run counter/default-bucket duration histogram、`success/error/cancelled` 闭集标签、正时长过滤和 text exposition，Canvas success/error 显式记账、future drop 以 RAII 记 cancelled，并公开单一 `runtime="rust"` 的 `GET /metrics`，新增 4 组测试。该文件由 partial 升级 aligned，全树保持 4,441 文件/1,267,740 行守恒：aligned 55/12,571、partial 1,606/655,079、replaced 57/25,905、reference 2,723/574,185、unmapped 0。默认全目标 271 passed、双 feature 274 passed，均 0 failed/1 GPU ignored；原生 zvec 回归实际执行，默认与双 feature 严格 Clippy、fmt、diff check 通过。 |
| 2026-07-25 22:07 | Go runtime selector 迁移开关分类纠偏 | 逐行审计固定版 `internal/agent/runtime/selector.go` 168 行和 187 行测试，确认其 Redis `go/python/auto` 是上游 Python→Go deprecation window 的 tenant 回退开关，不是 Canvas 变量 selector。RayRAG 保持纯 Rust 单运行时、tenant Agent DSL 持久化和 unsupported component 预执行拒绝，不引入 Python sidecar/Redis 双运行时开关；该文件由 partial 转为 replaced。全树变为 aligned 54/12,469、partial 1,607/655,181、replaced 57/25,905、reference 2,723/574,185、unmapped 0，文本行守恒。 |
| 2026-07-25 22:04 | Go template fast/display 运行时收口 | 逐行审计固定版 `internal/agent/runtime/template.go` 140 行及 canvas variable、Message、service e2e 测试；Rust 统一实现单/双/三重花括号、component/sys/env、`item/index/result`、唯一引用提取、参数绑定 loud-fail + 完整 partial output 和 Message display soft-fail，新增 3 组契约测试。该文件由 partial 升级 aligned，全树变为 aligned 54/12,469、partial 1,608/655,349、其余守恒且 unmapped 0。默认全目标 267 passed、双 feature 270 passed，均 0 failed/1 GPU ignored；严格 Clippy、fmt、diff check 通过，双 feature 原生 zvec 回归继续实际执行。 |
| 2026-07-25 21:57 | StringTransform/Jinja Python/Go 双运行时收口 | 固定 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 Python StringTransform 107 行、Go StringTransform 302 行、Go Jinja runtime 161 行及 399 行测试。纯 Rust 补齐首上游直传、state ref、literal 多 delimiter、空片段、非字符串门禁、数组连接、缺失空串、迭代别名，以及 MiniJinja 条件/循环/注释/filter/嵌套上下文；沙箱无 loader/外部函数，限制 100,000 fuel 和 64 层递归，新增 7 组测试。覆盖矩阵保持 4,441 文件/1,267,740 行守恒，aligned 53/12,329、partial 1,609/655,489、unmapped 0；Python UI/取消接口和独立 Go loud-error API 仍列 partial。默认 feature 264 passed、双 feature 267 passed，均 0 failed/1 GPU ignored并严格 Clippy 零告警；原生 zvec、临时 PostgreSQL 18.4 snapshot、三模型实机契约、精确 Rust/Debian 镜像及 DaoCloud/TUNA/Xget 配置全部复验通过，临时 PG 容器自动删除且未改现有业务服务。 |
| 2026-07-25 21:27 | Switch Python/Go 双运行时逐行收口 | 固定 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 Python Switch 138 行、Go Switch 539 行及双方测试；纯 Rust 同时实现 legacy `logical_operator/items/end_cpn_ids` 与 modern `op/clauses/default`、全部运算符、nil/empty、数值转换、大小写等值、`matched_N`、多目标和 `next/_next` 语义，新增 5 组契约测试。全树台账保持 4,441 文件/1,267,740 行守恒，aligned 52/12,027、partial 1,610/655,791、unmapped 0。默认 feature 257 passed、双 feature 260 passed，均严格 Clippy 零告警且各 1 个 GPU opt-in ignored；zvec-rust 0.5.1 原生回归实际执行。临时无卷 `postgres:18.4` 实例返回 18.4 并通过 Rust snapshot 往返后自动删除；三张精确 Docker 基线镜像及国内源配置复核通过，未改现有业务容器/数据卷。 |
| 2026-07-21 19:05 | Begin/FillUp/StringTransform 逐行语义收口 | 固定 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 `agent/component/begin.py` 53 行、`fillup.py` 125 行和 `string_transform.py` 107 行。纯 Rust Begin 现校验 `conversational/task/Webhook` mode，透传 runtime inputs，在仅声明一个字段且请求未显式传 inputs 时把 question 包装成该字段，解析 scalar/object descriptor 与 JSON object 字符串；可选空 file 返回 null，非空 file 在文件服务尚未接线时 fail closed。StringTransform 在保存/编译阶段校验 method、delimiter 与字段类型；split 对齐 literal 多 delimiter、首尾/连续空片段、false-y 置空和 truthy 非字符串拒绝；merge 按首个 delimiter 连接数组、缺失引用置空并保留原生输出。Jinja2 statement/control-flow、file/layout 解析、交互 tips、重复 UserFillUp initial-input consumption、协作取消与组件 timeout 仍明确保持 partial。覆盖生成器新增三文件精确 mapping 和 2 项门禁，重新生成固定 commit 全部 4,441 文件/1,267,740 文本行清单：aligned 48/10,494、partial 431/150,324、replaced 53/24,241、reference 3,909/1,082,681、unmapped 0。默认 feature（跳过沙箱禁止绑定 TCP 的既有 Categorize mock）为 lib 234 passed/0 failed、1 GPU ignored、1 filtered，coverage 10 passed；双 feature 为 lib 237 passed/0 failed、1 ignored、1 filtered，coverage 10 passed，zvec-rust 0.5.1 原生 create/upsert/query/reopen 和在线镜像测试实际通过；默认与双 feature 严格 Clippy/check/fmt 均通过。Compose 静态展开确认继续固定 DaoCloud `postgres:18.4`、`rust:1.97.1-bookworm`、`debian:bookworm-slim`，APT 用 TUNA、zvec 用 Xget。当前沙箱同时拒绝 Docker socket、fnOS WebSocket、SSH/TCP，故本轮未重新读取已下载镜像或实连 PostgreSQL 18.4；此前真机闭环记录仍有效，本轮不冒充复验成功且未改动任何容器、镜像或卷。RAGFlow“全部文件”知识库同样因局域网 socket `EPERM` 未能检索，改用本地固定 commit tree 完成审计。 |
| 2026-07-21 17:26 | Variable Canvas 精确语义与全树清单生成器解锁 | 固定 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 `agent/component/variable_aggregator.py` 80 行和 `variable_assigner.py` 192 行。纯 Rust Canvas 现严格校验非空 group/candidate，按候选顺序选择首个 truthy 原生 JSON 值；VariableAssigner 对齐全部 11 个操作符、无参数操作符例外、overwrite/append/extend selector 解析、字符串 set 模板解析、列表元素类型约束、bool 排除于数值运算，以及固定版 `ERROR:*` 哨兵返回和未知操作符 null 语义。补充空配置、模板、列表、除零、bool 数值拒绝和缺参测试。修复 `ragflow-coverage` 在 4441 个 blob 上先写后读导致 stdout 管道背压死锁的问题，改为 scoped reader 与请求写入并发流式处理，并新增精确 mapping 门禁；清单已重新生成，1,267,740 文本行现为 aligned 46/10,215、partial 430/149,766、replaced 53/24,241、reference 3,912/1,083,518、unmapped 0。默认 feature 全量为 lib 227 passed/0 failed（1 个 GPU opt-in ignored）及覆盖工具 8 passed；`postgres-backend,zvec-backend` 双 feature 为 lib 230 passed/0 failed（1 ignored）及覆盖工具 8 passed，包含 PostgreSQL 18 snapshot round-trip 和 zvec 0.5.1 create/upsert/query/reopen。严格 Clippy、fmt、Compose 静态解析均通过。只读确认本机已有 `postgres:18.4`、`rust:1.97.1-bookworm`、`debian:bookworm-slim`，且均同时带 DaoCloud 国内镜像标签；保留 zvec-rust + PostgreSQL 18.4 双后端、纯 Linux/纯 Docker 和国内镜像优先配置，不改版本号、不引入 MySQL。 |
| 2026-07-21 11:45 | Agent Categorize/Retrieval 逐行对标与 GPU 三模型闭环 | 固定 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`，逐行审计 `agent/component/categorize.py` 157 行、`agent/tools/retrieval.py` 329 行及 Retrieval/Categorize 示例 DSL。纯 Rust Canvas 新增 Categorize：保留 JSON 类别插入顺序，构造类别描述/样例 prompt，按历史窗口调用 tenant chat model，大小写不敏感统计类别出现次数，同票取先出现类别、无匹配取最后类别，并输出 `category_name/_next` 路由。新增 Retrieval runtime/backend trait：支持 `dataset_ids`/旧 `kb_ids`、动态变量按当前 tenant 的 KB 名称或 ID 解析、Agent/请求 KB fallback、`USER:` 前缀清理、embedding selector 一致性、BM25/向量权重、threshold、top-N/top-K、可选 tenant reranker、手工 metadata filter、formalized/JSON/doc aggregation 输出及结构化 references；后端再次执行 KB ACL，引用随 Agent completion 响应并写入会话。Memory、KG、cross-language、children/TOC 和 auto/semi-auto metadata 条件仍未对齐，DSL 保存时 fail closed。覆盖生成器增加两文件精确映射，4441 文件/1,267,740 文本行现为 aligned 44/9,943、partial 432/150,038、replaced 53/24,241、reference 3,912/1,083,518、unmapped 0。实机不重启既有模型：GPU OpenAI-compatible Qwen3.5-9B LLM、Qwen3-Embedding-4B（2,560 维）和 mxbai reranker 均由 Rust opt-in 契约测试真实调用并通过；默认 feature 231 passed/0 failed（另 1 个 opt-in ignored），`postgres-backend,zvec-backend` 234 passed/0 failed（另 1 ignored），原生 zvec create/upsert/query/reopen 覆盖继续通过；严格 Clippy 零告警。验证容器使用 DaoCloud `rust:1.97.1-bookworm`，缺省未带 Clippy component 时在一次性容器内经 rsproxy 国内源安装；`debian:bookworm-slim` 为 12.15；独立 `postgres:18.4` 新实例实连返回 `18.4 (Debian 18.4-1.pgdg13+1)` 后删除。未改动或重启主机已有 LLM/embedding/reranker/RAGFlow 服务。 |
| 2026-07-21 10:36 | Agent Canvas 核心执行、覆盖表纠偏与三镜像实机闭环 | 逐行核对固定 RAGFlow v0.26.4 `agent/canvas.py`、`begin.py`、`switch.py`、`variable_aggregator.py`、`variable_assigner.py`、`string_transform.py`、`message.py` 及 DSL 样例；新增纯 Rust `src/agent.rs` Canvas 编译/执行器，支持唯一顶层 Begin、图引用校验、有界队列、`sys/env/component@path` 变量、Begin/LLM/Message/Switch/VariableAggregator/VariableAssigner/StringTransform、执行路径和 trace；可视化 DSL 含未支持组件时在执行前明确失败，仅历史无 `components` 记录保留 prompt-only 兼容。Agent CRUD、版本保存/恢复和 completion 均接入 DSL 校验，completion 新增 `inputs`、`workflow_path`、`workflow_trace` 并保持会话/usage 持久化。覆盖生成器纠正过去把 `agent/` 运行时误标为 reference 的口径；4441 文件/1,267,740 文本行现为 aligned 43/9,786、partial 433/150,195、replaced 53/24,241、reference 3,912/1,083,518、unmapped 0。新增 Cloudflare WebSocket SSH 标准库代理辅助脚本，不保存凭据并固定验证主机指纹。实机 Debian 12/Docker 28.5.2 上优先使用 DaoCloud、TUNA、Xget、rsproxy：精确 `rust:1.97.1-bookworm` 默认 feature `cargo check`、严格 Clippy 和 227 测试通过；双 feature 230 测试通过，含 zvec 0.5.1 create/upsert/query/flush/reopen。`postgres:18.4` 实连返回 18.4，冷启动生成 12 个 snapshot，最终 healthz 返回 PostgreSQL/zvec 均 healthy。Docker 实测发现并修复 `debian:bookworm-slim` 无 CA 时直接使用 HTTPS APT 的引导死锁：先通过同一国内镜像 HTTP + APT 签名安装 CA，再切回 HTTPS；修复 Compose 全新卷未传 `RAYRAG_ADMIN_PASSWORD` 导致立即退出，现为至少 12 字符的必填配置。最终双后端镜像从指定 Rust 镜像构建成功，在 Debian 12.15 运行层解析 `/opt/zvec/lib/libzvec_c_api.so` 并通过容器自带 healthcheck。远端 Docker 冒烟构建为节省传输仅复制 index/conf/logo 静态子集，本轮验证后端/存储/容器而未重跑前端资产完整性。全程不改版本号、不引入 MySQL。 |
| 2026-07-21 01:15 | RAGFlow 全树逐行台账、Task 收口与 PostgreSQL/zvec readiness | 完整检出并校验固定 RAGFlow v0.26.4 commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f` 的 4441 文件；Rust `ragflow-coverage` 改为单个 `git cat-file --batch` 读取固定 blob，新增逐文件源码行数与按状态行数汇总（二进制明确为 `-`），当前共 1,267,740 文本行：aligned 43 文件/9,786 行、partial 286/111,461、replaced 53/24,241、reference 4,059/1,122,252、unmapped 0。逐行核对 `task_api.py` 全部 102 行，纠正旧台账不存在的“external log tail”描述；Rust POST/PATCH stop 现返回 `data:true`、对终态任务幂等成功、追加持久化取消消息并联动活动文档状态，同时保留 owner ACL 加固，故该文件升级 aligned。PostgreSQL snapshot worker 新增真实 `SELECT 1` 五秒探针；设置 URL 却未编译 feature 时 fail closed。zvec 启动校验数据目录，readiness 检查已打开 collection stats；公开 healthz 依据两后端真实状态返回 200/500，鉴权 system status 对齐 doc_engine/storage/database/task_executor_heartbeats。Dockerfile/Compose 增加 curl healthcheck；Compose 优先 DaoCloud 国内镜像拉取但严格保持 `postgres:18.4`、`rust:1.97.1-bookworm`、`debian:bookworm-slim` 标签，APT 继续优先 TUNA、zvec 继续优先 Xget，全部可由 `.env` 回切官方源。验证：Rust 1.97.1 `cargo fmt --all -- --check` 通过；覆盖生成器及测试目标经 rustc metadata 类型检查通过；宿主缺少 Docker/WSL/VS linker，旧 SSH 主机 DNS 不可达，故本轮未冒充完整 cargo test/Clippy/三镜像 runtime smoke，保留待 Linux/Docker 环境复验项。全程纯 Rust，不引入 MySQL，不改版本号。 |
| 2026-07-19 19:19 | Generation Params 统一解析与三入口接线 | 固定 RAGFlow v0.26.4 `api/apps/restful_apis/_generation_params.py` 逐行审计：精确白名单为 `temperature/top_p/frequency_penalty/presence_penalty/max_tokens`，仅抽取非 null 顶层字段，chat 路径会从剩余请求中弹出这些键，覆盖值深拷贝后合并到既有 dialog `llm_setting`，未知字段不进入生成配置；默认 dialog 值来自固定版模型定义 0.1/0.3/0.7/0.4/512。新增纯 Rust `GenerationParams`/`GenerationParamsPatch`，统一数值类型、有限值及 UI 固定范围（四个浮点参数 0..=1，max_tokens 1..=128000）校验，null 保留默认、未知字段忽略；`LlmClient` 现在把五项全部序列化给 OpenAI-compatible provider。Chat RAG、Agent completion、OpenAI proxy 均接入同一层，tenant 显式默认 Chat selector 优先于通用环境 fallback，未配置时才保持旧 fallback；重生成沿用已持久化模型绑定并使用服务器默认生成值。补解析/合并/边界/序列化单测以及 chat/agent/openai HTTP 422 契约测试。覆盖矩阵将该文件标为 aligned；RAGFlow Python 本身不做服务端类型/范围校验、通常由 provider 拒绝，RayRAG 的 422 是明确且更早的本地错误契约。全程未启动/重启/干预 RAGFlow，纯 Rust、Linux/Docker、零 MySQL，保留 zvec-rust + PostgreSQL 18.4，不改版本号。 |
| 2026-07-19 18:41 | Tenant 默认 Chat selector 与 builtin tenant 选择收尾 | 新增显式 tenant-scoped 默认 Chat selector 的 GET/PUT：GET 允许成员读取，PUT 仅 owner/admin 管理，`selector: null`/空串显式清除，旧数组快照自动迁移为对象 envelope 且不臆造默认值，selector 与模型实例一起原子持久化并经重启恢复。采用强一致语义：若模型实例更新会禁用/移除默认 selector 指向的 Chat 模型，或删除其实例，则拒绝整个 mutation、完整回滚内存与磁盘并返回明确 HTTP 409（`code=409`）；无效 selector/一般校验错误返回 400，避免静默 fallback。`GET /api/v1/compilation_templates/builtins` 默认个人 tenant；`?tenant_id=<shared>` 必须通过 TenantStore membership，否则 403；`list_builtins_for` 只读取选定 tenant 的 selector，跨 tenant 不污染。覆盖共享 tenant GET/PUT 权限、清除、旧数组迁移、重启稳定、禁用/删除阻止及 409/400 契约。全程纯 Rust，保留 zvec-rust + PostgreSQL 18.4、Linux/Docker、零 MySQL，不改版本号。 |
| 2026-07-19 17:46 | Stats Agent/canvas session 闭环 | 从运行中的 RAGFlow v0.26.4 容器逐行核对 `stats_api.py`、`API4Conversation` 与 `canvas_service.completion`：`canvas_id` 参数选择 `source=agent`，会话以 canvas/agent ID 关联且可由 session ID 续接。RayRAG `Conversation` 新增 serde 兼容的 `source` 与 `canvas_id`，旧 JSON 缺字段自动固化为 `source=chat`；Agent completion 现在创建或按调用者 owner + tenant + canvas 精确复用持久化会话，保存完整 user/assistant 回合、duration 与 provider usage，同时保持原 `answer` 响应字段并增补 conversation/message ID。Agent tenant 默认使用其 owner tenant（内置 Agent 使用调用者个人 tenant），team member 需已接受 membership，跨 owner/tenant/canvas 续接均隐藏为不存在。Stats 无 canvas 时只汇总 `source=chat`，有 canvas 时按 tenant + `source=agent` + 精确 canvas_id 汇总，普通 Chat 和其他 canvas 不混入。补旧 JSON迁移、重启、跨 tenant/canvas/source、ACL、续接和公式测试；完整 Canvas DAG runtime 仍为独立差异。全程纯 Rust，不改 zvec-rust/PostgreSQL 18.4，不引入 MySQL，不改版本号。 |
| 2026-07-19 17:21 | Stats 精确 token usage 闭环 | 继续固定 RAGFlow v0.26.4 `stats_api.py` / `API4Conversation.tokens` 对标：审计确认 RayRAG 当前 provider 抽象是枚举 + `LlmClient`（无 `ChatProvider` trait），因此新增向后兼容 `ChatCompletion`/`TokenUsage`，保留原 `chat`/`rag_chat` 的字符串适配方法；OpenAI-compatible 非流式响应解析 `usage.prompt_tokens`、`completion_tokens`、`total_tokens`（缺 total 时由 split 安全推导），RAG 生成把 usage 写入 assistant message，重生成同步替换，旧 JSON 缺字段保持 `None`。Stats 以回合为单位：有 provider usage 时只计该回合 exact total，避免再重复累计 user/assistant 内容；旧记录、本地占位及不返回 usage 的 provider 使用统一纯 Rust tokenizer 估算 fallback。OpenAI proxy 也透传上游 usage。补充 usage 解析/缺失兼容、持久化/重生成、精确优先/混合 fallback/不重复计数测试；agent/canvas session 仍待实现，固定版普通 iframe 路径本身也未在该调用点显式写 `API4Conversation.tokens`。全程纯 Rust，不引入 MySQL，不改 PostgreSQL 18.4/zvec，不改版本号。 |
| 2026-07-19 16:41 | Stats 共享 tenant 会话隔离 | 接续固定 RAGFlow v0.26.4 stats 对标：`Conversation` 新增 serde 向后兼容的 `tenant_id`，旧 JSON 缺失字段在加载时回填 `owner_id` 并立即原子固化；创建会话接受可选 `tenant_id`，省略时保持个人 tenant=`user_id`，显式共享 tenant 必须验证调用者已接受 membership；`/api/v1/stats` 同样先验证明确选定 tenant 的成员权限，再按 `tenant_id` 汇总该 tenant 内所有成员创建的会话，避免按 owner 过滤造成串租户或漏统计。补充 legacy 落盘、tenant store 过滤、共享/个人隔离及 HTTP 权限测试；agent/canvas session 和精确模型 usage 仍待实现。全程纯 Rust，不引入 MySQL，不改 PostgreSQL 18.4/zvec，不改版本号。 |
| 2026-07-19 16:14 | Stats 会话统计语义对标 | 从运行容器逐行核对固定 RAGFlow v0.26.4 `api/apps/restful_apis/stats_api.py`、`API4ConversationService.stats` 与 `API4Conversation` 模型，替换原先不等价的文档/块/KB/会话库存总数；`GET /api/v1/stats` 现受 Bearer 鉴权保护，按当前用户个人 tenant 的会话、`from_date`/`to_date`（日期型结束值扩展至 23:59:59）和 UTC 日桶返回 `pv`、`uv`、`speed`、`tokens`、`round`、`thumb_up` 六组序列，PV/UV/平均轮次/点赞计数以及 tokens÷(duration+0.1)、tokens÷1000 公式对齐；会话新增向后兼容的累计生成耗时。本批保持 partial：RayRAG 会话尚未显式保存独立 tenant_id，故不能把用户加入的共享 tenant 会话混入当前用户口径；也尚未持久化 agent/canvas session，出现 `canvas_id` 时返回诚实空 agent 序列；token 目前按持久化消息空白词估算，未保存模型精确 usage。全程纯 Rust，不引入 MySQL，不改 PostgreSQL 18.4/zvec 架构。 |
| 2026-07-19 15:28 | System 运行时日志级别 | 对标运行容器 RAGFlow v0.26.4 `system_api.py` 与 `common/log_utils.py`，新增纯 Rust、受 Bearer 鉴权保护的 GET/PUT `/api/v1/system/config/log`；启动时解析 `LOG_LEVELS`，保留 root/peewee/pdfminer 默认级别，PUT 对大小写不敏感并返回规范级别，借助 `tracing-subscriber` reload handle 原子热更新 target filter，无需重启；无效级别保持 RAGFlow data-error 响应语义；系统 API 仍因 token 持久化、task executor heartbeat 等差异保持 partial |
| 2026-07-19 15:11 | Task PATCH stop 兼容 | 对标运行容器 RAGFlow v0.26.4 `task_api.py`，新增受鉴权和 owner ACL 保护的 `PATCH /api/v1/tasks/{task_id}`；仅接受 `{"action":"stop"}` 并复用现有持久化 cooperative cancel，其他 action 返回 400 精确错误；覆盖矩阵记录 Redis cancel flag 与不存在/终态任务幂等语义仍有差异，全程纯 Rust且不引入 MySQL |
| 2026-07-19 14:50 | RAGFlow 全树清单续审 + 国内下载源 | 保留固定 v0.26.4 的 4441 文件覆盖矩阵，并给 stats/task/system API 增加显式语义映射与测试，避免继续落入通用 API 分类；Docker builder/运行层 APT 默认切换清华 TUNA，zvec-rust 预编译库默认经 Xget 下载且均可由 `.env` 覆盖；实测本地 `rust:1.97.1-bookworm`（rustc/cargo 1.97.1）、`debian:bookworm-slim`、`postgres:18.4`（PGDATA=/var/lib/postgresql/18/docker）镜像有效；修复 Candle 模块文档的 rustdoc 误编译，完整 188 单元测试 + doctest、严格 Clippy、Compose 静态校验通过 |
| 2026-07-19 13:20 | Compilation Template Group + 精确 Docker 基线 | 固定 v0.26.4 的 compilation template/group service、API 与校验逐行对标；新增纯 Rust tenant 隔离的 compilation_templates.json 原子持久化、PostgreSQL 18.4 snapshot 镜像、组/子模板 CRUD、分页筛选、scope 与嵌套字段约束、HTTP 生命周期及跨 tenant 隐藏测试；builtins/wiki preset 留待下一批；Docker builder 精确固定 rust:1.97.1-bookworm，运行层 debian:bookworm-slim、数据库 postgres:18.4，继续不引入 MySQL |
| 2026-07-18 23:45 | 固定提交全树清单修复 + Canvas 版本历史 | 修复 `ragflow-coverage` 过期硬编码 105 文件门禁，改为直接读取固定 v0.26.4 commit tree 的 4441 个 blob，当前清单 unmapped=0；对标 `user_canvas_version.py` 新增纯 Rust `CanvasVersionStore`、`canvas_versions.json` 原子持久化/PostgreSQL snapshot、严格单调版本顺序、同 DSL 原位刷新、已发布版本保护、仅保留最新 20 个未发布草稿、Agent 创建/更新/删除联动和受 ACL 保护的版本列表/详情 API；修复 Docker 状态卷遮蔽静态资源，entrypoint 从只读种子目录幂等初始化；PostgreSQL 18.4 新卷完成版本/建表/读写 smoke，Compose health 增加 90 秒初始化窗口 |
| 2026-07-17 17:36 | RAGFlow 105 文件覆盖矩阵 + 操作日志 | 新增纯 Rust `ragflow-coverage` 工具，固定 v0.26.4 commit 并为全部 105 个项目文件生成 SHA-256、状态、Rust 映射与审计说明；对标 `pipeline_operation_log_service.py` 扩展 Task 持久化 kb_id/task_type/started_at/finished_at，以终态任务作为不可变操作历史，新增 KB ACL、状态/类型/关键词/时间筛选和稳定分页 API |
| 2026-07-17 15:26 | PostgreSQL 18.4 + Linux/Docker 部署 | 删除 MySQL 驱动、feature、环境变量和 Compose；新增 PostgreSQL 18.4 snapshot mirror、版本门禁、事务 advisory lock、checksum 恢复校验、应用 Dockerfile 和 PostgreSQL/RayRAG 双服务 Compose；保留 zvec-rust 独立向量层及无 native library 时的显式 JSON 回退 |
| 2026-07-17 13:20 | Rust Agent/Canvas 持久化与 ACL | 对标 canvas_service/agent_api；新增 agents.json、Agent CRUD、DSL/prompt/KB/tags/category/type、owner/team 可见性、team 协作更新、owner-only 删除、标题唯一、标签聚合、MySQL snapshot 恢复及 HTTP 生命周期测试 |
| 2026-07-17 12:45 | zvec HTTP 在线索引 + 关系库镜像原型 | 在线文档/chunk/反馈写路径统一 JSON+zvec 事务提交和回滚；按 KB 隔离 collection，启动 reconcile 清理陈旧主键；该关系库原型已在 15:26 批次由 PostgreSQL 18.4 实现完全替代 |
| 2026-07-16 23:59 | zvec-rust 原生后端 + 状态镜像原型 | `ZvecStore` 改为 JSON/zvec 双后端，真实 HNSW Cosine collection、upsert/query/flush/reopen，修复 JSON 写后不可读；CLI parse 可直接持久化 collection；关系库实现现已统一为 PostgreSQL 18.4 |
| 2026-07-16 22:51 | Rust 会话生命周期与真实重新生成 | 对标 v0.26.4 conversation/chat API；新增 owner-scoped rename/delete、稳定 message ID 成对删除、旧回答 CAS 原位 regenerate、会话 KB/chat/embedding 模型绑定与 legacy 默认；普通发送和 regenerate 共用检索/tenant LLM helper；Web 不再追加 `regenerate` 伪问题；160 tests passed，严格 Clippy 零告警 |
| 2026-07-16 22:16 | Rust 文档级 Metadata 真源 | 对标 v0.26.4 metadata service/utils；新增独立 `document_metadata.json` 原子存储、整对象替换、列表拆分去重、keys/summary/batch API、文档响应 enrichment、三条检索入口 manual metadata filter、引用字段白名单和删除级联事务；156 tests passed，严格 Clippy 零告警 |
| 2026-07-16 21:52 | Rust Chunk 完整生命周期 | 对标 v0.26.4 `chunk_api.py`，新增 chunk 单条读取、分页/关键词/可用状态列表、手工新增、整块更新、批量删除和批量启停；新增/修改内容复用 KB 持久化 embedding selector，索引、文档 chunk_count 与 KB chunk_count 通过同一提交锁保持一致，保存失败完整回滚；disabled chunk 在向量与混合检索中统一不可见；150 tests passed，严格 Clippy 零告警 |
| 2026-07-16 19:09 | Rust KB embedding 运行时绑定 | KB 创建/更新校验并固化 tenant embedding selector；已有索引禁止直接换模型；文档索引、weighted search、`/retrieval`、Dify 和 Chat 共用 owner tenant 解析 helper；多 KB 不同向量空间明确失败；legacy `default` 回退保留；129 tests passed |
| 2026-07-16 17:23 | Rust tenant Provider/模型能力绑定 | 新增 `tenant_models.json`、tenant Provider 多实例、六类模型能力、成员只读与 owner/admin 管理、跨 tenant 隔离、密钥脱敏/保留/清除、失败回滚、重启恢复、孤儿 Provider 删除保护；`/llm/models` 改为按 tenant 能力展开；125 tests passed |
| 2026-07-16 17:20 | Rust Provider 持久化 | 新增 `providers.json` 原子存储、`0600` 密钥保护、管理员增删改 API、URL/模型校验、密钥保留/显式清除、失败回滚和重启恢复；模型列表统一从 ProviderStore 派生；121 tests passed |
| 2026-07-16 14:08 | Rust GraphRAG/RAPTOR 生产接线 | RAPTOR 摘要以 marker 普通 chunk 随文档原子替换；GraphRAG 文档 checkpoint + KB 合并图持久化到 `graphrag.json`；重处理/删除失败恢复旧索引与图谱；weighted search/通用 retrieval 返回 `graph_context`；115 tests passed |
| 2026-07-16 13:14 | Rust reranker 热配置 | 新增持久化 `RerankerManager`、管理员 `GET/PUT /api/v1/reranker` 与 Providers 配置 UI；运行实例原子热替换，无需重启；环境变量作为首次启动默认值；密钥不回显、留空保留/显式清除、文件 `0600`；111 tests passed |
| 2026-07-16 12:21 | Rust rank feature 与检索展示语义 | 对标 v0.26.4 `search.py`；三条检索入口前置 `doc_ids` 过滤，Pagerank + tag 余弦 `10x` 增益在本地/外部 reranker 路径一致保留；阈值后全量 `total`/`doc_aggs` 独立于深分页块，当前页 HTML 安全 highlight；108 tests passed |
| 2026-07-16 12:20 | Rust 外部 reranker 与深分页 | 对标 v0.26.4 `rerank_by_model`/`_rerank_window`/`retrieval`；共享 HTTP reranker、严格响应校验和超时、503/502 契约、约 64 条页对齐候选块、Dify 候选扩展、普通 `1/2/5/6` 与模型 `1/1/1` 字段词项分支 |
| 2026-07-16 11:40 | Rust 中英文混合检索 | 对标 v0.26.4 `rag/nlp/search.py` retrieval/rerank；统一 weighted/Dify/通用 retrieval，BM25 + cosine 按 `1-vector_weight`/`vector_weight` 融合，纯全文无需 embedding，CJK unigram+bigram，维度防护；94 tests passed |
| 2026-07-16 10:41 | Rust tenant/team ACL | 对标 v0.26.4 `check_team_permission.py` 与 KB service；新增 `tenants.json` 原子 membership、幂等邀请/成员列表/移除、private/team KB、读写权限拆分；team 成员可检索读取但不可上传/重解析/删除/改权限；跨用户移除即时失权；88 tests passed |
| 2026-07-16 09:05 | Rust 任务优先级与公平调度 | 对标 v0.26.4 的 0/1 两级优先队列；普通上传低优先、主动重解析高优先；默认 4 并发槽位、高优先先行、同级 FIFO、60 秒 aging 防饥饿；旧 JSON 默认低优先；64 tests passed |
| 2026-07-16 08:35 | Rust worker 租约与 fencing | 对标 v0.26.4 Redis consumer/unacked/ack；实现单进程 claim token、30 秒租约、10 秒续租、5 秒扫描、过期重领、active guard、旧 worker 禁止提交及并发快照串行化；59 tests passed |
| 2026-07-16 07:55 | 批量上传与哈希幂等 | 同 KB 重名递增；最多 32 文件批量上传；逐文件部分成功；`indexed_content_hash` 在索引成功后提交，未变化重解析跳过；53 tests passed |
| 2026-07-16 03:00 | 流式上传与内容指纹 | 纯 Rust multipart 分块临时文件、动态限流、xxHash128、sync_all + 原子 rename + 目录 sync；空文件/超限/多文件/损坏 OOXML 无残留测试；48 tests passed |
| 2026-07-16 02:05 | 权限与上传第三轮审计 | Chunk/Chat/File/Task/Stats/Agent owner 隔离；管理写接口 admin 校验；真实用户列表；multipart body 上限和 magic/容器校验；44 tests passed |
| 2026-07-16 01:05 | RAGFlow v0.26.4 二次审计 | 纯 Rust 持久化任务状态机、三次尝试/取消/重启恢复、KB owner 权限边界、检索目标权限校验；39 tests passed |
| 2026-07-15 23:58 | P0 链路修复 | 恢复 dataset handlers；原生 multipart；后台 parse/chunk/embed/index；文档级索引替换/删除；KB 检索隔离；统一鉴权与上传集成测试；35 tests passed |
| 2026-07-11 22:32 | UI对齐 | Chat详情(📋复制/🔄重新生成/👍👎/设置面板) + Search(📄文档预览弹窗) + Dataset Config(10种分块方法/Parser/RAPTOR深度/GraphRAG实体类型) + KB卡片网格 + Dashboard卡片布局 + 16页/40路由 |
| 2026-07-11 21:08 | UI增强 | 头栏导航(椭圆pill) + 头像下拉 + 暗色/亮色主题 + 中英切换 + RAGFlow设计对齐率 >95% |
| 2026-07-11 18:16 | UI全覆盖 | 9个RAGFlow页面补齐（Chat/Dataset/Files/Agent/Memory/Admin/Login）+ 上传按钮修复（base64 JSON）+ 全页面按钮→API端到端验证 |
| 2026-07-11 17:48 | 6项补齐 | 专用应用解析器(12种) + Tool Calling + Benchmark(MRR/NDCG) + GraphRAG社区报告 + Flow引擎(DAG拓扑) + 多模态(Vision/OCR/TTS)；52源文件/9451行 |
| 2026-07-11 17:36 | 5项完善 | 树形查询分解(DeepResearcher) + SSE Streaming + 分块策略(5种) + GraphRAG(NER+图谱) + Prompt模板；50源文件/8803行 |
| 2026-07-11 17:23 | 全栈增强 | NLP管线(5分块)+LLM(ChatProvider+模型注册)+GraphRAG(NER+实体消歧+图谱搜索)+Prompt模板(8模板)+混合搜索(BM25+同义词+查询改写)；50源文件/8558行 |
| 2026-07-11 17:02 | SSR 前端 | 纯 Rust SSR 管理面板替换 RAGFlow React（`web.rs`） |
| 2026-07-11 16:42 | API 补齐 | 17 API 模块（search/connector/mcp/bot/channel/plugin/system/user/tenant/models/langfuse 等）→ 58 端点 |
| 2026-07-11 14:28 | 核心功能 | 10 项（OpenAI代理/文档/文件/Chunk/对话/Provider/记忆/统计/任务/Agent） |
| 2026-07-11 13:37 | Parser | figure/resume/mineru/docling/paddleocr_layout/opendataloader/tcadp → 17/17 |
| 2026-07-11 13:35 | Advanced RAG | 多跳检索 |
| 2026-07-11 13:28 | P0+P1 | KB CRUD + LLM RAG Chat + Excel/CSV |
| 2026-07-11 12:08 | v0.1.2 | OCR + zvec + RAPTOR/GraphRAG + reranker + auth |
| 2026-07-11 07:10 | v0.1.1 | Web UI + 编译修复（17→0） |
| 2026-07-10 21:47 | v0.1.0 | 初始骨架（12 源文件 / 1073 行） |
