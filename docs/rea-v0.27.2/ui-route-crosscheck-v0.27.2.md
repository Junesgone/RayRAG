# UI 路由对账 — 上游 v0.27.2 × 遍历轨道 × RayRAG（任务-3 输入）

> REA 轮⑪产出（2026-10-12）。输入：轮⑧ UI 地图资产 + `web/src/routes.tsx`@`a024bea` + 双实机探针（RayRAG `:9390` 服务端语义；上游 `:11526` Chromium 浏览器实测）。
> 服务对象：任务-3（CDP 实点批次的遍历轨道与路由收敛）。

## 1. 导航形式实测（浏览器，上游 vs RayRAG）

| URL 形式 | 上游 v0.27.2（浏览器实测） | RayRAG（实机） | 对账 |
| --- | --- | --- | --- |
| `/dataset/files/{id}` | ✓ 渲染（数据集 Files 页） | `200` ✓ | **一致（规范形式）** |
| `/dataset/{id}` | 未实测（预计 404 页） | `307` → `/dataset/files/{id}` | RayRAG 容差（超集） |
| `/dataset//files/{id}` | **404 页**（React Router 不匹配） | `404` | **一致**。上游源码常量 `Dataset = ${DatasetBase}/${Files}` 会拼出 `//` 形式——实机不可用，属上游自身代码现象 |
| `/dataset/{id}/files` | **404 页** | `404`（段序不符） | **一致** |

> 上游为 SPA：curl 对任意路径一律 200，路由行为须浏览器判别；上表上游列均为 Chromium 实测（会话内已登录）。

## 2. RayRAG 实机覆盖（51 项探针，2026-10-12）

**200 ✓（37 项）**：
`/login` `/login-next` `/datasets` `/chats` `/chats/share` `/chats/widget` `/searches` `/search/{id}` `/search/share` `/agents` `/agent-list` `/agent-templates` `/agent/{id}/explore` `/agent/share` `/agent-log-page/{id}` `/memories` `/memory/memory-message/{id}` `/files` `/files/skills` `/compilation-templates/edit-next` `/dataflow-result` `/chunk` `/chunk/parsed/chunks` `/chunk/chunk/{id}` `/chunk/result/{id}` `/dataset/compilation/{id}` `/dataset/files/{id}` `/dataset/retrieval/{id}` `/user-setting/profile` `/user-setting/team` `/user-setting/api` `/user-setting/mcp` `/user-setting/model` `/user-setting/data-source` `/user-setting/chat-channel` `/user-setting/data-source/data-source-detail-page` `/admin`

**307（语义正确，8 项）**：
`/user-setting` → `/user-setting/model`（上游同款重定向）；`/admin/services|users|sandbox-settings|whitelist|roles|monitoring` 未认证 → `/admin`（上游 `AdminAuthorizedLayout` 守卫的 server 端移植）；`/dataset/{id}` → 规范形式。

**404（语义正确或与上游一致，6 项）**：
`/document/{unknown}`（未知文档真 404，v0.3.5x 已审）；`/dataset//files/{id}`、`/memory//memory-message/{id}`、`/memory//memory-setting/{id}`（`//` 形式，与上游实测一致）；裸 `/dataset`（上游未实测，低影响备查）；`/memory/{id}`（上游无该挂载形式）。

## 3. 遍历轨道（`scripts/ui-traversal/traverse.mjs`）增补建议

当前静态清单 34 条。对照上游挂载（`routeConfigOptions`）建议增补 **4 条静态形式**（并入任务-3 下批执行）：

- `/login-next`（上游双入口，同一页面）
- `/chunk`、`/chunk/parsed/chunks`（chunk 子树静态面）
- `/user-setting/data-source/data-source-detail-page`

**复核项**：`/user-setting/locale` 在上游源码中已注释关闭（v0.27.2），而遍历清单仍在——下批一并裁决（移除或注明）。

**参数化二遍（真实 id）建议集合**：
`/document/{id}`、`/chat/{id}`、`/search/{id}`、`/dataset/files/{id}`、`/dataset/retrieval|logs|configuration|knowledge-graph/{id}`、`/dataset/compilation/{id}`、`/agent/{id}/explore`、`/agent-log-page/{id}`、`/memory/memory-message|memory-setting/{id}`。

## 4. 结论

1. RayRAG 页面路由对上游**可导航面全覆盖**（51 项探针全部通过或语义正确）；两侧对 `//` 与段序错排均 404，形式容忍度一致。
2. 上游 `Dataset` 常量拼串怪癖（源码 `//`、实机 404）——仅记录，**无需复刻**。
3. 打包给任务-3：遍历增补 4 条 + `/user-setting/locale` 裁决 + 参数二遍集合。
