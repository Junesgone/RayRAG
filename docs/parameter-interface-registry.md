# RayRAG 参数与功能接口登记表（内部文档，禁止发布）

> 用途：统一参数名称与功能接口，避免参数名写错、接口形状不一致、错误只静默不提示。
> 兼容目标：与 RAGFlow v0.27.2 的参数名/接口名保持一致；RayRAG 扩展单独标注。
> 本文件是内部工程资产：`scripts/publish-github.sh` 会把它从发布快照中剔除，禁止发布到 GitHub。
>
> 维护规则：新增/修改环境变量、HTTP 接口、模型选择器格式或错误契约时，必须同步更新本文件
> （env 与路由表由 `scripts/gen-interface-registry.py --write` 刷新，契约段手动维护）。

## 1. 环境变量登记表（与 RAGFlow 命名兼容）

- 读取位置：项目根目录 `.env`（首次登录引导页写回同一文件）；`.env.example` 是默认模板。
- 统一超时：所有对外命令/子进程必须读取 `RAYRAG_CMD_TIMEOUT`（默认 7200 秒 = 2 小时，硬上限）。
- 空值约定：值为空表示“未配置”，走代码默认或明确报错；不允许静默回退到错误端点。

<!-- BEGIN GENERATED ENV -->
| 变量 | 默认模板值 | 说明 |
| --- | --- | --- |
| `RAYRAG_TIMEZONE` | `Asia/Shanghai` | 时区（IANA 名称）：容器本地时间 = 你所在时区，日期筛选/日志时间才与浏览器一致。 Timezone (IANA name): keeps container-local timestamps aligned with your browser. |
| `RAYRAG_VECTOR_BACKEND` | `zvec` |  |
| `RAYRAG_ZVEC_DIR` | `./zvec-data` |  |
| `RAYRAG_EMBEDDING_DIMENSION` | `384` |  |
| `LLM_API_BASE` | `https://api.minimaxi.com/v1` | 模型端点（OpenAI 兼容）。首次登录的引导页会写回下面这些键，保存后的值与 本文件保持动态同步；未设置的键使用代码内置默认，或按“未配置”明确报错。 这些行默认注释：取消注释即生效，直接复制本模板不会改变运行行为。 Model endpoints (OpenAI-compatible). The first-login wizard writes these keys back into this very file; unset keys fall back to the built-in default or to an explicit "not configured" error. Commented by default on purpose. （模板中默认注释，取消注释即生效） |
| `LLM_API_KEY` | `` | （模板中默认注释，取消注释即生效） |
| `LLM_MODEL` | `MiniMax-M3` | （模板中默认注释，取消注释即生效） |
| `FALLBACK_LLM_BASE` | `` | 主 LLM 失败时的回退端点（可选）；模型默认 Qwen3.5-9B。 （模板中默认注释，取消注释即生效） |
| `FALLBACK_LLM_KEY` | `` | （模板中默认注释，取消注释即生效） |
| `FALLBACK_LLM_MODEL` | `Qwen3.5-9B` | （模板中默认注释，取消注释即生效） |
| `LLM_TIMEOUT_SECONDS` | `10` | 模型目录/探测请求超时（秒），默认 10，钳制 1..120。 （模板中默认注释，取消注释即生效） |
| `EMBED_API_BASE` | `` | （模板中默认注释，取消注释即生效） |
| `EMBED_API_KEY` | `` | （模板中默认注释，取消注释即生效） |
| `EMBED_MODEL` | `` | （模板中默认注释，取消注释即生效） |
| `TEI_MODEL` | `` | 需要显式模型名的向量服务（TEI / Xinference / vLLM 多模型），配合 COMPOSE_PROFILES 使用。 （模板中默认注释，取消注释即生效） |
| `RERANK_API_BASE` | `` | （模板中默认注释，取消注释即生效） |
| `RERANK_API_KEY` | `` | （模板中默认注释，取消注释即生效） |
| `RERANK_MODEL` | `` | （模板中默认注释，取消注释即生效） |
| `RAYRAG_ASR_API_BASE` | `` | 语音转写（ASR，OpenAI 兼容 /v1/audio/transcriptions）；不设置时音频解析回退为 元数据占位并在日志中说明，而不是静默产出空文档。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_ASR_API_KEY` | `` | （模板中默认注释，取消注释即生效） |
| `RAYRAG_ASR_MODEL` | `whisper-1` | （模板中默认注释，取消注释即生效） |
| `RAYRAG_MAX_CONCURRENT_TASKS` | `` | 资源与流水线。留空 = 按本机 CPU/内存/GPU 自动推导（启动日志会打印探测结果）， 引导页会把推荐值预填出来让用户确认，因此这里默认注释以免覆盖自动调优。 文档处理并发任务数上限（自动推导值见启动日志 host_resources/document_tasks）。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_MODEL_TIMEOUT` | `300` | 单次模型调用超时（秒），默认 300，钳制 1..1800；与命令级 RAYRAG_CMD_TIMEOUT（2h 上限）区分。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_HTTP_BODY_LIMIT_BYTES` | `33554432` | 单次 HTTP 响应体上限（字节），默认 33554432（32 MiB）。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_CONNECTOR_BODY_LIMIT_BYTES` | `268435456` | 连接器（S3/WebDAV/HTTP）单次下载上限（字节），默认 268435456（256 MiB）。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_PDF_STREAM_LIMIT_BYTES` | `67108864` | 单个 PDF 流解码后的输出上限（字节），默认 67108864（64 MiB）；小于 1 MiB 的取值被忽略。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_ZVEC_MAX_BUFFER_BYTES` | `67108864` | zvec 单集合写缓冲上限（字节），默认 67108864（64 MiB）；小于 1 MiB 的取值被忽略。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_MAX_UPLOAD_BYTES` | `67108864` | 单次上传大小上限（字节），默认 67108864（64 MiB）。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_NO_FSYNC` | `` | 容器 overlay 上 fsync 极慢时可设 1 跳过（崩溃最多丢最后一次快照，JSON 可重建）。 （模板中默认注释，取消注释即生效） |
| `CHUNK_FEEDBACK_ENABLED` | `` | 分块反馈（点赞/点踩）：enabled 取 1/true 打开；weighting 取 uniform\|relevance（默认 relevance）。 （模板中默认注释，取消注释即生效） |
| `CHUNK_FEEDBACK_WEIGHTING` | `relevance` | （模板中默认注释，取消注释即生效） |
| `RAYRAG_CORS_ORIGIN` | `` | 网络与服务 浏览器跨域来源（如 https://rag.example.com）；不设置则不挂载 CORS 层。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_ADVERTISE_HOST` | `127.0.0.1` | 管理控制台“服务状态”页展示的对外主机/端口；默认 127.0.0.1:9380。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_ADVERTISE_PORT` | `9380` | （模板中默认注释，取消注释即生效） |
| `RAYRAG_POSTGRES_PORT` | `5432` | 宿主映射的 PostgreSQL 端口（仅用于服务状态页展示），默认 5432。 （模板中默认注释，取消注释即生效） |
| `ALLOW_ANY_HOST` | `` | MCP 客户端是否允许任意 Host 头（对齐上游 ALLOW_ANY_HOST），默认 false。 （模板中默认注释，取消注释即生效） |
| `LOG_LEVELS` | `` | 每目标日志级别覆盖，形如 info,ragflow=debug,h2=warn；默认全仓 INFO。 （模板中默认注释，取消注释即生效） |
| `ES_INDEX_PREFIX` | `` | 消息索引前缀（对齐上游 ES_INDEX_PREFIX），默认空。 （模板中默认注释，取消注释即生效） |
| `EXESQL_PORT` | `5432` | ExeSQL 组件默认连接端口，默认 5432。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_SANDBOX_PROVIDER` | `self_managed` | 代码执行沙箱（管理控制台的沙箱设置写入运行期配置；这里的环境变量优先级更高） 提供商：local\|self_managed\|ssh\|aliyun_codeinterpreter\|e2b\|tenki\|ucloud_agent_sandbox，默认 self_managed。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_SANDBOX_TIMEOUT` | `` | 沙箱单次执行超时（秒）；不设置时读管理控制台配置，再回退内置默认。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_EXECUTOR_MANAGER_URL` | `` | 执行器管理器地址（优先于 RAYRAG_SANDBOX_HOST 与管理控制台配置）。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_SANDBOX_HOST` | `` | （模板中默认注释，取消注释即生效） |
| `RAYRAG_SANDBOX_PYTHON` | `python3` | 本机沙箱解释器路径，默认 python3 / node。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_SANDBOX_NODE` | `node` | （模板中默认注释，取消注释即生效） |
| `SANDBOX_HOST` | `` | 上游同名变量（对齐 RAGFlow sandbox 配置）：管理器地址/端口/池大小/令牌/限流与基础镜像。 （模板中默认注释，取消注释即生效） |
| `SANDBOX_EXECUTOR_MANAGER_PORT` | `9385` | （模板中默认注释，取消注释即生效） |
| `SANDBOX_EXECUTOR_MANAGER_POOL_SIZE` | `` | （模板中默认注释，取消注释即生效） |
| `SANDBOX_EXECUTOR_MANAGER_API_TOKEN` | `` | （模板中默认注释，取消注释即生效） |
| `SANDBOX_RUN_PREAUTH_RATE_LIMIT` | `` | （模板中默认注释，取消注释即生效） |
| `SANDBOX_BASE_PYTHON_IMAGE` | `sandbox-base-python:latest` | （模板中默认注释，取消注释即生效） |
| `SANDBOX_BASE_NODEJS_IMAGE` | `sandbox-base-nodejs:latest` | （模板中默认注释，取消注释即生效） |
| `RAGFLOW_MCP_HOST` | `` | MCP 服务端（沿用上游同名变量，供外部 MCP 客户端连接本实例） （模板中默认注释，取消注释即生效） |
| `RAGFLOW_MCP_PORT` | `9382` | （模板中默认注释，取消注释即生效） |
| `RAGFLOW_MCP_BASE_URL` | `` | （模板中默认注释，取消注释即生效） |
| `RAGFLOW_MCP_LAUNCH_MODE` | `` | （模板中默认注释，取消注释即生效） |
| `RAGFLOW_MCP_HOST_API_KEY` | `` | （模板中默认注释，取消注释即生效） |
| `RAGFLOW_MCP_TRANSPORT_SSE_ENABLED` | `true` | （模板中默认注释，取消注释即生效） |
| `RAGFLOW_MCP_TRANSPORT_STREAMABLE_ENABLED` | `true` | （模板中默认注释，取消注释即生效） |
| `RAGFLOW_MCP_JSON_RESPONSE` | `true` | （模板中默认注释，取消注释即生效） |
| `TAVILY_API_KEY` | `` | Optional Tavily credential for TavilySearch/TavilyExtract Canvas nodes and Agent tools. A non-empty per-tool api_key in the DSL takes precedence. |
| `QUERIT_API_KEY` | `` |  |
| `AIMLAPI_APP_URL` | `https://app.aimlapi.com` | Optional AIMLAPI agent-authorization (OAuth 2.0 device flow). Defaults match RAGFlow. |
| `AIMLAPI_SOURCE` | `agent/ragflow` |  |
| `AIMLAPI_PARTNER_ID` | `part_yNkcOvbGLtgxWLjy4sRysaer` |  |
| `AIMLAPI_PARTNER_NAME` | `RAGFlow` |  |
| `AIMLAPI_VERIFICATION_BASE_URL` | `https://aimlapi.com` |  |
| `AIMLAPI_REQUESTED_USD_LIMIT_MINOR` | `1000` |  |
| `BOCHA_API_KEY` | `` | 国内连接器（大陆直连、免代理）。除博查外均无需 key： Bing/百度/百度学术直接抓取结果页；腾讯财经读取 GBK 行情端点； 东方财富 A股新闻（akshare stock_news_em 同源 JSONP API，免 key）。 百度系对自动化请求偶发"安全验证"风控，连接器会返回明确错误供上层重试/切换。 |
| `JIN10_SECRET_KEY` | `` | 金十财经（Jin10 flash/calendar/symbols/news）：需金十开放数据 secret-key （https://open-data-api.jin10.com 申请；RAGFlow 默认占位 "xxx" 也会被接受校验）。 |
| `QWEATHER_API_KEY` | `` | 和风天气（QWeather weather/indices/airquality）：需 Web API key （https://dev.qweather.com 注册免费订阅；免费档走 devapi.qweather.com）。 |
| `SEARXNG_URL` | `` | SearXNG 元搜索实例基址（可自建，兼容大陆/全球引擎聚合；无 key）。 也可在画布节点参数里按节点传 searxng_url。 |
| `TUSHARE_TOKEN` | `` | 其它数据源与组件的可选凭据（全部可留空；留空即该组件返回"未配置"的明确错误）。 行情：Tushare token；问财 hexin-v（网页端 Cookie 中的 v 值）。 （模板中默认注释，取消注释即生效） |
| `IWENCAI_HEXIN_V` | `` | （模板中默认注释，取消注释即生效） |
| `TENCENT_REGION` | `` | 腾讯云文档解析（TCADP）区域，默认 ap-guangzhou。 （模板中默认注释，取消注释即生效） |
| `SERPAPI_API_KEY` | `` | SerpApi key：Google 结果。大陆环境无需它——Bing/百度/百度学术/博查等直连连接器默认可用。 （模板中默认注释，取消注释即生效） |
| `BAIDU_TRANSLATE_APPID` | `` | 百度翻译开放平台（文档翻译组件）。 （模板中默认注释，取消注释即生效） |
| `BAIDU_TRANSLATE_KEY` | `` | （模板中默认注释，取消注释即生效） |
| `MINERU_POLL_TIMEOUT_SECONDS` | `30` | MinerU 轮询超时（秒），默认 30；MINERU_TIMEOUT_SECONDS 优先。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_RES_PATH` | `` | 资源目录覆盖（一般无需设置，默认按可执行文件相对路径查找）。 （模板中默认注释，取消注释即生效） |
| `RAGFLOW_RES_PATH` | `` | （模板中默认注释，取消注释即生效） |
| `RAYRAG_TEST_LLM_BASE` | `http://127.0.0.1:8088` | GPU 本地模型端点（llama.cpp llama-server，OpenAI 兼容）。 测试套件用这些变量做真实推理闭环验证（--ignored 测试）。 |
| `RAYRAG_TEST_LLM_MODEL` | `Qwen3.5-9B-Q4_K_M.gguf` |  |
| `RAYRAG_TEST_EMBEDDING_BASE` | `http://127.0.0.1:8888` |  |
| `RAYRAG_TEST_EMBEDDING_MODEL` | `Qwen3-Embedding-4B-Q4_K_M.gguf` |  |
| `RAYRAG_TEST_RERANKER_BASE` | `http://127.0.0.1:8081` |  |
| `RAYRAG_TEST_VISION_BASE` | `http://127.0.0.1:8088/v1` |  |
| `RAYRAG_TEST_VISION_MODEL` | `Qwen3.5-9B-Q4_K_M.gguf` |  |
| `RAYRAG_STORAGE_ENDPOINT` | `http://127.0.0.1:9000` | S3-compatible object storage (MinIO / AWS S3 / Aliyun OSS / Tencent COS) Empty endpoint keeps local-filesystem mode; path-style defaults to true (required for MinIO and most self-hosted endpoints). （模板中默认注释，取消注释即生效） |
| `RAYRAG_STORAGE_REGION` | `us-east-1` | （模板中默认注释，取消注释即生效） |
| `RAYRAG_STORAGE_ACCESS_KEY` | `minioadmin` | （模板中默认注释，取消注释即生效） |
| `RAYRAG_STORAGE_SECRET_KEY` | `minioadmin` | （模板中默认注释，取消注释即生效） |
| `RAYRAG_STORAGE_PATH_STYLE` | `true` | （模板中默认注释，取消注释即生效） |
| `RAYRAG_TEST_RERANKER_KEY` | `mxbai-rerank-key` |  |
| `RAYRAG_TEST_LLM_KEY` | `` | 真实模型闭环测试里可选的 LLM key；PDF 大纲解析测试的夹具路径。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_TEST_PDF_OUTLINE` | `` | （模板中默认注释，取消注释即生效） |
| `RAYRAG_POSTGRES_TEST_URL` | `` | 面向真实 PostgreSQL 的集成测试 DSN（默认关闭，需显式 --ignored 运行）。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_OCR_PROVIDER` | `` | Optional OCR. Leave the provider empty to disable external OCR. PaddleOCR follows RAGFlow v0.26.4's /api/v2/ocr/jobs submit/poll/JSONL API. |
| `RAYRAG_OCR_BASE_URL` | `http://127.0.0.1:8097` |  |
| `PADDLEOCR_BASE_URL` | `https://paddleocr.aistudio-app.com` |  |
| `PADDLEOCR_ACCESS_TOKEN` | `` |  |
| `PADDLEOCR_ALGORITHM` | `PaddleOCR-VL` |  |
| `PADDLEOCR_REQUEST_TIMEOUT_SECONDS` | `600` |  |
| `OPENDATALOADER_APISERVER` | `` | Optional OpenDataLoader remote PDF parser. Leave the API server empty to disable it. |
| `OPENDATALOADER_API_KEY` | `` |  |
| `OPENDATALOADER_TIMEOUT` | `600` |  |
| `MINERU_APISERVER` | `` | Optional MinerU remote PDF parser. Leave the API server empty to disable it. Supports the Python ZIP/content-list protocol and the Go submit/task-result protocol. |
| `MINERU_API_KEY` | `` |  |
| `MINERU_OUTPUT_DIR` | `` |  |
| `MINERU_BACKEND` | `pipeline` |  |
| `MINERU_SERVER_URL` | `` |  |
| `MINERU_DELETE_OUTPUT` | `1` |  |
| `MINERU_REQUEST_TIMEOUT_SECONDS` | `1800` |  |
| `MINERU_TIMEOUT_SECONDS` | `30` |  |
| `SOMARK_BASE_URL` | `` | Optional SoMark async PDF parser. Set a private base URL, or set only the API key to use https://somark.tech/api/v1. Both empty keeps the provider disabled. |
| `SOMARK_API_KEY` | `` |  |
| `SOMARK_IMAGE_FORMAT` | `url` |  |
| `SOMARK_FORMULA_FORMAT` | `latex` |  |
| `SOMARK_TABLE_FORMAT` | `html` |  |
| `SOMARK_CS_FORMAT` | `image` |  |
| `SOMARK_ENABLE_TEXT_CROSS_PAGE` | `0` |  |
| `SOMARK_ENABLE_TABLE_CROSS_PAGE` | `0` |  |
| `SOMARK_ENABLE_TITLE_LEVEL_RECOGNITION` | `0` |  |
| `SOMARK_ENABLE_INLINE_IMAGE` | `1` |  |
| `SOMARK_ENABLE_TABLE_IMAGE` | `1` |  |
| `SOMARK_ENABLE_IMAGE_UNDERSTANDING` | `1` |  |
| `SOMARK_KEEP_HEADER_FOOTER` | `0` |  |
| `SOMARK_SUBMIT_REQUEST_TIMEOUT_SECONDS` | `60` |  |
| `SOMARK_POLL_REQUEST_TIMEOUT_SECONDS` | `30` |  |
| `SOMARK_SUBMIT_BUDGET_SECONDS` | `600` |  |
| `SOMARK_POLL_BUDGET_SECONDS` | `600` |  |
| `MONKEYOCRV2_SERVER_URL` | `` | Optional MonkeyOCRv2 remote PDF parser. Leave empty to disable the provider. |
| `MISTRAL_OCR_API_KEY` | `` | Optional Mistral OCR remote PDF parser (POST /v1/ocr). Set an API key to enable. |
| `MISTRAL_OCR_BASE_URL` | `https://api.mistral.ai/v1` |  |
| `MISTRAL_OCR_MODEL` | `mistral-ocr-latest` |  |
| `MISTRAL_OCR_TABLE_FORMAT` | `html` |  |
| `MISTRAL_OCR_KEEP_HEADER_FOOTER` | `0` |  |
| `RAYRAG_ADMIN_EMAIL` | `admin@rayrag.local` | 全新状态卷首次启动时必需；密码至少 12 个字符。 |
| `RAYRAG_ADMIN_PASSWORD` | `replace-with-a-strong-admin-password` |  |
| `RAYRAG_PASSWORD_PRIVATE_KEY_FILE` | `` | Optional RAGFlow-compatible password transport. Generate a deployment-only RSA PKCS#8/PKCS#1 key, mount it as a secret, and set the in-container path. Never reuse RAGFlow's repository-owned conf/private.pem. When unset, auth passwords are plain JSON and the deployment must terminate HTTPS upstream. |
| `RAYRAG_POSTGRES_PASSWORD` | `change-me` | Enable with: cargo run --bin rayrag --features postgres-backend -- serve RAYRAG_POSTGRES_PASSWORD 必须与下面 DSN 里的口令一致；docker compose 用它初始化数据库。 |
| `RAYRAG_POSTGRES_URL` | `postgresql://rayrag:change-me@127.0.0.1:5432/rayrag` | PostgreSQL 元数据/快照库（不使用 MySQL）。两种运行方式： * 纯 Docker：compose 会用它自己的 DSN 覆盖本行（指向 postgres 服务），本行无需改动； * 纯 Linux：需要先有一个可连的 PostgreSQL 18.x。上面这行是标准本机地址， 请把用户/口令/库名/端口改成你实际的值；连不上时启动会明确报出被连的地址 （口令已脱敏）并给出处理建议，而不是只打印一句 db error。 留空 = 使用内置文件存储（JSON），适合先跑起来试用。 |
| `RAYRAG_POSTGRES_REQUIRED_VERSION` | `18.4` |  |
| `RAYRAG_POSTGRES_NAMESPACE` | `default` |  |
| `RAYRAG_IMAGE` | `rayrag:local` | Docker application settings |
| `RAYRAG_PORT` | `9380` |  |
| `RAYRAG_FEATURES` | `postgres-backend,zvec-backend` |  |
| `POSTGRES_IMAGE` | `docker.m.daocloud.io/library/postgres:18.4` | 国内 Docker Hub 加速默认值；镜像仍严格使用要求的版本标签。 若该镜像站不可用，可改回 library/postgres:18.4、 library/rust:1.97.1-bookworm 和 library/debian:bookworm-slim。 |
| `RUST_IMAGE` | `docker.m.daocloud.io/library/rust:1.97.1-bookworm` |  |
| `RUNTIME_IMAGE` | `docker.m.daocloud.io/library/debian:bookworm-slim` |  |
| `RAYRAG_MIRROR_PROFILE` | `auto` | Docker 构建网络档位：auto 先探测国内镜像再决定，cn 全走国内镜像源， global 直连 crates.io / deb.debian.org / github.com。两侧互为回退。 |
| `DEBIAN_MIRROR` | `` | 留空表示按 RAYRAG_MIRROR_PROFILE 选择默认镜像源；需要固定某个源时再填写。 |
| `DEBIAN_SECURITY_MIRROR` | `` |  |
| `ZVEC_RELEASE_BASE` | `https://xget.xi-xu.me/gh/zvec-ai/zvec-rust/releases/download` |  |
| `ZVEC_RELEASE_FALLBACK_BASE` | `https://github.com/zvec-ai/zvec-rust/releases/download` |  |
| `ZVEC_DOWNLOAD_TIMEOUT` | `900` | 每个 zvec 镜像的下载上限（秒）：镜像站卡死时不会无限挂起，超时后自动改用另一个镜像。 |
| `RAYRAG_DISCORD_URL` | `https://discord.com/invite/NjYzJD3GM3` | 全局头部右上角外链（默认与 RAGFlow 上游逐字一致）；大陆部署可指向可达镜像。 |
| `RAYRAG_GITHUB_URL` | `https://github.com/infiniflow/ragflow` |  |
| `RAYRAG_HELP_URL` | `https://ragflow.io/docs/dev/category/user-guides` |  |
| `RAYRAG_ENV_FILE` | `/app/.env` | This file. The container mounts it at /app/.env, so the first-login setup page (/setup) writes the same file you are editing; keep it next to docker-compose.yml. Compose already reads it — leave this line as it is unless you moved the file. |
| `RAYRAG_CMD_TIMEOUT` | `7200` | 统一命令超时（秒），默认 7200（2 小时），上限 7200；所有外呼业务请求与沙箱默认值均以此为输入 |
| `RAYRAG_MEMORY_LIMIT` | `6g` | Memory ceiling for the rayrag container (docker mem_limit). A runaway must never freeze the host: the kernel kills the container instead. Steady state is a few hundred MB; raise this for very large corpora. |
| `REGISTER_ENABLED` | `1` | 上游 settings.REGISTER_ENABLED（0 关闭注册入口）/ DISABLE_PASSWORD_LOGIN（true 走 SSO-only 渠道登录） REGISTER_ENABLED 同时决定登录页是否显示注册入口（GET /api/v1/system/config 的 registerEnabled） 与 POST /api/v1/user/register 是否放行——两者读同一个解析值，不会出现"页面给了注册表单、提交却 403 Registration is disabled"。默认 1（与上游一致：不设置时注册开放）；要关闭注册请显式设为 0。 |
| `DISABLE_PASSWORD_LOGIN` | `false` |  |
| `RAYRAG_OAUTH_CONFIG` | `` | OAuth 登录渠道（对齐 RAGFlow service_conf.yaml 的 oauth: 块，JSON 形式；留空则登录页不显示渠道区） 例：RAYRAG_OAUTH_CONFIG={"github":{"type":"github","icon":"github","display_name":"Github","client_id":"x","client_secret":"y","redirect_uri":"http://127.0.0.1:9390/api/v1/auth/oauth/github/callback"}} |
| `RAYRAG_CHANNELS_CONFIG` | `` | 通知渠道 / 会话默认（管理控制台的渠道页写入运行期配置；环境变量优先级更高） 渠道总配置（JSON，优先级高于下面各单项）；留空时按单项变量拼装。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_WEBHOOK_CALLBACK_URL` | `` | Webhook 出站回调地址与可选 bearer token。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_WEBHOOK_TOKEN` | `` | （模板中默认注释，取消注释即生效） |
| `RAYRAG_FEISHU_APP_ID` | `` | 飞书渠道（app_id 与 app_secret 同时非空才启用）；domain 默认飞书开放平台。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_FEISHU_APP_SECRET` | `` | （模板中默认注释，取消注释即生效） |
| `RAYRAG_FEISHU_DOMAIN` | `` | （模板中默认注释，取消注释即生效） |
| `RAYRAG_CHAT_LANGUAGE` | `` | 新会话默认语言与系统提示词（留空表示由页面/请求决定）。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_CHAT_SYSTEM_PROMPT` | `` | （模板中默认注释，取消注释即生效） |
| `EMAIL_SMTP_PORT` | `465` | 邮件渠道 SMTP 端口，默认 465。 （模板中默认注释，取消注释即生效） |
| `GOOGLE_OAUTH_TOKEN_URL` | `` | OAuth 换 token 端点覆盖（一般无需设置，默认走各提供商官方端点）。 （模板中默认注释，取消注释即生效） |
| `BOX_OAUTH_TOKEN_URL` | `` | （模板中默认注释，取消注释即生效） |
<!-- END GENERATED ENV -->

## 2. HTTP 接口登记表

- 统一错误契约：`{ "code": <int>, "message": <string>, "data": <object|null> }`；`code=0` 成功。
- 不允许静默错误：失败必须返回非 0 code 与可读 message（前端就地展示），禁止只写日志不出错。
- 鉴权：`Authorization: <token>`（登录返回）；`/api/v1/auth/login` 与公开分享接口除外。

<!-- BEGIN GENERATED ROUTES -->
| 方法 | 路径 | 处理器 |
| --- | --- | --- |
| GET | `/` | `-` |
| GET | `/dashboard` | `-` |
| GET | `/kbs` | `-` |
| GET | `/kbs/{id}` | `-` |
| GET | `/datasets` | `-` |
| GET | `/dataset/{tab}/{id}` | `-` |
| GET | `/search` | `-` |
| GET | `/providers` | `-` |
| GET | `/user-setting` | `-` |
| GET | `/user-setting/data-source` | `-` |
| GET | `/user-setting/data-source/{id}` | `-` |
| GET | `/user-setting/chat-channel` | `-` |
| GET | `/user-setting/model` | `-` |
| GET | `/user-setting/mcp` | `-` |
| GET | `/user-setting/team` | `-` |
| GET | `/user-setting/profile` | `-` |
| GET | `/user-setting/api` | `-` |
| GET | `/skills` | `-` |
| GET | `/home` | `-` |
| GET | `/profile-setting` | `-` |
| GET | `/files/skills` | `-` |
| GET | `/explore` | `-` |
| GET | `/agent-templates` | `-` |
| GET | `/compilation-templates/edit-next` | `-` |
| GET | `/compilation-templates/edit-next/{id}` | `-` |
| GET | `/agent-list` | `-` |
| GET | `/logout` | `-` |
| GET | `/documents/{kb_id}/{doc_id}` | `-` |
| GET | `/settings` | `-` |
| GET | `/api-docs` | `-` |
| GET | `/status` | `-` |
| GET | `/chat` | `-` |
| GET | `/login-next` | `-` |
| GET | `/agent/{agent_id}` | `-` |
| GET | `/agent/{agent_id}/explore` | `-` |
| GET | `/agents/{agent_id}/explore` | `-` |
| GET | `/search/{search_id}` | `-` |
| GET | `/search/share` | `-` |
| GET | `/agent/share` | `-` |
| GET | `/dataflow-result` | `-` |
| GET | `/document/{doc_id}` | `-` |
| GET | `/chunk` | `-` |
| GET | `/chunk/parsed/chunks` | `-` |
| GET | `/chunk/chunk/{doc_id}` | `-` |
| GET | `/chunk/result/{doc_id}` | `-` |
| GET | `/chats` | `-` |
| GET | `/chats/share` | `-` |
| GET | `/chats/widget` | `-` |
| GET | `/chats/{id}` | `-` |
| GET | `/searches` | `-` |
| GET | `/searches/{id}` | `-` |
| GET | `/chat/{id}` | `-` |
| GET | `/dataset/{id}` | `-` |
| GET | `/datasources` | `-` |
| GET | `/files` | `-` |
| GET | `/agents` | `-` |
| GET | `/agents/{agent_id}` | `-` |
| GET | `/agent-log-page/{agent_id}` | `-` |
| GET | `/memories` | `-` |
| GET | `/memory` | `-` |
| GET | `/memory/memory-message/{memory_id}` | `-` |
| GET | `/memory/memory-setting/{memory_id}` | `-` |
| GET | `/admin` | `-` |
| GET | `/admin/services` | `-` |
| GET | `/admin/users` | `-` |
| GET | `/admin/users/{user_id}` | `-` |
| GET | `/admin/sandbox-settings` | `-` |
| GET | `/login` | `-` |
| GET | `/setup` | `-` |
| GET | `/metrics` | `-` |
| GET | `/api/v1/system/status` | `-` |
| GET | `/api/v1/version` | `-` |
| POST | `/api/v1/auth/login` | `-` |
| POST | `/api/v1/user/register` | `-` |
| GET | `/api/v1/system/password-public-key` | `-` |
| GET | `/api/v1/user/info` | `-` |
| GET | `/api/v1/components` | `-` |
| GET | `/api/v1/datasets` | `-` |
| GET | `/api/v1/datasets/{id}` | `-` |
| GET | `/api/v1/datasets/{id}/tags` | `-` |
| POST | `/api/v1/datasets/{id}/documents` | `-` |
| POST | `/api/v1/datasets/{dataset_id}/metadata/update` | `-` |
| GET | `/api/v1/datasets/metadata/flattened` | `-` |
| GET | `/api/v1/datasets/tags/aggregation` | `-` |
| POST | `/api/v1/datasets/{dataset_id}/documents/parse` | `-` |
| POST | `/api/v1/datasets/{dataset_id}/documents/stop` | `-` |
| GET | `/api/v1/datasets/{id}/documents/{did}` | `-` |
| POST | `/api/v1/datasets/{id}/documents/{did}/reparse` | `-` |
| GET | `/api/v1/documents/{doc_id}/preview` | `-` |
| GET | `/api/v1/documents/{doc_id}` | `-` |
| GET | `/api/v1/documents/{doc_id}/preview/sheets` | `-` |
| GET | `/api/v1/documents/{doc_id}/preview/slides` | `-` |
| GET | `/api/v1/datasets/{id}/documents/{did}/download` | `-` |
| GET | `/api/v1/datasets/{id}/documents/{did}/metadata` | `-` |
| GET | `/api/v1/datasets/{id}/metadata/keys` | `-` |
| ANY | `/api/v1/datasets/{id}/documents/metadatas` | `axum::routing` |
| POST | `/api/v1/datasets/{id}/metadata/batch` | `-` |
| GET | `/api/v1/datasets/{id}/documents/{did}/chunks` | `-` |
| GET | `/api/v1/datasets/{id}/documents/{did}/chunks/{chunk_id}` | `-` |
| POST | `/api/v1/retrieval` | `-` |
| POST | `/api/v1/datasets/search` | `-` |
| POST | `/api/v1/datasets/{dataset_id}/search` | `-` |
| ANY | `/api/v1/conversations/{conversation_id}` | `axum::routing` |
| ANY | `/api/v1/conversations/{conversation_id}/clear` | `axum::routing` |
| ANY | `/api/v1/conversations/{conversation_id}/export` | `axum::routing` |
| ANY | `/api/v1/user/password` | `axum::routing` |
| ANY | `/api/v1/datasets/{dataset_id}/graphrag/build` | `axum::routing` |
| ANY | `/api/v1/datasets/{dataset_id}/graphrag/query` | `axum::routing` |
| GET | `/api/v1/admin/ping` | `-` |
| POST | `/api/v1/admin/login` | `-` |
| GET | `/api/v1/admin/logout` | `-` |
| GET | `/api/v1/admin/services` | `-` |
| GET | `/api/v1/admin/services/{service_id}` | `-` |
| GET | `/api/v1/admin/users` | `-` |
| GET | `/api/v1/admin/users/{username}` | `-` |
| ANY | `/api/v1/admin/users/{username}/password` | `axum::routing` |
| ANY | `/api/v1/admin/users/{username}/activate` | `axum::routing` |
| ANY | `/api/v1/admin/users/{username}/admin` | `axum::routing` |
| GET | `/api/v1/admin/users/{username}/datasets` | `-` |
| GET | `/api/v1/admin/users/{username}/agents` | `-` |
| GET | `/api/v1/admin/users/{username}/keys` | `-` |
| ANY | `/api/v1/admin/users/{username}/keys/{key}` | `axum::routing` |
| GET | `/api/v1/admin/version` | `-` |
| GET | `/api/v1/admin/variables` | `-` |
| GET | `/api/v1/admin/configs` | `-` |
| GET | `/api/v1/admin/sandbox/providers` | `-` |
| GET | `/api/v1/admin/sandbox/providers/{provider_id}/schema` | `-` |
| GET | `/api/v1/admin/sandbox/config` | `-` |
| POST | `/api/v1/admin/sandbox/test` | `-` |
| GET | `/api/v1/admin/variables/{var_name}` | `-` |
| GET | `/api/v1/admin/environments` | `-` |
| GET | `/api/v1/admin/log_levels` | `-` |
| POST | `/api/v1/search` | `-` |
| GET | `/api/v1/chunks/{doc_id}` | `-` |
| POST | `/api/v1/chunks/feedback` | `-` |
| POST | `/api/v1/completions` | `-` |
| GET | `/api/v1/chats` | `-` |
| GET | `/api/v1/chats/{id}` | `-` |
| POST | `/api/v1/chats/{id}/completions` | `-` |
| PUT | `/api/v1/chats/{chat_id}/sessions/{session_id}/messages/{message_id}/feedback` | `-` |
| PUT | `/api/v1/chats/{session_id}/messages/{message_id}/feedback` | `-` |
| POST | `/api/v1/chats/{session_id}/messages/{message_id}` | `-` |
| POST | `/api/v1/chats/{session_id}/messages/{message_id}/regenerate` | `-` |
| GET | `/api/v1/chats/{chat_id}/sessions` | `-` |
| GET | `/api/v1/chats/{chat_id}/sessions/{session_id}` | `-` |
| POST | `/api/v1/chats/{chat_id}/sessions/{session_id}/messages/{message_id}` | `-` |
| POST | `/api/v1/chats/{chat_id}/sessions/{session_id}/messages/{message_id}/regenerate` | `-` |
| GET | `/api/v1/evaluations` | `-` |
| GET | `/api/v1/evaluations/{id}` | `-` |
| GET | `/api/v1/evaluations/{id}/cases` | `-` |
| POST | `/api/v1/evaluations/{id}/runs` | `-` |
| GET | `/api/v1/evaluation-runs/{id}` | `-` |
| GET | `/api/v1/evaluation-runs/{id}/recommendations` | `-` |
| GET | `/api/v1/files` | `-` |
| ANY | `/api/v1/files/folder` | `axum::routing` |
| GET | `/api/v1/files/{file_id}/parent` | `-` |
| GET | `/api/v1/files/{file_id}/ancestors` | `-` |
| ANY | `/api/v1/files/{file_id}` | `axum::routing` |
| POST | `/api/v1/files/{file_id}/copy` | `-` |
| ANY | `/api/v1/files/{file_id}/download` | `axum::routing` |
| GET | `/api/v1/data_sources` | `-` |
| GET | `/api/v1/chatapps` | `-` |
| ANY | `/api/v1/chatapps/{id}` | `axum::routing` |
| ANY | `/api/v1/chatbots/{chat_id}/info` | `axum::routing` |
| ANY | `/api/v1/chatbots/{session_id}/completions` | `axum::routing` |
| ANY | `/api/v1/data_sources/{id}` | `axum::routing` |
| ANY | `/api/v1/data_sources/{id}/logs` | `axum::routing` |
| POST | `/api/v1/data_sources/{id}/connect` | `-` |
| POST | `/api/v1/data_sources/{id}/test` | `-` |
| POST | `/api/v1/connectors/box/oauth/web/start` | `-` |
| GET | `/api/v1/connectors/box/oauth/web/callback` | `-` |
| POST | `/api/v1/connectors/box/oauth/web/result` | `-` |
| POST | `/api/v1/connectors/google/oauth/web/start` | `-` |
| POST | `/api/v1/connectors/google/oauth/web/result` | `-` |
| GET | `/api/v1/connectors/gmail/oauth/web/callback` | `-` |
| GET | `/api/v1/connectors/google-drive/oauth/web/callback` | `-` |
| POST | `/api/v1/data_sources/{id}/disconnect` | `-` |
| GET | `/api/v1/searchapps` | `-` |
| GET | `/api/v1/searchapps/{id}` | `-` |
| GET | `/api/v1/searches` | `-` |
| GET | `/api/v1/searches/{id}` | `-` |
| POST | `/api/v1/data_sources/{id}/files` | `-` |
| POST | `/api/v1/data_sources/{id}/sync` | `-` |
| POST | `/api/v1/files/upload` | `-` |
| POST | `/api/v1/chat/completions` | `-` |
| POST | `/api/v1/openai/{chat_id}/chat/completions` | `-` |
| GET | `/api/v1/models` | `-` |
| GET | `/api/v1/models/default` | `-` |
| GET | `/api/v1/models/search` | `-` |
| GET | `/api/v1/openai/models` | `-` |
| POST | `/api/v1/embeddings` | `-` |
| POST | `/api/v1/rerank` | `-` |
| GET | `/api/v1/providers` | `-` |
| PUT | `/api/v1/providers/` | `-` |
| GET | `/api/v1/providers/presets` | `-` |
| GET | `/api/v1/model-catalog/status` | `-` |
| GET | `/api/v1/model-catalog/providers` | `-` |
| GET | `/api/v1/model-catalog/lookup` | `-` |
| GET | `/api/v1/model-catalog/search` | `-` |
| POST | `/api/v1/model-catalog/refresh` | `-` |
| GET | `/api/v1/providers/{id}` | `-` |
| GET | `/api/v1/providers/{id}/models` | `-` |
| GET | `/api/v1/providers/{provider_name}/models/{model_name}` | `-` |
| GET | `/api/v1/providers/{provider_name}/instances` | `-` |
| POST | `/api/v1/providers/{provider_name}/connection` | `-` |
| GET | `/api/v1/providers/{provider_name}/instances/{instance_name}/models` | `-` |
| GET | `/api/v1/tenant/models` | `-` |
| GET | `/api/v1/tenant/default-chat-model` | `-` |
| PUT | `/api/v1/tenant/models/{provider_id}/{instance_id}` | `-` |
| GET | `/api/v1/reranker` | `-` |
| GET | `/api/v1/agents` | `-` |
| GET | `/api/v1/agents/templates` | `-` |
| GET | `/api/v1/agents/templates/{id}` | `-` |
| GET | `/api/v1/agents/tags` | `-` |
| GET | `/api/v1/agents/{id}` | `-` |
| PUT | `/api/v1/agents/{id}/tags` | `-` |
| POST | `/api/v1/agents/{id}/reset` | `-` |
| GET | `/api/v1/agents/{id}/components/{component_id}/input-form` | `-` |
| POST | `/api/v1/agents/{id}/components/{component_id}/debug` | `-` |
| POST | `/api/v1/agents/{id}/completions` | `-` |
| POST | `/api/v1/agents/chat/completions` | `-` |
| GET | `/api/v1/agentbots/{agent_id}/inputs` | `-` |
| POST | `/api/v1/agentbots/{agent_id}/completions` | `-` |
| GET | `/api/v1/agents/{agent_id}/logs/{message_id}` | `-` |
| GET | `/api/v1/agentbots/{shared_id}/logs/{message_id}` | `-` |
| GET | `/api/v1/agents/{id}/versions` | `-` |
| GET | `/api/v1/agents/{id}/versions/{version_id}` | `-` |
| GET | `/api/v1/agents/{id}/sessions` | `-` |
| GET | `/api/v1/agents/{id}/sessions/{session_id}` | `-` |
| GET | `/api/v1/canvas/{canvas_id}/sessions` | `-` |
| GET | `/v1/canvas/{canvas_id}/sessions` | `-` |
| GET | `/api/v1/compilation_template_groups` | `-` |
| GET | `/api/v1/compilation_template_groups/{id}` | `-` |
| GET | `/api/v1/compilation_templates/builtins` | `-` |
| GET | `/api/v1/compilation_templates/wiki_presets` | `-` |
| GET | `/api/v1/compilation-template-groups` | `-` |
| GET | `/api/v1/compilation-template-groups/{id}` | `-` |
| ANY | `/api/v1/compilation-template-groups/{id}/execute` | `axum::routing` |
| GET | `/api/v1/compilation-templates/builtins` | `-` |
| GET | `/api/v1/compilation-templates/wiki-presets` | `-` |
| GET | `/api/v1/stats` | `-` |
| GET | `/api/v1/system/stats` | `-` |
| GET | `/api/v1/system/ping` | `-` |
| GET | `/api/v1/system/gaussdb/status` | `-` |
| GET | `/api/v1/system/oceanbase/status` | `-` |
| GET | `/api/v1/language` | `-` |
| ANY | `/api/v1/auth/logout` | `axum::routing` |
| GET | `/api/v1/dify/retrieval/health` | `-` |
| GET | `/api/v1/tasks` | `-` |
| GET | `/api/v1/pipeline/operation-logs` | `-` |
| GET | `/api/v1/datasets/{dataset_id}/ingestions/summary` | `-` |
| GET | `/api/v1/datasets/{dataset_id}/ingestions` | `-` |
| GET | `/api/v1/datasets/{dataset_id}/ingestions/{log_id}` | `-` |
| POST | `/api/v1/agents/rerun` | `-` |
| POST | `/api/v1/tasks/{task_id}/cancel` | `-` |
| ANY | `/api/v1/tasks/{task_id}` | `axum::routing` |
| GET | `/api/v1/memories` | `-` |
| ANY | `/api/v1/memories/clear` | `axum::routing` |
| GET | `/api/v1/memories/{memory_id}` | `-` |
| GET | `/api/v1/memories/{memory_id}/config` | `-` |
| GET | `/api/v1/messages` | `-` |
| GET | `/api/v1/messages/search` | `-` |
| PUT | `/api/v1/messages/{message_ref}` | `-` |
| GET | `/api/v1/messages/{message_ref}/content` | `-` |
| GET | `/api/v1/skills/config` | `-` |
| POST | `/api/v1/skills/search` | `-` |
| POST | `/api/v1/skills/index` | `-` |
| POST | `/api/v1/skills/reindex` | `-` |
| GET | `/api/v1/users` | `-` |
| PUT | `/api/v1/users/{id}` | `-` |
| POST | `/api/v1/users/invite` | `-` |
| GET | `/api/v1/tenant` | `-` |
| GET | `/api/v1/tenants` | `-` |
| ANY | `/api/v1/tenants/{tenant_id}` | `axum::routing` |
| GET | `/api/v1/tenants/{tenant_id}/users` | `-` |
| GET | `/api/v1/tenant/members` | `-` |
| PUT | `/api/v1/tenant/members/{user_id}` | `-` |
| ANY | `/api/v1/tenant/invitations/{tenant_id}/accept` | `axum::routing` |
| GET | `/api/v1/connectors` | `-` |
| GET | `/api/v1/mcp/tools` | `-` |
| GET | `/api/v1/mcp/servers` | `-` |
| ANY | `/api/v1/mcp/servers/import` | `axum::routing` |
| GET | `/api/v1/mcp/servers/{id}` | `-` |
| ANY | `/api/v1/mcp/servers/{id}/test` | `axum::routing` |
| GET | `/api/v1/bots` | `-` |
| GET | `/api/v1/channels` | `-` |
| GET | `/api/v1/chat-channels` | `-` |
| GET | `/api/v1/chat-channels/{id}` | `-` |
| GET | `/api/v1/chat-channels/{id}/runtime` | `-` |
| GET | `/api/v1/plugins` | `-` |
| POST | `/api/v1/plugins/{id}/toggle` | `-` |
| GET | `/api/v1/system` | `-` |
| GET | `/api/v1/system/status/detail` | `-` |
| GET | `/api/v1/system/config/log` | `-` |
| GET | `/api/v1/system/variables` | `-` |
| GET | `/api/v1/system/variables/{var_name}` | `-` |
| GET | `/api/v1/system/healthz` | `-` |
| GET | `/api/v1/auth/login/channels` | `-` |
| GET | `/api/v1/auth/login/{channel}` | `-` |
| GET | `/api/v1/auth/oauth/{channel}/callback` | `-` |
| GET | `/api/v1/system/version` | `-` |
| GET | `/api/v1/system/config` | `-` |
| GET | `/api/v1/setup/status` | `-` |
| GET | `/api/v1/setup/options` | `-` |
| POST | `/api/v1/setup/complete` | `-` |
| POST | `/api/v1/system/new_api_key` | `-` |
| GET | `/api/v1/system/api_keys` | `-` |
| ANY | `/api/v1/system/api_keys/{key}` | `axum::routing` |
| GET | `/api/v1/system/tokens` | `-` |
| ANY | `/api/v1/system/tokens/{token}` | `axum::routing` |
| GET | `/api/v1/llm/models` | `-` |
| GET | `/api/v1/llm/factories` | `-` |
| GET | `/api/v1/llm/my_llms` | `-` |
| POST | `/api/v1/llm/aimlapi/authorize/start` | `-` |
| POST | `/api/v1/llm/aimlapi/authorize/poll` | `-` |
| POST | `/api/v1/document/upload_info` | `-` |
| POST | `/api/v1/sessions/related_questions` | `-` |
| POST | `/api/v1/searches/{search_id}/completions` | `-` |
| POST | `/api/v1/searches/{search_id}/completion` | `-` |
| POST | `/api/v1/chat/mindmap` | `-` |
| POST | `/api/v1/chat/recommendation` | `-` |
| POST | `/api/v1/searchbots/related_questions` | `-` |
| GET | `/api/v1/file/commits` | `-` |
| POST | `/api/v1/file2document` | `-` |
| GET | `/api/v1/langfuse/api-key` | `-` |
| GET | `/api/v1/langfuse` | `-` |
| GET | `/mcp/sse` | `-` |
| POST | `/mcp/messages` | `-` |
| POST | `/v1/chat/completions` | `-` |
| GET | `/v1/models` | `-` |
| GET | `/v1/models` | `-` |
| ANY | `/v1/chat/completions` | `axum::routing` |
<!-- END GENERATED ROUTES -->

## 3. 命名与数据契约

| 契约 | 规则 | 示例 |
| --- | --- | --- |
| 模型选择器 | `<model>@<provider>@<instance>`（缺省实例用 `default`） | `qwen3.8-max@Tongyi-Qianwen@default` |
| 数据集解析配置 | `parser_config` 内使用 RAGFlow camelCase 键（`chunk_token_num`/auto_keywords/raptorEn/graphEn…） | `{"chunk_token_num":2048,"raptorEn":true}` |
| 文档/切片状态 | `UNSTARTED / RUNNING / DONE / FAILED`；文档启用位 `status` 为 `"0"/"1"` | `{"run":"DONE","status":"1"}` |
| 运行形态 | 纯 Linux 与纯 Docker 等价；元数据 PostgreSQL，向量 zvec（不允许 MySQL） | `RAYRAG_VECTOR_BACKEND=zvec` |
| 超时 | 命令/HTTP 客户端统一读取 `RAYRAG_CMD_TIMEOUT` | `timeout: 7200` |
| Compiler 参数（流程组件） | `compilation_template_group_id`（单数优先）与 `compilation_template_group_ids`（传统复数回退）；`check()` 归一化为列表，空组报 `Compilation Template Group does not support empty value.` | `{"compilation_template_group_id":"g1"}` |
| Compiler 上游载荷 | `CompilerFromUpstream`：serde 别名 `_created_time`/`_elapsed_time`/`json`/`markdown`/`text`/`html`，`output_format ∈ {json,markdown,text,html,chunks}`，未知字段拒绝（↔ 上游 `extra="forbid"`） | `{"name":"doc.md","output_format":"markdown","markdown":"# a"}` |
| Compiler 切分常量 | `PARSER_TEXT_CHUNK_TOKEN_SIZE=1024` / `PARSER_CANDIDATE_TOKEN_SIZE=128` / `PARSER_MIN_COARSE_CHUNK_TOKEN_SIZE=256` / `PARSER_MIN_SPLIT_CHARACTERS=24`（与上游 1:1） | `slumber_candidates(text, level, Some(1024))` |
| agentic harness | 思考模式 `low`/`medium`/`high`/`ultra`（+`naive` 回退）；工具面 `retrieve`/`search_chunks`/`list_chunks`/`navigate_tree`/`navigate_structure`/`calculate`/`web_search`（`ultra` 另含 `graph_explore`）；关键词四面向 `entity`/`aliases`/`fact_type`/`qualifiers`（entity 与 qualifier ×3 加权、400 字符上限） | `get_mode("ultra")`：sca_max_rounds=5、turns=6 |
| 编译产物扩展 | 1 跳实体图：关系 ≤50/向、邻居 `name_kwd` ≤100、策略 ≤5 块、双拼写端点；合成页（wiki_page/artifact_page/essence）源块相似度缺省 0.9；HNSW `num_candidates` 256/相似度 0.1 | 见 src/harness/tools/compiled_expansion.rs |
| 导航工具 | nav-tree：叶 BFS 深度 6/叶 300、层搜索门槛 0.2/上限 12、内容召回兜底 ≤4/总 8；结构导航：向量束深度 ≤3/K=2/相对比 0.5（无向量时关键词 ≥1）、chunk 指针 ≤8、TOC 选择上限 120（默认关闭）、单文档 chunk 命中 ≤8/≤4、相关块 ≤4/文档、片段 180/300 字符 | 见 src/harness/tools/navigation.rs |
| 探索工具（KG/wiki） | `graph_explore`：种子 2（池 64、相似度 ≥0.8、mention_count 排序）、跳数 2、邻居 ≤128、每端点关系 ≤32；scope_kwd `dataset`|`doc`；不足时回取 `source_chunk_ids` 并窄化。`wiki_query`：`wiki_page_draft`，正文取 `content_md_rendered`→`content_md`→`content_md_raw`，doc_id 优先 slug | 见 src/harness/tools/exploration.rs |
| 检索腿与 SCA | hybrid/vector/BM25 三腿权重（0.3/1.0/0）、`effective_query` 400 字符、查询缓存键；`grep_search` 表块整文 + 正文 1 行窗；SCA：`sca_select`、claims 证据锚定 300 字符（表格整文）、`sub_queries`（unsatisfied 带 missing_fact + search_hint）、insufficient + 空 claims → `_global` 兜底 | 见 src/harness/{tools/search,orchestrator/sufficient_context}.rs |
| 确定性工具（harness） | 文本窄化与算术：句切分保留表格/块级 HTML、词干匹配、grep+sed 窄化（16 词上限、1200/16000 字符预算）、记忆检索（重叠比 ≥0.12）；算术白名单（无属性/下标/关键字参数；`len` 字符串字面量禁用；乘/幂仅数字；指数 ≤64；`letters` 计字母、`digit_sum` 逐位求和、`date_diff` 需恰好两个 ISO 日期；表达式上限 400 字符） | 见 src/harness/{tools/text_processing,grep_sed_narrow,memory,arithmetic}.rs |
| orchestrator 策略 | `direct`（单次混合检索 + entity 加权查询注入；`DirectTools.hybrid_search` 由运行时实现）；`rewrite_gap_to_query`（`sca_query_rewrite` 模板原文；gaps/bridge/research_context 渲染；queries 去重解析）；阶段名 `direct`/`rewrite` 进入 stats | `rewrite_gap_to_query(...) -> [{"query": …}]` |
| 结构图合并任务 | 任务类型 `structure_graph`/`structure_mindmap`/`timeline`/`session_graph`/`session_essence`/`structure`；行约定 `scope_kwd=doc|dataset`、`knowledge_graph_kwd ∈ {entity,relation}`、删除标记 `doc_deleted`（`deleted_doc_id`）、构建元数据 `kg_build_meta`（`build_timestamp_flt`）；常量 256 桶 / 500 冲洗 / 1000 页 / 32 嵌入批 | `is_structure_merge_task("structure_graph")` |

## 4. 变更流程

1. 新增参数/接口 → 先在 RAGFlow v0.27.2 源码中找同名或最接近的参数/接口，保持命名与形状一致。
2. 同步更新本文件（env/路由表用生成脚本刷新，契约段手改），并在 `CHANGELOG.md` 与 `运维记录.md` 记录。
3. 加回归测试：参数默认值、接口成功/失败形状、错误 message 可读（不允许静默）。
4. 发布前运行 `python3 scripts/gen-interface-registry.py --check`，确保登记表与代码一致。
