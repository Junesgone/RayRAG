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
| `RAYRAG_TEST_RERANKER_KEY` | `` | 测试用的 key 保持注释态：照抄模板时不应带有任何可用凭据。 （模板中默认注释，取消注释即生效） |
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
| `RAYRAG_ADMIN_PASSWORD` | `` | Admin password for the account named above. Leave it unset and the first-run setup window asks for one; set a real value to bootstrap the admin without the setup step. (Commented out on purpose: an active placeholder would silently make every deployment that forgets to edit it share the same, guessable admin password.) （模板中默认注释，取消注释即生效） |
| `RAYRAG_PASSWORD_PRIVATE_KEY_FILE` | `` | Optional RAGFlow-compatible password transport. Generate a deployment-only RSA PKCS#8/PKCS#1 key, mount it as a secret, and set the in-container path. Never reuse RAGFlow's repository-owned conf/private.pem. When unset, auth passwords are plain JSON and the deployment must terminate HTTPS upstream. |
| `RAYRAG_POSTGRES_PASSWORD` | `` | Enable with: cargo run --bin rayrag --features postgres-backend -- serve RAYRAG_POSTGRES_PASSWORD 必须与下面 DSN 里的口令一致；docker compose 用它初始化数据库。 必须改成真实口令（compose 未设置时会直接报错退出，不会静默用弱口令建库）。 注释态是有意的：留着 change-me 会让所有照抄模板的部署共用同一个数据库口令。 （模板中默认注释，取消注释即生效） |
| `RAYRAG_POSTGRES_URL` | `postgresql://rayrag:change-me@127.0.0.1:5432/rayrag` | PostgreSQL 元数据/快照库（不使用 MySQL）。两种运行方式： * 纯 Docker：compose 会用它自己的 DSN 覆盖本行（指向 postgres 服务），本行无需改动； * 纯 Linux：需要先有一个可连的 PostgreSQL 18.x。上面这行是标准本机地址， 请把用户/口令/库名/端口改成你实际的值；连不上时启动会明确报出被连的地址 （口令已脱敏）并给出处理建议，而不是只打印一句 db error。 留空 = 使用内置文件存储（JSON），适合先跑起来试用。 |
| `RAYRAG_POSTGRES_REQUIRED_VERSION` | `18.4` |  |
| `RAYRAG_POSTGRES_NAMESPACE` | `default` |  |
| `RAYRAG_IMAGE` | `rayrag:local` | Docker application settings |
| `RAYRAG_PORT` | `9380` | RayRAG's web port. 9380 is also RAGFlow's default: if RAGFlow already runs on this machine, change this one line (e.g. 9390) before the first `docker compose up -d`. |
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
| `RAYRAG_ALLOW_PRIVATE_DB_HOSTS` | `0` | Agent database component: allow `test_db_connection` to reach private/loopback addresses. Off by default (SSRF guard); turn it on when the database really is on your LAN, which is the normal case on a NAS. （模板中默认注释，取消注释即生效） |
| `RAYRAG_WEBHOOKS_FILE` | `` | --- Agent webhooks --------------------------------------------------------- One JSON document holding each agent's webhook and its delivery history. Default: webhooks.json beside the static directory's parent. （模板中默认注释，取消注释即生效） |
| `RAYRAG_STATIC_DIR` | `` | Directory holding the web UI and the JSON state files (users.json, uploads/). The CLI flag --static-dir wins over this value; default: web/static. （模板中默认注释，取消注释即生效） |
| `RAYRAG_PASSWORD_RESET_FILE` | `` | --- Password reset --------------------------------------------------------- One-time reset codes are stored here. Default: password_reset.json beside the static directory's parent. （模板中默认注释，取消注释即生效） |
| `EMAIL_SMTP_HOST` | `` | SMTP host used for outgoing mail (password-reset codes). When it is empty RayRAG does not send mail: the reset code is written to the server log instead, and the API reports delivered_by=server_log rather than claiming an email was sent. （模板中默认注释，取消注释即生效） |
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
| GET | `/api/v1/datasets/{dataset_id}/artifacts` | `-` |
| POST | `/api/v1/datasets/{dataset_id}/artifacts/compile` | `-` |
| GET | `/api/v1/datasets/{dataset_id}/artifacts/topics` | `-` |
| GET | `/api/v1/datasets/{dataset_id}/artifacts/structure` | `-` |
| GET | `/api/v1/datasets/{dataset_id}/artifacts/graph` | `-` |
| GET | `/api/v1/datasets/{dataset_id}/artifacts/alteration` | `-` |
| GET | `/api/v1/datasets/{dataset_id}/artifacts/{page_type}` | `-` |
| GET | `/api/v1/datasets/{dataset_id}/artifacts/{page_type}/{slug}` | `-` |
| GET | `/api/v1/datasets/{dataset_id}/navigation` | `-` |
| GET | `/api/v1/models/endpoints/check` | `-` |
| POST | `/api/v1/datasets/{dataset_id}/index` | `-` |
| ANY | `/api/v1/datasets/{dataset_id}/{index_type}` | `axum::routing` |
| POST | `/api/v1/auth/password/forgot/otp` | `-` |
| POST | `/api/v1/auth/password/forgot/otp/verify` | `-` |
| POST | `/api/v1/auth/password/reset` | `-` |
| GET | `/api/v1/agents/{agent_id}/webhook` | `-` |
| GET | `/api/v1/agents/{agent_id}/webhook/logs` | `-` |
| POST | `/api/v1/agents/{agent_id}/webhook/test` | `-` |
| GET | `/api/v1/users/me/models` | `-` |
| GET | `/api/v1/agents/prompts` | `-` |
| GET | `/api/v1/agents/download` | `-` |
| POST | `/api/v1/agents/test_db_connection` | `-` |
| GET | `/api/v1/searchbots/detail` | `-` |
| POST | `/api/v1/searchbots/retrieval_test` | `-` |
| POST | `/api/v1/searchbots/mindmap` | `-` |
| GET | `/api/v1/datasets/{dataset_id}/navigation/{name}/children` | `-` |
| ANY | `/api/v1/datasets/{dataset_id}/navigation/{name}` | `axum::routing` |
| GET | `/api/v1/datasets/metadata/flattened` | `-` |
| GET | `/api/v1/datasets/tags/aggregation` | `-` |
| POST | `/api/v1/datasets/{dataset_id}/documents/parse` | `-` |
| POST | `/api/v1/datasets/{dataset_id}/documents/stop` | `-` |
| POST | `/api/v1/datasets/{dataset_id}/chunks` | `-` |
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
| ANY | `/api/v1/datasets/{dataset_id}/graph` | `axum::routing` |
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


## 业务错误码契约（对齐上游 API 说明书）

上游说明书明确：**HTTP 状态码与响应体里的业务码是两件事，必须分别检查**。RayRAG 现在按同一张表返回（`crate::server::code`）：

| 业务码 | 含义 | 对应 HTTP |
|---|---|---|
| 0 | 成功 | 200 |
| 101 | 参数错误 | 400 |
| 102 | 数据缺失/无效（含"不属于你"） | 400 |
| 103 | 操作失败 | 400 |
| 105 | 连接失败 | 502 |
| 106 | 仍在进行中 | 400 |
| 108 | 无权限 | 403 |
| 109 | 未认证 | 401 |
| 409 | 冲突（重名等） | 409 |

构造器：`api_error_code(status, code, message)`，以及 `invalid_argument` / `invalid_or_missing` / `no_permission` / `unauthenticated` / `conflict`。
**迁移状态**：新写的端点（产物、导航、webhook、密码流程、索引维护）已按此表返回；历史端点仍在用 `api_error`（业务码=HTTP 码），逐个迁移中——**未迁移的不会被伪装成已迁移**。

## 聊天助手 vs 会话（0.11.5 起语义分离）

说明书把 `/api/v1/chats` 定义为**聊天助手（chat assistant）**集合，会话在 `/api/v1/chats/{chat_id}/sessions`。
RayRAG 此前让**会话记录**占用同一条路径（GET/POST/DELETE 都是会话），导致：
说明书客户端 `POST /api/v1/chats` 建的助手根本不出现在列表里；列表返回的 id 在 `/sessions` 上 404。

现在：

| 路径 | 语义（说明书一致） | RayRAG 存储 |
|---|---|---|
| `GET /api/v1/chats` | 助手列表，`data:{chats,total}`，含 `dataset_ids`/`kb_names`/`llm_id`/`llm_setting`/`prompt_config`/`rerank_id`/`top_k` | `chat_apps` |
| `POST /api/v1/chats` | 建助手，body `name`/`description`/`icon`/`dataset_ids`/`llm_id`/`llm_setting`/`prompt_config`/`rerank_id`/`top_k`；空名 → `101`；重名 → `102 duplicated chat name` | `chat_apps` |
| `DELETE /api/v1/chats` | 删助手，body `ids` / `delete_all` / 兼容 `chat_id`；**部分失败返回 102 并列出 errors，不静默成功** | `chat_apps` |
| `POST /api/v1/conversations` | 建**会话**（原 `POST /api/v1/chats` 的行为） | `conversations` |
| `DELETE /api/v1/conversations` | 批量删会话（原 `DELETE /api/v1/chats` 的行为） | `conversations` |
| `GET /api/v1/chats/{id}/sessions` | 该助手的会话列表（`data` 为数组） | 助手 store + 会话回退 |

**仍未对齐（登记）**：`/api/v1/chats/{id}/completions` 的路径参数在 RayRAG 里是**会话 id**（说明书是助手 id）；
`/api/v1/chatapps*` 是 RayRAG 自己的助手管理路径，与说明书路径并存。

### 0.11.6 补充：`/api/v1/chats/{chat_id}` 单条动词

| 端点 | 语义 |
|---|---|
| `GET /api/v1/chats/{chat_id}` | 取单个**助手**（含 `dataset_ids`/`kb_names`）；缺失或无权 → 业务码 **102 `no authorization`** |
| `PATCH /api/v1/chats/{chat_id}` | **部分更新**：`llm_setting`/`prompt_config` 深合并，未提及字段保留 |
| `PUT /api/v1/chats/{chat_id}` | **整体覆盖**：body 未提及的字段**重置为默认值**（说明书的 PUT/PATCH 差异） |
| `DELETE /api/v1/chats/{chat_id}` | `{"code":0,"data":true}` |
| `GET/PATCH/DELETE /api/v1/conversations/{id}` | 会话的取一条 / 改名 / 删除（原 `/api/v1/chats/{id}` 的行为） |

**仍未对齐**：`/api/v1/chats/{id}/completions` 的路径参数在 RayRAG 是会话 id、说明书是助手 id；
`/api/v1/chats/{id}/messages/...` 与 `/api/v1/chats/{id}/sessions/...` 同样存在两种 id 口径，需一并定夺。

### 0.11.7 补充：完成路径与会话动词

| 端点 | 语义 | 说明 |
|---|---|---|
| `POST /api/v1/chats/{chat_id}/completions` | 路径参数＝**助手** | 助手不存在 → **102**；`session_id` 不存在 → **102 `Session not found!`**；**无 session_id → 自动建会话**（用助手自己的 `dataset_ids`/`llm_id`，并把会话绑定到该助手） |
| `POST /api/v1/conversations/{id}/completions` | 会话内回答 | 原 `/api/v1/chats/{session_id}/completions` 的行为 |
| `DELETE/POST /api/v1/conversations/{id}/messages/{msg_id}`（`/regenerate`、`/feedback`） | 会话消息动词 | 原挂在 `/api/v1/chats/{session_id}/messages/...` |
| 请求体别名 | `session_id`←`conversation_id`、`chat_id`←`app_id` | 说明书客户端可直接使用说明书的字段名 |

**仍未对齐（登记，需单独一轮）**：`POST /api/v1/chat/completions` 在 RayRAG 上绑定的是 **OpenAI 形状**的处理器，
而说明书要求 `{code,message,data:{answer,reference}}` 形状与三种模式；这是**形状差异**而非改名问题。

### 0.11.8：`POST /api/v1/chat/completions` 恢复为说明书的形状

| 项 | 说明书 | RayRAG |
|---|---|---|
| 路径 | `POST /api/v1/chat/completions` | 现已实现（**OpenAI 形状的处理器仍在 `/v1/chat/completions`**，供 OpenAI 客户端使用） |
| 三种模式 | 无 chat_id/session_id → 租户默认模型；仅 chat_id → 用助手配置并**自动建会话**；有 session_id → 继续该会话 | 全部实现 |
| body | `messages` 或 `question`、`stream`（**默认 true**）、`llm_id`、`pass_all_history_messages`、`legacy` | 全部接受，另兼容 RayRAG 的 `conversation_id`/`chat_id`/`chat_model`/`kb_ids` |
| 流帧 | `{"code":0,"message":"","data":{"answer","reference","audio_binary","prompt","created_at","final":false,"id","session_id","chat_id"}}` 结尾 `data:true` | 一致；`legacy:true` → 累积 answer 且保留 `<think>` 字面量，否则剥离标签并给出 `start_to_think`/`end_to_think` |
| 失败 | `{"code":102,"message":"Please input your question."}` | 一致；生成失败时发**错误帧**，不让流以 `data:true` 掩盖空答案 |
| reference | `chunks`（含 `document_id`/`document_name`）+ `doc_aggs` | `ChunkReference` **新增 `document_id`/`document_name`**（检索路径从 chunk 的 `doc_id` 与文档库填充），`doc_aggs` 由真实文档名聚合 |

**仍未对齐（登记）**：参考 chunk 仍不含 `image_id`/`positions`/`url`/`doc_type`（RayRAG 目前不产出这些字段）。

### 0.12.0：工作区快照（commit）家族 —— 9 个接口 + 界面入口

上游 `api/apps/restful_apis/file_commit_api.py` 用 **f-string 前缀**注册 8 条路由，静态扫描看不到，
因此这一族此前既未实现也不在台账里。现已：

| 接口 | 说明 |
|---|---|
| `POST /api/v1/workspaces/{workspace_id}/commits` | 建快照；body `{message, files:[{file_id,file_name,operation,content,old_name,new_name}]}`，operation ∈ add/modify/delete/rename |
| `GET  /api/v1/workspaces/{workspace_id}/commits` | 分页 `{total,page,page_size,commits}`，`order_by=create_time\|file_count`、`desc` |
| `GET  …/commits/diff?from&to` | 两个快照之间的差异（add/modify/delete/rename + 新旧 hash/location） |
| `GET  …/changes` | 未提交的变更（与最后一次快照比较） |
| `GET  …/commits/{commit_id}` | 快照 + `files[]` |
| `GET  …/commits/{commit_id}/files` | 变更条目 |
| `GET  …/commits/{commit_id}/tree` | 该快照时的目录树（由扁平 `tree_state` 的 `parent_id` 还原） |
| `GET  …/commits/{commit_id}/files/{file_id}/content` | 该文件在该快照的内容（请求带 content 时用快照副本，否则读现文件） |
| `GET  /api/v1/workspace-files/{file_id}/versions` | 单文件版本历史（新→旧：`{commit_id,operation,hash,create_time,message}`） |

**作用域**：`/workspaces/{id}` 的 `id` 是文件库中的文件夹（`root` 或 `file_type=folder` 的记录），其状态是该文件夹（含子文件夹）下的文件；
`/datasets/{id}` 的 `id` 是知识库，其状态是该库的文档 —— 与上游把 artifact commit 写成 `folder_id = kb_id` 一致。两者互不混用（快照记录 `scope` 字段）。

**错误语义（与上游一致）**：缺 `message`/`files` → `101 required argument are missing: …`；未知作用域 → `102 Could not resolve folder '<id>'` / `102 Could not resolve datasets '<id>' to a folder`；未知快照 → `102 Commit not found in workspace`；快照内无此文件 → `102 File not found in this commit`。

**存储**：`workspace_commits.json`（与其它 store 一样启动时恢复、原子写入）。`hash` 是内容指纹（FNV-1a 16 位十六进制），**不是** sha256 —— 文档中如实标注，用于比较 add/modify/delete。

**界面**：知识库页新增“版本历史”卡片 —— “显示快照”列出来，“创建快照”先读 `GET /changes` 再提交**真实变更**（无变更时提示，不伪造）。

### 0.13.0：运行期附件（runtime attachments）—— 上传 + 下载/预览

| 接口 | 说明 |
|---|---|
| `POST /api/v1/documents/upload` | 二选一：multipart `file`（可多个）或查询参数 `?url=`。单文件返回对象、多文件返回数组：`{id,name,size,extension,mime_type,created_by,created_at,preview_url}` |
| `POST /api/v1/agents/{agent_id}/upload` | 同上，但记录到画布（`agent_id`）；画布不存在 → `102 canvas not found.` |
| `GET /api/v1/agents/attachments/{id}/download` | 返回字节流；`ext` / `mime_type` / `filename` 决定响应头，`disposition=inline` 走内联 |
| `GET /api/v1/agents/attachments/{id}/preview` | 同上但始终内联（`Content-Disposition: inline`） |

**错误语义（与上游一致）**：两者都带 → `101 Provide either multipart file(s) or ?url=..., not both.`；都没带 → `101 Missing input: provide multipart file(s) or url`；附件不存在 → `102 document not found`（HTTP 404）。
注意：这两条是**业务码**，不是 415/400 —— 因此 `/documents/upload` 使用原始请求自行解析 multipart，否则 axum 会在进入处理器前用 415 拒绝空 body。

**URL 抓取的安全约束（RayRAG 自加，已验证）**：仅 `http`/`https`；拒绝回环/私网/链路本地/未指定地址、`localhost`、`*.local`、`*.internal`；最多 10 次重定向且每跳都校验；响应体不超过上传上限。**先校验后落盘**：请求被拒时不产生附件。

**存储**：`attachments.json`（启动恢复 + 原子写入 + PostgreSQL 快照镜像），blob 存于 `uploads/attachments/<id>[.ext]`；`ext`/`mime_type` 只影响响应头，不改动字节。

**界面**：聊天输入区新增 📎 按钮 → 上传后显示文件名与大小，并给出 `download` 链接（走 `/api/v1/agents/attachments/{id}/download`）。

### 0.14.0：文档缩略图与图片端点

| 接口 | 说明 |
|---|---|
| `GET /api/v1/thumbnails?doc_ids=a&doc_ids=b` | 返回 `{doc_id: 缩略图URL \| null}`；`doc_ids` **可重复**（也接受逗号分隔）。缺参 → **HTTP 200 + 业务码 101 `Lack of "Document ID"`**（与上游一致，业务码与传输码分别判断） |
| `GET /api/v1/documents/images/{image_id}` | `{kb_id}-{name}.jpg` 返回生成的缩略图（image/jpeg，inline）；`{kb_id}-{name}.{ext}` 返回文档原图（按扩展名给 MIME）；找不到 → `404 / 102 document not found` |

**缩略图生成**：仅对**可解码的图片**文档（png/jpg/jpeg/webp/bmp/gif/tif/tiff）生成 —— 等比缩放到最长边 200px 后编为 JPEG（质量 80），缓存在 `uploads/thumbnails/{doc_id}.jpg`；PDF/Word 等需要渲染器的格式**没有**缩略图，映射值为 `null`（不伪造占位图）。小图仍会转码为 JPEG，保证格式一致、可按 id 缓存。

**图像 id 解析**：上游 id 为 32 位十六进制（无短横线），可按第一个短横线切分；RayRAG 的知识库 id 是**带短横线的 UUID**，故改为**最长前缀匹配**现有知识库，余下部分为文件名（文件名本身可含短横线）。文档 id 取“去**任意**扩展名”后的名字。

### 0.15.0：自动元数据模式（dataset / document 配置）

上游 `validation_utils.AutoMetadataConfig` 的形状：

```json
{ "metadata": [ {"key": "...", "type": "string|list|time|number", "description": "...", "enum": ["..."]} ],
  "built_in_metadata": [ ... 同形状 ... ] }
```

| 接口 | 说明 |
|---|---|
| `GET /api/v1/datasets/{id}/metadata/config` | 返回该数据集的模式；`built_in_metadata` 由**部署**提供（本实现为解析管线真正会写入的键：`author`/`tags`/`url`/`language`/`published_at`） |
| `PUT /api/v1/datasets/{id}/metadata/config` | 替换用户列表并回显；请求里自带 `built_in_metadata` 会被**覆盖**（它描述部署而非请求） |
| `PUT /api/v1/datasets/{id}/documents/{doc_id}/metadata/config` | 单文档覆盖 |

**校验（真实生效）**：`key` 去空白后非空且 ≤255 字符；`type` ∈ string/list/time/number；重复 `key` 拒绝（会让后续过滤歧义）；`enum` 中的空值拒绝。被拒 → **101** 并指出字段，且**已存模式不变**。
**错误语义**：未知数据集 → `404 / 102 The dataset doesn't exist`；数据集没有该文档 → `404 / 102 The dataset does not have the document.`
**清理**：删除数据集时一并清除其模式与各文档覆盖（否则同 id 新建数据集会"复活"旧模式）。
**存储**：`uploads/metadata_config.json`（启动恢复 + 原子写入 + PostgreSQL 快照镜像）。
**界面**：数据集配置页新增“元数据模式”卡片（增删行 + 保存 + 显示内置字段），整块自带 `<script>`（避免 0.13.0 聊天输入区那类字符串字面量陷阱）。

### 0.16.0：语音接口（TTS / ASR）

| 接口 | 行为 |
|---|---|
| `POST /api/v1/chat/audio/speech` | 入 `{"text": …}`；出 `audio/mpeg`，带 `Cache-Control: no-cache`、`Connection: keep-alive`、`X-Accel-Buffering: no` |
| `POST /api/v1/chat/audio/transcription` | 入 multipart `file` + `stream`；出 `{"text": …, "tokens": …}` 或 SSE |

**错误语义（与上游逐字一致，客户端会按字面匹配）**
- 未配置 TTS：`102 No default TTS model is set`
- 未配置 ASR：`102 No default ASR model is set`
- 缺 file 部件：`102 Missing 'file' in multipart form-data`
- 后缀不在白名单：`102 Unsupported audio format: .mp4. Allowed: .aac, .flac, .m4a, .mp3, .ogg, .opus, .wav, .webm, .wma`
- 缺 `text`：`400 / 101 required argument are missing: text`（上游直接下标取值会抛异常；RayRAG 明确指名字段）

以上 `102` 均为 **HTTP 200 + 业务码**（对应上游 `get_data_error_result`）。供应商调用失败 → **502 / 105** 并带供应商原文。

**两处有意差异（都不隐藏失败）**
1. speech 先合成再应答：供应商出错 → 明确的 105，而不是「响应头已 200 但音频被截断」。
2. transcription `stream=true` 只发**一个** `partial` 事件（载全部文本）+ `[DONE]`：底层转写调用本身不是流式的；伪造分段边界只会更像而不是更准。

**环境变量**：`RAYRAG_TTS_API_BASE` / `RAYRAG_TTS_API_KEY` / `RAYRAG_TTS_MODEL`（默认 `tts-1`）/ `RAYRAG_TTS_VOICE`（默认 `alloy`）；ASR 沿用 `RAYRAG_ASR_API_BASE` / `RAYRAG_ASR_API_KEY` / `RAYRAG_ASR_MODEL`（默认 `whisper-1`）。首次引导界面与 `.env.example` 同步新增这 4 个 TTS 字段。

**界面**：模型页新增 **Speech** 卡片 —— 输入文本点 🔊「朗读」（成功则用 `Blob` 播放 `audio/mpeg`），或选择音频点 📝「转写」。**失败以文字呈现**（未配置时显示 `No default TTS model is set` 之类原句），不会静默无反应。文件选择器的 `accept` 即上面的白名单。CDP 实测：卡片/两个按钮/accept 均存在，空文本提示「Type something first」，未配置 TTS 显示原句，未选文件提示「Choose an audio file first」，页面 **0 报错**。

:::note 仍待完成
RAGFlow 把麦克风放在**聊天输入区**；RayRAG 目前把这个能力放在模型页卡片上。把输入区拆成合适的元素后再接麦克风按钮仍是待办（0.13.0 曾因直接往拼接字符串里塞控件而撤回）。
:::

### 0.17.0：嵌入模型检查（embedding check）

`POST /api/v1/datasets/{dataset_id}/embedding/check` —— 判断「本数据集已存的向量是否仍与当前嵌入模型一致」，用于切换嵌入模型前的安全检查。

请求体：`{"embd_id": "...", "n": 8, "mode": "..."}`。

响应：
```json
{"summary": {"kb_id","model","sampled","valid","avg_cos_sim","min_cos_sim","max_cos_sim","match_mode","threshold"},
 "results": [{"chunk_id","doc_id","doc_name","vector_field","vector_dim","stored_vector_dim","cos_sim"}]}
```

- `avg_cos_sim >= 0.9` → `code 0`；否则 → **`code 10`（NOT_EFFECTIVE）**，且附带**同样的 data** + 上游原文消息（不会把可疑数值伪装成成功）。
- 拒绝语义：`` `embd_id` is required. `` / `no authorization` / `No embedded chunks are available to compare.`（无向量可比较时明确说「没有可比较对象」，而不是报 0 相似度）。
- `embd_id` 允许字面量 `default` → 解析为本部署配置的 `EMBED_MODEL`（否则客户端只能编一个模型名，把编造的标签印在真实数字旁边）。
- **等距抽样**（非随机）：同一数据集同一请求两次结果一致 —— 这个数字决定迁移是否安全。
- **维度不一致** → `cos_sim: 0.0`，并同时给出 `vector_dim` 与 `stored_vector_dim`。

**界面**：数据集配置页新增「Embedding check」按钮，运行后打印平均/最小/最大相似度、比较了多少块、以及实际模型名（含 code 10 结论）。

### 0.18.0：连接器实例族（connectors）

此前 `/api/v1/connectors` 返回**硬编码的三个假连接器**、`create` 只回显不落库（用户「新增」后刷新即消失），并缺 4 条文档化接口。现由**单一持久化、按属主隔离**的存储承载：

| 接口 | 行为 |
|---|---|
| `POST /api/v1/connectors` | 校验名称与类型（类型取自 `ConnectorRegistry::kinds()`，不是第二份清单），落库 |
| `GET /api/v1/connectors/{id}` | 实例详情；否则 `404 / 102 Connector not found.` |
| `PATCH` / `DELETE /api/v1/connectors/{id}` | 改名/启停/改配置，或连同历史一起删除 |
| `GET /api/v1/connectors/{id}/logs` | 历史（最新在前） |
| `POST /api/v1/connectors/{id}/test` | **真正构建连接器并列举来源** |
| `POST /api/v1/connectors/{id}/rebuild` | 报告来源当前提供什么，并**声明没有写入任何文档** |

- **拒绝语义**：未知类型 → `101` 且**列出已知类型**；请求体非法 → `400/101`（不是 axum 的纯文本 rejection）；不存在 → `404/102`。
- **`rebuild` 不谎报**：`{"scanned": N, "imported": 0}` + 消息说明「导入是解析流水线的职责」。
- **`test` 真实探测**：经注册表构建 → 加载凭据 → 统一超时内列举；结果写入 `last_test` 与历史。
- **历史上限**：保留 2000 行、单次返回 ≤500 行。
- **配置键**：`url` 为来源位置（`LocalConnector` 以 url 为根目录），`path` 为其友好别名；子目录用 `target`。
- **权限**：写动词（create/patch/delete/test/rebuild）保持**管理写** → 非管理员 `403`；读（list/get/logs）按属主隔离。
- **界面**：数据源设置页新增「Connectors」卡片（逐行 Test/Rebuild/Logs/Delete + 从注册表渲染类型的新建表单）。

### 0.19.0：工具目录与能力报告（去假数据）

| 接口 | 行为 |
|---|---|
| `GET /api/v1/plugin/tools` | 返回本部署提供给 LLM 的工具（OpenAI function 形状：`{type:"function", function:{name, description, parameters}}`），来源为 `mcp_server::ToolRegistry` |
| `GET /api/v1/mcp/tools` | **同一个目录**（此前返回两个无法执行的硬编码工具 `search_knowledge` / `list_datasets`） |
| `GET /api/v1/plugins` | 逐项能力报告：`{id, name, available, model?, configured_by, detail?}`，取自运行状态 |
| `POST /api/v1/plugins/{id}/toggle` | **拒绝并说明原因**（`103`）：这些能力由部署配置提供，不可运行时切换；未知 id → `102 Unknown capability '<id>'. Known: …` |

- **单一来源**：工具目录来自注册表，并用**调用者自己的数据集**充实 retrieval 工具的描述 —— 模型不可能传入它从未见过的 dataset id。
- **能力报告**：`llm` / `embedding` / `rerank` / `vision` / `ocr` / `tts` / `asr`；每行都给出**配置位置**（`LLM_API_BASE / LLM_MODEL` 等）；**不可用项必须带 `detail` 说明原因**（测试强制）。
- **不再谎报成功**：`toggle` 原返回 `"Plugin X toggled"` 而什么都没做，现改为明确拒绝 + 原因。
- **界面**：模型页新增「Capabilities」卡片，渲染该报告并把缺失项的原因列在下方。
