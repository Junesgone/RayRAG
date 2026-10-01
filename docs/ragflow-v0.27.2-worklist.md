# RAGFlow v0.27.2 Parity Worklist（内部工作清单，禁止发布）

> 来源：`docs/ragflow-v0.27.2-coverage.md`（固定 commit `a024bea0cd93f39e6652a42bf84dd20c55bc560b`，5977 文件）。
> 生成：`scripts/gen-parity-worklist.py`（v0.3.10g 起）；台账状态变更后重跑刷新。
> 用途：v0.27.2 对标收敛的有序工作队列；每批完成后回写台账并刷新本清单。

## 现状摘要

- 文件总数：5977；unmapped=0（v0.3.10g 全量分类完成）
- partial：81（UI 残留 42 + 实施队列 39）
- re-audit：440（上游内容变更、需复核对齐的已审计行）
- replaced / N/A / reference：无逐文件动作（基础设施替代与审计参考材料）

## 实施队列

### W-1 搜索/提供商接入（0 项）

| 文件 | 行数 | RayRAG 对应 | 台账注记 |
|---|---:|---|---|

### W-2 数据源接入（0 项）

| 文件 | 行数 | RayRAG 对应 | 台账注记 |
|---|---:|---|---|

### W-3 OCR/视觉解析器（0 项）

| 文件 | 行数 | RayRAG 对应 | 台账注记 |
|---|---:|---|---|

### W-4 Agentic harness 移植（最大队列）（27 项）

| 文件 | 行数 | RayRAG 对应 | 台账注记 |
|---|---:|---|---|
| `rag/advanced_rag/agentic_rag.py` | 995 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：v0.27.2 Agentic-RAG 能力层（RAGTools 检索原语）；RayRAG 有多跳检索与画布智能体运行时，agentic 编排层待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/agentic_rag_graph.py` | 1766 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：agentic-search 图编排（1766 行）；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/__init__.py` | 1 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：agentic harness 包；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/action_session.py` | 2142 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：动作会话执行器（2142 行）；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/arithmetic.py` | 420 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：算式求值工具；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/chunk_utils.py` | 55 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：harness 分块工具；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/config.py` | 133 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：harness 配置；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/grep_sed_narrow.py` | 449 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：窄域 grep/sed 文本检索；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/keywords.py` | 169 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：加权关键词抽取；RayRAG 关键词工具部分就位，抽取语义待对齐——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/memory.py` | 390 | `src/advanced_rag.rs; src/agent.rs; src/memory.rs` | （v0.3.10g 审计：harness 会话记忆；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/orchestrator/__init__.py` | 12 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：orchestrator 包；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/orchestrator/direct.py` | 60 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：直接回答编排；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/orchestrator/query_rewriter.py` | 98 | `src/advanced_rag.rs; src/agent.rs; src/prompts.rs` | （v0.3.10g 审计：查询改写编排；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/orchestrator/sufficient_context.py` | 419 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：上下文充分性判定；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/prompts/__init__.py` | 1 | `src/advanced_rag.rs; src/agent.rs; src/prompts.rs` | （v0.3.10g 审计：harness 提示词包；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/prompts/report_prompt.py` | 33 | `src/advanced_rag.rs; src/agent.rs; src/prompts.rs` | （v0.3.10g 审计：报告提示词；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/stats.py` | 448 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：LLM 用量统计（CountingChatModel）；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/structure_qa.py` | 102 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：结构化 QA；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/tools/__init__.py` | 20 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：harness 工具包；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/tools/compiled_expansion.py` | 514 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：编译扩展工具；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/tools/exploration.py` | 468 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：探索工具；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/tools/navigation.py` | 1936 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：导航工具（1936 行）；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/tools/search.py` | 685 | `src/advanced_rag.rs; src/agent.rs; src/search.rs` | （v0.3.10g 审计：harness 搜索工具；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/harness/tools/text_processing.py` | 467 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：文本处理工具；待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/knowlege_compile/runner.py` | 750 | `src/structure_compile.rs` | （v0.3.10g 审计：知识编译 runner（服务集成）；RayRAG 编译管线在 src/structure_compile.rs，runner 集成待对齐——worklist W-4（agentic harness）） |
| `rag/advanced_rag/knowlege_compile/wiki_incremental.py` | 5004 | `src/structure_compile.rs` | （v0.3.10g 审计：wiki 增量编译（5004 行）；RayRAG wiki 管线基础在 structure_compile.rs，增量语义待移植——worklist W-4（agentic harness）） |
| `rag/advanced_rag/think_log.py` | 95 | `src/advanced_rag.rs; src/agent.rs` | （v0.3.10g 审计：思考日志；待移植——worklist W-4（agentic harness）） |

### W-5 结构图/编译器/切分（10 项）

| 文件 | 行数 | RayRAG 对应 | 台账注记 |
|---|---:|---|---|
| `agent/templates/compiler.json` | 447 | `src/agent_templates.rs; src/structure_compile.rs` | （v0.3.10g 审计：DSL-only Compiler 流程模板；RayRAG 编译模板流程就位，该内置模板未接入——worklist W-5（结构图/编译器/切分）） |
| `api/apps/services/structure_graph_common.py` | 598 | `src/graphrag_enhanced.rs; src/hypergraph.rs` | （v0.3.10g 审计：结构图子图采样服务；RayRAG 图谱管线部分就位，采样端点待对齐——worklist W-5（结构图/编译器/切分）） |
| `api/channels/targets.py` | 34 | `src/server.rs; src/agent_templates.rs` | （v0.3.10g 审计：渠道智能体目标校验；RayRAG 已有 agent_canvas 类别校验，渠道目标助手待对齐——worklist W-5（结构图/编译器/切分）） |
| `api/db/services/document_counter_service.py` | 78 | `src/api/document.rs; src/server.rs` | （v0.3.10g 审计：重解析计数器回滚；RayRAG 重解析路径存在，计数器释放助手待对齐——worklist W-5（结构图/编译器/切分）） |
| `rag/flow/chunker/_sentence_boundary.py` | 29 | `src/chunk/` | （v0.3.10g 审计：标题/令牌分块共享句界定义；RayRAG 分块器在 src/chunk/，共享句界定待统一——worklist W-5（结构图/编译器/切分）） |
| `rag/flow/compiler/__init__.py` | 18 | `src/structure_compile.rs; src/raptor.rs` | （v0.3.10g 审计：Compiler 流程包；RayRAG 编译模板/管线部分就位（raptor、structure_compile），compiler 流程待对齐——worklist W-5（结构图/编译器/切分）） |
| `rag/flow/compiler/compiler.py` | 711 | `src/structure_compile.rs; src/raptor.rs` | （v0.3.10g 审计：Compiler 主流程（711 行）；待对齐——worklist W-5（结构图/编译器/切分）） |
| `rag/flow/compiler/schema.py` | 35 | `src/structure_compile.rs` | （v0.3.10g 审计：Compiler schema；待对齐——worklist W-5（结构图/编译器/切分）） |
| `rag/nlp/delim.py` | 179 | `src/parser/` | （v0.3.10g 审计：parser_config.delimiter 统一解析；RayRAG 各解析器内联处理，统一化待对齐——worklist W-5（结构图/编译器/切分）） |
| `rag/svr/task_executor_refactor/dataset_structure_merger.py` | 1027 | `src/graphrag_enhanced.rs; src/structure_compile.rs` | （v0.3.10g 审计：数据集级结构图合并任务（增量/分桶）；待移植——worklist W-5（结构图/编译器/切分）） |

### W-6 沙箱提供商与鉴权（2 项）

| 文件 | 行数 | RayRAG 对应 | 台账注记 |
|---|---:|---|---|
| `agent/sandbox/providers/tenki.py` | 521 | `src/sandbox.rs; src/code_exec.rs` | （v0.3.10g 审计：Tenki 沙箱提供商；RayRAG 沙箱提供商框架就位（aliyun 已移植），该提供商待接入——worklist W-6（沙箱提供商/鉴权）） |
| `agent/sandbox/providers/ucloud_agent_sandbox.py` | 471 | `src/sandbox.rs; src/code_exec.rs` | （v0.3.10g 审计：UCloud 沙箱提供商；待接入——worklist W-6（沙箱提供商/鉴权）） |

## UI 对齐残留（42 项，任务-3 的 CDP 实点批次收敛）

| 文件 | 行数 | RayRAG 对应 | 台账注记 |
|---|---:|---|---|
| `web/src/components/model-tree-select.tsx` | 309 | `src/web.rs; src/web_css.txt; src/api/tenant_models.rs` | Rust Model providers now builds the fixed provider→instance→model selector tree in catalog order, filters all six capabilities, normalizes `__` names, deduplicates exact selectors, defaults branches open, renders selected model/instance, and persists canonical default wires. Isolated Chrome real clicks cover every selector, branch/search/No data, leaf save, clear, Escape/outside, rollback and reload. Fixed provider image assets and every non-Model-providers consumer of this shared React component remain pending (v0.3.3bn). First componentization step landed: the trigger+popover+search+`role='tree'` markup is now the standalone reusable `web::push_default_model_selector(&mut String, &DefaultModelField)` instead of inline `providers_page` code; proven output-neutral by byte-diff of the deployed `/user-setting/model` page (321502 B before == after). Cross-page consumers still blocked on extracting the tree JS (`modelField`/`defaultModelState`/`toggleDefaultModelTree`/`searchDefaultModelTree`), which remains providers-page-local (v0.3.4b). v0.3.4f: the behaviour script is extracted to module consts `DEFAULT_MODEL_TREE_JS_HEAD`/`DEFAULT_MODEL_TREE_JS_BODY` (byte-identical construction proof; providers page output unchanged) — cross-page consumers unblocked. v0.3.4g: first consumer landed — the kb/dataset configuration embedding field wires the shared markup + behaviour script with a form persist hook (`saveModelDefault`), a catalog loader (`loadKbModelCatalog`) and a stored-value normalizer (server embd_id round-trips as the internal provider/instance/model id); CDP real-click verified with save and reload persistence (v0.3.4g); v0.3.4j: Radix keyboard parity on the shared selector — ArrowDown/ArrowUp wrap navigation, Home/End jumps, typeahead by title prefix and Enter-select, with scrollIntoView({block:'nearest'}) on the focused row; CDP keyboard flow verified (ArrowDown x2, then 'q' typeahead, End, Enter, save, reload) (v0.3.4j); v0.3.4p: page-local saveModelDefault typeMaps drop reranking_id (retrieval-test rerank is form-local per upstream testing-form) with a form-local fallback for unmapped types, and check_model_available validates the tenant instance's own model rows before the factory gate so tenant-local providers can persist defaults — CDP real-click verified (composite pick, retrieval body rerank_id, no default PATCH); v0.3.4q: memory-model-form consumer landed on the shared selector (embd/llm form-local save), CDP real-click verified end-to-end [v0.27.2 content changed: re-audit] |
| `web/src/components/tree-select.tsx` | 319 | `src/web.rs; src/web_css.txt` | Rust Model providers has a native button/dialog/tree slice with default expansion, branch-only toggles, recursive case-insensitive Search, search-path expansion, leaf-only selection, optional clear, outside/Escape close, retained Search, focus return and ARIA tree/group state. It deliberately keeps matching-parent descendants selectable instead of reproducing the fixed source's empty-branch pseudo-leaf bug. v0.3.7w: the shared selector now implements the Radix keyboard contract (Arrow Right expands/enters, Arrow Left collapses/returns to the parent, a one-second typeahead buffer that wraps, roving `tabindex` so the tree is a single tab stop, and `aria-level` per row); exact Radix *visual* focus styling remains approximated. Markup layer de-duplicated first: `web::push_default_model_selector` is now a standalone function shared by all six default-model fields (previously inline), byte-identical deployed HTML verified; v0.3.7w: the shared behaviour script carries the Radix keyboard contract (arrow expand/collapse, buffered typeahead, roving tabindex); exact Radix visual focus styling remains approximated (v0.3.4b). v0.3.4f: the behaviour script is now shared via `DEFAULT_MODEL_TREE_JS_HEAD`/`DEFAULT_MODEL_TREE_JS_BODY` module consts (byte-identical extraction). v0.3.4g: the kb/dataset configuration embedding field consumes it with a form persist hook + catalog loader + stored-value normalizer; CDP-verified save + reload persistence (v0.3.4g); v0.3.4j: Radix keyboard parity landed on the shared selector body (ArrowDown/ArrowUp/Home/End/typeahead/Enter-select + scrollIntoView) and verified by the kb CDP keyboard flow (v0.3.4j); v0.3.4p: same seam — form-local retrieval-test rerank (no tenant-default PATCH) and entity-first default-model validation; CDP real-click verified; v0.3.4q: memory-model-form consumer landed (embd/llm form-local save), CDP real-click verified end-to-end [v0.27.2 content changed: re-audit] |
| `web/src/pages/admin/layouts/authorized-layout.tsx` | 13 | `src/server.rs; src/web.rs` | v0.3.5y: every console page resolves the presented bearer/cookie token server-side and answers a 307 back to `/admin` when it is missing or belongs to a non-administrator, which is the upstream `AdminAuthorizedLayout` guard (`!authorization \|\| !userInfo -> Navigate to Routes.Admin`) moved from the client to the server; v0.3.8e: the cache half of upstream's `!!authorizationUtil.getAuthorization() && userInfo` condition is now written and cleared by the console (the `userInfo` key), the gate itself stays server-side because RayRAG renders each console page from the request token |
| `web/src/pages/admin/layouts/navigation-layout.tsx` | 154 | `src/web.rs; src/web_css.txt` | v0.3.5y: the console renders the upstream full-screen `main` + 288 px `aside`: logo (`size-8 mr-5`) + `admin.title`, the three community links in source order (`admin.serviceStatus` / `admin.userManagement` / `admin.sandboxSettings`) with the lucide `ServerCrash`/`UserCog`/`Zap` path data at `size-[1em]`, `px-4 py-3 rounded-lg` with the `bg-bg-card text-text-primary` active state (plus `aria-current`), the `admin.version` line with the ThemeSwitch, and the `header.logout` button bound to `GET /api/v1/admin/logout`. The enterprise-only whitelist/roles/monitoring entries stay unregistered exactly like the upstream `IS_ENTERPRISE` guard |
| `web/src/pages/admin/layouts/root-layout.tsx` | 55 | `src/web.rs; src/api/admin.rs` | v0.3.5y: the console shell plays the role of the React context provider -- each page is rendered inside the navigation layout, and the authenticated identity comes from the request token rather than a localStorage-cached user object. v0.3.8e: the localStorage `CurrentUserInfo` cache and its `source` discriminator are ported — `ADMIN_USER_INFO`/`adminSetUserInfo`/`adminCurrentUserInfo` mirror the context (localStorage cache → `source: 'localStorage'`, login payload → `source: 'serverRequest'`, otherwise `{userInfo: null, source: null}`), the console login writes the `Authorization`/`token`/`userInfo` trio as `{...loginData, name: nickname}`, logout clears it with `{null, null}`, and `pages/admin/users.tsx`'s own-row check reads `userInfo?.email` with the request token as fallback |
| `web/src/pages/admin/login.tsx` | 257 | `src/web.rs; src/web_css.txt; src/api/admin.rs; src/server.rs` | v0.3.5y: `/admin` is now the upstream console **login** (`pages/admin/login.tsx`) instead of the old RayRAG dashboard -- the shared `login-next` neon background and three spotlights, the logo + `RAGFlow` brand row, the `admin.loginTitle` heading (`Admin console`, English in both languages because `locales/zh.ts` has no `admin:` block), the Card with `login.emailLabel`/`emailPlaceholder`/`passwordLabel`/`passwordPlaceholder`/`rememberMe` and the full-width `login.login` submit, the centred ThemeSwitch below the card, and the `POST /api/v1/admin/login` flow that stores the token and navigates to `/admin/services` (signed-in administrators skip the card through a 307). v0.3.8f: the residual closes — the three `Spotlight` layers (opacity 0.4/0.3/0.3, `rgb(128, 255, 248)`, coverage 60/12/12), the upstream head/column geometry (`mt-12 ml-12` brand, `mt-[6.5rem] text-4xl font-medium mb-12` title, `max-w-[540px] mt-72 mb-48`), the card split into `px-10 pt-14 pb-10` content + `px-10 pt-8 pb-14` footer, the zod `FormSchema` messages (`min(1)` → the two placeholders, `.email()` → `Invalid email`) as per-field `FormMessage` slots that gate the request, the remember-me checkbox with its label tint, and the `variant="highlighted"` submit with its `loading` spinner. Ledger correction: this file has **no** QR/SSO block — it ends at the ThemeSwitch; that section belongs to `login-next/index.tsx` |
| `web/src/pages/admin/sandbox-settings.tsx` | 1048 | `src/web.rs; src/api/system_settings.rs` | v0.3.5y: `/admin/sandbox-settings` carries the upstream title and description (`Configure your code execution sandbox provider. The sandbox is used by the Code component in agents.`) and hosts the durable sandbox variables table (name/type/value + per-row Save over `/api/v1/system/variables`) that the local executor reads. v0.3.7r: the upstream provider surface is implemented — `/api/v1/admin/sandbox/{providers,providers/{id}/schema,config,test}` carry the exact five-provider registry and per-provider schemas, the page renders the provider grid, the schema-driven configuration card and the connection-test modal, and the probe runs the upstream source in the local sandbox or through sandbox-executor-manager. v0.3.8j: **the SSH provider now executes code for real** — `src/ssh_exec.rs` implements upstream `SSHProvider`'s field set, either credential, the configured `python_bin`/`node_bin` and the `work_dir` workspace, running the snippet through the system OpenSSH client (`ssh`, password auth via `sshpass -f`) and returning `{success, message, details{exit_code, execution_time, stdout, stderr}}`; the runtime image gains `openssh-client` and `sshpass`. **Remaining semantic difference**: the E2B and Aliyun providers store configuration and validate it, but *code execution* through them is still not implemented in this build (the test endpoint reports that instead of faking a pass); the durable variables card is a RayRAG addition (upstream has no such table on this page). [v0.27.2 content changed: re-audit] |
| `web/src/pages/admin/service-status.tsx` | 467 | `src/web.rs; src/api/admin.rs; src/server.rs` | v0.3.5y: `/admin/services` renders the upstream cards and table -- `admin.serviceStatus` title, the service-type filter with `admin.all` and `admin.reset`, and the `ID / Name / Service type / Host / Port / Status / Actions` columns fed by `GET /api/v1/admin/services` (the five components RayRAG runs: `ragflow_server`, `meta_data`/PostgreSQL, `retrieval`/zvec, `file_store`, `task_executor`), a `Show details` action bound to `GET /api/v1/admin/services/{id}` and the `admin.extraInfo` dialog with the upstream `Close` footer. v0.3.7h: the card footer's `RAGFlowPagination` and the toolbar's global search are ported — the table filters by service type **and** a search term over id/name/service_type/host/port, then paginates with `Total N` + page size 10/20/50/100, and `admin.reset` clears both. Ledger corrections: upstream sets `enableSorting: false` on the table (so the tanstack sorting in the note was never reachable) and its `useQuery` has no `refetchInterval` (no auto-refresh polling) — both residuals withdrawn. v0.3.7i: the extra-info dialog renders the payload through a `JsonView`-style typed tree (`adminJsonValue`: `{n}`/`[n]` collapse toggles with `aria-expanded`, per-type colours for string/number/boolean/null, monospace indentation) instead of a JSON string, closing that residual.\| v0.3.7v: the row actions are the upstream hover icons (`ClipboardList` → service details, `Settings2` → extra information) and the details dialog switches between `TaskExecutorDetail` and `ServiceDetail`, with the `admin.taskExecutorDetail` / `Service {{name}} detail` titles. |
| `web/src/pages/admin/user-detail.tsx` | 451 | `src/web.rs; src/api/admin.rs` | v0.3.5y: `/admin/users/{id}` shows the account summary (ID / Email / Nickname / Role / Status / Created / Last login) from `GET /api/v1/admin/users/{username}` plus the status, password and delete actions, and answers `User not found` for unknown ids. v0.3.7s: the upstream assets are complete — `GET /api/v1/admin/users/{username}/datasets` and `/agents` follow `UserServiceMgr.get_user_datasets`/`get_user_agents` (agent rows carry `title`/`permission`/`canvas_category` with only the first `_` segment/`avatar`), the detail payload is the upstream *list* read as `data.data[0]` with the upstream field set, and the page fetches the three endpoints in parallel and paginates both asset tabs. v0.3.8d: the asset tables land on the upstream *single-column* rendering — the `TableHeader` block is commented out upstream, so each row is just the avatar+name cell and the extra columns (language/permission/canvas_category/…) stay commented out; the empty state is `TableEmpty` (one cell, `h-24 text-center`, `common.noResults`), the tabs are the bordered pills that fill `bg-bg-card` when active, and the way back is the outline button with `LucideArrowLeft`. [v0.27.2 content changed: re-audit] |
| `web/src/pages/admin/users.tsx` | 831 | `src/web.rs; src/api/admin.rs; src/web_css.txt` | v0.3.5y: `/admin/users` renders the upstream table (`admin.email` / `admin.nickname` / `admin.role` / `admin.status` / `admin.userType` / `admin.actions`) with the role and status filters and the `admin.reset` button, per-row `Enable`/`Disable` (`PUT /api/v1/admin/users/{username}/activate`), `Change password` (`.../password`), `Delete` (with the upstream confirmation copy) and the `admin.newUser` dialog posting `{username,password,role}`; the email cell links to `/admin/users/:id`. v0.3.7j: the global search box, the sortable headers (this file keeps sorting enabled, unlike `service-status.tsx`) and the footer `RAGFlowPagination` are ported — the table filters over email/username/nickname/role/status, sorts by Email/Nickname/Role/Status with an ↑/↓ marker and paginates with `Total N` + 10/20/50/100 per page; `admin.reset` also clears the search term and the sort state. v0.3.7k: the status cell now matches upstream — the signed-in admin's own row keeps an Active/Inactive badge while other rows use an active/inactive select that saves immediately (the actions cell no longer carries Enable/Disable; the page inlines `ADMIN_ME` from the request token). Ledger corrections: this file has no Excel import dialog (repo-wide grep finds none), so that residual is withdrawn, and the inline role dropdown is enterprise-only (`enabled: IS_ENTERPRISE`), matching how the navigation row already treats enterprise entries. v0.3.8c: the remaining two cells land — the `admin.userType` column becomes the upstream normal/superuser `Select` (`grantSuperuser` `PUT` / `revokeSuperuser` `DELETE` on `/admin/users/{u}/admin`, secondary `admin.superuser` badge on the signed-in administrator's own row) and the actions cell becomes the hover-revealed lucide trio (`ClipboardList` → account page, `UserLock` → change password, danger `Trash2` → delete, the last two off-self only); the delete modal is re-laid out to the upstream `DialogTitle` + `deleteUserConfirmation` description + bordered account box [v0.27.2 content changed: re-audit] |
| `web/src/pages/dataset/compilation/constants.ts` | 47 | `src/web.rs` | （v0.3.10d 复核：数据集 Artifacts 页签外壳/视图选择器/空态/Generate 门控已在 src/web.rs 实现并经真点验证；wiki 生成与深层组件未复刻） |
| `web/src/pages/dataset/compilation/empty-state.tsx` | 146 | `src/web.rs` | （v0.3.10d 复核：数据集 Artifacts 页签外壳/视图选择器/空态/Generate 门控已在 src/web.rs 实现并经真点验证；wiki 生成与深层组件未复刻） |
| `web/src/pages/dataset/compilation/index.tsx` | 79 | `src/web.rs` | （v0.3.10d 复核：数据集 Artifacts 页签外壳/视图选择器/空态/Generate 门控已在 src/web.rs 实现并经真点验证；wiki 生成与深层组件未复刻） |
| `web/src/pages/dataset/compilation/llm-wiki-view.tsx` | 117 | `src/web.rs` | （v0.3.10d 复核：数据集 Artifacts 页签外壳/视图选择器/空态/Generate 门控已在 src/web.rs 实现并经真点验证；wiki 生成与深层组件未复刻） |
| `web/src/pages/dataset/testing/testing-form.tsx` | 159 | `src/web.rs; web/static/` | Rust test panels (kb_detail + dataset) carry the full upstream form contract: query input, Top-K/similarity/vector sliders with live value spans, and RerankFormFields via the shared `push_default_model_selector(TEST_RERANK_SELECTOR_FIELD)` tree selector (data-testid `default-model-reranking_id`, accepted types `reranking`), zh/en tooltip copy through html_escape; runTest/testSearch send `body.rerank_id=` when a model is selected. 49 web tests green incl. reranking_id assertions (v0.3.4o). v0.3.7z: the React composition is implemented — the question textarea with the disabled-until-filled Run button, the `TopSelectFormItem` size picker (`common.top`), the conditional rerank `top_k` slider and the `MetadataFilter` blocks (method picker, manual condition rows with the 14 `SwitchOperatorOptions`, semi-automatic key rows fed by `/datasets/metadata/flattened`) all render and drive the request body. v0.3.8l: that residual is closed — `auto` and `semi_auto` now generate their conditions with the tenant chat model (`metadata_filter::gen_meta_filter`), so the panel's three methods all reach the backend; verified end-to-end against a stub chat model (only the generated condition's document came back). v0.3.4p: removed the accidental tenant-default PATCH for this field (reranking_id was mapped in the shared page-local saveModelDefault typeMap, contradicting this file's not-persisted contract); fixed broken onclick quotes on the Test/testSearch buttons (dead since v0.3.4o) and removed dead legacy checkbox reads that crashed runTest/testSearch with null.checked; CDP real-click verified: tree opens, composite picked, /api/v1/retrieval body carries rerank_id, zero /models/default calls, reload leaves the selection empty (upstream parity), clean probe deletion. v0.3.5i: `/dataset/retrieval/{id}` now renders the upstream left/right two-column form inside `.ds-content` (the retrieval panel used to fall outside the content column because a malformed `data-tip` attribute swallowed the tags that followed it), the Reranking-model label renders as text instead of a bogus `<reranking>` element, and the Test button keeps the `use_kg`/`cross_languages` payload fields. Exact React composition remains reference [v0.27.2 content changed: re-audit] |
| `web/src/pages/login-next/bg.tsx` | 161 | `src/web.rs; web/static/` | v0.3.5n: ported. `LOGIN_BG_SVG` reproduces the three bands verbatim (paths, `viewBox 0 0 1440 240/466/704`, `preserveAspectRatio="none"`, `#00BEB4` base stroke at 0.1 opacity, `#80FFF8` glow gradient at 0.8 behind a dash mask, `#FFF`/`#FFD700` highlight path, `feGaussianBlur` 5.2/5.5) plus the `.animate-glow`/`.animate-highlight` 16s dash flow and the `ml-10`/`-mt-48`/`-mt-72` band offsets; the three `Spotlight` radial gradients (`opcity` 0.4/0.3/0.3, `coverage` 60/12/12, X 50%/10%/90%, Y 190%/-10%/-10%, `rgb(128,255,248)`) render as CSS custom-property gradients with `backdrop-filter: blur(30px)` and `z-index:-1`. CDP-verified band heights 240/466/704, 3 spotlights, `auth-glow`/`auth-highlight-flow` at 16s, `pointer-events:none` |
| `web/src/pages/login-next/index.tsx` | 417 | `src/web.rs; src/web_css.txt` | Rust `/login` keeps the upstream composition: a standalone shell (no global header), Email/Password + Nickname register faces, `Remember me`, the `Sign in with {channel}` button list rendered from `/api/v1/auth/login/channels` (hidden when the tenant exposes none), `auth-email`/`auth-password`/`auth-nickname`/`auth-submit`/`auth-toggle-register`/`auth-toggle-login` test ids and a post-login redirect to `/`. v0.3.5n: the 3D background art and the `registerEnabled` gate are landed — the page paints `LOGIN_BG_SVG` + three Spotlights and `loadAuthSystemConfig()` reads `GET /api/v1/system/config`, hiding `#authSwitchToRegister` and refusing `toggleAuth()` toward the register face when `registerEnabled === 0` (CDP-verified with a stubbed 0: switch hidden, `AUTH_REGISTER_ENABLED=false`, active face stays `authLoginFace`). v0.3.5o: `disablePasswordLogin` is honoured (both password forms hidden, the channel section switches to the upstream `py-8`/`w-full` solo layout, both switch links hidden) and the channel buttons now call the upstream redirect `GET /api/v1/auth/login/{channel}`, which exists and composes the authorization URL. v0.3.7x: the callback (`src/api/system.rs::oauth_callback`) exchanges the code, fetches the provider profile, registers or logs the user in, sets the `rayrag_token` cookie and redirects to `/?auth={user_id}` with the upstream `?error=` codes; the shell now consumes both parameters exactly like `hooks/auth-hooks.ts::useOAuthCallback` (store the session and drop the parameter, or show the provider error and bounce to `/login`). v0.3.8g: the zod schema is now enforced per field on both faces — `email`/`password` report the two placeholders and "Invalid email", the register face adds the required nickname plus the `NICKNAME_PATTERN` message, each field owns a `FormMessage` slot, the submits are gated on the resolver, and the remember-me label tints with its checkbox (upstream passes the raw `nicknamePlaceholder` key as the required message; RayRAG renders the readable placeholder instead, recorded as a deliberate deviation). [v0.27.2 content changed: re-audit] |
| `web/src/pages/next-search/hooks.ts` | 639 | `src/web.rs; src/api/system.rs; src/api/searchapp_mgr.rs` | Rust `/search` page consumes the search app runtime: `/search?sapp={id}` seeds kb chips/select, the rerank model-tree picker (`SEARCH_PAGE_RERANK_SELECTOR_FIELD`, revealed by the rerank switch, seeded from `__SAPP__.rerank_id`), rerank/hybrid switches, an injected Top-K option; `runSearch` sends the app config (kb_ids/top_k/rerank_id/similarity_threshold/vector_similarity_weight) to `/api/v1/retrieval`, and the picker stays form-local (no `PATCH /api/v1/models/default`, matching the upstream react-hook-form drawer — CDP probe asserts 0 default PATCHes); v0.3.5v: the related-search flow landed — the `related_search` switch seeds from `search_config` (and from the app on `/searches/{id}`), `runSearch` issues `POST /api/v1/chat/recommendation` with `{question, search_id}`, up to five answers render as chips and a chip click re-runs the search with its text (CDP probe `search-related-cdp.mjs`); the handler resolves the app's `chat_id` instance + `llm_setting`; mindmap/highlight/AI-summary flows remain simplified v0.3.6c: `useSearching`'s answer stream is wired -- `runSearch` now drives `POST /api/v1/searches/{search_id}/completions` (upstream `api.searchCompletion`), the SSE is parsed for `chat.completion.chunk` deltas and the `chat.completion.references` event, the stream can be aborted from the submit/stop control, and the retrieval request falls back to `search_config.kb_ids` while the chip list is still loading (upstream reads the app config directly). v0.3.8a: the document-metadata chips are rendered (`formatMetadataValue` + the upstream chip row, with `include_metadata: true` now sent by both retrieval surfaces). v0.3.8i: the progress is now upstream's own — `usePendingMindMap` is a *synthetic* ramp (a 1s interval that clears itself past 40 and feeds `Number(((count / 43) * 100).toFixed(0))`, so it walks ~2% a second and settles at 98), rendered through the `Progress` primitives (`h-1 min-w-10 rounded-full bg-bg-accent`, indicator moved by `translateX(-(100 - value)%)`, `role='progressbar'` with aria values) and started/stopped with the sheet's request. Ledger correction: the earlier "real multi-step progress" framing is withdrawn — upstream has no real progress either. v0.3.6d: `runSearch` now builds the upstream request (`{question, highlight, page, size, search_id, kb_id, doc_ids}`) instead of reconstructing the app config client-side; the sidebar controls only override `top_k`/`similarity_threshold`/`vector_similarity_weight` once the user actually edits them (`window.__SEARCH_TOUCHED`), and the `BM25+Vector` switch no longer zeroes the vector weight (upstream drives that value from `SimilaritySliderFormField`, not from a hybrid switch). [v0.27.2 content changed: re-audit] |
| `web/src/pages/next-search/search-setting.tsx` | 528 | `src/web.rs; src/web_css.txt; src/api/searchapp_mgr.rs` | Rust search-app detail settings (`/searches/{id}`) carry the upstream rerank section (use_rerank switch reveals the shared rerank ModelTreeSelect, SEARCH_RERANK_SELECTOR_FIELD) and the similarity slider pair (similarity_threshold/vector_similarity_weight persisted on the app, seeded into the `/search?sapp=` runtime and sent to `/api/v1/retrieval`); kb multi-select stays simplified; v0.3.5v: the `related_search` Switch (`search.enableRelatedSearch`) is rendered both in the app settings page and in the `/search` settings sidebar, persists through `PUT /api/v1/searchapps/{id}`, and the app's full upstream `search_config` (doc_ids/chat_id/llm_setting/cross_languages/use_kg/highlight/keyword/web_search/related_search/query_mindmap/summary, `top_k` default 1024) round-trips through create/update/GET v0.3.6c: the `/search` sidebar keeps the upstream defaults for `similarity_threshold`/`vector_similarity_weight`, but the page still forces `vector_similarity_weight: 0` when the `BM25+Vector` switch is off, so a keyword-free question finds nothing until the switch is turned on; upstream drives the weight from the settings form independently of the hybrid switch. Open item for the next slice. v0.3.6d: the weight/`threshold` mismatch recorded in v0.3.6c is resolved by the app-aware request above; the remaining gap is the page-level settings drawer itself (`RetrievalDocuments` popover and the empty-state illustration) rather than the retrieval parameters.\| v0.3.8m: the drawer itself landed — trigger + 440px slide-out shell (title row with a 16px `X`, scrollable body, pinned `search.cancelText` / `search.okText` footer with `data-testid="search-settings-save"`), the upstream field order (avatar/name/description, datasets, metadata filter, reference-metadata switch + field list, similarity sliders, rerank switch + required model tree + Top K, AI-summary switch + LLM rows, related-search and query-mindmap switches), `meta_data_filter` / `reference_metadata` / `summary` / `query_mindmap` riding the retrieval request, and Save through `PUT /api/v1/searchapps/{id}`. CDP-verified end to end (field order, switch-driven reveals, manual + semi_auto payloads, save persistence, cancel rollback). Residual: the avatar uses a plain data-URL preview instead of the shared crop dialog, and `AvatarNameDescription`'s exact Radix composition stays approximated [v0.27.2 content changed: re-audit] |
| `web/src/pages/next-search/search-view.tsx` | 395 | `src/web.rs; src/web_css.txt` | Rust `/search` renders the retrieval list plus the upstream related-search block: divider + `search.relatedSearch` heading (`text-xl` equivalent 16px) and transparent chip buttons carrying the recommendation text, appended below the chunks and removed when the switch is off or the list is empty (v0.3.5v). Residual: the answer/markdown stream, query mindmap sheet and the empty-state illustration remain approximated v0.3.6c: the residual answer/markdown stream and the mind-map sheet landed -- `/search` now renders the upstream greeting row (placeholder `search.searchGreeting`, lucide `X` clear, separator and the round submit button that swaps to a stop control while the stream runs), the `AI summary` block (24px heading, skeleton while streaming, streamed markdown answer in a bordered 208 px scroll box with citation chips and the trailing divider) fed by the new `POST /api/v1/searches/{search_id}/completions` SSE endpoint, the `Total: N` bar with the lucide `ListTree` button, and the mind-map sheet (`chunk.mind` title, progress bar, indented tree over `POST /api/v1/chat/mindmap`, `X` close). v0.3.7t: `RetrievalDocuments` is now the upstream popover (Files trigger with the selected/total counter, X clear, separator and ChevronDown; command popover with per-document checkbox rows, `common.noResults` empty state and the Clear/Close footer) and the selection is sent as `doc_ids`; the document-type filter is gone because upstream has none. v0.3.7u: the empty state is the upstream `Empty` illustration (both theme variants inlined, 80px, `common.noResults`). Residual: the answer text/mind-map generation needs a reachable chat model. [v0.27.2 content changed: re-audit] |
| `web/src/pages/skills/components/search-config-modal.tsx` | 432 | `src/web.rs; web/static/` | The embedding slice is wired through the shared default-model tree selector (SKILL_EMBD_SELECTOR_FIELD + MODEL_FIELDS payload + shared HEAD/BODY behaviour + form persist hook + catalog loader + stored-value normalizer), with the upstream save validation message (pleaseSelectEmbeddingModel) and CDP-verified real click, save, reopen-persistence and original-config restore; v0.3.5t: the modal now matches `search-config-modal.tsx` end to end — `skillSearch.configTitle` + `configDesc` header, `Embedding Model` with the `Select an embedding model` placeholder, Vector Similarity Weight as a 0..1/0.1 slider with the live `getSearchTypeLabel` copy (`Keyword Only` / `Vector Only` / `Hybrid (N% Keyword + M% Vector)`) plus the `Keyword Only`/`Balanced`/`Vector Only` captions, Similarity Threshold as a 0..1/0.05 slider with the `.toFixed(1)` reading, `Top K Results` bounded 1..100 and the `Index Fields` section (`indexFieldsDesc` + per-field switch and weight inputs for Name/Tags/Description/Content) whose values now drive `field_config` on save and restore on open (previously hard-coded with a single content checkbox); all copy is bilingual via the injected `SKILL_LABELS` table [v0.27.2 content changed: re-audit] |
| `web/src/pages/user-setting/chat-channel/constant/index.tsx` | 777 | `src/web.rs` | Rust registry now covers all 23 ChatChannelKey entries (clickclack/discord/dingtalk/feishu/googlechat/irc/line/matrix/mattermost/msteams/nextcloud_talk/nostr/qqbot/slack/synology_chat/telegram/tlon/twitch/wecom/whatsapp/yuanbao/zalo/zalouser) with their fixed names, descriptions and full per-channel credential field sets (select/number/switch/textarea/tag types incl. feishu domain select, googlechat auth_mode select, wecom connection_type select with showIf websocket/webhook conditionals); dingtalk corrected to client_id/client_secret. All 23 `setting.chatChannelDesc` English strings are now verbatim upstream (machine diff vs `locales/en.ts` = 0/23 after correcting irc/line/twitch/yuanbao/zalouser, regression-asserted in `user_setting_chat_channel_page_wires_add_edit_delete_flows`, verified on the deployed container page); all 23 `CHANNEL_NAMES` also diff clean. Chinese `chatChannelDesc` copy is now verbatim upstream for lang=zh-CN (CC_DESC_ZH injected only when zh is requested, `ccDesc()` fallback keeps the en page unchanged; machine-diffed 23/23 en vs en.ts and 23/23 zh vs zh.ts, regression-asserted); upstream FormFieldType Tag/placeholder pixel parity remains partial (v0.3.4c). v0.3.4m: resolved — the upstream 23-channel/67-field map was extracted and audited (22/23 exact), the 16 missing placeholders were added verbatim, the IRC nickserv fields now use the upstream nested names (nickserv.account/nickserv.password with dot-path set/get on save and restore), and the Tag field (nostr Relays) renders the upstream chips input (Enter/comma commit, duplicate skip, × remove, Backspace undo) with the placeholder on the draft input; verified by the cc-dialog CDP probe (chips dedupe, relays array persisted, nested nickserv payload, diff-based id capture, cleanup) (v0.3.4m) [v0.27.2 content changed: re-audit] |
| `web/src/pages/user-setting/compilation-templates/edit-next/components/template-configuration.tsx` | 301 | `src/web.rs; src/server.rs; src/api/compilation_templates.rs` | （v0.3.10c 复核：edit-next 的 Layout-less 外壳/Name/Description/模板类型/全局规则/重新切分与规则/Save-Back，以及创建、编辑回填已由 Rust SSR + /api/v1/compilation_template_groups 实现并经真浏览器探针验证；Blueprint/分节字段/预览弹层/树模板等深层组件未复刻） |
| `web/src/pages/user-setting/compilation-templates/edit-next/constant.ts` | 79 | `src/web.rs; src/server.rs; src/api/compilation_templates.rs` | （v0.3.10c 复核：edit-next 的 Layout-less 外壳/Name/Description/模板类型/全局规则/重新切分与规则/Save-Back，以及创建、编辑回填已由 Rust SSR + /api/v1/compilation_template_groups 实现并经真浏览器探针验证；Blueprint/分节字段/预览弹层/树模板等深层组件未复刻） |
| `web/src/pages/user-setting/compilation-templates/edit-next/hooks/use-available-kind-options.ts` | 69 | `src/web.rs; src/server.rs; src/api/compilation_templates.rs` | （v0.3.10c 复核：edit-next 的 Layout-less 外壳/Name/Description/模板类型/全局规则/重新切分与规则/Save-Back，以及创建、编辑回填已由 Rust SSR + /api/v1/compilation_template_groups 实现并经真浏览器探针验证；Blueprint/分节字段/预览弹层/树模板等深层组件未复刻） |
| `web/src/pages/user-setting/compilation-templates/edit-next/hooks/use-builtin-template.ts` | 48 | `src/web.rs; src/server.rs; src/api/compilation_templates.rs` | （v0.3.10c 复核：edit-next 的 Layout-less 外壳/Name/Description/模板类型/全局规则/重新切分与规则/Save-Back，以及创建、编辑回填已由 Rust SSR + /api/v1/compilation_template_groups 实现并经真浏览器探针验证；Blueprint/分节字段/预览弹层/树模板等深层组件未复刻） |
| `web/src/pages/user-setting/compilation-templates/edit-next/hooks/use-compilation-template-group-form.ts` | 50 | `src/web.rs; src/server.rs; src/api/compilation_templates.rs` | （v0.3.10c 复核：edit-next 的 Layout-less 外壳/Name/Description/模板类型/全局规则/重新切分与规则/Save-Back，以及创建、编辑回填已由 Rust SSR + /api/v1/compilation_template_groups 实现并经真浏览器探针验证；Blueprint/分节字段/预览弹层/树模板等深层组件未复刻） |
| `web/src/pages/user-setting/compilation-templates/edit-next/hooks/use-compilation-template-group-submit.ts` | 64 | `src/web.rs; src/server.rs; src/api/compilation_templates.rs` | （v0.3.10c 复核：edit-next 的 Layout-less 外壳/Name/Description/模板类型/全局规则/重新切分与规则/Save-Back，以及创建、编辑回填已由 Rust SSR + /api/v1/compilation_template_groups 实现并经真浏览器探针验证；Blueprint/分节字段/预览弹层/树模板等深层组件未复刻） |
| `web/src/pages/user-setting/compilation-templates/edit-next/hooks/use-edit-next-compilation-template-group.ts` | 82 | `src/web.rs; src/server.rs; src/api/compilation_templates.rs` | （v0.3.10c 复核：edit-next 的 Layout-less 外壳/Name/Description/模板类型/全局规则/重新切分与规则/Save-Back，以及创建、编辑回填已由 Rust SSR + /api/v1/compilation_template_groups 实现并经真浏览器探针验证；Blueprint/分节字段/预览弹层/树模板等深层组件未复刻） |
| `web/src/pages/user-setting/compilation-templates/edit-next/hooks/use-template-kind-change.ts` | 54 | `src/web.rs; src/server.rs; src/api/compilation_templates.rs` | （v0.3.10c 复核：edit-next 的 Layout-less 外壳/Name/Description/模板类型/全局规则/重新切分与规则/Save-Back，以及创建、编辑回填已由 Rust SSR + /api/v1/compilation_template_groups 实现并经真浏览器探针验证；Blueprint/分节字段/预览弹层/树模板等深层组件未复刻） |
| `web/src/pages/user-setting/compilation-templates/edit-next/index.tsx` | 102 | `src/web.rs; src/server.rs; src/api/compilation_templates.rs` | （v0.3.10c 复核：edit-next 的 Layout-less 外壳/Name/Description/模板类型/全局规则/重新切分与规则/Save-Back，以及创建、编辑回填已由 Rust SSR + /api/v1/compilation_template_groups 实现并经真浏览器探针验证；Blueprint/分节字段/预览弹层/树模板等深层组件未复刻） |
| `web/src/pages/user-setting/compilation-templates/edit-next/schema.ts` | 86 | `src/web.rs; src/server.rs; src/api/compilation_templates.rs` | （v0.3.10c 复核：edit-next 的 Layout-less 外壳/Name/Description/模板类型/全局规则/重新切分与规则/Save-Back，以及创建、编辑回填已由 Rust SSR + /api/v1/compilation_template_groups 实现并经真浏览器探针验证；Blueprint/分节字段/预览弹层/树模板等深层组件未复刻） |
| `web/src/pages/user-setting/compilation-templates/edit-next/utils.ts` | 340 | `src/web.rs; src/server.rs; src/api/compilation_templates.rs` | （v0.3.10c 复核：edit-next 的 Layout-less 外壳/Name/Description/模板类型/全局规则/重新切分与规则/Save-Back，以及创建、编辑回填已由 Rust SSR + /api/v1/compilation_template_groups 实现并经真浏览器探针验证；Blueprint/分节字段/预览弹层/树模板等深层组件未复刻） |
| `web/src/pages/user-setting/data-source/component/box-token-field.tsx` | 468 | `src/web.rs; src/api/oauth_web.rs` | Rust Box token field replaces the raw textarea: Configure/Get Box credentials button, Configured/Authorized chips, Client ID/Secret/Redirect URI dialog, Submit & Authorize popup + Refresh status polling against the implemented start/callback/result endpoints; CDP-verified the full start→pending(106)→callback→authorized→create loop with an offline-aborted popup. Gmail/Drive fields and the real Box token exchange (requires `BOX_OAUTH_TOKEN_URL`) remain partial (v0.3.3cd) [v0.27.2 content changed: re-audit] |
| `web/src/pages/user-setting/data-source/component/gmail-token-field.tsx` | 477 | `src/web.rs; src/api/oauth_web.rs` | Rust serves the shared Google token field for Gmail (JSON uploader, redirect extraction, Authorize with Google, Refresh status) through the implemented Google OAuth endpoints; the per-provider popup listener and real token minting remain partial (v0.3.3ce) [v0.27.2 content changed: re-audit] |
| `web/src/pages/user-setting/data-source/index.tsx` | 165 | `src/web.rs; src/data_source/form_fields.rs` | Rust list page renders the fixed 35-card Available composition (card set and names re-verified machine-side against `generateDataSourceInfo`: 35 upstream vs 35 rendered, 0 missing, 0 extra, 0 name mismatch; upstream `common.search`/`common.noData` copy also matches the tree — `Search` / `No data available`) (icon/name/description, hover + Add, click opens the schema modal titled `Create your {name} connector`), grouped Added cards and the empty state; CDP-verified 35 cards, hover Add, create/gear/delete flows. Exact React grid/card geometry and per-source search remain partial (v0.3.3cc) [v0.27.2 content changed: re-audit] |
| `web/src/pages/user-setting/index.tsx` | 38 | `src/web.rs; src/web_css.txt; src/server.rs` | Rust keeps the global Header and renders a shared responsive settings shell around every registered child route; isolated Chrome verified the 303 px desktop and 64 px mobile layouts. Exact RootLayout/Tailwind geometry and child-page content remain partial (v0.3.3bo) [v0.27.2 content changed: re-audit] |
| `web/src/pages/user-setting/profile/constants.ts` | 19 | `src/web.rs; src/api/system.rs` | v0.3.5m: the client now enforces the same upstream rules through `PROFILE_NAME_PATTERN=/^[\p{L}\p{N} ._\'-]+$/u` plus the 100-character limit, and both messages (`Name must be at most 100 characters`, `Only letters, numbers and . _ \'- are allowed`) come from the injected bilingual label table; the server-side guard is unchanged [v0.27.2 content changed: re-audit] |
| `web/src/pages/user-setting/profile/index.tsx` | 438 | `src/web.rs; src/web_css.txt; src/api/system.rs` | Rust Profile page renders Username/Avatar/Time Zone/Email/Password rows with Edit Name / Edit Time Zone / Edit Password dialogs and inline avatar upload; CDP-verified name, timezone, avatar and password change. The timezone list now follows the upstream runtime algorithm exactly (`Intl.supportedValuesOf` + longOffset GMT names, offset-sorted, searchable select with `Select a timeZone` placeholder, `select your timezone` save validation, display find-by-name-else-empty via `tzRenderList`/`PROFILE_LOADED`), CDP re-verified end-to-end (418 options, Tokyo filter, option click, save persisted, display refresh). v0.3.5m: the page now carries the upstream header (`setting.profile` + `setting.profileDescription`), the 190px label column with bordered value boxes, a `PenLine`-style edit button, the `setting.emailDescription` and `setting.avatarTip` copy, and a full `AvatarUpload` control (64×64 dashed plus/`common.upload` button when empty → cropped 64×64 PNG preview with hover pencil re-pick → circular `Remove image` button that clears the avatar server-side). All labels, placeholders, validation copy and dialog titles are localized for lang=zh-CN (概要/用户名/头像/时区/邮箱/密码/编辑, 请输入用户名, 请确认新密码 …) while the upstream `modalTitle` strings stay English exactly as in the React source; v0.3.8k: the avatar field now opens **the same interactive crop dialog as `components/avatar-upload.tsx`** — the 80%-of-the-smaller-side selection, drag in image coordinates clamped to the picture, wheel resize by 0.9/1.1 with the 20px floor holding the centre ratio, the `2px dashed #fff` box over a `0 0 0 9999px rgba(0,0,0,.5)` mask and the 64×64 PNG output — instead of the automatic centred crop; OK resolves the crop (the promise the save path awaits) and Cancel resolves `null`. CDP real-input verified: real file input, real mouse drag (moved and clamped), real wheel events (in, then 40 ticks to the floor) and a real OK click that produced exactly one `PATCH /api/v1/users/me` carrying a 64×64 PNG (v0.3.8k) [v0.27.2 content changed: re-audit] |
| `web/src/pages/user-setting/setting-model/index.tsx` | 353 | `src/web.rs; src/web_css.txt; src/server.rs; src/api/tenant_models.rs; src/providers.rs` | Rust serves the fixed `/user-setting/model` route with Set default models, Added models and the fixed 63-card Available composition, exact tenant routes, canonical Search/filter/card actions and tenant instance modal. The Langfuse card, Bedrock/SoMark dedicated modals and the 10 ProviderConfigMap dynamic credential field sets are implemented and CDP-verified. v0.3.7f closed the layout residual: the page is the upstream bordered rounded card split into a 3/5 column (`SystemSetting` + `UsedModel`, `gap-4 px-5 border-r-[0.5px]`) and a 2/5 column (`AvailableModels`), each scrolling independently, with the single `<Spotlight />` gradient (`opacity-80 rounded-lg`, coverage 60, centre 50%/190%, white in dark mode / `rgb(194,221,243)` in light) behind the content. v0.3.7e handled the viewMode half: that entry point is dead code upstream (see the `used-model.tsx` row), so RayRAG's add-then-save modal is behaviourally equivalent. [v0.27.2 content changed: re-audit] |
| `web/src/pages/user-setting/sidebar/hooks.tsx` | 47 | `src/web.rs; src/web_css.txt; src/server.rs` | Rust derives one exact active key from the current user-setting route; server and browser gates cover the root redirect, seven item clicks and back/forward navigation. React client-side navigation state remains partial (v0.3.3bo) [v0.27.2 content changed: re-audit] |
| `web/src/routes.tsx` | 466 | `src/server.rs; src/web.rs` | Rust registers the `/dataset/{files\|retrieval\|logs\|configuration\|knowledge-graph}/{id}` detail routes; `/dataset/{id}` answers a 307 redirect into `/dataset/files/{id}` (upstream `Routes.Dataset = /dataset/files`) and `/kbs/{id}` keeps the legacy 3-tab shell. The dataset sidebar mirrors `pages/dataset/sidebar/index.tsx`: five real route links in source order with the `knowledgeDetails.*` labels (Files / Retrieval testing / Logs / Configuration / Knowledge graph), the active entry marked client-side from the URL path and the panel selected on load; the parse-log table was merged into the Logs panel so the entry count matches upstream. Remaining gaps: `/dataset/testing`, `/dataset/knowledge-graph` and `/dataset/configuration` are the same Rust shell with a preselected panel rather than separate page bundles (v0.3.5g). v0.3.5x added a machine audit of the whole upstream route table (52 paths against the deployed instance) and closed six: `/login-next` (upstream serves `/login` and `/login-next` from the same page), the canonical singular `/agent/:id` and `/search/:id` detail paths (RayRAG keeps `/agents/{id}` and `/searches/{id}` as compatible aliases), upstream's standalone `/document/:id` viewer (resolved through the document record, real 404 for unknown ids) and the public `/search/share` embed (`?tenantId=` app resolution plus the `visible_avatar` owner row, sharing the `/search` markup through `search_page_markup`). v0.3.8n closed `/agent/:id/explore` (plus the `/agents/:id/explore` alias): the Launch surface renders upstream's PageHeader breadcrumb, the 296px session column and the SessionChat column, and the toolbar Launch button navigates there like `navigateToAgentExplore`. Still open from the same table: the `/admin` subtree (`/admin` is upstream's admin *login*, plus `/admin/services`, `/admin/users`, `/admin/users/:id`, `/admin/sandbox-settings` and the enterprise-only whitelist/roles/monitoring — all registered by RayRAG, per-file residuals tracked on their own rows), the `/chunk` subtree (`/chunk`, `/chunk/parsed/chunks`, `/chunk/chunk/:id`, `/chunk/result/:id`), v0.3.8o closed `/agent/share` (the embeddable chat with the `APIToken.beta` credential, `/api/v1/system/tokens` and the `agentbots` inputs/completions endpoints; the beta-only `logs/<message_id>` trace and the floating-widget embed remain open). v0.3.8p closed `/dataflow-result` (the ingestion-log viewer with the timeline, the step result panel, the chunk result bar, the rerun flow and the dataset-row `View result` entry, backed by the new ingestion endpoints). Still open from the same table: `/agent/:id/explore`, `/agent/share` and `/dataflow-result` are registered, while the beta-only `agentbots/logs/<message_id>` trace, the floating-widget embed and the enterprise-only `/admin` whitelist/roles/monitoring entries remain [v0.27.2 content changed: re-audit] |

## 复核对账队列（re-audit，440 项）

上游 v0.27.2 相对 v0.26.4 变更过的已审计行；逐行复核后移除 `[v0.27.2 content changed: re-audit]` 标记并更新注记。

| 文件 | 状态 | RayRAG 对应 |
|---|---|---|
| `Dockerfile` | replaced | `Dockerfile` |
| `Dockerfile_deepdoc_oss` | replaced | `Dockerfile` |
| `admin/client/parser.py` | full | `src/api/admin.rs; src/server.rs` |
| `admin/client/ragflow_client.py` | full | `src/api/admin.rs; src/server.rs` |
| `admin/server/admin_server.py` | full | `src/api/admin.rs; src/server.rs` |
| `admin/server/auth.py` | full | `src/api/admin.rs; src/server.rs` |
| `admin/server/config.py` | full | `src/api/admin.rs; src/server.rs` |
| `admin/server/routes.py` | full | `src/api/admin.rs; src/api/sandbox_admin.rs; src/server.rs` |
| `admin/server/services.py` | full | `src/api/admin.rs; src/server.rs; src/api/sandbox_admin.rs` |
| `agent/canvas.py` | full | `src/agent.rs; src/settings.rs` |
| `agent/component/agent_with_tools.py` | full | `src/runtime.rs` |
| `agent/component/base.py` | full | `src/runtime.rs` |
| `agent/component/begin.py` | full | `src/runtime.rs` |
| `agent/component/browser.py` | full | `src/runtime.rs` |
| `agent/component/categorize.py` | aligned | `src/agent.rs; src/llm.rs; src/api/features.rs` |
| `agent/component/docs_generator.py` | full | `src/runtime.rs` |
| `agent/component/excel_processor.py` | full | `src/runtime.rs` |
| `agent/component/fillup.py` | full | `src/runtime.rs` |
| `agent/component/invoke.py` | full | `src/runtime.rs` |
| `agent/component/iteration.py` | full | `src/runtime.rs` |
| `agent/component/list_operations.py` | aligned | `src/agent.rs` |
| `agent/component/llm.py` | full | `src/runtime.rs` |
| `agent/component/message.py` | full | `src/runtime.rs` |
| `agent/component/string_transform.py` | full | `src/runtime.rs` |
| `agent/component/variable_aggregator.py` | aligned | `src/agent.rs` |
| `agent/sandbox/client.py` | full | `src/sandbox.rs; src/code_exec.rs` |
| `agent/sandbox/executor_manager/api/handlers.py` | full | `src/sandbox.rs; src/code_exec.rs` |
| `agent/sandbox/executor_manager/api/routes.py` | full | `src/sandbox.rs; src/code_exec.rs` |
| `agent/sandbox/executor_manager/core/config.py` | full | `src/sandbox.rs; src/code_exec.rs` |
| `agent/sandbox/executor_manager/core/container.py` | full | `src/sandbox.rs; src/code_exec.rs` |
| `agent/sandbox/executor_manager/services/execution.py` | full | `src/sandbox.rs; src/code_exec.rs` |
| `agent/sandbox/executor_manager/services/limiter.py` | full | `src/sandbox.rs; src/code_exec.rs` |
| `agent/sandbox/providers/__init__.py` | full | `src/sandbox.rs; src/code_exec.rs` |
| `agent/sandbox/providers/base.py` | full | `src/sandbox.rs; src/code_exec.rs` |
| `agent/sandbox/providers/self_managed.py` | full | `src/sandbox.rs; src/code_exec.rs` |
| `agent/tools/base.py` | full | `src/runtime.rs; src/agent.rs` |
| `agent/tools/code_exec.py` | full | `src/code_exec.rs; src/agent.rs; src/runtime.rs` |
| `agent/tools/deepl.py` | full | `src/translate.rs; src/runtime.rs; src/agent.rs` |
| `agent/tools/duckduckgo.py` | full | `src/runtime.rs; src/agent.rs` |
| `agent/tools/email.py` | full | `src/email.rs; src/agent.rs; src/runtime.rs` |
| `agent/tools/exesql.py` | full | `src/exesql.rs; src/agent.rs; src/runtime.rs` |
| `agent/tools/github.py` | full | `src/runtime.rs; src/agent.rs` |
| `agent/tools/google.py` | full | `src/runtime.rs; src/agent.rs` |
| `agent/tools/keenable.py` | full | `src/runtime.rs; src/agent.rs` |
| `agent/tools/qweather.py` | full | `src/runtime.rs; src/agent.rs` |
| `agent/tools/retrieval.py` | full | `src/runtime.rs; src/agent.rs` |
| `api/apps/auth/README.md` | aligned | `src/server.rs; src/api/` |
| `api/apps/auth/oidc.py` | aligned | `src/server.rs; src/api/` |
| `api/apps/backward_compat.py` | aligned | `src/server.rs; src/api/` |
| `api/apps/restful_apis/_generation_params.py` | aligned | `src/generation_params.rs; src/llm.rs; src/api/features.rs; src/api/openai_proxy.rs` |
| `api/apps/restful_apis/agent_api.py` | aligned | `src/server.rs; src/api/agent_log.rs; src/api/features.rs` |
| `api/apps/restful_apis/bot_api.py` | aligned | `src/server.rs; src/api/` |
| `api/apps/restful_apis/chat_api.py` | aligned | `src/server.rs; src/api/system.rs; src/api/searchapp_mgr.rs` |
| `api/apps/restful_apis/chat_channel_api.py` | aligned | `src/server.rs; src/api/chat_channel_mgr.rs` |
| `api/apps/restful_apis/chunk_api.py` | aligned | `src/server.rs; src/api/` |
| `api/apps/restful_apis/compilation_template_api.py` | aligned | `src/api/compilation_templates.rs; src/api/fixtures/; src/server.rs` |
| `api/apps/restful_apis/compilation_template_group_api.py` | aligned | `src/api/compilation_templates.rs; src/server.rs` |
| `api/apps/restful_apis/connector_api.py` | aligned | `src/server.rs; src/api/data_source_mgr.rs; src/api/oauth_web.rs` |
| `api/apps/restful_apis/dataset_api.py` | aligned | `src/server.rs; src/api/` |
| `api/apps/restful_apis/dify_retrieval_api.py` | aligned | `src/server.rs; src/api/` |
| `api/apps/restful_apis/document_api.py` | aligned | `src/server.rs; src/api/` |
| `api/apps/restful_apis/file2document_api.py` | aligned | `src/server.rs; src/api/` |
| `api/apps/restful_apis/file_api.py` | aligned | `src/server.rs; src/api/` |
| `api/apps/restful_apis/file_commit_api.py` | aligned | `src/server.rs; src/api/` |
| `api/apps/restful_apis/mcp_api.py` | aligned | `src/server.rs; src/api/mcp_mgr.rs` |
| `api/apps/restful_apis/memory_api.py` | full | `src/server.rs; src/api/features.rs` |
| `api/apps/restful_apis/models_api.py` | full | `src/api/tenant_models.rs; src/api/system.rs; src/server.rs` |
| `api/apps/restful_apis/openai_api.py` | aligned | `src/server.rs; src/api/` |
| `api/apps/restful_apis/provider_api.py` | aligned | `src/api/features.rs; src/api/tenant_models.rs; src/model_meta.rs; src/server.rs` |
| `api/apps/restful_apis/search_api.py` | aligned | `src/server.rs; src/api/` |
| `api/apps/restful_apis/system_api.py` | aligned | `src/api/system.rs; src/logging.rs; src/server.rs` |
| `api/apps/restful_apis/tenant_api.py` | aligned | `src/server.rs; src/api/system.rs` |
| `api/apps/restful_apis/user_api.py` | aligned | `src/server.rs; src/api/system.rs; src/auth.rs` |
| `api/apps/restful_apis/utils/compilation_template_validation.py` | aligned | `src/server.rs; src/api/` |
| `api/apps/services/dataset_api_service.py` | aligned | `src/server.rs; src/api/` |
| `api/apps/services/document_api_service.py` | aligned | `src/server.rs; src/api/` |
| `api/apps/services/file_api_service.py` | aligned | `src/server.rs; src/api/` |
| `api/apps/services/memory_api_service.py` | full | `src/api/features.rs; src/api/joint_services.rs; src/memory.rs; src/server.rs` |
| `api/apps/services/models_api_service.py` | full | `src/api/tenant_models.rs; src/api/system.rs; src/server.rs` |
| `api/apps/services/provider_api_service.py` | aligned | `src/api/features.rs; src/api/tenant_models.rs; src/model_meta.rs; src/providers.rs; src/web.rs` |
| `api/channels/bootstrap.py` | full | `src/channels.rs; src/server.rs` |
| `api/db/__init__.py` | aligned | `src/api/db.rs; src/api/utils.rs; src/api/joint_services.rs; src/api/common.rs; src/api/features.rs; src/agent_templates.rs; src/web.rs` |
| `api/db/db_models.py` | replaced | `src/auth.rs; src/kb.rs; src/stores.rs; src/api/` |
| `api/db/db_utils.py` | replaced | `src/persistence.rs` |
| `api/db/init_data.py` | full | `src/api/joint_services.rs` |
| `api/db/init_data/compilation_templates/knowledge_graph.yaml` | aligned | `src/api/fixtures/compilation_template_builtins.v0.26.4.json; src/api/fixtures/compilation_template_wiki_presets.v0.26.4.json` |
| `api/db/init_data/compilation_templates/mind_map.yaml` | aligned | `src/api/fixtures/compilation_template_builtins.v0.26.4.json; src/api/fixtures/compilation_template_wiki_presets.v0.26.4.json` |
| `api/db/init_data/compilation_templates/page_index.yaml` | aligned | `src/api/fixtures/compilation_template_builtins.v0.26.4.json; src/api/fixtures/compilation_template_wiki_presets.v0.26.4.json` |
| `api/db/init_data/compilation_templates/session_essence.yaml` | aligned | `src/api/fixtures/compilation_template_builtins.v0.26.4.json; src/api/fixtures/compilation_template_wiki_presets.v0.26.4.json` |
| `api/db/init_data/compilation_templates/session_graph.yaml` | aligned | `src/api/fixtures/compilation_template_builtins.v0.26.4.json; src/api/fixtures/compilation_template_wiki_presets.v0.26.4.json` |
| `api/db/init_data/compilation_templates/timeline.yaml` | aligned | `src/api/fixtures/compilation_template_builtins.v0.26.4.json; src/api/fixtures/compilation_template_wiki_presets.v0.26.4.json` |
| `api/db/init_data/compilation_templates/tree.yaml` | aligned | `src/api/fixtures/compilation_template_builtins.v0.26.4.json; src/api/fixtures/compilation_template_wiki_presets.v0.26.4.json` |
| `api/db/init_data/compilation_templates/wiki.yaml` | aligned | `src/api/fixtures/compilation_template_builtins.v0.26.4.json; src/api/fixtures/compilation_template_wiki_presets.v0.26.4.json` |
| `api/db/init_data/compilation_templates/wiki/brand.yaml` | aligned | `src/api/fixtures/compilation_template_builtins.v0.26.4.json; src/api/fixtures/compilation_template_wiki_presets.v0.26.4.json` |
| `api/db/init_data/compilation_templates/wiki/engineering.yaml` | aligned | `src/api/fixtures/compilation_template_builtins.v0.26.4.json; src/api/fixtures/compilation_template_wiki_presets.v0.26.4.json` |
| `api/db/init_data/compilation_templates/wiki/general.yaml` | aligned | `src/api/fixtures/compilation_template_builtins.v0.26.4.json; src/api/fixtures/compilation_template_wiki_presets.v0.26.4.json` |
| `api/db/init_data/compilation_templates/wiki/market.yaml` | aligned | `src/api/fixtures/compilation_template_builtins.v0.26.4.json; src/api/fixtures/compilation_template_wiki_presets.v0.26.4.json` |
| `api/db/init_data/compilation_templates/wiki/product.yaml` | aligned | `src/api/fixtures/compilation_template_builtins.v0.26.4.json; src/api/fixtures/compilation_template_wiki_presets.v0.26.4.json` |
| `api/db/init_data/compilation_templates/wiki/user_interview.yaml` | aligned | `src/api/fixtures/compilation_template_builtins.v0.26.4.json; src/api/fixtures/compilation_template_wiki_presets.v0.26.4.json` |
| `api/db/joint_services/memory_message_service.py` | full | `src/api/joint_services.rs; src/api/features.rs; src/memory.rs; src/llm.rs; src/server.rs` |
| `api/db/joint_services/tenant_model_service.py` | full | `src/api/*` |
| `api/db/joint_services/user_account_service.py` | full | `src/api/*` |
| `api/db/services/api_service.py` | aligned | `src/auth.rs; src/api/system.rs` |
| `api/db/services/canvas_service.py` | full | `src/api/joint_services.rs` |
| `api/db/services/chat_channel_service.py` | full | `src/api/joint_services.rs` |
| `api/db/services/common_service.py` | replaced | `src/persistence.rs; src/stores.rs` |
| `api/db/services/compilation_template_group_service.py` | aligned | `src/api/compilation_templates.rs` |
| `api/db/services/compilation_template_service.py` | aligned | `src/api/compilation_templates.rs; src/api/fixtures/; src/server.rs` |
| `api/db/services/connector_service.py` | full | `src/api/joint_services.rs` |
| `api/db/services/conversation_service.py` | aligned | `src/llm.rs; src/server.rs` |
| `api/db/services/dialog_service.py` | full | `src/api/joint_services.rs` |
| `api/db/services/doc_metadata_service.py` | aligned | `src/api/document_metadata.rs` |
| `api/db/services/document_service.py` | aligned | `src/api/document.rs; src/server.rs` |
| `api/db/services/file_commit_service.py` | replaced | `src/persistence.rs; src/api/file_mgr.rs` |
| `api/db/services/file_service.py` | aligned | `src/api/file_mgr.rs` |
| `api/db/services/knowledgebase_service.py` | aligned | `src/kb.rs; src/server.rs` |
| `api/db/services/llm_service.py` | aligned | `src/llm.rs; src/llm_enhanced.rs; src/api/tenant_models.rs` |
| `api/db/services/memory_service.py` | full | `src/api/features.rs; src/api/joint_services.rs; src/memory.rs; src/auth.rs` |
| `api/db/services/pipeline_operation_log_service.py` | aligned | `src/api/features.rs; src/server.rs` |
| `api/db/services/task_service.py` | full | `src/api/joint_services.rs` |
| `api/db/services/tenant_llm_service.py` | aligned | `src/api/tenant_models.rs; src/kb.rs` |
| `api/db/services/tenant_model_service.py` | aligned | `src/api/tenant_models.rs; src/kb.rs` |
| `api/db/services/user_canvas_version.py` | aligned | `src/api/features.rs; src/server.rs` |
| `api/db/services/user_service.py` | aligned | `src/auth.rs; src/api/system.rs` |
| `api/ragflow_server.py` | aligned | `src/main.rs; src/server.rs` |
| `api/utils/api_utils.py` | full | `src/api/utils.rs` |
| `api/utils/common.py` | full | `src/api/utils.rs` |
| `api/utils/crypt.py` | full | `src/password_transport.rs; src/auth.rs; src/server.rs` |
| `api/utils/file_response.py` | full | `src/api/utils.rs` |
| `api/utils/file_utils.py` | full | `src/api/utils.rs` |
| `api/utils/health_utils.py` | full | `src/api/utils.rs` |
| `api/utils/pagination_utils.py` | full | `src/api/utils.rs` |
| `api/utils/validation_utils.py` | full | `src/api/utils.rs` |
| `api/utils/web_utils.py` | full | `src/api/utils.rs` |
| `api/validation.py` | full | `src/common.rs` |
| `build.sh` | replaced | `Cargo.toml; Dockerfile; docker-compose.yml; scripts/mirror.sh` |
| `common/config_utils.py` | full | `src/common.rs; src/api/common.rs` |
| `common/constants.py` | full | `src/common.rs; src/api/common.rs` |
| `common/data_source/__init__.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/airtable_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/asana_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/azure_blob_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/bigquery_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/bitbucket/connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/blob_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/box_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/config.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/confluence_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/connector_runner.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/dingtalk_ai_table_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/discord_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/dropbox_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/github/connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/gitlab_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/gmail_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/google_drive/connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/html_utils.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/imap_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/interfaces.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/jira/connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/moodle_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/notion_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/onedrive_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/outlook_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/rdbms_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/rest_api_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/rss_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/salesforce_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/seafile_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/sharepoint_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/slack_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/teams_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/webdav_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/data_source/zendesk_connector.py` | full | `src/connectors.rs; src/api/data_source_mgr.rs` |
| `common/doc_store/doc_store_base.py` | full | `src/doc_store.rs` |
| `common/doc_store/es_conn_base.py` | full | `src/doc_store.rs` |
| `common/doc_store/es_conn_pool.py` | full | `src/doc_store.rs` |
| `common/doc_store/infinity_conn_base.py` | full | `src/doc_store.rs` |
| `common/doc_store/infinity_conn_pool.py` | full | `src/doc_store.rs` |
| `common/doc_store/ob_conn_pool.py` | full | `src/doc_store.rs` |
| `common/float_utils.py` | full | `src/common.rs; src/api/common.rs` |
| `common/llm_request_context.py` | full | `src/common.rs; src/api/common.rs` |
| `common/mcp_tool_call_conn.py` | full | `src/common.rs; src/api/common.rs` |
| `common/metadata_infinity_filter.py` | full | `src/common.rs; src/api/common.rs` |
| `common/metadata_utils.py` | full | `src/metadata_filter.rs; src/api/document_metadata.rs` |
| `common/misc_utils.py` | full | `src/common.rs; src/api/common.rs` |
| `common/parser_config_utils.py` | full | `src/common.rs; src/api/common.rs` |
| `common/settings.py` | full | `src/common.rs; src/api/common.rs` |
| `common/time_utils.py` | full | `src/common.rs; src/api/common.rs` |
| `common/token_utils.py` | full | `src/common.rs; src/api/common.rs` |
| `conf/all_models.json` | full | `.env.example; docker-compose.yml; src/main.rs` |
| `conf/infinity_mapping.json` | full | `src/settings.rs; src/store/infinity_chunk.rs; src/store/mod.rs; src/search.rs; src/persistence.rs; src/api/chunks.rs; src/api/search.rs; src/server.rs` |
| `conf/service_conf.yaml` | full | `.env.example; docker-compose.yml; src/main.rs` |
| `conf/system_settings.json` | aligned | `src/api/runtime_config.rs; src/api/system_settings.rs; src/api/joint_services.rs; src/code_exec.rs; src/agent.rs; src/server.rs; src/web.rs` |
| `deepdoc/parser/docling_parser.py` | full | `src/parser/excel.rs; src/parser/docx.rs; src/parser/paddleocr_layout.rs` |
| `deepdoc/parser/excel_parser.py` | aligned | `src/parser/excel.rs` |
| `deepdoc/parser/figure_parser.py` | aligned | `src/parser/figure.rs; src/vision.rs; src/ocr.rs` |
| `deepdoc/parser/html_parser.py` | aligned | `src/parser/html.rs` |
| `deepdoc/parser/json_parser.py` | aligned | `src/parser/json.rs` |
| `deepdoc/parser/markdown_parser.py` | aligned | `src/parser/markdown.rs` |
| `deepdoc/parser/mineru_parser.py` | full | `src/parser/excel.rs; src/parser/docx.rs; src/parser/paddleocr_layout.rs` |
| `deepdoc/parser/opendataloader_parser.py` | full | `src/parser/excel.rs; src/parser/docx.rs; src/parser/paddleocr_layout.rs` |
| `deepdoc/parser/paddleocr_parser.py` | full | `src/parser/paddleocr_layout.rs; src/parser/figure.rs; src/ocr.rs` |
| `deepdoc/parser/pdf_parser.py` | full | `src/parser/excel.rs; src/parser/docx.rs; src/parser/paddleocr_layout.rs` |
| `deepdoc/parser/somark_parser.py` | full | `src/parser/excel.rs; src/parser/docx.rs; src/parser/paddleocr_layout.rs` |
| `deepdoc/parser/tcadp_parser.py` | full | `src/parser/tcadp.rs` |
| `deepdoc/parser/txt_parser.py` | aligned | `src/parser/txt.rs` |
| `deepdoc/parser/utils.py` | full | `src/parser/excel.rs; src/parser/docx.rs; src/parser/paddleocr_layout.rs` |
| `deepdoc/vision/layout_recognizer.py` | aligned | `src/ocr.rs; src/parser/paddleocr_layout.rs; src/parser/figure.rs` |
| `deepdoc/vision/ocr.py` | aligned | `src/ocr.rs; src/parser/paddleocr_layout.rs; src/parser/figure.rs` |
| `deepdoc/vision/operators.py` | aligned | `src/ocr.rs; src/parser/paddleocr_layout.rs; src/parser/figure.rs` |
| `deepdoc/vision/recognizer.py` | aligned | `src/ocr.rs; src/parser/paddleocr_layout.rs; src/parser/figure.rs` |
| `docker/.env` | replaced | `Dockerfile; docker-compose.yml` |
| `docker/README.md` | replaced | `Dockerfile; docker-compose.yml` |
| `docker/docker-compose-CN-oc9.yml` | replaced | `Dockerfile; docker-compose.yml` |
| `docker/docker-compose-base.yml` | replaced | `Dockerfile; docker-compose.yml` |
| `docker/docker-compose-macos.yml` | replaced | `Dockerfile; docker-compose.yml` |
| `docker/docker-compose.yml` | replaced | `Dockerfile; docker-compose.yml` |
| `docker/entrypoint.sh` | replaced | `Dockerfile; docker-compose.yml` |
| `docker/infinity_conf.toml` | replaced | `Dockerfile; docker-compose.yml` |
| `docker/launch_backend_service.sh` | replaced | `Dockerfile; docker-compose.yml` |
| `docker/migration.sh` | replaced | `Dockerfile; docker-compose.yml` |
| `docker/nginx/ragflow.conf.golang` | replaced | `Dockerfile; docker-compose.yml` |
| `docker/nginx/ragflow.conf.hybrid` | replaced | `Dockerfile; docker-compose.yml` |
| `docker/nginx/ragflow.conf.python` | replaced | `Dockerfile; docker-compose.yml` |
| `docker/service_conf.yaml.template` | replaced | `Dockerfile; docker-compose.yml` |
| `go.mod` | replaced | `Cargo.toml; Cargo.lock` |
| `go.sum` | replaced | `Cargo.lock` |
| `helm/values.yaml` | replaced | `helm/Chart.yaml; helm/values.yaml; helm/templates/deployment.yaml; helm/templates/service.yaml; helm/templates/configmap.yaml; helm/README.md` |
| `internal/agent/canvas/cancel.go` | replaced | `src/agent.rs; src/api/features.rs; src/metrics.rs` |
| `internal/agent/canvas/canvas.go` | replaced | `src/agent.rs; src/runtime.rs` |
| `internal/agent/canvas/stream.go` | replaced | `src/agent.rs` |
| `internal/agent/component/data_operations.go` | aligned | `src/agent.rs; src/runtime.rs` |
| `internal/agent/component/io/markdown_writer.go` | aligned | `src/document_writer.rs` |
| `internal/agent/component/io/txt_writer.go` | aligned | `src/document_writer.rs` |
| `internal/agent/component/list_operations.go` | aligned | `src/agent.rs` |
| `internal/agent/component/string_transform.go` | aligned | `src/agent.rs` |
| `internal/agent/component/switch.go` | aligned | `src/agent.rs` |
| `internal/agent/dsl/errors.go` | replaced | `src/agent.rs; src/api/features.rs` |
| `internal/agent/dsl/extract.go` | aligned | `src/agent.rs; src/api/features.rs; src/server.rs` |
| `internal/agent/dsl/normalize.go` | aligned | `src/agent.rs; src/api/features.rs; src/server.rs` |
| `internal/agent/dsl/reset.go` | aligned | `src/agent.rs; src/api/features.rs; src/server.rs` |
| `internal/agent/runtime/component.go` | replaced | `src/agent.rs` |
| `internal/agent/runtime/context.go` | replaced | `src/agent.rs` |
| `internal/agent/runtime/helpers.go` | aligned | `src/runtime.rs` |
| `internal/agent/runtime/state.go` | replaced | `src/agent.rs` |
| `internal/agent/runtime/template.go` | aligned | `src/agent.rs` |
| `internal/handler/chat_recommendation.go` | aligned | `src/api/system.rs; src/server.rs` |
| `internal/service/related_question.go` | aligned | `src/api/system.rs; src/llm.rs; src/server.rs` |
| `mcp/server/server.py` | full | `src/mcp_server.rs` |
| `memory/services/messages.py` | full | `src/memory.rs; src/api/joint_services.rs; src/api/features.rs` |
| `memory/utils/es_conn.py` | full | `src/memory.rs` |
| `memory/utils/infinity_conn.py` | full | `src/memory.rs` |
| `memory/utils/prompt_util.py` | full | `src/memory.rs; src/api/features.rs` |
| `pyproject.toml` | replaced | `Cargo.toml; Cargo.lock` |
| `rag/advanced_rag/__init__.py` | full | `src/advanced_rag.rs` |
| `rag/advanced_rag/knowlege_compile/_common.py` | full | `src/advanced_rag.rs` |
| `rag/advanced_rag/knowlege_compile/dataset_nav.py` | full | `src/advanced_rag.rs` |
| `rag/advanced_rag/knowlege_compile/raptor.py` | full | `src/advanced_rag.rs` |
| `rag/advanced_rag/knowlege_compile/structure.py` | full | `src/advanced_rag.rs` |
| `rag/advanced_rag/knowlege_compile/wiki.py` | full | `src/advanced_rag.rs` |
| `rag/app/audio.py` | full | `src/app_parsers.rs` |
| `rag/app/book.py` | full | `src/app_parsers.rs` |
| `rag/app/email.py` | full | `src/app_parsers.rs` |
| `rag/app/laws.py` | full | `src/app_parsers.rs` |
| `rag/app/manual.py` | full | `src/app_parsers.rs` |
| `rag/app/naive.py` | full | `src/app_parsers.rs` |
| `rag/app/one.py` | full | `src/app_parsers.rs` |
| `rag/app/paper.py` | full | `src/app_parsers.rs` |
| `rag/app/picture.py` | full | `src/parser/figure.rs; src/vision.rs; src/ocr.rs; src/pipeline/mod.rs; src/server.rs` |
| `rag/app/presentation.py` | aligned | `src/presentation.rs; src/parser/ppt.rs` |
| `rag/app/qa.py` | full | `src/app_parsers.rs` |
| `rag/app/resume.py` | aligned | `src/resume.rs` |
| `rag/app/table.py` | aligned | `src/table.rs; src/parser/excel.rs` |
| `rag/app/tag.py` | full | `src/app_parsers.rs` |
| `rag/benchmark.py` | full | `src/benchmark.rs` |
| `rag/flow/chunker/title_chunker/common.py` | full | `src/chunk/title.rs` |
| `rag/flow/chunker/title_chunker/group_chunker.py` | full | `src/chunk/title.rs` |
| `rag/flow/chunker/token_chunker.py` | full | `src/chunk/token.rs` |
| `rag/flow/extractor/extractor.py` | full | `src/extractor.rs` |
| `rag/flow/parser/parser.py` | full | `src/pipeline/flow.rs` |
| `rag/flow/parser/pdf_chunk_metadata.py` | full | `src/pipeline/flow.rs` |
| `rag/flow/parser/utils.py` | full | `src/pipeline/flow.rs` |
| `rag/flow/pipeline.py` | full | `src/pipeline/flow.rs` |
| `rag/flow/tokenizer/tokenizer.py` | full | `src/pipeline/flow.rs` |
| `rag/graphrag/entity_resolution.py` | full | `src/structure_compile.rs; src/prompts.rs` |
| `rag/graphrag/general/smoke.py` | full | `src/structure_compile.rs` |
| `rag/graphrag/light/smoke.py` | full | `src/structure_compile.rs` |
| `rag/graphrag/search.py` | full | `src/structure_compile.rs` |
| `rag/llm/__init__.py` | full | `src/llm.rs` |
| `rag/llm/chat_model.py` | full | `src/llm.rs` |
| `rag/llm/cv_model.py` | full | `src/llm.rs` |
| `rag/llm/embedding_model.py` | full | `src/llm.rs` |
| `rag/llm/key_utils.py` | aligned | `src/model_meta.rs; src/api/features.rs; src/api/tenant_models.rs` |
| `rag/llm/model_meta.py` | full | `src/llm.rs` |
| `rag/llm/ocr_model.py` | full | `src/llm.rs` |
| `rag/llm/rerank_model.py` | full | `src/llm.rs` |
| `rag/llm/sequence2txt_model.py` | full | `src/llm.rs` |
| `rag/llm/tool_decorator.py` | full | `src/llm.rs` |
| `rag/llm/tts_model.py` | full | `src/llm.rs` |
| `rag/nlp/__init__.py` | full | `src/nlp.rs` |
| `rag/nlp/query.py` | full | `src/nlp.rs` |
| `rag/nlp/search.py` | full | `src/nlp.rs` |
| `rag/nlp/term_weight.py` | full | `src/nlp.rs` |
| `rag/prompts/ask_summary.md` | full | `src/prompts.rs` |
| `rag/prompts/cross_languages_sys_prompt.md` | full | `src/prompts.rs` |
| `rag/prompts/generator.py` | full | `src/chat.rs` |
| `rag/prompts/vision_llm_figure_describe_prompt.md` | full | `src/prompts.rs` |
| `rag/prompts/vision_llm_figure_describe_prompt_with_context.md` | full | `src/prompts.rs` |
| `rag/svr/sync_data_source.py` | full | `src/task_executor.rs; src/api/file_mgr.rs` |
| `rag/svr/task_executor.py` | full | `src/task_executor.rs; src/api/features.rs; src/api/document.rs; src/server.rs` |
| `rag/svr/task_executor_refactor/chunk_builder.py` | full | `src/task_executor.rs; src/api/file_mgr.rs` |
| `rag/svr/task_executor_refactor/chunk_post_processor.py` | full | `src/task_executor.rs; src/api/file_mgr.rs` |
| `rag/svr/task_executor_refactor/chunk_service.py` | full | `src/task_executor.rs; src/api/file_mgr.rs` |
| `rag/svr/task_executor_refactor/dataflow_service.py` | full | `src/task_executor.rs; src/api/file_mgr.rs` |
| `rag/svr/task_executor_refactor/dataset_skill_generator.py` | full | `src/task_executor.rs; src/api/file_mgr.rs` |
| `rag/svr/task_executor_refactor/dataset_wiki_generator.py` | full | `src/task_executor.rs; src/api/file_mgr.rs` |
| `rag/svr/task_executor_refactor/embedding_utils.py` | full | `src/task_executor.rs; src/api/file_mgr.rs` |
| `rag/svr/task_executor_refactor/raptor_service.py` | full | `src/task_executor.rs; src/api/file_mgr.rs` |
| `rag/svr/task_executor_refactor/task_context.py` | full | `src/task_executor.rs; src/api/file_mgr.rs` |
| `rag/svr/task_executor_refactor/task_handler.py` | full | `src/task_executor.rs; src/api/file_mgr.rs` |
| `rag/utils/azure_sas_conn.py` | replaced | `src/store/mod.rs; src/persistence.rs` |
| `rag/utils/azure_spn_conn.py` | replaced | `src/store/mod.rs; src/persistence.rs` |
| `rag/utils/es_conn.py` | replaced | `src/store/mod.rs; src/persistence.rs` |
| `rag/utils/file_utils.py` | replaced | `src/store/mod.rs; src/persistence.rs` |
| `rag/utils/infinity_conn.py` | replaced | `src/store/mod.rs; src/persistence.rs` |
| `rag/utils/ob_conn.py` | replaced | `src/store/mod.rs; src/persistence.rs` |
| `rag/utils/opensearch_conn.py` | replaced | `src/store/mod.rs; src/persistence.rs` |
| `rag/utils/raptor_utils.py` | replaced | `src/store/mod.rs; src/persistence.rs` |
| `rag/utils/redis_conn.py` | replaced | `src/store/mod.rs; src/persistence.rs` |
| `rag/utils/s3_conn.py` | replaced | `src/storage.rs` |
| `rag/utils/table_es_metadata.py` | replaced | `src/store/mod.rs; src/persistence.rs` |
| `rag/utils/tavily_conn.py` | replaced | `src/store/mod.rs; src/persistence.rs` |
| `uv.lock` | replaced | `Cargo.lock` |
| `web/src/components/confirm-delete-dialog.tsx` | aligned | `src/web.rs; src/web_css.txt` |
| `web/src/components/model-tree-select.tsx` | partial | `src/web.rs; src/web_css.txt; src/api/tenant_models.rs` |
| `web/src/components/tree-select.tsx` | partial | `src/web.rs; src/web_css.txt` |
| `web/src/layouts/components/global-navbar.tsx` | aligned | `src/web.rs; src/web_css.txt` |
| `web/src/pages/admin/sandbox-settings.tsx` | partial | `src/web.rs; src/api/system_settings.rs` |
| `web/src/pages/admin/user-detail.tsx` | partial | `src/web.rs; src/api/admin.rs` |
| `web/src/pages/admin/users.tsx` | partial | `src/web.rs; src/api/admin.rs; src/web_css.txt` |
| `web/src/pages/agents/agent-card.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/agents/agent-log-page.tsx` | aligned | `src/web.rs; src/api/agent_log.rs` |
| `web/src/pages/agents/agent-tag-editor.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/agents/agent-templates.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/agents/constant.ts` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/agents/create-agent-dialog.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/agents/create-agent-form.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/agents/hooks/use-export-agent-log.ts` | aligned | `src/web.rs` |
| `web/src/pages/agents/hooks/use-select-filters.ts` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/agents/index.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/agents/template-card.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/agents/template-sidebar.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/agents/upload-agent-dialog/upload-agent-form.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/agents/use-import-json.ts` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/dataset/testing/testing-form.tsx` | partial | `src/web.rs; web/static/` |
| `web/src/pages/datasets/dataset-creating-dialog.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/datasets/hooks.ts` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/datasets/index.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/datasets/use-select-owners.ts` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/files/action-cell.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/files/create-folder-dialog/create-folder-form.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/files/files-table.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/files/hooks.ts` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/files/index.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/files/link-to-dataset-dialog.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/files/move-dialog.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/home/agent-list.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/home/applications.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/home/datasets.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/login-next/index.tsx` | partial | `src/web.rs; src/web_css.txt` |
| `web/src/pages/memories/constants/index.tsx` | aligned | `src/web.rs; src/server.rs; src/api/features.rs` |
| `web/src/pages/memories/hooks.ts` | aligned | `src/web.rs; src/server.rs; src/api/features.rs` |
| `web/src/pages/memories/index.tsx` | aligned | `src/web.rs; src/server.rs; src/api/features.rs` |
| `web/src/pages/memories/memory-card.tsx` | aligned | `src/web.rs; src/server.rs; src/api/features.rs` |
| `web/src/pages/memory/memory-message/hook.ts` | aligned | `src/web.rs; src/server.rs; src/api/features.rs` |
| `web/src/pages/memory/memory-message/message-table.tsx` | aligned | `src/web.rs; src/web_css.txt; src/server.rs; src/api/features.rs` |
| `web/src/pages/memory/memory-setting/advanced-settings-form.tsx` | aligned | `src/web.rs; src/server.rs; src/api/features.rs` |
| `web/src/pages/memory/memory-setting/basic-form.tsx` | aligned | `src/web.rs; src/server.rs; src/api/features.rs` |
| `web/src/pages/memory/memory-setting/index.tsx` | aligned | `src/web.rs; src/server.rs; src/api/features.rs` |
| `web/src/pages/memory/memory-setting/memory-model-form.tsx` | aligned | `src/web.rs; src/server.rs; src/api/features.rs; src/api/tenant_models.rs` |
| `web/src/pages/memory/sidebar/index.tsx` | aligned | `src/web.rs; src/server.rs; src/api/features.rs` |
| `web/src/pages/next-chats/index.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/next-search/hooks.ts` | partial | `src/web.rs; src/api/system.rs; src/api/searchapp_mgr.rs` |
| `web/src/pages/next-search/search-setting.tsx` | partial | `src/web.rs; src/web_css.txt; src/api/searchapp_mgr.rs` |
| `web/src/pages/next-search/search-view.tsx` | partial | `src/web.rs; src/web_css.txt` |
| `web/src/pages/next-searches/index.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/next-searches/search-card.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/next-searches/search-dropdown.tsx` | aligned | `src/web.rs; web/static/` |
| `web/src/pages/skills/components/search-config-modal.tsx` | partial | `src/web.rs; web/static/` |
| `web/src/pages/user-setting/chat-channel/add-channel-modal.tsx` | full | `src/web.rs; src/api/chat_channel_mgr.rs` |
| `web/src/pages/user-setting/chat-channel/component/added-channel-card.tsx` | aligned | `src/web.rs; src/web_css.txt` |
| `web/src/pages/user-setting/chat-channel/component/delete-channel-modal.tsx` | aligned | `src/web.rs; src/api/chat_channel_mgr.rs` |
| `web/src/pages/user-setting/chat-channel/constant/index.tsx` | partial | `src/web.rs` |
| `web/src/pages/user-setting/chat-channel/hooks.ts` | aligned | `src/web.rs; src/api/chat_channel_mgr.rs` |
| `web/src/pages/user-setting/chat-channel/index.tsx` | aligned | `src/web.rs; src/web_css.txt; src/api/chat_channel_mgr.rs` |
| `web/src/pages/user-setting/data-source/add-datasource-modal.tsx` | aligned | `src/web.rs; src/api/data_source_mgr.rs; src/data_source/form_fields.rs` |
| `web/src/pages/user-setting/data-source/component/added-source-card.tsx` | aligned | `src/web.rs; src/data_source/form_fields.rs` |
| `web/src/pages/user-setting/data-source/component/box-token-field.tsx` | partial | `src/web.rs; src/api/oauth_web.rs` |
| `web/src/pages/user-setting/data-source/component/delete-source-modal.tsx` | aligned | `src/web.rs` |
| `web/src/pages/user-setting/data-source/component/gmail-token-field.tsx` | partial | `src/web.rs; src/api/oauth_web.rs` |
| `web/src/pages/user-setting/data-source/component/google-drive-token-field.tsx` | aligned | `src/web.rs; src/api/oauth_web.rs` |
| `web/src/pages/user-setting/data-source/constant/bitbucket-constant.tsx` | aligned | `src/data_source/form_fields.rs; src/data_source/form_schema.json` |
| `web/src/pages/user-setting/data-source/constant/confluence-constant.tsx` | aligned | `src/data_source/form_fields.rs; src/data_source/form_schema.json` |
| `web/src/pages/user-setting/data-source/constant/index.tsx` | aligned | `src/data_source/form_fields.rs; src/data_source/form_schema.json` |
| `web/src/pages/user-setting/data-source/constant/jira-constant.tsx` | aligned | `src/data_source/form_fields.rs; src/data_source/form_schema.json` |
| `web/src/pages/user-setting/data-source/constant/s3-constant.tsx` | aligned | `src/data_source/form_fields.rs; src/data_source/form_schema.json` |
| `web/src/pages/user-setting/data-source/constant/seafile-constant.tsx` | aligned | `src/data_source/form_fields.rs; src/data_source/form_schema.json` |
| `web/src/pages/user-setting/data-source/data-source-detail-page/index.tsx` | aligned | `src/web.rs; src/api/data_source_mgr.rs; src/data_source/form_fields.rs` |
| `web/src/pages/user-setting/data-source/data-source-detail-page/log-table.tsx` | aligned | `src/web.rs; src/api/data_source_mgr.rs` |
| `web/src/pages/user-setting/data-source/index.tsx` | partial | `src/web.rs; src/data_source/form_fields.rs` |
| `web/src/pages/user-setting/index.tsx` | partial | `src/web.rs; src/web_css.txt; src/server.rs` |
| `web/src/pages/user-setting/mcp/edit-mcp-dialog.tsx` | full | `src/web.rs; src/web_css.txt; src/api/mcp_mgr.rs` |
| `web/src/pages/user-setting/mcp/edit-mcp-form.tsx` | full | `src/web.rs; src/api/mcp_mgr.rs` |
| `web/src/pages/user-setting/mcp/import-mcp-dialog/import-mcp-form.tsx` | full | `src/web.rs; src/api/mcp_mgr.rs` |
| `web/src/pages/user-setting/mcp/import-mcp-dialog/index.tsx` | full | `src/web.rs; src/api/mcp_mgr.rs` |
| `web/src/pages/user-setting/mcp/index.tsx` | full | `src/web.rs; src/web_css.txt; src/api/mcp_mgr.rs` |
| `web/src/pages/user-setting/mcp/mcp-card.tsx` | full | `src/web.rs; src/web_css.txt` |
| `web/src/pages/user-setting/mcp/mcp-operation.tsx` | full | `src/web.rs; src/api/mcp_mgr.rs` |
| `web/src/pages/user-setting/mcp/tool-card.tsx` | full | `src/web.rs` |
| `web/src/pages/user-setting/mcp/use-bulk-operate-mcp.tsx` | full | `src/web.rs; src/api/mcp_mgr.rs` |
| `web/src/pages/user-setting/mcp/use-edit-mcp.ts` | full | `src/web.rs; src/api/mcp_mgr.rs` |
| `web/src/pages/user-setting/mcp/use-export-mcp.ts` | full | `src/web.rs; src/api/mcp_mgr.rs` |
| `web/src/pages/user-setting/mcp/use-import-mcp.ts` | full | `src/web.rs; src/api/mcp_mgr.rs` |
| `web/src/pages/user-setting/profile/constants.ts` | partial | `src/web.rs; src/api/system.rs` |
| `web/src/pages/user-setting/profile/hooks/use-profile.ts` | aligned | `src/web.rs; src/api/system.rs` |
| `web/src/pages/user-setting/profile/index.tsx` | partial | `src/web.rs; src/web_css.txt; src/api/system.rs` |
| `web/src/pages/user-setting/setting-model/index.tsx` | partial | `src/web.rs; src/web_css.txt; src/server.rs; src/api/tenant_models.rs; src/providers.rs` |
| `web/src/pages/user-setting/setting-model/langfuse/index.tsx` | aligned | `src/web.rs; src/web_css.txt; src/api/langfuse.rs` |
| `web/src/pages/user-setting/setting-model/langfuse/langfuse-configuration-dialog.tsx` | aligned | `src/web.rs; src/web_css.txt; src/api/langfuse.rs` |
| `web/src/pages/user-setting/setting-model/langfuse/langfuse-configuration-form.tsx` | aligned | `src/web.rs; src/api/langfuse.rs` |
| `web/src/pages/user-setting/setting-model/langfuse/use-save-langfuse-configuration.tsx` | aligned | `src/web.rs; src/api/langfuse.rs` |
| `web/src/pages/user-setting/setting-team/add-user-modal.tsx` | aligned | `src/web.rs; src/api/system.rs` |
| `web/src/pages/user-setting/setting-team/hooks.ts` | aligned | `src/web.rs; src/api/system.rs` |
| `web/src/pages/user-setting/setting-team/index.tsx` | aligned | `src/web.rs; src/web_css.txt; src/api/system.rs` |
| `web/src/pages/user-setting/setting-team/tenant-table.tsx` | aligned | `src/web.rs; src/web_css.txt; src/api/system.rs` |
| `web/src/pages/user-setting/setting-team/user-table.tsx` | aligned | `src/web.rs; src/api/system.rs` |
| `web/src/pages/user-setting/sidebar/hooks.tsx` | partial | `src/web.rs; src/web_css.txt; src/server.rs` |
| `web/src/pages/user-setting/sidebar/index.tsx` | aligned | `src/web.rs; src/web_css.txt; src/server.rs` |
| `web/src/routes.tsx` | partial | `src/server.rs; src/web.rs` |

## 执行顺序与证据标准

- 顺序建议：W-3（小、独立）→ W-1 → W-6 → W-2 → W-5 → W-4（最大，按子模块分批）。UI 残留随任务-3 的实点批次；re-audit 队列按状态分组插入 W 批次之间。
- 每批证据：`cargo fmt --check` + `cargo check --locked --features postgres-backend,zvec-backend` + 定向单测；涉及部署跑 `docker compose build/up` 健康检查；台账行更新为 aligned/partial 并写明批次；`运维记录.md` 追加；每轮发布新版本号（GitHub Release）。
- 纪律：先查证据再下结论；不得无证据翻正；reranker 停机期间不做其深度重测；内部资产（本清单、台账、运维记录、参数登记表）禁止发布。

