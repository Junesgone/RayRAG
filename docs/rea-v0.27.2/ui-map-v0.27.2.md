# UI 地图（RAGFlow v0.27.2：路由 · 懒加载块 · 标签）

> REA 轮次⑧产出（2026-10-12）。来源：前端产物全量提取。
> 工作笔记：`~/.cache/rea/ragflow-v0.27.2/notes/08-ui-map.md`；原始资产：`~/.cache/rea/ragflow-v0.27.2/round8-ui/`（routes.json / lazy-chunks.tsv / locale-en-full.json / locale-zh-full.json / 两份 locale 原始 JS）。

## 侧栏（7 项，v0.27.2）

`首页 / 知识库 / 聊天 / 搜索 / 智能体 / 记忆 / 文件管理`（en: Home / Dataset / Chat / Search / Agent / Memory / File）
Skills 在 文件管理 → `/files/skills`；来源 `src/layouts/components/global-navbar.tsx`（bundle 内嵌源路径）。

## 路由常量（65 个，分组）

- **主导航**：Home `/home` · Datasets `/datasets` · Chats `/chats` · Searches `/searches` · Agents `/agents` · Memories `/memories` · Files `/files` · Skills `/files/skills`
- **datasets 区**：`/dataset` · `/dataset//files` · `/retrieval` · `/knowledge-graph` · `/logs` · `/configuration` · `/chunk` `/chunk/chunk` `/chunk/parsed` `/chunk/result` · `/parsed` · `/result` · `/compilation`
- **chat 区**：`/chat` · `/chats/share` · `/chats/widget`
- **agent 区**：`/agent` · `/agent/:id/explore` · `/agent-list` · `/agent-log-page` · `/agent/share` · `/agent-templates` · `/explore`
- **search/memory/file 区**：`/search` · `/search/share` · `/memory` · `/memory-message` · `/memory-setting` · `/files`
- **user-setting 区**：`/user-setting` · `/profile*` · `/team` · `/model` · `/mcp` · `/data-source(-detail-page)` · `/chat-channel` · `/api`
- **admin 区**：`/admin` · `/admin/monitoring` `/admin/roles` `/admin/sandbox-settings` `/admin/services` `/admin/users` `/admin/whitelist`
- **misc**：`/login-next` · `/logout` · `/prompt` · `/plan`
- 全量 65：`round8-ui/routes.json`。

## 懒加载块（55，新家族节选）

`/searches`→index-D0L6fAec.js · `/search/:id`→index-C0lwxalH.js · `/search/share`→index-CJp1psx3.js · `/memories`→index-3xTvPApR.js · `/memory`→index-DEppQTUH.js · `/memory//memory-message/:id`→index-CLZyTqMq.js · `/memory//memory-setting/:id`→index-DX-urahz.js · `/files`→index-ZfFZXqgt.js · `/files/skills`→index-Cqa44F49.js · `/dataset/compilation/:id`→index-yz7KVwz9.js。全量：`round8-ui/lazy-chunks.tsv`。

## 标签资产（locale）

- en 29 节 / zh 30 节，全量 JSON 已固化（见上路径）。
- 关键映射：
  - `header`：导航文案（zh：首页/知识库/聊天/搜索/智能体/记忆/文件管理）
  - `search`：Search apps / Create search / Search settings / AI summary / Enable web search / Enable related search / Show query mindmap / Embed app
  - `memories`+`memory`：四类记忆定义（raw/semantic/episodic/procedural）+ 配置项（memory size 公式、forget policy、temperature、prompts）+ 消息表 + sideBar Messages/Configuration
  - `skills`：技能空间 / semver / **Version history**
  - `knowledgeCompilation`：模板组 / 四种规格（Entity·Relation·Concept·Claim）/ Wiki·Entity·Topic 模式 / RAPTOR 设置 / **Version history · Commit · Confirm commit**
  - `fileManager`：文件页文案

## 家族 ↔ 页面映射（与 REA 轮次）

| 轮 | 家族 | 页面 |
| --- | --- | --- |
| ⑤ | searches | `/searches` `/search/:id` `/search/share` |
| ⑥ | memories/messages | `/memories` `/memory` `/memory/memory-message/:id` `/memory/memory-setting/:id` |
| ⑦ | file-commits | `/files`（工作区版本）· `/dataset/compilation/:id`（artifact 版本）· `/files/skills`（技能版本） |

## 备注

- bundle 内嵌 `data-inspector-relative-path:"src/…tsx"` → UI 一比一可直接对上游源码文件。
- `/memory//memory-message/:id` 双斜杠为原样提取（上游路由拼写如此）。
