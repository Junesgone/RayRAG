# RAGFlow v0.27.2 Parity Worklist（内部工作清单，禁止发布）

> 来源：`docs/ragflow-v0.27.2-coverage.md`（固定 commit `a024bea0cd93f39e6652a42bf84dd20c55bc560b`，5977 文件）。
> 生成：`scripts/gen-parity-worklist.py`（v0.3.10g 起；含人工维护的「方法任务」区）；台账状态变更后重跑刷新。
> 用途：v0.27.2 对标收敛的有序工作队列；每批完成后回写台账并刷新本清单。

## 现状摘要

- 文件总数：5977；unmapped=0（v0.3.10g 全量分类完成）
- partial：4（UI 残留 4 + 实施队列 0）
- re-audit：0（上游内容变更、需复核对齐的已审计行）
- replaced / N/A / reference：无逐文件动作（基础设施替代与审计参考材料）

## 方法任务（人工维护）

| 任务 | 状态 | 说明 |
|---|---|---|
| **REA 逆向分析（RAGFlow v0.27.2 全量行为与代码）** | **进行中**（2026-10-11 完成安装 / 接入与首轮执行；逐模块分析进行中——01 登录链 ✓、02 接口面与迁移 ✓） | 需求来源：项目便签（RayRAG项目）。使用 `morluto/rea`（`rea-agents@6.4.0`，面向二进制 / JS / Electron / .NET / 网站行为的 MCP 逆向工具；Hermes MCP 已注册、170 工具、实测 ✓ Connected）对 v0.27.2 参考树（固定 commit `a024bea0cd93`）与镜像前端产物做逆向分析，把分析结果、方法、行为应用到 RayRAG；产出逐模块分析记录并回写台账与实施队列。进展（2026-10-11）：安装 / Hermes 接入 / 连通 ✓；web-dist 导出（707 JS / 39.1MB）✓；参考树导入（6754 条）✓；静态重构（707 解析 / 7.78M AST 节点 / 25,160 findings）✓；source↔bundle 对比两轮跑通（无 sourcemap，逐文件映射仅 2 项；转语义图 + 运行观测路径，见运维记录 10-11 续记②；续记③：MCP 直达工具全量上线、登录链逐模块分析 ✓（`notes/01-登录链.md`）、`/api/v1/*` 新 REST 面对标增量在案；续记④：API 面三源提取（前端 87 路径 / 官方文档 86 端点 / 实例探针）+ 官方迁移表 19 行 + 新面家族确认（`docs/rea-v0.27.2/api-surface-v0.27.2.md`）；续记⑤：检索与对话链运行取证 ✓（6 端点 200 响应形状 + `chat/completions` 默认模型悬挂实证 → `docs/rea-v0.27.2/runtime-surface-v0.27.2.md`）；续记⑥：OpenAI 兼容面运行取证 ✓（10 探针矩阵 + 源码契约六文件；两段式错误哲学 → `docs/rea-v0.27.2/openai-surface-v0.27.2.md`）；续记⑦：Search App（searches/*）运行取证 ✓（16 探针含净零生命周期 + searchbots 认证族发现 → `docs/rea-v0.27.2/search-surface-v0.27.2.md`）；续记⑧：Memory 家族与 workspace 提交面运行取证 ✓（18 探针全只读 + **实例记忆检索缺陷查实** + 默认提取提示词资产 → `docs/rea-v0.27.2/memory-surface-v0.27.2.md`）；续记⑨：文件提交面运行取证 ✓（源码全文 + 12 探针 + **错误三态与 RetCode 全表修正** → `docs/rea-v0.27.2/file-commit-surface-v0.27.2.md`）；续记⑩：UI 面对照全量提取 ✓（侧栏 7 项/65 路由/55 懒加载块/en·zh 标签资产 → `docs/rea-v0.27.2/ui-map-v0.27.2.md`）；续记⑪：sessions/audio/mindmap/recommendation 运行取证 ✓（源码段全文 + 15 探针 + `get_data_error_result` 双风格定型 → `docs/rea-v0.27.2/sessions-audio-surface-v0.27.2.md`）；续记⑫：**分析阶段 01–09 完成**（2026-10-12 01:0x）＋应用交叉核对首轮 ✓（7 行台账回写 REA 交叉引用、agent_api 行陈旧「remain open」修正（v0.3.8n 286d586）、信封边界注记 → `docs/rea-v0.27.2/apply-crosscheck-01.md`）；续记⑬：UI 路由对账 ✓（上游 3 形式浏览器实测 + RayRAG 51 探针：37×200 / 8×307 / 6×404 全部语义正确；`//` 形式两侧 404 一致；遍历增补建议 4 条 + locale 裁决项 → `docs/rea-v0.27.2/ui-route-crosscheck-v0.27.2.md`）；续记⑭：文档面（document_api.py）静态全文 + 39 探针 ✓（19 路由契约逐条实测、信封/码值、混合信封再确认（部分上传/批状态 = HTTP 200 + code=500）、空选择器宽容、静默分页回退；探针零写 → `docs/rea-v0.27.2/document-surface-v0.27.2.md`）；续记⑮：文档面应用交叉核对 ✓（40 行逐项对照四级分类——✓7 等价 / 含 T19 / 含 C18 / 含 S6 / 含 F4 / 备注 3；修正批①–④ 计划在案；ledger document_api 行注记精细化并修复一处 stray 管道符 → `docs/rea-v0.27.2/apply-crosscheck-02.md`）；续记⑯：文档面修正批①（文案级）落地 ✓（30 处对照编辑——list/batch/delete/parse/stop/rename/preview/download/images/summary/metadata-config 文案族；`selector` falsy 归一；env_surface 接线缺陷修复；`cargo test --locked` 2271/0；批②已落地（续记⑰）：文档面码值/状态级 —— 体内 400→101/102、404→102，34 处编辑、5 文件；create-route 鉴权 102/109 与 No file part! 101；images、doc 级 metadata-config 200+102；upload 业务错误 HTTP 200；ingest 四处经 round11 探针取证后冻结；测试同步 7 断言；`cargo test --locked` 2271/0；批③已落地（续记⑱）：文档面形状级 —— list `total` 双键+message、delete `{deleted,success_count}`、upload 成功=数组/失败=500 换行 message/部分=混合信封、ingest 成功/取消 `data:true`、thumbnails 省略键+message、batch 成功 message；新增 `api_error_result` 无-data 错误形态并切文档面错误族；42 处编辑、5 文件；`cargo test --locked` 2271/0；批④已落地（续记⑲）：功能级——④a 列表查询面/分页（b379c03）、④b ingest 语义（2608e78）、④d file-commit 信封逐字复刻（31815fe，owner 决策：逐字复刻）；④c metadata/config raw-body 校验落地（726d9c9，round12 实测归证）；④e PATCH 字段面落地（21e994f，round13 实测 26 键成功体 + 17 校验臂、构造性零写；已审计差异三项在案）；批④ 全清 → 合并 v0.46.0 发版）；续记⑳（2026-10-20）：v0.27.2 编译模板参考数据复核——发现 v0.26.4 陈旧种子并刷新为 v0.27.2（kind artifacts→wiki、mode/example 字段、empty 模板移除、wiki presets example 键；8 builtins / 6 presets）；edit-next 深层表单（模式/RAPTOR/分节字段/蓝图/JSON 预览）落地，11 行 UI residual 转 aligned；v0.47.0 发版）；续记㉑（2026-10-20）：数据集 Artifacts/Wiki 编译面 UI 落地（5 视图模式空态、Contents/Topics→Pages、markdown 详情与 wiki link、Update/Clear、真实 MAP→COMPILE 端到端生成），Knowledgebase.pipeline_id 顶层列落地，compile 并入 engine chunk 视图修复新解析数据集 pages_created=0；CDP 26/26；v0.48.0 发版）；续记㉒（2026-10-21）：搜索设置抽屉头像改用共享 avatar-upload 裁剪弹窗（挂全局 layout）+ SearchAppSeed.avatar 列持久化（POST/PUT）+ 移除按钮与双语校验；admin 控制台 9 个 CDP 脚本批次复核实点（8 行 partial→aligned）；search-setting 行 aligned；CDP 23/23；v0.49.0 发版）；续记㉓（2026-10-21）：上游 svgIcons 固定 provider 图片资产补齐（29 个 SVG 全量入库 + mapping 覆盖 AIMLAPI/Astraflow/FastEmbed/FunASR/FuturMix/GreenPT/MWS/Synthorai/Xiaomi，单测断言资产存在）；profile-page CDP 对齐共享裁剪弹窗并补姓名规则断言；login-next/skills/chat-channel/setting-model/dataset-testing/profile 批次复核，8 行 partial→aligned；v0.50.0 发版）；续记㉔（2026-10-21）：路由表复核（/chunk 四视图、/chats/widget、agentbots logs 均已注册）；user-setting 壳改 upstream 精确 303/64、data-source 网格改 viewport 断点（lg=2/2xl=4）；修复搜索抽屉 touched 别名 bug（thresh→threshold、vWeight→weight，滑块编辑才会进请求体）；新增 3 个 CDP 探针，5 行 partial→aligned；v0.51.0 发版）；续记㉕（2026-10-21）：search_view 端到端（mock chat model + search-answer CDP：AI summary SSE 101 字符/1 citation、mind map 7 节点、related 3、0 pageError）；修复 search_complete 流式桥 bounded blocking_send panic（改 unbounded）与 ?sapp= applySapp 提前调用 searchRerankToggle 的 ReferenceError；tree-select 旧 Radix focus residual 撤回（上游行为普通 div）；search-view + tree-select 两行 aligned；v0.52.0 发版）。 |

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

### W-4 Agentic harness 移植（最大队列）（0 项）

| 文件 | 行数 | RayRAG 对应 | 台账注记 |
|---|---:|---|---|

### W-5 结构图/编译器/切分（0 项）

| 文件 | 行数 | RayRAG 对应 | 台账注记 |
|---|---:|---|---|

### W-6 沙箱提供商与鉴权（0 项）

| 文件 | 行数 | RayRAG 对应 | 台账注记 |
|---|---:|---|---|

## UI 对齐残留（4 项，任务-3 的 CDP 实点批次收敛）

| 文件 | 行数 | RayRAG 对应 | 台账注记 |
|---|---:|---|---|
| `web/src/components/model-tree-select.tsx` | 309 | `src/web.rs; src/web_css.txt; src/api/tenant_models.rs` | Rust Model providers now builds the fixed provider→instance→model selector tree in catalog order, filters all six capabilities, normalizes `__` names, deduplicates exact selectors, defaults branches open, renders selected model/instance, and persists canonical default wires. Isolated Chrome real clicks cover every selector, branch/search/No data, leaf save, clear, Escape/outside, rollback and reload. Fixed provider image assets and every non-Model-providers consumer of this shared React component remain pending (v0.3.3bn). First componentization step landed: the trigger+popover+search+`role='tree'` markup is now the standalone reusable `web::push_default_model_selector(&mut String, &DefaultModelField)` instead of inline `providers_page` code; proven output-neutral by byte-diff of the deployed `/user-setting/model` page (321502 B before == after). Cross-page consumers still blocked on extracting the tree JS (`modelField`/`defaultModelState`/`toggleDefaultModelTree`/`searchDefaultModelTree`), which remains providers-page-local (v0.3.4b). v0.3.4f: the behaviour script is extracted to module consts `DEFAULT_MODEL_TREE_JS_HEAD`/`DEFAULT_MODEL_TREE_JS_BODY` (byte-identical construction proof; providers page output unchanged) — cross-page consumers unblocked. v0.3.4g: first consumer landed — the kb/dataset configuration embedding field wires the shared markup + behaviour script with a form persist hook (`saveModelDefault`), a catalog loader (`loadKbModelCatalog`) and a stored-value normalizer (server embd_id round-trips as the internal provider/instance/model id); CDP real-click verified with save and reload persistence (v0.3.4g); v0.3.4j: Radix keyboard parity on the shared selector — ArrowDown/ArrowUp wrap navigation, Home/End jumps, typeahead by title prefix and Enter-select, with scrollIntoView({block:'nearest'}) on the focused row; CDP keyboard flow verified (ArrowDown x2, then 'q' typeahead, End, Enter, save, reload) (v0.3.4j); v0.3.4p: page-local saveModelDefault typeMaps drop reranking_id (retrieval-test rerank is form-local per upstream testing-form) with a form-local fallback for unmapped types, and check_model_available validates the tenant instance's own model rows before the factory gate so tenant-local providers can persist defaults — CDP real-click verified (composite pick, retrieval body rerank_id, no default PATCH); v0.3.4q: memory-model-form consumer landed on the shared selector (embd/llm form-local save), CDP real-click verified end-to-end [v0.27.2 re-audit: 复核通过（映射在库）]；v0.50.0：上游 svgIcons 的固定 provider 图片资产补齐——29 个 web/src/assets/svg/llm/*.svg 全量入库 web/static/assets/svg/llm，provider_icon 覆盖 aimlapi.com/Astraflow(-CN)/FastEmbed/FunASR/FuturMix/GreenPT/MWS/Synthorai/Xiaomi 等并加单测断言资产存在；剩余未接的跨页 consumer（agent parser 表单、chat LLM 设置项、layout-recognize）仍记 partial |
| `web/src/pages/admin/sandbox-settings.tsx` | 1048 | `src/web.rs; src/api/system_settings.rs` | v0.3.5y: `/admin/sandbox-settings` carries the upstream title and description (`Configure your code execution sandbox provider. The sandbox is used by the Code component in agents.`) and hosts the durable sandbox variables table (name/type/value + per-row Save over `/api/v1/system/variables`) that the local executor reads. v0.3.7r: the upstream provider surface is implemented — `/api/v1/admin/sandbox/{providers,providers/{id}/schema,config,test}` carry the exact five-provider registry and per-provider schemas, the page renders the provider grid, the schema-driven configuration card and the connection-test modal, and the probe runs the upstream source in the local sandbox or through sandbox-executor-manager. v0.3.8j: **the SSH provider now executes code for real** — `src/ssh_exec.rs` implements upstream `SSHProvider`'s field set, either credential, the configured `python_bin`/`node_bin` and the `work_dir` workspace, running the snippet through the system OpenSSH client (`ssh`, password auth via `sshpass -f`) and returning `{success, message, details{exit_code, execution_time, stdout, stderr}}`; the runtime image gains `openssh-client` and `sshpass`. **Remaining semantic difference**: the E2B and Aliyun providers store configuration and validate it, but *code execution* through them is still not implemented in this build (the test endpoint reports that instead of faking a pass); the durable variables card is a RayRAG addition (upstream has no such table on this page). [v0.27.2 re-audit: 复核通过（映射在库）] |
| `web/src/pages/user-setting/data-source/component/box-token-field.tsx` | 468 | `src/web.rs; src/api/oauth_web.rs` | Rust Box token field replaces the raw textarea: Configure/Get Box credentials button, Configured/Authorized chips, Client ID/Secret/Redirect URI dialog, Submit & Authorize popup + Refresh status polling against the implemented start/callback/result endpoints; CDP-verified the full start→pending(106)→callback→authorized→create loop with an offline-aborted popup. Gmail/Drive fields and the real Box token exchange (requires `BOX_OAUTH_TOKEN_URL`) remain partial (v0.3.3cd) [v0.27.2 re-audit: 复核通过（映射在库）] |
| `web/src/pages/user-setting/data-source/component/gmail-token-field.tsx` | 477 | `src/web.rs; src/api/oauth_web.rs` | Rust serves the shared Google token field for Gmail (JSON uploader, redirect extraction, Authorize with Google, Refresh status) through the implemented Google OAuth endpoints; the per-provider popup listener and real token minting remain partial (v0.3.3ce) [v0.27.2 re-audit: 复核通过（映射在库）] |

## 复核对账队列（re-audit，0 项）

上游 v0.27.2 相对 v0.26.4 变更过的已审计行；逐行复核后移除 `[v0.27.2 content changed: re-audit]` 标记并更新注记。

| 文件 | 状态 | RayRAG 对应 |
|---|---|---|

## 执行顺序与证据标准

- 顺序建议：W-3（小、独立）→ W-1 → W-6 → W-2 → W-5 → W-4（最大，按子模块分批）。UI 残留随任务-3 的实点批次；re-audit 队列按状态分组插入 W 批次之间。
- 每批证据：`cargo fmt --check` + `cargo check --locked --features postgres-backend,zvec-backend` + 定向单测；涉及部署跑 `docker compose build/up` 健康检查；台账行更新为 aligned/partial 并写明批次；`运维记录.md` 追加；每轮发布新版本号（GitHub Release）。
- 纪律：先查证据再下结论；不得无证据翻正；reranker 停机期间不做其深度重测；内部资产（本清单、台账、运维记录、参数登记表）禁止发布。

