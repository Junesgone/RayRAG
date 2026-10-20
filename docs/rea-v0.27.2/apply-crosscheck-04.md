# 数据集 Artifacts/Wiki 编译面（REA 轮次 15，v0.27.2）

> 轮次：2026-10-20（v0.48.0 切片）
> 基线：RAGFlow v0.27.2 固定树 `~/.cache/ragflow-v0.27.2`（commit `a024bea0cd93`）
> 产物：本文件 + `docs/parameter-interface-registry.md` 的「数据集 Artifacts / Wiki 编译面」节 + 台账 4 行 UI residual 转 aligned + 2 行 full/aligned 补证据

## 1. 复核对象与残留

- 台账 partial：`pages/dataset/compilation/{constants,empty-state,index,llm-wiki-view}`（4 行，旧注记「wiki 生成与深层组件未复刻」）。
- 静态契约：`pages/dataset/compilation/**`（index/empty-state/llm-wiki-view/wiki-left-panel/wiki-detail-*/update-*/hooks/constants）、`web/src/hooks/use-dataset-generate.ts`、`wiki-generation-eligibility.ts`、`api/apps/restful_apis/dataset_api.py`（artifacts + navigation 面）、`api/db/db_models.py::Knowledgebase.pipeline_id`。
- RayRAG 侧：`src/api/dataset_artifacts.rs`（list/topics/page/update/clear/compile/structure/graph/alteration）、`src/api/dataset_navigation.rs`（nav 树）、`src/web.rs` tab5（旧：外壳 + Generate 门控，无 wiki 视图/生成后内容）。

## 2. 本轮发现

1. **生成门控在 RayRAG 永远关闭**：upstream `canGenerateWiki` 要求 `chunk_count>0 && pipeline_id`，但 RayRAG 的 `KnowledgeBase` 结构没有 `pipeline_id` 字段、create/update 也不接受该键 → 数据集 JSON 无此字段 → Generate 永远 disabled。upstream `Knowledgebase` 确有 `pipeline_id = CharField(max_length=32, null=True, index=True)`（`db_models.py:1279`）。
2. **新解析数据集 compile 静默 0 页**：`compile_artifacts` 的 MAP 从搜索 engine 读文档 chunk，但 COMPILE 的 `current` chunk state 只从 doc-store 扫描（`scan_current_chunk_state`）；RayRAG 解析流水线写入 engine（index.json / zvec），未镜像成 doc-store chunk 行 → `load_map_extracts_for_state` 为空 → `pages_created: 0` 且无 error（探针实证：mapped=1/extracts=4/pages_created=0，DB 只有 `wiki_map_extract` 行、无 chunk 行）。
3. **UI 缺失**：tab5 只有外壳 + 空态 + Generate，无视图模式对应内容、无 topics/页面列表、无 markdown 详情、无 Update/Clear、无生成汇总反馈；树/结构视图仅提示 "not available"。

## 3. 本轮落地

1. **`Knowledgebase.pipeline_id` 顶层列**（kb.rs + server create/update）：absent=保持、null/空=清除、字符串=绑定；数据集 JSON 暴露该列；新增 `set_pipeline_id`。
2. **compile 状态修复**（`src/api/dataset_artifacts.rs`）：`current` 在 doc-store 扫描结果上并入 engine chunk 视图（id → {doc_id, content hash}）；新解析数据集一次请求内完成 MAP→COMPILE。
3. **Artifacts tab UI**（`src/web.rs`：`cwp_labels`/`cwp_markup`/`CWP_JS`）：
   - `index.tsx` 外壳：Back + avatar + `{name}{suffix}` + 视图选择器（Wiki / Tree-PageIndex / Graph / Mind map / Timeline）+ loading/empty/workspace 三态；
   - `empty-state.tsx`：5 种空态标题（上游 en/zh 逐字）、Generate 门控与两种缺项提示、编译汇总/失败文案、结构模式 autoCompiled 提示；
   - `llm-wiki-view.tsx`：Contents/Graph 双 tab、Topics→Pages 导航与面包屑、搜索、Update（重编译）/Clear（上游确认文案）、markdown 详情（标题/段落/列表/代码/外链/[[wiki link]] 点击）、来源文档；
   - Tree 模式：navigation 树（cluster→children，doc 详情 description/keywords/entities/graph_content）；
   - 结构模式：structure artifact 列表+详情+autoCompiled 提示。
4. **离线端到端验证通道**：`scripts/ui-traversal/mock-compilation-model-server.py`（OpenAI 兼容 MAP JSON / partition / route / 页面 markdown）+ `scripts/ui-traversal/compilation-artifacts-view-cdp.mjs`（26 项 CDP 断言）。

## 4. 证据

- **真实端到端生成**（debug 实例 :9490 + mock model）：探针数据集 pipeline_id 绑定 → upload + add chunk（chunk_count=1）→ UI 点击 Generate → `Mapped 1 of 1 document(s), extracts 4, pages created 1, modified 0.` → workspace 出现 Topics/Pages → 打开页面 detail 渲染 markdown（h1/p/Overview）→ 搜索过滤 → Graph tab 空态 → Graph/Mindmap/Timeline/Tree 空态标题逐一验证 → Clear（confirm）→ HEAD /artifacts 404 + 空态复原；**26/26 checks、0 pageerror**。
- **单元/集成测试**：`cargo test --locked --lib dataset_` **58 passed / 0 failed**（含新增 `dataset_artifacts_tab_renders_upstream_compilation_shell`、`dataset_artifacts_labels_cover_markup_and_locales`、`dataset_pipeline_id_is_a_top_level_column`）。
- **静态契约**：`web/src/pages/dataset/compilation/*`、`wiki-generation-eligibility.ts`、`api/db/db_models.py`、`api/apps/restful_apis/dataset_api.py`、`web/src/hooks/use-dataset-generate.ts`。

## 5. 在案差异（未随批）

- 生成是同步请求（RayRAG 无 upstream Go 调度任务/进度环；上游以 `useDatasetGenerate` + status 轮询展示百分比与暂停）。UI 以 "Compiling…" 忙碌态 + 完成后汇总替代，未伪造百分比/暂停。
- `meta.json`/页面版本历史（`wiki-commit-modal`/`version-history-*`/`export`）仍为台账 reference 组件；`PUT artifacts/{type}/{slug}` 已存在但本轮未接编辑/提交 UI。
- 结构视图（graph/mindmap/timeline）为 Rust-native 列表+详情管理面（对应组件在台账为 reference），非 1:1 图渲染。
- upstream `create-directory` 本身是 TODO stub（按钮 hidden），未实现。
