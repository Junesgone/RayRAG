# RayRAG ↔ RAGFlow HTTP 接口对照工作清单（内部文档，禁止发布）

> 生成：`scripts/api-parity-audit.py --write`；数据源 = 固定 RAGFlow v0.27.2 参考树 `api/apps/restful_apis/*.py` 与 `src/server.rs` 的 `.route(...)` 注册表。
> 用途：把「每个上游接口 RayRAG 是否应答」变成可复核的数字，并按批收敛。

## 现状

- 上游路由：**181**；RayRAG `/api/v1` 路由：**263**
- 同名应答（exact）：**88**
- 别名应答（alias，上游拼写已注册但走 RayRAG 自己的命名）：**38**
- 形状不同但已被应答（covered）：**2**
- 尚未实现（missing，已登记）：**53**
- 路径已有但方法缺失（method）：**4**
- 未登记的缺口（mismatch，`--check` 视为失败）：**0**

## 未实现清单（按上游文件分组）

| 上游路径 | 来源文件 | 说明 |
| --- | --- | --- |
| `/agents/<agent_id>/upload` | `agent_api.py` | Agent 附件上传（画布内文件输入） |
| `/agents/<agent_id>/webhook` | `agent_api.py` | Agent Webhook 配置（含 logs/test） |
| `/agents/<agent_id>/webhook/logs` | `agent_api.py` | Agent Webhook 调用日志 |
| `/agents/<agent_id>/webhook/test` | `agent_api.py` | Agent Webhook 连通性测试 |
| `/agents/attachments/<attachment_id>/download` | `agent_api.py` | Agent 附件下载 |
| `/agents/attachments/<attachment_id>/preview` | `agent_api.py` | Agent 附件预览 |
| `/agents/download` | `agent_api.py` | Agent DSL/画布导出下载 |
| `/agents/prompts` | `agent_api.py` | Agent 提示词模板列表 |
| `/agents/test_db_connection` | `agent_api.py` | Agent 数据库组件连接测试 |
| `/searchbots/ask` | `bot_api.py` | 搜索应用问答机器人（上游 bot_api 的公开问答面） |
| `/searchbots/detail` | `bot_api.py` | 搜索应用机器人详情 |
| `/searchbots/mindmap` | `bot_api.py` | 搜索应用脑图 |
| `/searchbots/retrieval_test` | `bot_api.py` | 搜索应用检索测试 |
| `/chat/audio/speech` | `chat_api.py` | TTS 语音合成 |
| `/chat/audio/transcription` | `chat_api.py` | ASR 语音转写（当前仅有解析流水线内的 ASR） |
| `/chats/<chat_id>/sessions` | `chat_api.py` | 会话（session）列表 |
| `/chats/<chat_id>/sessions/<session_id>` | `chat_api.py` | 会话（session）详情/删除 |
| `/datasets/<dataset_id>/chunks` | `chunk_api.py` | 数据集级分块列表 |
| `/datasets/<dataset_id>/documents/<document_id>/structure/graph` | `chunk_api.py` | 文档结构图（编译产物） |
| `/connectors/<connector_id>` | `connector_api.py` | 连接器实例读写（RayRAG 用 data_sources 面） |
| `/connectors/<connector_id>/logs` | `connector_api.py` | 连接器同步日志 |
| `/connectors/<connector_id>/rebuild` | `connector_api.py` | 连接器重建索引 |
| `/connectors/<connector_id>/test` | `connector_api.py` | 连接器连通性测试 |
| `/datasets/<dataset_id>/<index_type>` | `dataset_api.py` | 按索引类型读取数据集 |
| `/datasets/<dataset_id>/artifacts` | `dataset_api.py` | 数据集 Artifacts（wiki/topics/structure/graph） |
| `/datasets/<dataset_id>/artifacts/<page_type>/<path:slug>` | `dataset_api.py` | Artifacts 单页内容 |
| `/datasets/<dataset_id>/artifacts/alteration` | `dataset_api.py` | Artifacts 变更记录 |
| `/datasets/<dataset_id>/artifacts/graph` | `dataset_api.py` | Artifacts 图视图 |
| `/datasets/<dataset_id>/artifacts/structure` | `dataset_api.py` | Artifacts 结构视图 |
| `/datasets/<dataset_id>/artifacts/topics` | `dataset_api.py` | Artifacts 主题视图 |
| `/datasets/<dataset_id>/embedding/check` | `dataset_api.py` | 嵌入模型可用性检查 |
| `/datasets/<dataset_id>/graph` | `dataset_api.py` | 数据集知识图谱读取 |
| `/datasets/<dataset_id>/index` | `dataset_api.py` | 数据集索引重建 |
| `/datasets/<dataset_id>/metadata/config` | `dataset_api.py` | 数据集元数据配置 |
| `/datasets/<dataset_id>/navigation` | `dataset_api.py` | 数据集导航树 |
| `/datasets/<dataset_id>/navigation/<path:name>` | `dataset_api.py` | 导航树单节点 |
| `/datasets/<dataset_id>/navigation/<path:name>/children` | `dataset_api.py` | 导航树子节点 |
| `/datasets/<dataset_id>/navigation/search` | `dataset_api.py` | 导航树检索 |
| `/datasets/<dataset_id>/skills` | `dataset_api.py` | 数据集 Skills 列表 |
| `/datasets/<dataset_id>/skills/<path:skill_kwd>` | `dataset_api.py` | 数据集 Skill 详情/更新 |
| `/datasets/<dataset_id>/documents/<document_id>/metadata/config` | `document_api.py` | 文档级元数据配置 |
| `/documents/artifact/<filename>` | `document_api.py` | 文档产物文件读取 |
| `/documents/images/<image_id>` | `document_api.py` | 文档内图片读取 |
| `/documents/upload` | `document_api.py` | 文档上传（RayRAG 用 /datasets/{id}/documents 与 /files/upload） |
| `/thumbnails` | `document_api.py` | 缩略图服务 |
| `/files/link-to-datasets` | `file2document_api.py` | 文件关联数据集 |
| `/workspace-files/<file_id>/versions` | `file_commit_api.py` | 文件版本历史（file_commit） |
| `/plugin/tools` | `plugin_api.py` | 插件工具列表 |
| `/auth/password/forgot/captcha` | `user_api.py` | 忘记密码-图形验证码 |
| `/auth/password/forgot/otp` | `user_api.py` | 忘记密码-发送 OTP |
| `/auth/password/forgot/otp/verify` | `user_api.py` | 忘记密码-校验 OTP |
| `/auth/password/reset` | `user_api.py` | 忘记密码-重置 |
| `/users/me/models` | `user_api.py` | 当前用户可用模型列表 |

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
| `/agents/<agent_id>/sessions` | agent_api.py [DELETE] |
| `/providers/<provider_id_or_name>/instances/<instance_id_or_name>` | provider_api.py [PUT] |
| `/providers/<provider_id_or_name>/instances/<instance_id_or_name>/models` | provider_api.py [DELETE] |
| `/providers/<provider_id_or_name>/instances/<instance_id_or_name>/models/<path:model_name>` | provider_api.py [POST] |
