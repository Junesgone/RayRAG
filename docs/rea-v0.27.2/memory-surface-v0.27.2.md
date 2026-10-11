# Memory 家族与 workspace 提交面（RAGFlow v0.27.2 运行取证）

> REA 轮次⑥产出（2026-10-11）。来源：源码契约 + 18 探针（全只读）+ 服务端堆栈。
> 工作笔记：`~/.cache/rea/ragflow-v0.27.2/notes/06-memories-workspaces.md`；证据：`round6-forensics/`。

## 路由

| 路由 | 说明 |
| --- | --- |
| `POST/GET /api/v1/memories` | 建（必填 name,memory_type,embd_id,llm_id）/ 列表（memory_type,tenant_id,owner_ids,ids,storage_type,keywords 过滤） |
| `GET/PUT/DELETE /api/v1/memories/{id}` | 详情（消息列表+storage_type）/ 更新（白名单字段）/ 删除 |
| `GET /api/v1/memories/{id}/config` | 全配置（system_prompt 等） |
| `POST/GET /api/v1/messages` | 记账（必填 memory_id,agent_id,session_id,user_input,agent_response）/ 列表（memory_id 必填） |
| `PUT/DELETE /api/v1/messages/{memory_id}:{message_id}` | `{status:bool}` 开关 / 遗忘 |
| `GET /api/v1/messages/search`、`/messages/{mid}:{id}/content` | 语义检索 / 内容 |
| `GET /api/v1/workspaces/{entity_id}/commits*` | 文件提交面（与 `/datasets/{id}/commits*` 同源；工作区文件版本历史） |
| `GET /api/v1/models`、`/models/default` | 模型列表 / 每类型默认 |

## 要点

- **代码语义**：404=NOT_FOUND；100=通用错误（`ValueError("…")`/`LookupError("…")` repr 直出）；101 亦用于服务端参数检查。
- **Memory 记录结构**：`memory_type:["raw","semantic","episodic","procedural"]`、`forgetting_policy:"FIFO"`、`memory_size:5242880`、`temperature:0.5`、`storage_type:"table"`；默认提取提示词 3604 字符（见 `memory-default-system-prompt.txt`，RayRAG 可直接复用）。
- **归因策略**：JWT 不可冒用 subject；API-key 调用方可携 `user_id` 外部主体；日志仅记 trusted/client 不落 id。
- **workspaces**：`_register_commit_routes` 双挂载（datasets/workspaces）；非法 entity → `ValueError: Could not resolve folder`（`file_commit_api.py:111`）；memories/skills 提交路由未挂载。
- **模型引用两格式**：tenant_model id（32hex，规范）/ 复合名 `name@instance@provider`；记录中出现的 `name___provider@instance@provider` 变体会解析失败。

## ⚠ 实例缺陷（重要）

记忆记录「记忆」的 `embd_id` 为陈旧复合格式 → `messages/search` 一律 `code:100 LookupError`（`tenant_model_service.py:330`）。修复 = 规范化 `embd_id` 为 tenant_model id（`0e994d42bc1e11f1902779b1174dca50`）。**2026-10-12 用户决定：不需要修复（实例保持原样，事项关闭，不做 DB 写）。**
