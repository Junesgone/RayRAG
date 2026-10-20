# Search AI summary / mind map 端到端（REA 轮次 19，v0.27.2）

> 轮次：2026-10-21（v0.52.0 切片）
> 基线：RAGFlow v0.27.2 固定树 ~/.cache/ragflow-v0.27.2（commit a024bea0cd93）
> 产物：本文件 + registry「Search AI summary / mind map 端到端」+ 台账 2 行 partial→aligned + mock model server 扩展 + CDP 探针修复

## 1. 本轮发现

1. search-view 残留「answer text/mind-map generation needs a reachable chat model」可从环境层解掉：用 mock OpenAI 兼容模型（/v1/chat/completions，含 stream SSE）配置 tenant 默认 chat 模型后，AI summary 与 mind map 都能真实跑通。
2. **真实 panic bug**：search app 的流式补全桥（search_complete）用 tokio::sync::mpsc::channel(16) + blocking_send，回调在 async runtime 内执行 → 首个流式 chunk 即 panic（"Cannot block the current thread from within a runtime"），前端只能显示 "The AI summary is unavailable"。改 unbounded channel 后 SSE（chunk → references → answer → [DONE]）正常。
3. **页面级 ReferenceError**：/search?sapp=... 载入时 applySapp 在第二段脚本定义 searchRerankToggle 之前调用它 → ReferenceError: searchRerankToggle is not defined（同时中断 applySapp，导致 mindmap 开关未恢复、脑图按钮隐藏）。加 typeof 守卫后 0 pageError。
4. tree-select 残留「Radix visual focus styling approximated」在 v0.27.2 已不适用：上游 tree-select 的行是普通 div（Popover 内 renderTree），只有 hover:bg-accent / selected bg-accent text-accent-foreground，没有 Radix Tree、没有 focus-visible/ring；RayRAG 的 roving tabindex/方向键/typeahead 是无障碍超集。residual 撤回。

## 2. 落地（v0.3.10ao）

1. search_complete 流式桥改 unbounded_channel + UnboundedReceiverStream，回调改 tx.send（不再 blocking_send）。
2. applySapp 的 searchRerankToggle() 调用加 typeof === 'function' 守卫；web 测试标记断言。
3. mock model server（scripts/ui-traversal/mock-compilation-model-server.py）扩展：
   - RAG chat system prompt（"Use the context to answer questions" / "Relevant document context"）→ 返回 grounded 答案；
   - related questions prompt → 3 条编号问题；
   - knowledge-mapping prompt → 合法 mind map JSON；
   - stream:true 时按 OpenAI SSE 帧输出 chunk + [DONE]。
4. search-answer-cdp.mjs 修整：凭证支持 env；提交只点一次（此前第二次点击在流式态会变成 Stop，取消 summary 请求）。
5. 配置 tenant 默认 chat 模型（PUT provider instance + PATCH /api/v1/models/default）与探针数据集（upload→自动 parse/embed）后，CDP 全链路通过。

## 3. 证据

- debug 实例（:9490 + mock model :18089）search-answer-cdp.mjs：errors []、pageErrors []；AI summary title "AI summary"、answerLength=101、refs=1、divider=true；mind button visible、mindSheet 7 nodes、root "RayRAG"、close ok；related chips 3、Total: 1、Clear 复原；app 创建/清理成功。
- 手动 curl /api/v1/searches/{id}/completions：chunk 帧×5 → references → answer（101 chars）→ [DONE]。
- 全量 cargo test --locked：2290 passed / 0 failed / 27 ignored（+集成 6）。
- 测试稳定性：oauth_web 的 box/google round-trip 与 flow_registry 测试共享进程级 registry，并行下曾互相 clear；加 FLOW_TEST_LOCK 串行化。
- 台账：next-search/search-view.tsx、components/tree-select.tsx → aligned；**partial 6→4**。

## 4. 在案（下轮）

- 剩余 partial 4 行：model-tree-select（跨页 consumer）、admin/sandbox-settings（E2B/Aliyun 真实执行）、data-source box-token（真实 OAuth 兑换）、data-source gmail-token（真实 token minting/逐 provider popup）。
- search-related-cdp.mjs 仍按旧侧栏写法（#relatedSearch 已入抽屉），本轮未使用；如需复核需按抽屉重写。
- mock model server 的 RAG 答案/mindmap JSON 为验证用占位数据，非真实模型质量。
