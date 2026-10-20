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
- **`available` 的语义是「已配置」而非「当前可达」**：响应内含 `note` 明确说明，计数字段名为 `configured`，界面用词也是 *configured*（本机 rerank 端点已配置但服务已停，若读成「rerank 可用」就是错的）。
- **`available` 的语义是「已配置」而非「当前可达」**：响应内含 `note` 明确说明，计数字段名为 `configured`，界面用词也是 *configured*（本机 rerank 端点已配置但服务已停，若读成「rerank 可用」就是错的）。
- **不再谎报成功**：`toggle` 原返回 `"Plugin X toggled"` 而什么都没做，现改为明确拒绝 + 原因。
- **界面**：模型页新增「Capabilities」卡片，渲染该报告并把缺失项的原因列在下方。

### 0.20.0：四处「字面量列表」改为读真实来源

用「扫描返回字面量数组的 handler」这一检查（发现连接器族与工具目录的同一方法）又找到四处：

| 接口 | 原返回 | 现读取 |
|---|---|---|
| `GET /api/v1/llm/factories` | 固定三项（`OpenAI`/`MiniMax`/`Qwen`） | 提供商目录（每个 factory 一行） |
| `GET /api/v1/channels` | 固定两行（`feishu`/`webchat`） | 运行时所用的渠道配置 |
| `GET /api/v1/bots` | 固定两行（`slack-local`/`discord-local`） | 配置中带 `callback_url` 的渠道（这才是「机器人」） |
| `GET /api/v1/file/commits` | 硬编码空数组 | workspace 提交存储（新增 `recent(limit)`） |

- **factory 行**：`{name, id, logo, tags, status, configured, domestic, model_count, api_key_env, base_url}`；`tags` 由 `ProviderKind` 映射（`LLM`/`TEXT EMBEDDING`/`RERANK`/`IMAGE2TEXT`/`TTS`/`SPEECH2TEXT`/`OCR`）；`domestic` 标记**中国大陆友好**的厂商（用户在此网络环境下正是按这个挑）。与提供商页面同源（`PROVIDER_PRESETS`），不可能互相矛盾。
- **channels / bots**：空列表会带 `note` 说明来源（`RAYRAG_CHANNELS_CONFIG` 或各渠道环境变量），不会看起来像页面坏了。
- **bots 的定义**：配置中带 `callback_url` 的渠道 —— 否则它只是渠道。
- **commits**：只有存储为空才报「没有提交」。

**实机验证抓到的两个错误（第一版实现）**
1. 渠道配置是**包裹**的（`{"channels": {…}}`，运行时消费的形状）；读顶层会把包裹键当成一个名为 `channels` 的渠道。现改为纯函数 `channel_entries()` / `channel_rows_from_config()` / `bot_rows_from_config()`，并**对包裹与未包裹两种形状都做单测**。
2. factory 的 `configured` 原先只看「是否存在 provider 行」，而引导流程会为每个本地引擎播种行 —— 于是三个云端 factory 在没有任何密钥时也显示已配置。现要求**密钥存在**才算：`configured`（需密钥）与 `instances`（实例数）分开报告。

### 0.21.0：验证码接口 + 发布快照的结构性清理

- **接口**：`GET|POST /api/v1/auth/password/forgot/captcha?email=...` → `image/png`（300×120），响应头 `Cache-Control: no-store`、`x-captcha-ttl: 60`；缺 `email` → `400` + 明确文案。上游只注册 POST、却把 MIME 标成 `image/JPEG`；RayRAG 同时接受 GET（浏览器取图用 GET）并回真正的 `image/png`。
- **验证码参数**：长度 **4**（上游 `OTP_LENGTH`）、有效期 **60 秒**、字符集 `A-Z0-9`、**大小写不敏感**、**一次性**（答对即作废，答错保留重试）。
- **闸门**：`POST /api/v1/auth/password/forgot/otp` 新增必填 `captcha`；缺失与错误返回**不同文案**（`102`）：「先取图」与「答案错误或已过期，请重新取图」。
- **免登录修复**：`/auth/password/forgot/captcha`、`/forgot/otp`、`/forgot/otp/verify`、`/auth/password/reset` 原本被 bearer 中间件拦住（忘记密码的人没有 token），现与上游一致置于未认证组，有单测钉住。
- **UI**：登录页新增自包含「重置密码」卡片 `#resetCard`（`data-testid='reset-card|reset-email|reset-captcha-image|reset-captcha|reset-reload|reset-send|reset-code|reset-new-password|reset-submit|reset-status'`）；占位符用 `@TOKEN@` + `replace`。
- **发布规则（重要）**：`web/*.json` 与 `web/uploads/*.json` 一律**不随快照发布**（运行时状态：文档、元数据、记忆、令牌、轨迹），发布脚本对导出结果做结构性删除 + **任一残留即 FATAL 拒绝发布**；源仓库同步 `git rm --cached` + `.gitignore`。教训：按名单排除的机制对「后来新增的文件」没有记忆。

### 0.22.0：逐文档结构图（GET + DELETE）—— 顺带修掉条件匹配的语义错误

- **接口**：`GET /api/v1/datasets/{dataset_id}/documents/{document_id}/structure/graph`（可选 `?keywords=`）与 `DELETE` 同路径（body `{"template_id": "<id>|legacy:<compile_kwd>|raptor"}`）。
- **响应**：`{total_entities, total_relations, returned_entities, returned_relations, templates[{template_id, template_name, kind, entities, relations}], scanned_rows, truncated, note}`；空图时 `note` 说明「编译产物由知识编译在解析后写入」。
- **分桶规则（对齐上游）**：按 `compilation_template_ids` 分桶；无标记的行进 `legacy:<compile_kwd>` 桶（不丢数据）；数据集已配置的模板排在前面（按配置顺序）；`wiki` 模板跳过；无实体且无关系的桶不显示；RAPTOR 摘要行单独成 `RAPTOR Summary` 桶。
- **本轮修掉的关键缺陷**：`DocStore` 的条件匹配原为严格相等（`row.get(k) == Some(v)`），而上游条件是搜索引擎的 `terms` 语义（如 `{"knowledge_graph_kwd": ["entity","relation"]}`）—— 该形状**永远匹配不到任何行**，`structure_graph_common` 因此从未真正工作过。现语义：标量=精确相等；数组=命中其中任一值（标量字段）或多值字段有交集。
- **分桶实现**：按**行 id** 分组后再投影（用「doc + compile_kwd」作 scope 会连带命中带模板标记的行 → 重复计数）。
- **keywords**：服务端按实体 `name` / 文本子串过滤（上游还会走向量检索补候选；此处确定性过滤，差异已登记）。
- **DELETE**：删除该模板的 compact graph 行 + entity/relation 行；`raptor` 只删投影行（摘要分块仍可检索）；空 `template_id` → `102 \`template_id\` is required`。
- **易错点**：`filter_entities_with_relations` 返回的是**实体**（不是关系），误用会把关系替换成节点列表。

### 0.23.0：嵌入搜索机器人 `POST /searchbots/ask`（SSE）

- **请求**：`{question, kb_ids|[kb_id], search_id?, top_k?, similarity_threshold?, vector_similarity_weight?, rerank_id?}`；`question`/`kb_ids` 缺失 → `101`；`search_id` 存在且其 `search_config.kb_ids` 非空时**优先使用该列表**（上游顺序）。
- **越权**：任一无权数据集 → `102 You don't own the requested dataset`。
- **帧格式（上游自有包封）**：`data:{"code":0,"message":"","data":{"answer":…,"reference":{chunks,doc_aggs,total}}}`；失败帧 `code:500` + `message` + `data.answer="**ERROR**: …"`；**结束帧** `data:{"code":0,"message":"","data":true}`。
- **响应头**：`text/event-stream; charset=utf-8`、`Cache-Control: no-cache`、`Connection: keep-alive`、`X-Accel-Buffering: no`。
- **检索复用** `/api/v1/search` 的现有实现（避免第二套打分规则漂移）；回答走 `state.tenant_models.resolve(..., Chat)` → `llm_client().chat_completion()`；**无可用聊天模型时在帧内报错并附带已检索到的引用**，不返回空答案。
- **UI**：`/search` 页新增 `#askCard`（`data-testid='ask-card|ask-question|ask-send|ask-status|ask-answer|ask-refs'`），逐帧读取流、识别 `data:true` 结束、浏览器不支持流式时回退整体读取；数据集取自页面自身选择器 `searchDocsSelectedKbs()`。

### 0.24.0：沙箱产物（artifact）落盘 + 接口

- **接口**：`GET /api/v1/documents/artifact/{filename}`（上游同名）、`DELETE` 同路径、`GET /api/v1/documents/artifacts`（RayRAG 补充的列表，便于 UI 提供下载按钮）。
- **拒绝（顺序与文案对齐上游）**：非裸文件名（含 `/`、`\\`、`basename != filename`）→ `102 Invalid filename.`；扩展名不在可服务类型（`.png .jpg .jpeg .svg .pdf .csv .json .html`）→ `102 invalid file type`；不存在或非本人 → `102 Artifact not found.`
- **成功响应**：原始字节 + 存储的 MIME + `Content-Disposition: inline; filename="<净化后的名字>"`。
- **归属**：产物属于产生它的运行用户；管理员可见全部（与上游「属于该用户的会话」模型一致的最接近实现）。
- **存储**：`uploads/artifacts.json`（+ PostgreSQL 镜像）；单文件上限 16 MB、总量 500，占用超限拒绝并记日志（**不能静默丢弃**）。
- **生产者**：agent 代码执行路径把 `SandboxResult.artifacts` 写入 `ArtifactStore::shared()`（执行器拿不到 AppState）；**API 持有同一个 Arc 实例**，否则会读到陈旧副本（只在文件型存储的生产环境才暴露）。
- **UI**：沙箱设置页 `#sbArtifactCard`（`data-testid='sandbox-artifact-card|sandbox-artifact-refresh|sandbox-artifact-status|sandbox-artifact-download|sandbox-artifact-delete'`）；下载走带 token 的 fetch + Blob（裸 `<a href>` 不带 token 会被拒）。

### 0.25.0：技能树（Skills）

- **上游契约**：每个技能节点一行（`compile_kwd="skill"`，字段 `skill_kwd` / `depth_int` / `children_kwd` / `source_doc_ids` / `md_with_weight`），再加一行聚合（`compile_kwd="skill_all"`，正文为整棵树的 markdown）。常量与上游一致：`_SKILL_COMPILE_KWD="skill"`、`_SKILL_ALL_COMPILE_KWD="skill_all"`。
- **接口**：
  - `GET /api/v1/datasets/{id}/skills` → `{code:0,data:<聚合行|null>}`
  - `HEAD` 同路径 → 有树 `200`、无树 `404`，无正文；无权限 `403`（HEAD 无法带拒绝正文，由状态码承载）
  - `DELETE` 同路径 → `{code:0,data:{deleted:n}}`（聚合行计 1）
  - `GET /api/v1/datasets/{id}/skills/<path:skill_kwd>` → `{code:0,data:<节点行|null>}`（支持 `install/linux` 形式）
  - `DELETE` 同路径 → `{code:0,data:{deleted:n}}`，删除子树（含后代），父节点同步移除该子键
  - **RayRAG 补充**：`POST /api/v1/datasets/{id}/skills/compile`（上游由 UI 驱动知识编译，无 REST 路由；按钮需要端点）
- **权限拒绝**：`{code:500, message:"no authorization"}` —— **故意**照抄上游这些路由的业务码 500，与 RayRAG 其他数据集路由常用的 102 不同，属于「1:1 对齐上游」的例外，已在运维记录中标注。
- **编译来源（重要差异）**：上游用 LLM（Corpus2Skill）生成技能树；RayRAG **从语料结构编译**：按已索引分块的标题层级（`#`..`######`）在文档内建树，节点正文即该标题下的原文；无标题语料回退为「每文档一个节点」，键取文档名去扩展名。**不合成任何文本**，无需模型，同序同结果。空数据集返回 `102 The dataset has no indexed content to compile skills from`，不落一棵看起来像编译成功的空树。
- **读取顺序与上限**：按文档分组（稳定排序，保留文档内原序），`inc_search_page` 分页 500 行/页，累计上限 20000 行；超出时记 warning。
- **存储**：`uploads/skill_trees.json`（键为数据集 id；+ PostgreSQL 镜像）。
- **UI**：数据集页 `#dsSkillCard`（`data-testid='dataset-skill-card|compile|reload|delete-all|status|tree|toggle|view|delete'`）。

### 0.26.0：引用块（reference chunk）字段对齐

- **上游契约（检索 `POST /api/v1/retrieval`）**：`content, content_ltks, document_id, document_keyword, highlight, id, image_id, important_keywords, tag_kwd, dataset_id, positions, similarity, term_similarity, vector_similarity`。
- **上游契约（chat/agent 引用）**：`id, content, document_id, document_name, document_metadata, dataset_id, image_id, positions, url, similarity, vector_similarity, term_similarity, doc_type`。
- **实现**：`src/api/chunk_citation.rs` 统一生成引用块；合并进 4 个构造点（`server.rs` 的 `/api/v1/retrieval`、`api/search.rs::weighted_search`、`api/features.rs::references_payload`、chat 流式引用帧）。
- **取值规则**：字段值一律取存储值，无则为空（与上游一致：无位置数据时上游也发 `positions: []`、`image_id: ""`）；**不臆造坐标**。`content_ltks` 优先取存储的 `content_ltks`，否则用与关键词索引相同的分词器（`harness::knowlege_dataset_nav::tokenize`）现场生成。
- **positions 解析**：JSON 数组原样；JSON 字符串解析；逗号元组（`12,11,11`）包装为「列表的列表」（上游形状）；**畸形 JSON → `[]`**，不从半截元组里猜坐标。
- **兼容**：RayRAG 原有字段名（`chunk_id`/`doc_id`/`kb_id`/`doc_name`/`score`）保留；检索路径上 `similarity` 与 `score` 同值两名。
- **注意（重复实现）**：`server.rs` 的 `/api/v1/retrieval` 与 `api/search.rs::weighted_search` 是**同一响应形状的两份实现**，首轮补丁只覆盖了后者，由路由测试在真实 handler 上发现。现由同一 helper 供给全部 4 处，避免再次漂移。

### 0.27.0：语音交互的 UI 控件（麦克风 / 朗读）

- **接口**（早已存在，本轮补 UI）：`POST /api/v1/chat/audio/speech`（TTS，返回二进制音频流，`Content-Type: audio/mpeg`）、`POST /api/v1/chat/audio/transcription`（ASR，multipart 字段 `file`，`stream=false` 时返回 `{code:0,data:{text}}`）。
- **UI**：聊天详情页 `#chatAudioCard`（`data-testid='chat-audio-card|chat-mic|chat-speak-last|chat-audio-status'`）；助手消息操作行追加 `data-testid='chat-message-speak'`（🔊），由一段脚本 + `MutationObserver` 注入，**不改动既有消息标记**。
- **行为**：麦克风用 `MediaRecorder` 录音（优先 `audio/webm`，按 `mimeType` 推定扩展名），提交后把 `data.text` 追加进 `#chatInput`；朗读把最后一条助手消息 POST 给 TTS，用 Blob URL 播放并在 `ended` 时 `revokeObjectURL`。
- **错误可见**：`Transcription failed.` / `Speech synthesis failed.` / `This browser refused microphone access.` / `Nothing was recorded or transcribed.` 全部带底层错误写入状态行 —— **不允许静默失败**；无麦克风权限的浏览器明确告知而不是留一个死按钮。
- **插入纪律（沿用前两轮教训）**：控件块放在页面**脚本闭合之后**；页面测试断言「控件标记不得落在 script 内」「script 内不得出现裸 CSS 声明」「占位符必须以引号替换」。

### 0.28.0：页面文案必须走翻译助手（i18n 审计）

- **规则（由测试强制）**：`src/web.rs` 中凡是渲染标记的行（含 `push_str` / `format!(`），其 `>文案<` 文本必须通过 `t("...", zh)` 输出；测试 `every_page_label_goes_through_the_translation_helper` 用 `include_str!("web.rs")` 自读源码执行该审计，**上限为 0**。
- **审计口径（重要，勿误读为全量 i18n 覆盖）**：只看**标记里的可见文本**；不检查仅存在于 JS 字符串中的文案；不检查 `json_script_string(t(...))` 之外的其他注入路径。
- **本轮**：修复 49 处未走翻译助手的标签（其中 43 处由脚本拆分字面量：前缀 / `t(...)` / 后缀；6 处整句位于 `format!` 与三处 admin 页面裸字符串中，手工拆分）。
- **教训**：Python 粗扫得出「剩 25 处」，而测试口径实测为 6 处 —— 两者规则不同。**以测试口径为准**，避免用没被强制的数字写进发布说明。
- **验证**：故意注入 `b.push_str("<p>Brand new English Label</p>");` 并把上限设为 0，测试确实失败并指名到行 —— 守卫不是摆设。

#### i18n 审计的两套口径与遗留清单（务必区分）

| 口径 | 规则 | 实测 |
|---|---|---|
| **强制口径**（`every_page_label_goes_through_the_translation_helper`） | 渲染行（含 `push_str`/`format!(`）里 `>文案<` 的**标记字面量文本**，且该行未调用 `t(` | **0**（上限 0，新增即失败） |
| **严格口径**（未强制，仅记录） | 同一行内**逐标签**判断：任何 `>文案<`，含通过 JS 字符串注入 innerHTML / `<option>` 的文案 | **59** 处待办 |

严格口径下待翻译的 59 处集中在：JS 注入的 `<option>` 列表（如 `All files`）、innerHTML 片段（`No graph has been built for this dataset yet.`、`Building... (LLM entity extraction)`、`Load failed`）、以及配置项文案（`Naive (token-based)`、`Generation scope`、`Entity resolution`、`Community reports`、`Empty canvas`、`No versions yet`、`Local sources` 等）。**中文页面上这些仍是英文**（已实机确认 `<option>All files</option>` 不随语言切换）。下一轮按此清单推进，并把强制口径逐步收紧到严格口径。

## i18n: page-label contract (0.29.0)

| Item | Value |
| --- | --- |
| Translator | `web::t(key, zh)` — two match arms: `if !zh { return match .. }` (English early-return, ends with `other => other,`) then the Chinese table (ends with `_ => k,`) |
| Correct insertion anchor for a Chinese key | the **last** `_ => k,` inside `t()`, immediately before the closing brace and `fn detect_lang` |
| Direction guard | `web::tests::translation_table_points_the_right_way` — asserts `t(label, true) == zh` and `t(label, false) == label`; proven to fail when a key is missing or lands in the wrong arm |
| Page-label audit | `web::tests::every_page_label_goes_through_the_translation_helper` — per-label rule; ceiling `BACKLOG = 0` (reached in 0.30.0), may not grow |
| Audit scope | lines that build markup (`push_str` / `format!`); strings that never reach markup are outside it |
| JS-injected labels | translated by splitting the literal at the label boundary into `prefix + t(label, zh) + suffix` — output bytes are identical, so the page structure cannot change |
| Admin console labels | `AdminConsoleSection::label()` returns the upstream English label; **callers must pass it through `t()`** |

## i18n: label-splitting shapes (0.30.0)

Splitting a markup literal at a label boundary is always byte-identical, but four shapes defeated earlier
passes. A future bulk pass must handle all of them:

| Shape | Symptom | Handling |
| --- | --- | --- |
| Label appears twice on one line | Only the first occurrence is rewritten; the second survives | Split **every** occurrence, from the last to the first |
| Whitespace inside the text node (`<label> Show citations</label>`) | `>Label<` never matches while the trimming audit keeps counting it | Match `>(\s*)Label(\s*)<` and keep the whitespace in the prefix/suffix |
| `r#"..."#` raw literal | A regex expecting `push_str("` does not match | Support `r#`…`r###` openings; a raw prefix may not contain `"#` |
| Literal that opens on one line and closes on another | Every line-based pass skips it, silently | Relax the pattern to allow an unterminated literal and keep the continuation in the suffix statement |
| Browser tab `<title>` | Not a `>text<` node, so no audit sees it | `page_head` passes its title through `t()` |

## Dataset detail page levels (0.31.0)

| Route segment | Panel id | Sidebar label (en / zh) |
| --- | --- | --- |
| `files` | `tab0` | Files / 文件列表 |
| `retrieval` | `tab2` | Retrieval testing / 检索测试 |
| `logs` | `tab1` | **Logs / 日志** — upstream key `knowledgeDetails.overview`, value from `en.ts:488` / `zh.ts:435` |
| `configuration` | `tab3` | Configuration / 配置 |
| `knowledge-graph` | `tab4` | deep link only, not a sidebar entry |
| `compilation` | `tab5` | Artifacts / Artifacts |

`showTab(n)` activates the panel from `location.pathname`, so **all five routes serve the same HTML** and the
difference exists only after the client script runs. Verify page levels by rendered visibility
(`offsetParent` / computed `display`), never by comparing response bytes.

**i18n rule:** a label is the translation **value** for its key, not the key name. Check `locales/en.ts` and
`locales/zh.ts` for the value before writing a label into the UI.

## 编译模板参数面（RAGFlow v0.27.2 对齐，v0.3.10aj）

> 供后续代码/接口编写查询：编译模板（knowledge compilation template）的 kind 词表、config 键、接口路由与逐字校验文案。对标基线 RAGFlow v0.27.2（commit `a024bea0cd93`），数据源 `api/db/init_data/compilation_templates/`。

### kind 词表（v0.27.2 重命名）

| kind | 含义 | 编辑器可选 | scope |
| --- | --- | --- | --- |
| `wiki` | 图式 wiki（v0.26.4 旧名 `artifacts`，启动时自动迁移） | ✓ | 单独成组 → `dataset` |
| `knowledge_graph` | 知识图谱 | ✓ | `file` |
| `timeline` | 时间线/列表 | ✓ | `file` |
| `mind_map` | 思维导图 | ✓ | `file` |
| `page_index` | PageIndex 目录树 | ✓ | `file` |
| `tree` | RAPTOR 文档树 | ✓ | `file`（组内最多一个 `raptor.rechunk=true`） |
| `session_essence` / `session_graph` | 会话类模板（内置数据在库，但编辑器 `ExcludedBuiltinKinds` 不提供） | ✗ | － |
| `empty` | 前端常量保留，v0.27.2 无种子文件 | ✗ | － |

内置数据基线：**8 条 builtins**（按 `_sort_builtins`：无 empty 时按 display_name 升序）+ **6 条 wiki presets**（文件名序 brand/engineering/general/market/product/user_interview）。

### 接口路由

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/v1/compilation-template-groups` | 组列表（keywords/scope/page/page_size/orderby/desc） |
| GET/PUT/DELETE | `/api/v1/compilation-template-groups/{id}` | 组详情/更新/删除 |
| POST | `/api/v1/compilation-template-groups` | 建组 |
| POST | `/api/v1/compilation-template-groups/{id}/execute` | 执行组内模板（RayRAG 扩展，用于 tree/hypergraph 试跑） |
| GET | `/api/v1/compilation-templates/builtins` | 内置模板面板（v0.27.2 形状 `{id,kind,display_name,description,config}`） |
| GET | `/api/v1/compilation-templates/wiki-presets` | wiki 蓝图预设（v0.27.2 形状 `{id,topic,instruction,example}`，旧 `page_example` 废止） |
| 别名 | 下划线形式 `/api/v1/compilation_template_groups`、`/api/v1/compilation_templates/wiki_presets` … | 兼容旧前端调用，语义相同 |

页面路由：`/compilation-templates/edit-next`（创建）、`/compilation-templates/edit-next/{id}`（编辑）。

### 组/模板请求体（与上游 edit-next `transformFormToPayload` 一致）

```json
{
  "name": "<组名，= 模板 name>",
  "description": "",
  "templates": [{
    "id": "<编辑时回传>",
    "name": "<模板名>",
    "description": "",
    "kind": "wiki|knowledge_graph|timeline|mind_map|page_index|tree",
    "config": {
      "kind": "<same as kind>",
      "mode": "entity|topic",
      "global_rules": "...",
      "instruction": "...",
      "example": "...",
      "use_blueprint": true,
      "synthesis": {"compile_kwd": "...", "enabled": true, "example": "..."},
      "rechunk": false,
      "rechunk_rules": "...",
      "raptor": {"prompt": "...", "max_token": 512, "clustering_threshold": 0.3, "clustering_ratio": 0.5, "rechunk": false},
      "entity": {"description": "...", "fields": [{"type": "...", "description": "...", "rule": "..."}]},
      "relation": {"description": "...", "fields": [{"type": "...", "description": "...", "rule": "..."}]},
      "claim": {"fields": [{"statement": "...", "subject": "..."}]},
      "concept": {"fields": [{"term": "...", "definition_excerpt": "..."}]}
    }
  }]
}
```

- `wiki`：`mode` 必填（entity/topic）；`instruction`/`example` 仅在 `use_blueprint=true` 时保留（trim 后写入），否则写空串；`claim`/`concept` 字段必填。
- `tree`：只有 `raptor` 配置，无分节字段与 rechunk 元键；`raptor.rechunk` 组内至多一个为 true。
- 其余 kind：分节字段来自对应 builtin 的 config（entity/relation，及可能存在的 synthesis）。
- 字段键序（Add field 弹层）：`type,description,rule` / `statement,subject` / `definition_excerpt,term`；卡片标题/描述映射：entity·relation = type/description，claim = statement/subject，concept = definition_excerpt/term。
- 类型去重：entity/relation 的 `type` 在分节内不可重复；可选项来自 builtin 分节的 `type` 集合。

### 组校验规则与逐字错误文案（HTTP 200 信封）

- 成功：`{code:0, message:"success", data:...}`（list/detail/create/update/delete 均 HTTP 200）。
- 缺参（`@validate_request`）：`{code:101, message:"required argument are missing: name,templates; ", data:null}`。
- 校验/不存在：`{code:102, message}`（无 data 键，HTTP 200）。

| 条件 | 文案 |
| --- | --- |
| 缺 name/templates | `Missing required field: {key}.` |
| 组名为空/超 128 字节 | `Invalid template group name.` / `Template group name is too long.` |
| 模板缺字段 | `Missing required field: {key}.` |
| 模板名非法 | `Invalid template name.` |
| kind 非法 | `Invalid template kind.` |
| config 非对象 | `Invalid template config.` |
| global_rules 超 4096 字符 | `Global compilation rules is too long.` |
| entity/relation type 缺失/重复 | `{Section} type is required.` / `{Section} type can not be duplicated.` |
| entity/relation 描述缺失 | `{Section} field description is required.` |
| wiki mode 非 entity/topic | `Wiki mode must be either 'entity' or 'topic'.` |
| wiki claim 字段缺失 | `Claim statement is required.` / `Claim subject is required.` |
| wiki concept 字段缺失 | `Concept term is required.` / `Concept definition excerpt is required.` |
| wiki 与其它模板同组 | `A wiki template cannot be combined with other templates in the same group.` |
| 组内两个 tree 都开 rechunk | `Only one tree template in a group may enable re-chunking.` |
| 组名重复 | `Duplicated compilation template group name.` |

### 编辑器本地校验（v0.3.10aj 落地）

页面 `/compilation-templates/edit-next` 客户端校验与上游一致：模板名必填（Please input template name）、kind 必选（Please select a kind）、wiki mode 必选（Please select a Wiki mode）、分节字段按 kind 检查 type/description、claim statement/subject、concept term/definition_excerpt；提交后走组 POST/PUT，成功跳 `/agents`。JSON 预览弹层展示表单态 `{name,description,avatar,templates:[{id,name,description,kind,config}]}`。

### 运维注意

- 旧数据（v0.26.4 kind=artifacts）在 `CompilationTemplateStore::new` 载入时迁移为 `wiki`（同时 `config.kind`、补 `mode=entity`）并持久化。
- internal assets：本文件、对照台账、运维记录不进入 GitHub 发布快照。

## 数据集 Artifacts / Wiki 编译面（RAGFlow v0.27.2 对齐，v0.3.10ak）

> 供后续代码/接口编写查询：知识编译页（Artifacts tab）的接口形状、生成门控、UI 行为与证据方法。基线与数据同前（v0.27.2，commit `a024bea0cd93`）。

### 生成门控（upstream `wiki-generation-eligibility.ts`）

`canGenerateWiki(kb) = (kb.chunk_count ?? 0) > 0 && Boolean((kb.pipeline_id ?? "").trim())`。

- `chunk_count`：数据集已解析 chunk 数（RayRAG 由解析流水线维护）。
- `pipeline_id`：`Knowledgebase.pipeline_id`（upstream `CharField(max_length=32, null=True)`）——**数据集顶层列**；RayRAG 的 `POST/PUT /api/v1/datasets` 接受该键（absent=保持、null/空串=清除、字符串=绑定），并随数据集 JSON 返回。
- 前端按钮态：不满足时 Generate 禁用，并按 `chunk_count>0 ? 缺 pipeline : 缺 chunk` 给出提示；结构类视图（graph/mindmap/timeline）显示 `autoCompiled`（文档解析时自动编译）。

### 接口（`src/api/dataset_artifacts.rs` / `src/api/dataset_navigation.rs`）

| 方法 | 路径 | 形状 |
| --- | --- | --- |
| HEAD | `/api/v1/datasets/{id}/artifacts` | 200=有任何 wiki 页面；404=空（upstream `has_any_wiki`） |
| GET | `/api/v1/datasets/{id}/artifacts?page&page_size&page_type&keywords&topic` | `{total, items:[{slug,title,page_type}]}`，默认 page_size=200、上限 1000 |
| GET | `/api/v1/datasets/{id}/artifacts/topics?page_size&keywords` | `{total, items:[{topic,title,slug}]}`（page_type=topic） |
| GET | `/api/v1/datasets/{id}/artifacts/{page_type}/{slug}` | `{slug,title,page_type,content,summary,entity_names,source_doc_ids,source_chunk_ids}`（slug 含 `/` 时按整段匹配） |
| PUT | `/api/v1/datasets/{id}/artifacts/{page_type}/{slug}` | body `{content_md,title?,comments?}` → `{slug,page_type,saved:true}`；仅更新已存在页面 |
| DELETE | `/api/v1/datasets/{id}/artifacts` | `{deleted}`（清空全部 wiki_page 行） |
| POST | `/api/v1/datasets/{id}/artifacts/compile` | MAP+COMPILE，见下 |
| GET/DELETE | `/api/v1/datasets/{id}/artifacts/structure` | 结构类 artifact 列表/清空 |
| GET | `/api/v1/datasets/{id}/artifacts/graph|alteration` | 同列表形状（按 page_type 过滤） |
| GET | `/api/v1/datasets/{id}/navigation?page_size&keywords&top_k` | `{total, items:[nav 节点]}`；节点含 `name/description/keywords/entities/graph_content/doc_count/type(cluster|doc)` |
| GET | `/api/v1/datasets/{id}/navigation/{name}/children` | 同上的子节点列表 |
| POST/DELETE | `/api/v1/datasets/{id}/navigation` | 生成/删除导航树（扩展/对齐 upstream dataset_api） |

### 编译响应（`POST .../artifacts/compile`）

```json
{"code":0,"data":{
  "mapped":1,"total":1,"extracts":4,"failed":0,"skipped":[],
  "compile":{"pages_created":1,"pages_modified":0,"pages_deleted":0,"errors":[]},
  "message":"..."
}}
```

- 需要服务端已配置 chat model；缺少时返回数据错误文案（不是静默空成功）。
- MAP 按文档抽取 entities/concepts/claims/relations/topics 并持久化 extract 行；COMPILE 从 extract 行合成页面（entity/concept 页 + 可能的 topic 页）。
- **修复（v0.3.10ak）**：compile 的 current chunk state 除 doc-store 扫描外，并入搜索 engine 的 chunk 视图（id + content hash）。修复前，新解析数据集因 chunk 行未镜像到 doc store 而 `pages_created=0` 且无错误。

### UI（`/dataset/compilation/{id}`）

- 外壳：Back + avatar + `{name}{titleSuffix}` + 视图选择器（Wiki / Tree-PageIndex / Graph / Mind map / Timeline）；loading / empty / workspace 三态。
- 空态标题逐字：`No wiki yet` / `No graph yet` / `No mind map yet` / `No timeline yet` / `No navigation nodes`；zh 值按上游 locale。
- Wiki 视图：Contents/Graph 双 tab；Topics→Pages 导航 + 面包屑；搜索（客户端过滤）；Update（重编译）/Clear（上游确认文案）；详情 markdown（标题/段落/列表/代码/外链/[[wiki link]] 点击解析到页面）；来源文档行。
- 生成后状态文案：Mapped X of Y document(s), extracts N, pages created K, modified M（+skipped/失败提示）。
- Tree 视图：navigation 树（cluster→children→doc 详情：description/keywords/entities/graph_content）。
- 结构视图：structure artifact 列表 + 详情 + autoCompiled 提示。

### 证据方法（可复跑）

1. `scripts/ui-traversal/mock-compilation-model-server.py`：本地 OpenAI 兼容 stub（/v1/embeddings + /v1/chat/completions；MAP JSON、partition/route JSON、SUMMARY/TITLE/TOPIC 页面 markdown），让 MAP→COMPILE 端到端离线可跑。
2. debug 实例 env：`LLM_API_BASE=http://127.0.0.1:18089/v1 LLM_MODEL=mock-model EMBED_API_BASE=... EMBED_MODEL=mock-embedding RAYRAG_EMBEDDING_DIMENSION=384`。
3. `scripts/ui-traversal/compilation-artifacts-view-cdp.mjs`：创建带 pipeline_id 的探针数据集→上传→加 chunk→CDP 点击 Generate→workspace/topic/page 详情/搜索/Graph tab/模式切换/Clear；断言 26 项（0 pageerror）。

## 搜索应用（searchapps）头像面（RAGFlow v0.27.2 对齐，v0.3.10al）

> 供后续代码/接口编写查询：搜索应用的 avatar 列、共享头像裁剪对话框与 CDP 验证方法。基线同前（commit a024bea0cd93）。

### 数据列

| 字段 | 位置 | 语义 |
| --- | --- | --- |
| avatar | SearchAppRecord / SearchAppCreate / UpdateSearchAppRequest | 64×64 PNG data URL（data:image/png;base64,...）；空串=无头像；serde(default) 兼容旧记录 |

- POST /api/v1/searchapps：body 可带 avatar（可选，缺省空）。
- PUT /api/v1/searchapps/{id} / PUT /api/v1/searches/{id}：局部更新，avatar 缺省保持、空串清除；响应 {code:0,message:"Updated"}（avatar 通过 GET 回读）。
- GET /api/v1/searchapps/{id}：返回完整记录，含 avatar。
- 上游等价：ISearchAppDetailProps.avatar + search-setting.tsx 的 avatar form 字段 + useUpdateSearch。

### UI 行为

- 设置抽屉 Avatar 行（#ssAvatarBtn / #ssAvatarFile / #ssAvatarImg / #ssAvatarEmpty / #ssAvatarClear）：
  - 点击 → 文件选择（accept=image/*）；前端校验扩展名 JPG/JPEG/PNG/WebP/BMP 与 ≤4MB，错误文案 SEARCH_LABELS.avatarTypeError / avatarSizeError（中英）。
  - 选择后打开共享裁剪对话框 #avatarCropModal（data-testid='avatar-crop-dialog'；拖拽选区、滚轮缩放、Cancel/OK）→ openAvatarCropModal(file,'','ssAvatarImg') 返回 64×64 data URL → 写入预览并显示移除按钮。
  - 移除按钮 → 清空预览 + 隐藏自身；ssLoad(app) 按 app.avatar 恢复/清空。
- 共享壳：fn layout 每页挂载 avatar_crop_markup(zh) + AVATAR_CROP_JS，恰好一份 #avatarCropModal；layout_plain（登录/无头页）不挂。
- 上游对照：共享组件 components/avatar-upload.tsx 同时被 profile 与 search-setting 使用；本轮把该对话框从 user-setting 壳上移到全局 layout。

### 证据方法

    RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
    RAYRAG_LOGIN_PASSWORD=<admin pass> \
    RAYRAG_ADMIN_EMAIL=admin@rayrag.local \
    node scripts/ui-traversal/search-settings-drawer-cdp.mjs
    # 23/23 checks：crop 对话框打开、64×64 data URL 预览、保存持久化、重载恢复、清空持久化

### 管理控制台复核批次（同轮）

scripts/ui-traversal/admin-*.mjs 9 个脚本（console / login-form / identity-cache / services-table / service-dialogs / user-detail / user-detail-tables / users-cells / user-forms）在当前构建全绿（0 error）；sandbox 两脚本需 /tmp/rr-sshd fixture。台账对应 7 行 admin partial 已翻 aligned。

## Provider 固定图标资产（RAGFlow v0.27.2 对齐，v0.3.10am）

> 供后续实现查询：模型 provider 的固定图标映射与静态资产位置。

- 上游语义：components/svg-icon.tsx 的 IconMap 把 factory 名映射为图标 id；svgIcons 内的 factory 用固定 SVG 图片（themeIcons 再追加 -bright/-dark），其余用 iconfont glyph。
- RayRAG：fn provider_icon(provider_name) -> ProviderIcon::{Svg(&'static str), Font(&'static str)}：
  - Svg 文件位于 web/static/assets/svg/llm/<file>，页面经 <img class='provider-brand-icon' src='/assets/svg/llm/<file>'> 渲染；
  - Font 经 <svg><use href='/icons.svg#icon-<symbol>'></use></svg> 渲染；
  - 未知 factory -> Font("moxing-default")。
- 当前 svg mapping（v0.27.2 内实际存在的 factory）：LocalAI->local-ai.svg、Gemini->gemini.svg、StepFun->stepfun.svg、MinerU->mineru-bright.svg、PaddleOCR->paddleocr.svg、n1n->n1n.svg、Avian->avian.svg、RAGcon->ragcon.svg、SoMark->somark.svg、New API->new-api.svg、aimlapi.com->aimlapi.svg、Astraflow/Astraflow-CN->astraflow.svg、FastEmbed->fastembed.svg、FunASR->funasr.svg、FuturMix->futurmix.svg、GreenPT->greenpt.svg、MWS->mws.svg、Synthorai->synthorai.svg、Xiaomi->xiaomi.svg。
- 资产来源：RAGFlow v0.27.2 web/src/assets/svg/llm/*.svg 全量（29 个），复制到 web/static/assets/svg/llm/。
- 单测 provider_icons_resolve_svg_font_and_fallback 断言每个映射文件真实存在；CDP provider-cards-cdp.mjs 可验证 provider 卡片图标 <img>。
