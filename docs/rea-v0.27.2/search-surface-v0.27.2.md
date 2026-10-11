# Search App 与 share 面（RAGFlow v0.27.2 运行取证）

> REA 轮次⑤产出（2026-10-11）。来源：源码契约 + 16 探针（含净零生命周期）+ 服务端日志 + 前端产物。
> 工作笔记：`~/.cache/rea/ragflow-v0.27.2/notes/05-search-apps.md`；原始证据：`round5-forensics/`。

## 路由

| 路由 | 说明 |
| --- | --- |
| `POST /api/v1/searches` | 创建 Search App（必填 name；返回 `{search_id}`） |
| `GET /api/v1/searches` | 列表（keywords/page/page_size/orderby/desc/owner_ids）→ `{search_apps,total}` |
| `GET/PUT/DELETE /api/v1/searches/{id}` | 详情 / 更新（name+search_config 必填、config 合并）/ 删除 |
| `POST /api/v1/searches/{id}/completion`、`/completions` | SSE 问答（双拼写同 handler） |
| `POST /api/v1/searchbots/{ask,retrieval_test,related_questions,mindmap}`、`GET /searchbots/detail` | share/嵌入面；**仅 AUTH_BETA 认证族** |

## 要点

- **代码语义**：103=越权（"Has no permission for this operation."）、109=未授权（"no authorization"）、101=缺参、102=DATA_ERROR（BETA 拒收复用该码："API key is invalid!"；全表见 `file-commit-surface-v0.27.2.md`）。
- **completion 的 SSE 信封**：`data:{"code":0,"message":"","data":ans}` 逐块；错误 → `{"code":500,…,"data":{"answer":"**ERROR**: …","reference":[]}}`；终止 `{"code":0,"data":true}`。
- **kb 优先级**：`search_config.kb_ids ∨ req.kb_ids`（记录优先）。
- **模型解析缺口（本实例实测）**：search_config 无 `chat_id` 时，`resolve_model_config` 对 None 调 `rsplit("@")` 抛错（`tenant_model_service.py:218`）→ SSE code:500（17.9ms 即时，无默认模型回退）。chat 链有默认注入，search 链没有。
- **认证族维度**：`DEFAULT_AUTH_TYPES=(JWT, API)`；searchbots 走 `AUTH_BETA="BETA"`。
- **前端**：路由 `/searches`（列表页，懒 chunk `index-D0L6fAec.js`）、`/search`、`/search/share`；API 表 createSearch/getSearchList/getSearchDetail/updateSearchSetting/deleteSearch/searchCompletion。

## 生命周期实测（净零）

创建（`rea-probe-tmp`）→ 详情 → 更新（`vector_similarity_weight:0.25` 合并成功）→ completion（即时 code:500）→ 单数拼写别名（101）→ 删除（`data:true`）→ 列表回验 `total:0`。

## 对标增量（待入实施队列）

1. Search App CRUD 全链 + 更新合并语义。
2. completion SSE（信封内嵌 SSE）与双拼写路由。
3. share 面 + 认证族门控（JWT/API/BETA）。
