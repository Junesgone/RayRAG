# RayRAG ↔ RAGFlow HTTP 接口对照工作清单（内部文档，禁止发布）

> 生成：`scripts/api-parity-audit.py --write`；数据源 = 固定 RAGFlow v0.27.2 参考树 `api/apps/restful_apis/*.py` 与 `src/server.rs` 的 `.route(...)` 注册表。
> 用途：把「每个上游接口 RayRAG 是否应答」变成可复核的数字，并按批收敛。

## 现状

- 上游路由：**181**；RayRAG `/api/v1` 路由：**295**
- 同名应答（exact）：**117**
- 别名应答（alias，上游拼写已注册但走 RayRAG 自己的命名）：**38**
- 形状不同但已被应答（covered）：**2**
- 尚未实现（missing，已登记）：**24**
- 路径已有但方法缺失（method）：**1**
- 未登记的缺口（mismatch，`--check` 视为失败）：**0**

## 未实现清单（按上游文件分组）

| 上游路径 | 来源文件 | 说明 |
| --- | --- | --- |
| `/agents/<agent_id>/upload` | `agent_api.py` | 功能级缺口：画布附件上传（Agent 附件子系统）未实现 |
| `/agents/attachments/<attachment_id>/download` | `agent_api.py` | 功能级缺口：同上（Agent 附件子系统） |
| `/agents/attachments/<attachment_id>/preview` | `agent_api.py` | 功能级缺口：同上（Agent 附件子系统） |
| `/searchbots/ask` | `bot_api.py` | 功能级缺口：嵌入问答使用上游自己的 SSE 帧格式（code/message/data 包封），与既有搜索补全的 chat.completion 帧不同，需要单独的流式处理器 |
| `/chat/audio/speech` | `chat_api.py` | 功能级缺口：TTS 语音合成未实现（ASR 已在解析流水线内） |
| `/chat/audio/transcription` | `chat_api.py` | 功能级缺口：独立 ASR 转写接口未实现（RAYRAG_ASR_* 目前只服务解析流水线） |
| `/datasets/<dataset_id>/documents/<document_id>/structure/graph` | `chunk_api.py` | 功能级缺口：逐文档结构图（编译产物按文档/模板存储）未实现；RayRAG 的 GraphRAG 产物按数据集存储 |
| `/connectors/<connector_id>` | `connector_api.py` | 功能级缺口：连接器实例面（RayRAG 的数据源在 /api/v1/data_sources/*，语义不同，未做别名） |
| `/connectors/<connector_id>/logs` | `connector_api.py` | 功能级缺口：同上（连接器实例面） |
| `/connectors/<connector_id>/rebuild` | `connector_api.py` | 功能级缺口：同上（连接器实例面） |
| `/connectors/<connector_id>/test` | `connector_api.py` | 功能级缺口：同上（连接器实例面） |
| `/datasets/<dataset_id>/embedding/check` | `dataset_api.py` | 功能级缺口：换模型重嵌入相似度校验未实现 |
| `/datasets/<dataset_id>/metadata/config` | `dataset_api.py` | 功能级缺口：数据集自动元数据配置未实现 |
| `/datasets/<dataset_id>/skills` | `dataset_api.py` | 功能级缺口：数据集 Skills 面未实现（Skills 索引仅有全局 /api/v1/skills/*） |
| `/datasets/<dataset_id>/skills/<path:skill_kwd>` | `dataset_api.py` | 功能级缺口：同上（Skills 面未实现） |
| `/datasets/<dataset_id>/documents/<document_id>/metadata/config` | `document_api.py` | 功能级缺口：文档级元数据配置未实现 |
| `/documents/artifact/<filename>` | `document_api.py` | 功能级缺口：文档产物文件读取未实现 |
| `/documents/images/<image_id>` | `document_api.py` | 功能级缺口：文档内图片读取未实现 |
| `/documents/upload` | `document_api.py` | 功能级缺口：独立文档上传入口未实现（RayRAG 走 /datasets/{id}/documents） |
| `/thumbnails` | `document_api.py` | 功能级缺口：缩略图服务未实现 |
| `/files/link-to-datasets` | `file2document_api.py` | 功能级缺口：文件关联数据集（file2document 目前为占位实现） |
| `/workspace-files/<file_id>/versions` | `file_commit_api.py` | 功能级缺口：文件版本历史（FileCommitService）未移植，/api/v1/file/commits 目前为空实现 |
| `/plugin/tools` | `plugin_api.py` | 功能级缺口：上游 pluginlib 插件注册表（LLM 工具元数据）未移植；RayRAG 的画布工具目录在 /api/v1/components |
| `/auth/password/forgot/captcha` | `user_api.py` | 功能级缺口：忘记密码整体流程（验证码/OTP/重置）未实现 |

## 别名对照（上游拼写 → RayRAG 路由）

| 上游路径 | RayRAG 路由 |
| --- | --- |
| `/compilation-template-groups` | `/api/v1/compilation_template_groups` |
| `/compilation-template-groups/<group_id>` | `/api/v1/compilation_template_groups/{id}` |
| `/compilation-templates/builtins` | `/api/v1/compilation_templates/builtins` |
| `/compilation-templates/wiki-presets` | `/api/v1/compilation_templates/wiki_presets` |
| `/searches/<search_id>/completions` | `/api/v1/searches/{search_id}/completions` |
| `/system/stats` | `/api/v1/stats` |

## 方法级缺口（路径已应答，但上游方法未实现）

| 上游路径 | 来源与缺失方法 |
| --- | --- |
| `/agents/<agent_id>/webhook/test` | agent_api.py [DELETE,GET,HEAD,PATCH,PUT] |

## 已知响应结构偏差（不得当作已完成）

- `GET /api/v1/datasets/{dataset_id}/documents`：说明书写 **`data: {"docs": [...], "total_datasets": n}`**，
  RayRAG 目前返回 **`data: [...]`（裸数组）**。实机确认（2026-10-03，0.10.3）：
  `{"code":0,"data":[{"id":…,"name":"link2.txt",…}]}`。
  **影响**：按说明书编写的客户端取 `data.docs` 会得到 null。
  **未修的原因**：该端点被 UI（`loadFiles()` 等）与多条测试按数组消费，改结构必须同时改 UI 与测试，
  半迁移比不改更危险。**下一轮作为独立改动处理，本轮不声称已对齐。**

## 2026-10-03 更新（0.10.3）

- `/files/link-to-datasets` 已实现（真实转换，非占位），从 `KNOWN_MISSING` 移除。
- 实测账本：`upstream=181 rayrgg=308 exact=118 alias=38 covered=2 missing=23 method=1 mismatch=0`。
- 注：`verify-all.sh` 的 parity 检查会因"KNOWN_MISSING 里的条目已被实现"而失败——这次失败**是账本正确工作的表现**，
  不是回归；更新账本后通过。

## 已实现的字段名对齐（0.11.2）

`api::compat` 在所有列表/详情端点上**附加**说明书记载的字段名（原字段保留）：
`embedding_model`←`embd_id`、`dataset_ids`←`kb_ids`、`created_by`/`tenant_id`←`owner_id`、
`create_time`/`update_time`←`created_at`/`updated_at`、`location`←`storage_name`、`title`←`name`（agent），
并把 `parser_config` 由 JSON 字符串解析为说明书要求的对象、把 `chunk_method` 提升为同级字段。
审计工具：`scripts/api-field-audit.py`（**报告而非门禁**，未接入 verify-all）。
首轮 48 → 22 项缺失，agent/chunk_list/memory/retrieval/search_app 已全绿。

## 仍未对齐的响应结构（不得当作已完成）

1. `GET /api/v1/files`：说明书 `data:{total, files, parent_folder}`，RayRAG 返回**裸数组**。
   与此前已修的 documents 同类；UI 的 `loadFiles()` 与多条测试按数组消费，需同步改。
2. `GET /api/v1/chats/{chat_id}/sessions`：说明书 `data:[…]`（数组），RayRAG 返回 `data:{items,total,page,page_size}`。
3. **语义差异**：说明书的 `/api/v1/chats` 列的是"聊天助手"，RayRAG 返回的是**会话记录**
   （`conversations`）。0.11.2 只修了"列表返回的 id 在 sessions 端点 404"这一可见症状（加了回退），
   **集合语义本身尚未对齐**，属未完成项。
