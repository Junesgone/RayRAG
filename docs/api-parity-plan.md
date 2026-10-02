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


## 数据集侧栏对标结论（本轮从上游源码确认）

来源：`web/src/pages/dataset/sidebar/index.tsx`（`items` 列表）与 `web/src/routes.tsx`（路由表）。

上游 v0.27.2 数据集侧栏**恰好 5 项**：

| # | 上游 key | 标签 | 图标 |
|---|---|---|---|
| 1 | `Routes.Files` | `knowledgeDetails.subbarFiles` | FolderOpen |
| 2 | `Routes.DatasetTesting` | `knowledgeDetails.testing` | TextSearch |
| 3 | `Routes.DataSetOverview` | **`knowledgeDetails.overview`** | **Logs** |
| 4 | `Routes.DataSetSetting` | `knowledgeDetails.configuration` | Settings |
| 5 | `Routes.Compilation` | `Artifacts` | BookText |

**知识图谱（`Routes.KnowledgeGraph` = `/knowledge-graph`）在 v0.27.2 里是一条独立路由，不在数据集侧栏**（`routes.tsx:214`），从概览页等处进入。

**RayRAG 现状与差距**：侧栏为 文件列表 / **日志** / 检索测试 / 配置 / Artifacts —— 第 3 项被实现成 **Logs**，而上游这一项的**标签是 Overview（概览）只是复用了 Logs 图标**。

**因此用户「想不起来的那个功能」= 数据集概览（Dataset Overview）**，对应上游 `pages/dataset/dataset-overview/overview-table.tsx`（该文件里同时引用了 knowledgeGraph，概览页是进入知识图谱的入口之一）。

**待办**：
1. 把第 3 项从「日志」改为「概览」，面板内容按上游 overview 表（文档数/分块数/存储/最近活动）实现，日志移入概览页内的入口（上游同样是 Logs 图标 + overview 标签）。
2. 知识图谱：保持不在侧栏（与上游一致），但要给出**可达入口**（上游在概览页/解析配置里的图谱开关），并确认 `#tab4` 的 Build Graph 在接上 `/datasets/{id}/graph` 后真正可用。
3. 用户报障「配置页排版溢出」已在 0.6.9 修复（页面横向溢出 0）。

## 唯一剩下的方法缺口：`POST /datasets/{id}/navigation`（下一轮的施工单）

**上游契约**（`api/apps/restful_apis/dataset_api.py::generate_dataset_nav`，行 1171 起）：
- 方法/路径：`POST /datasets/<dataset_id>/navigation`
- 请求体：`{"documents": [...]}`（可选；缺省表示该数据集全部文档）
- 成功：`get_result(data=result)`——即 `{"code":0,"data":<service 的返回值>}`
- 语义：对该数据集的文档做一遍导航树生成（LLM 聚类 + 嵌入），已存在的树按文档增量更新

**RayRAG 侧已具备的零件**（都已移植且有单测，位于 `src/knowlege_dataset_nav.rs`）：
- `upsert_dataset_nav_doc(store, embed, chat, lock, tenant_id, kb_id, doc_id, summary_or_tree)` —— **单文档入口**（async）：内部完成摘要/图谱文本 → 嵌入 → 找最佳簇 `find_best_cluster` → 必要时 `maybe_split_cluster` → 落行
- `nav_embed(embed, text)`、`llm_merge`、`llm_create_summary`、`readable_cluster_name`、`nav_keywords(_limited)`、`nav_entities(_limited)`
- 行写入用 `store_upsert` / `store_delete`，索引为 `knowlege_dataset_nav::index_name(tenant)`
- 存储已接通：`AppState.doc_store`（`DocStorePool`），**必须用 `spawn_blocking`**（`with_store` 模式，见 `src/api/dataset_artifacts.rs`）

**实现要点（下一轮照此施工）**：
1. 解析请求体 `documents`（缺省 = `state.docs.list(&kb_id)` 的全部文档 id）；
2. 对每个文档取已解析分块文本（`state.engine` 中的索引分块，按 `doc_id` 过滤）拼成 `summary_or_tree`；
3. 取租户默认嵌入与对话模型（`state.tenant_models` + `state.embedder`/`state.llm`）；**缺失时返回 `code 102` 并说明缺哪一个**，不要静默跳过——这正是 UI 上用户需要看到的提示；
4. 逐个文档调用 `upsert_dataset_nav_doc`，**按需、流水线式**（不要一次性把所有文档读进内存，遵循用户的"避免内存泄漏"要求）；
5. 返回值带 `generated: <文档数>`、`skipped: [...]`（无分块/无摘要的文档及其原因），让部分失败可见；
6. 测试：无嵌入模型 → 102 且文案指明缺什么；有分块 → 生成后 `GET /navigation` 能读到节点；无分块的文档 → 计入 `skipped` 而不是静默略过。

**UI 联动**：`/dataset/compilation`（Artifacts）与数据集导航视图在生成后应能立即读到新树（读取接口已实现）。
