# RAGFlow v0.27.2 — 运行面取证：检索与对话链（round 3）

> 方式：对运行实例（`docker-ragflow-cpu-1`，:11526）以 KB 作用域 API key 发真实请求；请求级证据由服务端容器日志补齐（CDP 网络捕获受 `attach_limited` 为空）。
> 产物：`~/.cache/rea/ragflow-v0.27.2/round3-forensics/`（round3-summary.tsv + 各端点 .code/.body）；工作笔记 `notes/03-检索与对话链.md`。
> 基线：固定 commit `a024bea0cd93`（v0.27.2）；工作清单见 `docs/ragflow-v0.27.2-worklist.md` 方法任务区「续记⑤」。

## 实测矩阵（7 端点）

| 端点 | 方法 | HTTP | 字节 | 形状要点 |
|---|---|---:|---:|---|
| `/api/v1/system/healthz` | GET | 200 | 72 | `{db, doc_engine, redis, status, storage}` 全 `ok` |
| `/api/v1/datasets?page_size=5` | GET | 200 | 2156 | `{code, data[], total_datasets}`；元素 47 字段（id/name/chunk_count/embedding_model/parser_config/…） |
| `/api/v1/datasets/{id}/documents?page_size=3` | GET | 200 | 7275 | `{code, data:{docs[], total}, message}`；docs 元素 28 字段（name/type/suffix/size/chunk_count/progress/run/…）；total=30329 |
| `/api/v1/retrieval` | POST | 200 | 31676 | `{code, data:{chunks[], doc_aggs[], total}}`；chunk 14 字段（content/content_ltks/similarity/vector_similarity/positions/…）；total=64 |
| `/api/v1/chats?page_size=5` | GET | 200 | 61 | `{code, data:{chats[], total:0}, message}`（空表） |
| `/api/v1/agents?page_size=3` | GET | 200 | 729 | `{code, data:{canvas[], total:2}, message}`；canvas 元素 14 字段（title/type=agent/canvas_category=agent_canvas/permission=me/…） |
| `/api/v1/chat/completions` | POST | **000** | – | 客户端 120s / 240s 两轮均无响应——见下 |

## `chat/completions` 悬挂实证（默认模型链）

服务端日志（逐字）：

```
empty chat_model_id in req, use default chat model.
LiteLLM completion() model= MiniMax-M3; provider = openai
Retrying request to /chat/completions in 0.48 / 0.80 / 1.63 / 3.62 / 6.07 s
Error: SERVER_ERROR. Retrying in 204.20 seconds... (Attempt 1/5)
POST /api/v1/chat/completions 1.1 - - 120000133   （客户端 120s 断开）
POST /api/v1/chat/completions 1.1 - - 240000190   （客户端 240s 断开）
Request failed: NoneType: None
```

- 行为链：无 `model` 时取**租户默认对话模型**（实例 = MiniMax-M3 @ openai 兼容端点）→ 上游 `SERVER_ERROR` → LiteLLM 指数退避重试（上限 5 attempts、单次等待可达 204s）→ 端点自身**无超时快返**，请求悬挂直至客户端断开。
- 定性：悬挂源于**上游默认模型当前不可达**（环境态），非端点逻辑缺陷；端点对空 `model` 的缺省选择行为本身可对齐。

## 行为要点（供 RayRAG 对齐）

1. 鉴权分域：KB key（`Authorization: Bearer`）覆盖 datasets/documents/retrieval/chats/agents 全链；`/users/me*` 走登录会话（JWT），API key 下 401 —— 两套鉴权面分离。
2. 响应信封统一 `{"code":0,"data":…,"message":"success"}`；空态形状逐端点固定（chats → `{chats:[],total:0}`）。
3. `documents` 带 `progress/progress_msg/run`（解析状态机）与 `pipeline_id/pipeline_name`；`datasets` 带多套任务 id 后缀族（graphrag/raptor/mindmap/skill/timeline/wiki/structure…）—— 对齐时按族建字段表。
4. `retrieval` 命中 chunk 含 `content` 与三相似度（`similarity`/`vector_similarity`/`term_similarity`）及 `positions`（分页定位）；`doc_aggs` = `{count, doc_id, doc_name}` —— RayRAG 检索响应以其为字段基准。
5. retrieval 分页参数生效（`top_k:3`/`page_size:3` → chunks=3、`data.total=64`、`doc_aggs=4`）。
6. JWT 告警噪声：无会话请求伴随 `load_user from jwt got exception No b'.' found in value`（WARNING），不代表请求失败。

## 未决 / 待补

- `chat/completions` **成功响应形状**待默认模型恢复后补测（本实例 MiniMax-M3 不可达期间不阻塞其余对标）。
- `agents` 创建/运行链、`openai/{chat_id}/chat/completions` 兼容面、`searches/*` 深链 —— 列入后续轮次。
