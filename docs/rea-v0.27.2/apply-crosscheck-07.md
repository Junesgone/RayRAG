# 路由复核 + 设置壳/数据源网格 + 搜索请求体 touched 语义（REA 轮次 18，v0.27.2）

> 轮次：2026-10-21（v0.51.0 切片）
> 基线：RAGFlow v0.27.2 固定树 ~/.cache/ragflow-v0.27.2（commit a024bea0cd93）
> 产物：本文件 + registry「搜索请求体 touched 语义 / user-setting 壳几何」+ 台账 5 行 partial→aligned + 3 个新 CDP 探针

## 1. 本轮发现

1. **routes.tsx 残留已过时**：/chunk 四路由、/chats/widget、/agent/share、/dataflow-result、/admin 企业子页均已注册；/chunk 是完整 chunk workbench（Parsed/Chunk/Result 三视图），/chats/widget 是 FloatingChatWidget 页面，agentbots logs 端点（/api/v1/agentbots/{shared_id}/logs/{message_id}）已存在。台账仍记 partial（v0.3.8p 时代遗留）。
2. **data-source 网格断点错误**：上游用 Tailwind viewport 断点（lg=1024→2 列、2xl=1536→4 列），RayRAG 用 @container 容器查询，容器宽度常 <1024/1536 → 实测 1024/1280 只 1 列、1536/1920 只 2 列，4 列永不出现。上游 v0.27.2 该页没有 per-source search（源码搜索已注释），旧 residual 应撤回。
3. **user-setting 壳几何近似**：RayRAG clamp(200px,21vw,340px) 只在 1440px 视口≈303px；上游是 w-16 md:w-[303px]（<768 → 64px，≥768 → 303px）。
4. **真实请求体 bug**：搜索抽屉滑块 ssTouch('thresh'/'vWeight') 写 __SEARCH_TOUCHED.thresh/vWeight，而 runSearch() 检查 .threshold/.weight → 用户拖动阈值/权重后编辑值从未进入检索请求。探针首轮实证：touched 有 thresh 但 body 无 similarity_threshold。

## 2. 落地（v0.3.10an）

1. ssTouch 增加 canonical 别名（thresh→threshold、vWeight→weight），并加 web 测试标记断言。
2. user-setting 壳：grid-template-columns:303px minmax(0,1fr) + max-width:767px 断点 → 64px（与 upstream md 断点一致）。
3. data-source 网格：@container → @media (min-width:1024px) 2 列 / (min-width:1536px) 4 列；测试 pin 的规则串同步更新。
4. 新增 3 个 CDP 探针：
   - data-source-grid-cdp.mjs：6 档宽度网格列数 + hover Add + 无搜索框 + 0 pageError；
   - user-setting-shell-cdp.mjs：303/64 宽、7 页 aria-current 唯一且匹配、back/forward active 同步；
   - search-hooks-cdp.mjs：search_id 未触碰请求不覆盖 app 配置、拖动滑块后请求带 similarity_threshold=0.73。

## 3. 证据

- 探针（debug 实例 :9490，probe 账号）：
  - data-source grid → 390:1 / 768:1 / 1024:2 / 1280:2 / 1536:4 / 1920:4，cardCount 40（35 上游 + 5 本地扩展），hasSearchInput=false，hoverAdd opacity 1；
  - user-setting shell → desktopWidth 303、mobileWidth 64、7 行 activeKeys 各自唯一、backActive=/model、forwardActive=/profile；
  - search hooks → untouched body {question,highlight,page,size,include_metadata,search_id}；touched body 追加 similarity_threshold:0.73；两请求 0 pageError。
- 路由：31 条关键路径 curl 全 200（含 /chunk 四路由、/chats/widget、/agent/share、/agent/{id}/explore、/dataflow-result、/admin 子页）；ui-route-crosscheck 文档 51 探针记录可复核。
- 全量 cargo test --locked：2290 passed / 0 failed / 27 ignored（+集成 6）。
- 台账：routes.tsx、user-setting/index、user-setting/sidebar/hooks、next-search/hooks、data-source/index 共 5 行 partial→aligned；**partial 11→6**。

## 4. 在案（下轮）

- 剩余 partial 6 行：model-tree-select（跨页 consumer）、tree-select（Radix 视觉 focus）、admin/sandbox-settings（E2B/Aliyun 真实执行）、next-search/search-view（answer/mind-map 需可达 chat model）、data-source box/gmail token 字段（真实 OAuth 兑换/逐 provider popup）。
- search-related-cdp.mjs 等旧脚本仍按 v0.24 侧栏选择器编写（#relatedSearch 在 v0.27.2 已移入抽屉），未用于翻正；如需继续复核需按抽屉重写。
- routes.tsx 已 flip，但 ui-route-crosscheck 文档建议的 /user-setting/locale 裁决仍可记录。
