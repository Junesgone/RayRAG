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
| `RAYRAG_TEST_LLM_BASE` | `http://127.0.0.1:8088` | GPU 本地模型端点（llama.cpp llama-server，OpenAI 兼容）。 测试套件用这些变量做真实推理闭环验证（--ignored 测试）。 |
| `RAYRAG_TEST_LLM_MODEL` | `Qwen3.5-9B-Q4_K_M.gguf` |  |
| `RAYRAG_TEST_EMBEDDING_BASE` | `http://127.0.0.1:8888` |  |
| `RAYRAG_TEST_EMBEDDING_MODEL` | `Qwen3-Embedding-4B-Q4_K_M.gguf` |  |
| `RAYRAG_TEST_RERANKER_BASE` | `http://127.0.0.1:8081` |  |
| `RAYRAG_TEST_VISION_BASE` | `http://127.0.0.1:8088/v1` |  |
| `RAYRAG_TEST_VISION_MODEL` | `Qwen3.5-9B-Q4_K_M.gguf` |  |
| `RAYRAG_TEST_RERANKER_KEY` | `mxbai-rerank-key` | S3-compatible object storage (MinIO / AWS S3 / Aliyun OSS / Tencent COS) Empty endpoint keeps local-filesystem mode; path-style defaults to true (required for MinIO and most self-hosted endpoints). RAYRAG_STORAGE_ENDPOINT=http://127.0.0.1:9000 RAYRAG_STORAGE_REGION=us-east-1 RAYRAG_STORAGE_ACCESS_KEY=minioadmin RAYRAG_STORAGE_SECRET_KEY=minioadmin RAYRAG_STORAGE_PATH_STYLE=true |
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
| `RAYRAG_POSTGRES_PASSWORD` | `change-me` | Enable with: cargo run --bin rayrag --features postgres-backend -- serve |
| `RAYRAG_POSTGRES_URL` | `postgresql://rayrag:change-me@127.0.0.1:5432/rayrag` |  |
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
| GET | `/api/v1/datasets/{id}/documents/{did}` | `-` |
| POST | `/api/v1/datasets/{id}/documents/{did}/reparse` | `-` |
| GET | `/api/v1/documents/{doc_id}/preview` | `-` |
| GET | `/api/v1/documents/{doc_id}` | `-` |
| GET | `/api/v1/documents/{doc_id}/preview/sheets` | `-` |
| GET | `/api/v1/documents/{doc_id}/preview/slides` | `-` |
| GET | `/api/v1/datasets/{id}/documents/{did}/download` | `-` |
| GET | `/api/v1/datasets/{id}/documents/{did}/metadata` | `-` |
| GET | `/api/v1/datasets/{id}/metadata/keys` | `-` |
| POST | `/api/v1/datasets/{id}/metadata/summary` | `-` |
| ANY | `/api/v1/datasets/{id}/documents/metadatas` | `axum::routing` |
| POST | `/api/v1/datasets/{id}/metadata/batch` | `-` |
| GET | `/api/v1/datasets/{id}/documents/{did}/chunks` | `-` |
| GET | `/api/v1/datasets/{id}/documents/{did}/chunks/{chunk_id}` | `-` |
| POST | `/api/v1/retrieval` | `-` |
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
| POST | `/api/v1/dify/retrieval` | `-` |
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
| GET | `/api/v1/providers/{provider_name}/instances/{instance_name}` | `-` |
| GET | `/api/v1/providers/{provider_name}/instances/{instance_name}/models` | `-` |
| ANY | `/api/v1/providers/{provider_name}/instances/{instance_name}/models/{model_name}` | `axum::routing` |
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
| GET | `/api/v1/stats` | `-` |
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
| ANY | `/api/v1/users/me` | `axum::routing` |
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

## 4. 变更流程

1. 新增参数/接口 → 先在 RAGFlow v0.27.2 源码中找同名或最接近的参数/接口，保持命名与形状一致。
2. 同步更新本文件（env/路由表用生成脚本刷新，契约段手改），并在 `CHANGELOG.md` 与 `运维记录.md` 记录。
3. 加回归测试：参数默认值、接口成功/失败形状、错误 message 可读（不允许静默）。
4. 发布前运行 `python3 scripts/gen-interface-registry.py --check`，确保登记表与代码一致。
