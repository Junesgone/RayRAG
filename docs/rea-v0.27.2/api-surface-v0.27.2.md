# RAGFlow v0.27.2 API 面（REA 逆向提取）

来源：部署镜像前端产物 `web-dist`（REA 语义提取）+ 内嵌官方文档 + 运行实例探针（2026-10-11）。
原始证据目录：`~/.cache/rea/ragflow-v0.27.2/`（`endpoints-documented.txt`、`deprecated-map.tsv`、`endpoint-probe-results.tsv`、`endpoint-probe-bodies.tsv`、`notes/01-登录链.md`、`notes/02-接口面与迁移.md`）。

## 清单规模

| 来源 | 内容 | 数量 |
|---|---|---|
| 前端 API 表（llm-util 提取） | UI 实际调用路径 | 87 |
| 内嵌官方文档 | 文档端点（method+URL 去重） | 86 |
| 运行探针 | 全量 GET+POST 存活/分层实测 | 87+19 |

## 行为要点（实测）

- 统一信封 `{"code","data","message"}`；**405 内嵌 `code:100`（HTTP 200）**；缺参 `code:101`；鉴权 `code:401/102`；缺路由 `code:404`。
- `GET /api/v1/system/version` → `v0.27.2`；`registerEnabled:1`。
- `GET /v1/system/healthz` 与 `GET /api/v1/system/healthz` **双活**（兼容层）。

## 弃用 → 替代（官方迁移表：v0.24.0 兼容层保留旧路径）

| 弃用 | 替代 |
|---|---|
| POST /api/v1/chats_openai/{chat_id}/chat/completions | POST /api/v1/openai/{chat_id}/chat/completions |
| PUT /api/v1/chats/{chat_id}/sessions/{session_id} | PATCH 同路径 |
| POST /api/v1/chats/{chat_id}/completions | POST /api/v1/chat/completions |
| POST /api/v1/sessions/related_questions | POST /api/v1/chat/recommendation |
| PUT ../documents/{document_id}/chunks/{chunk_id} | PATCH 同路径 |
| GET /v1/system/healthz | GET /api/v1/system/healthz |
| POST /v1/document/upload_info | POST /api/v1/documents/upload |
| POST /api/v1/file/upload、file/create | POST /api/v1/files |
| GET /api/v1/file/list、file/root_folder | GET /api/v1/files |
| GET /api/v1/file/parent_folder | GET /api/v1/files/{file_id}/parent |
| GET /api/v1/file/all_parent_folder | GET /api/v1/files/{file_id}/ancestors |
| POST /api/v1/file/rm | DELETE /api/v1/files |
| POST /api/v1/file/rename、file/mv | POST /api/v1/files/move |
| GET /api/v1/file/get/{file_id} | GET /api/v1/files/{file_id} |
| POST /api/v1/file/convert | POST /api/v1/files/link-to-datasets |
| POST /api/v1/agents_openai/{agent_id}/chat/completions | POST /api/v1/agents/chat/completions + `"openai-compatible": true` |
| DELETE /api/v1/chats：body `chat_id` | 改用 `ids` 列表 |

## v0.27.2 新面家族（探针确认存活，需鉴权）

- `workspaces/{id}/commits|changes|diff|tree|files/…`、`workspace-files/{file_id}/versions`（工作区版本化）
- `searches/*`（含 `searches/{id}/completions`）、`memories/*`、`messages/*`
- `openai/{chat_id}/chat/completions`、`agents_openai/*`（OpenAI 兼容层）
- `chat/audio/speech|transcription`、`chat/mindmap`、`chat/recommendation`
- `files/*` 新 CRUD、`documents/upload|ingest`、`datasets/{id}/metadata/*`、`datasets/{id}/commits`、`retrieval`

## 实例缺族（前端有引用但实例 404）

`/api/v1/skills/*`、`/api/v1/admin/whitelist*`、`/api/v1/pipelines*`、`/api/v1/compilation-templates` 根 —— 视为超前/门控代码，记录备查。
