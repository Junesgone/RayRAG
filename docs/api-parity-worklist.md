# RayRAG ↔ RAGFlow HTTP 接口对照工作清单（内部文档，禁止发布）

> 生成：`scripts/api-parity-audit.py --write`；数据源 = 固定 RAGFlow v0.27.2 参考树 `api/apps/restful_apis/*.py` 与 `src/server.rs` 的 `.route(...)` 注册表。
> 用途：把「每个上游接口 RayRAG 是否应答」变成可复核的数字，并按批收敛。

## 现状

- 上游路由：**181**；RayRAG `/api/v1` 路由：**269**
- 同名应答（exact）：**94**
- 别名应答（alias，上游拼写已注册但走 RayRAG 自己的命名）：**38**
- 形状不同但已被应答（covered）：**2**
- 尚未实现（missing，已登记）：**47**
- 路径已有但方法缺失（method）：**0**
- 未登记的缺口（mismatch，`--check` 视为失败）：**0**

## 未实现清单（按上游文件分组）

| 上游路径 | 来源文件 | 说明 |
| --- | --- | --- |
| `/agents/<agent_id>/upload` | `agent_api.py` | 功能级缺口：画布附件上传（Agent 附件子系统）未实现 |
| `/agents/<agent_id>/webhook` | `agent_api.py` | 功能级缺口：Agent Webhook 子系统未实现 |
| `/agents/<agent_id>/webhook/logs` | `agent_api.py` | 功能级缺口：同上（Agent Webhook） |
| `/agents/<agent_id>/webhook/test` | `agent_api.py` | 功能级缺口：同上（Agent Webhook） |
| `/agents/attachments/<attachment_id>/download` | `agent_api.py` | 功能级缺口：同上（Agent 附件子系统） |
| `/agents/attachments/<attachment_id>/preview` | `agent_api.py` | 功能级缺口：同上（Agent 附件子系统） |
| `/agents/download` | `agent_api.py` | 功能级缺口：Agent 导出下载未实现 |
| `/agents/prompts` | `agent_api.py` | 功能级缺口：提示词模板列表未实现 |
| `/agents/test_db_connection` | `agent_api.py` | 功能级缺口：数据库组件连接测试未实现 |
| `/searchbots/ask` | `bot_api.py` | 功能级缺口：搜索应用公开机器人面（beta token 换应用 + 问答）未实现 |
| `/searchbots/detail` | `bot_api.py` | 功能级缺口：同上（搜索应用公开机器人面） |
| `/searchbots/mindmap` | `bot_api.py` | 功能级缺口：同上（搜索应用公开机器人面） |
| `/searchbots/retrieval_test` | `bot_api.py` | 功能级缺口：同上（搜索应用公开机器人面） |
| `/chat/audio/speech` | `chat_api.py` | 功能级缺口：TTS 语音合成未实现（ASR 已在解析流水线内） |
| `/chat/audio/transcription` | `chat_api.py` | 功能级缺口：独立 ASR 转写接口未实现（RAYRAG_ASR_* 目前只服务解析流水线） |
| `/datasets/<dataset_id>/documents/<document_id>/structure/graph` | `chunk_api.py` | 功能级缺口：逐文档结构图（编译产物按文档/模板存储）未实现；RayRAG 的 GraphRAG 产物按数据集存储 |
| `/connectors/<connector_id>` | `connector_api.py` | 功能级缺口：连接器实例面（RayRAG 的数据源在 /api/v1/data_sources/*，语义不同，未做别名） |
| `/connectors/<connector_id>/logs` | `connector_api.py` | 功能级缺口：同上（连接器实例面） |
| `/connectors/<connector_id>/rebuild` | `connector_api.py` | 功能级缺口：同上（连接器实例面） |
| `/connectors/<connector_id>/test` | `connector_api.py` | 功能级缺口：同上（连接器实例面） |
| `/datasets/<dataset_id>/<index_type>` | `dataset_api.py` | 功能级缺口：按索引类型删除索引未实现 |
| `/datasets/<dataset_id>/artifacts/alteration` | `dataset_api.py` | 功能级缺口：同上（产物引擎未接线） |
| `/datasets/<dataset_id>/artifacts/graph` | `dataset_api.py` | 功能级缺口：同上（产物引擎未接线） |
| `/datasets/<dataset_id>/artifacts/structure` | `dataset_api.py` | 功能级缺口：同上（产物引擎未接线） |
| `/datasets/<dataset_id>/embedding/check` | `dataset_api.py` | 功能级缺口：换模型重嵌入相似度校验未实现 |
| `/datasets/<dataset_id>/graph` | `dataset_api.py` | 功能级缺口：数据集知识图谱读取未实现（产物按 kb 存储但缺公开读取路径） |
| `/datasets/<dataset_id>/index` | `dataset_api.py` | 功能级缺口：按索引类型重建索引未实现 |
| `/datasets/<dataset_id>/metadata/config` | `dataset_api.py` | 功能级缺口：数据集自动元数据配置未实现 |
| `/datasets/<dataset_id>/navigation` | `dataset_api.py` | 功能级缺口：数据集导航引擎已移植（knowlege_dataset_nav.rs）但未接入运行期存储 |
| `/datasets/<dataset_id>/navigation/<path:name>` | `dataset_api.py` | 功能级缺口：同上（导航引擎未接线） |
| `/datasets/<dataset_id>/navigation/<path:name>/children` | `dataset_api.py` | 功能级缺口：同上（导航引擎未接线） |
| `/datasets/<dataset_id>/navigation/search` | `dataset_api.py` | 功能级缺口：同上（导航引擎未接线） |
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
| `/auth/password/forgot/otp` | `user_api.py` | 功能级缺口：同上（忘记密码流程） |
| `/auth/password/forgot/otp/verify` | `user_api.py` | 功能级缺口：同上（忘记密码流程） |
| `/auth/password/reset` | `user_api.py` | 功能级缺口：同上（忘记密码流程） |
| `/users/me/models` | `user_api.py` | 功能级缺口：租户默认模型信息（llm_id/embd_id/asr_id/img2txt_id…）读取接口未实现；RayRAG 的对应数据在 /api/v1/tenant/models |

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
| — | — |
