# RayRAG ↔ RAGFlow v0.27.2 剩余接口推进计划（内部文档，不发布）

数据来源：`scripts/api-parity-audit.py`（同名 105 / 别名 38 / 覆盖 2 / **未实现 36** / 方法缺口 1 / 不匹配 0）。

按来源文件分组，并给出**性质**与**前置条件**——性质决定它属于「接线」还是「功能」，前置条件决定它能不能先做。

## dataset_api.py（10 条）

**性质/前置条件**：多为功能级：artifacts 的 structure/graph/alteration 需要编译产物行（wiki 关系行 / 结构编译输出）；embedding/check、metadata/config、index / index_type 各自需要子系统。

- `DELETE /datasets/<dataset_id>/<index_type>`
- `GET /datasets/<dataset_id>/artifacts/alteration`
- `GET /datasets/<dataset_id>/artifacts/graph`
- `DELETE/GET /datasets/<dataset_id>/artifacts/structure`
- `POST /datasets/<dataset_id>/embedding/check`
- `GET /datasets/<dataset_id>/graph`
- `DELETE/GET/POST /datasets/<dataset_id>/index`
- `GET/PUT /datasets/<dataset_id>/metadata/config`
- `DELETE/GET/HEAD /datasets/<dataset_id>/skills`
- `DELETE/GET /datasets/<dataset_id>/skills/<path:skill_kwd>`

## agent_api.py（6 条）

**性质/前置条件**：功能级新增子系统：Webhook（配置/日志/测试）、附件（上传/下载/预览）、导出下载。可独立立项，互不阻塞。

- `POST /agents/<agent_id>/upload`
- `DELETE/GET/HEAD/PATCH/POST/PUT /agents/<agent_id>/webhook`
- `GET /agents/<agent_id>/webhook/logs`
- `DELETE/GET/HEAD/PATCH/POST/PUT /agents/<agent_id>/webhook/test`
- `GET /agents/attachments/<attachment_id>/download`
- `GET /agents/attachments/<attachment_id>/preview`

## document_api.py（5 条）

**性质/前置条件**：缩略图、文档内图片、文档产物读取需要一个「文档产物目录」的落盘约定；`/documents/upload`、`/documents/ingest` 是入口形态差异。

- `PUT /datasets/<dataset_id>/documents/<document_id>/metadata/config`
- `GET /documents/artifact/<filename>`
- `GET /documents/images/<image_id>`
- `POST /documents/upload`
- `GET /thumbnails`

## user_api.py（4 条）

**性质/前置条件**：忘记密码整体流程（验证码/OTP/校验/重置）需要邮件或 OTP 通道，属于独立功能。

- `POST /auth/password/forgot/captcha`
- `POST /auth/password/forgot/otp`
- `POST /auth/password/forgot/otp/verify`
- `POST /auth/password/reset`

## connector_api.py（4 条）

**性质/前置条件**：连接器实例面（`/connectors/{id}` 及其 logs/rebuild/test）与 RayRAG 的 `/data_sources/*` 语义不同，需要先决定是适配还是移植。

- `DELETE/GET/PATCH /connectors/<connector_id>`
- `GET /connectors/<connector_id>/logs`
- `POST /connectors/<connector_id>/rebuild`
- `POST /connectors/<connector_id>/test`

## chat_api.py（2 条）

**性质/前置条件**：TTS 语音合成；独立 ASR 转写接口（现有 ASR 只服务解析流水线）。

- `POST /chat/audio/speech`
- `POST /chat/audio/transcription`

## chunk_api.py（1 条）

**性质/前置条件**：需要逐条确认是接线还是功能。

- `DELETE/GET /datasets/<dataset_id>/documents/<document_id>/structure/graph`

## file2document_api.py（1 条）

**性质/前置条件**：需要逐条确认是接线还是功能。

- `POST /files/link-to-datasets`

## plugin_api.py（1 条）

**性质/前置条件**：上游 pluginlib 插件注册表（LLM 工具元数据）未移植；RayRAG 的画布工具目录在 `/api/v1/components`。

- `GET /plugin/tools`

## bot_api.py（1 条）

**性质/前置条件**：`/searchbots/ask` 的 SSE 帧格式与既有搜索补全不同，需要单独的流式处理器。

- `POST /searchbots/ask`

## file_commit_api.py（1 条）

**性质/前置条件**：需要逐条确认是接线还是功能。

- `GET /workspace-files/<file_id>/versions`

## 方法缺口（路径已实现，缺方法）

- `/datasets/<dataset_id>/navigation` — dataset_api.py [POST]

## 建议顺序

1. **生成任务**（`POST /datasets/{id}/navigation` 与产物生成）：解锁 UI 上 Generate 按钮与 artifacts/navigation 的**数据来源**，是其余产物类接口的前置条件。
2. **Agent Webhook**（3 条）：独立子系统，边界清晰，可一次做完。
3. **Agent 附件**（3 条）：与 Webhook 同属画布侧，共用上传/下载路径。
4. **忘记密码流程**（4 条）：独立且用户可感知；需要先定邮件/OTP 通道的配置项。
5. **文档产物族**（缩略图/图片/产物/上传，5 条）：先定落盘约定，再暴露接口。
6. 其余（连接器实例面、TTS/独立 ASR、搜索机器人流式问答、文件版本历史、插件工具元数据）：按需立项。

