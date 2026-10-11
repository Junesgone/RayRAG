# 应用交叉核对 01 — REA 契约 × RayRAG 实现（v0.27.2）

> REA 轮次⑩产出（2026-10-12 凌晨）。方法：只读核对 RayRAG `src/` 实现与 REA 轮④–⑨ 已固化的上游契约；证据一律给源码位置或提交号。
> 目的：把"分析结果"落到"实现侧的可核销证据"，并修正台账中随批次推进而过期的注记。

## 结论总表

| 面 | REA 证据 | RayRAG 实现（证据） | 结论 |
| --- | --- | --- | --- |
| searchbots / 嵌入面 | 轮⑤：五路由 + AUTH_BETA 认证族（KB key 被拒 102） | `src/server.rs:4861-4867/5133`（detail/retrieval_test/mindmap/ask 路由）；beta token 独立存储 `server.rs:86-94`、`tenant_for_beta` 解析 `server.rs:4303-4314` | **已实现** ✓（BETA 族建模与上游一致） |
| sessions（chat/agent） | 轮⑨：chat 7 路由全契约（prologue 注入、成对删除、feedback 链、orderby 白名单） | `server.rs:5189-5205` chat sessions 全链（含 regenerate）；`server.rs:5755-5774` agent sessions（**POST + 批量 DELETE 已落地：v0.3.8n `286d586`**；处理器 `src/api/agent_log.rs:490/609`） | **已实现** ✓；台账旧「remain open」注记已修正 |
| 文件提交面 | 轮⑦：八端点双挂载契约、错误三态、RetCode 全表 | `src/api/workspaces.rs` 全家族（`a574065` 补审后按真实存储实现：快照/diff/未提交变更/tree/内容/版本史）；双挂载 `server.rs:5263-5318` | **已实现** ✓；**信封边界注记**（见下） |
| memory 面 | 轮⑥：四类记忆、消息复合键、归因策略、提示词资产 | 台账 full（v0.3.3af）；`code::INVALID_OR_MISSING_DATA` 引用 `src/api/features.rs:425/436/581` | 对齐 ✓（提示词资产在库：`docs/rea-v0.27.2/memory-default-system-prompt.txt`） |
| OpenAI 兼容面 | 轮④：两段式错误、SSE、弃用别名 | 台账 aligned（`src/server.rs; src/api/`） | 引用已挂 ✓ |
| RetCode 值 | 轮⑦：上游全表（0/10/100/101/102/103/105/106/108/109 + HTTP 4xx 族） | 抽查：`code::INVALID_OR_MISSING_DATA=102`（`server.rs:1595`）、`101 INVALID_ARGUMENT`、`EXCEPTION_ERROR=100`（`user_register.rs:27`）、`OPERATING_ERROR=103`（`:30`） | 抽查一致 ✓；**全表逐项对照留后续轮** |

## 信封边界注记（文件提交面）

- 上游（轮⑦实测）：`get_data_error_result("Commit not found")` → **HTTP 200** + `{"code":"Commit not found","message":"Sorry! Data missing!"}`（字符串 code 怪癖）。
- RayRAG：`api_error_code(StatusCode::NOT_FOUND, 102, …)`（`src/api/workspaces.rs:377-383`）→ **真实 HTTP 状态** + `code: i32`。
- 定性：RayRAG 响应模型 `code` 为 i32（`server.rs:1607-1617`），结构上无法承载字符串 code。属**系统性归一化**，与台账政策「aligned means audited behavior, not a literal Python-to-Rust transliteration」一致。
- **待决**：是否需要为严格逐字兼容复刻 string-code 怪癖（影响面：仅个别路径；上游前端不依赖）。记入后续决策项。

## 台账动作（本轮）

1. 7 行回写 REA 交叉引用（agent/bot/chat/file_commit/memory/openai/search_api）。
2. `agent_api.py` 行「POST + batch DELETE remain open」陈旧注记修正为 v0.3.8n 已落地。
3. 覆盖校验：`cargo run --locked --bin ragflow-coverage -- --check <checkout>`（本轮启动，结果回填本节）。

## 后续

- RetCode 全表逐项对照；file-commit 信封决策；轮⑧ UI 地图 → 任务-3 CDP 批次输入清单。
