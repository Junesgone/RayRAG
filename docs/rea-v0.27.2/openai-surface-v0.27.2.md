# OpenAI 兼容面（RAGFlow v0.27.2 运行取证）

> REA 轮次④产出（2026-10-11）。来源：源码契约提取 + 10 探针运行矩阵（KB key）+ 容器日志。
> 工作笔记：`~/.cache/rea/ragflow-v0.27.2/notes/04-openai兼容面.md`；原始证据：`round4-forensics/`。

## 路由与别名

| 路由 | 状态 | 说明 |
| --- | --- | --- |
| `POST /api/v1/openai/{chat_id}/chat/completions` | 现行 | chat 侧 OpenAI 兼容端点（`openai_api.py:237`） |
| `POST /api/v1/agents/chat/completions` | 现行 | agent 侧；`openai-compatible: true` 切 OpenAI 线格式（`agent_api.py:1444`） |
| `POST /api/v1/chats_openai/{chat_id}/chat/completions` | 弃用 | 转发至 `openai/*` + 日志 WARNING |
| `POST /api/v1/agents_openai/{agent_id}/chat/completions` | 弃用 | 转发至 `agents/chat/completions`（自动置 `openai-compatible`）+ WARNING |

## 契约要点

**chat 侧**（`openai_api.py`）：
- 必填 `model`、`messages[]`；`stream`；`extra_body{reference, reference_metadata, metadata_condition}`。
- `model:"model"` 占位符 → 使用会话已配模型；否则校验存在性 + key 可用性。
- 响应：成功 = OpenAI 形状（`chatcmpl-<chat_id>`）；**前置失败 = `{code,data,message}` 信封**（401 / 101 缺参 / 102 业务 / 100 405）。
- SSE：`chat.completion.chunk` + `reasoning_content`（think）+ 末块 `usage`/`reference`/`final_content` + `data:[DONE]`。

**agent 侧**（`agent_api.py` + `canvas_service.py:429`）：
- `agent_id` + `messages[]` + `openai-compatible:true`；`session_id`（或 `id`/`metadata.id`）续接。
- **运行期错误 = HTTP 200 + OpenAI 形状 + `content:"**ERROR**: …"`**（实测 `Agent not found.`，抛自 `canvas_service.py:486`）。
- 成功：`id = session_id ∨ uuid4`；`model = agent_id`；`param: null`；usage 三零 details。

**公共**：生成参数 `temperature/top_p/frequency_penalty/presence_penalty/max_tokens`（`*_enabled` 门控）；形状模板 `api_utils.py:424 get_data_openai`。

## 探针矩阵（2026-10-11 23:46）

| 探针 | HTTP | code | 返回 |
| --- | --- | --- | --- |
| openai 无鉴权 | 401 | 401 | `<Unauthorized '401: Unauthorized'>` |
| openai 假 chat | 200 | 102 | You don't own the chat … |
| openai 缺/空 messages | 200 | 101/102 | required argument … / You have to provide messages. |
| openai GET | 200 | 100 | MethodNotAllowed |
| 两别名路径 | 200 | — | 与各自母路由一致 + 服务端 WARNING |
| agents 假 id | 200 | — | `**ERROR**: Agent not found.` |
| agents 真 id（ui-probe-agent） | 200 | — | 空内容、completion_tokens=0 |

## 产品侧观察

- 前端产物中端点路径字面量 `openai/` 仅见于文档 chunk（`index-D3fhcQ52.js`）；其余 "openai" 出现为模型 provider 名。
- 两段式错误哲学：预先校验走信封；进入执行后的错误包装为对话内容（agent 侧）或 SSE 内 `**ERROR**`（chat 流式）。

## 对标增量（待入实施队列）

1. `openai/{chat_id}/chat/completions`（形状 + 占位符 + extra_body + SSE 细节）。
2. `agents/chat/completions` 的 `openai-compatible` 模式及错误语义。
3. 弃用别名 2 条。
4. 生成参数 5 键 + 门控。
