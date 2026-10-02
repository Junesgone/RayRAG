# RayRAG ↔ RAGFlow v0.27.2 剩余接口推进计划（内部文档，不发布）

数据来源：`scripts/api-parity-audit.py`（同名 105 / 别名 38 / 覆盖 2 / **未实现 36** / 方法缺口 1 / 不匹配 0）。

按来源文件分组，并给出**性质**与**前置条件**——性质决定它属于「接线」还是「功能」，前置条件决定它能不能先做。

## dataset_api.py（10 条）

**性质/前置条件**：多为功能级：artifacts 的 structure/graph/alteration 需要编译产物行（wiki 关系行 / 结构编译输出）；embedding/check、metadata/config、index / index_type 各自需要子系统。

- `DELETE /datasets/<dataset_id>/<index_type>`
- `GET /datasets/<dataset_id>/artifacts/alteration`
- `GET /datasets/<dataset_id>/artifacts/graph`
- `DELETE/GET /datasets/<dataset_id>/artifacts/structure`
- `POST /datasets/<dataset_id>/embedding/check`
- `GET /datasets/<dataset_id>/graph`
- `DELETE/GET/POST /datasets/<dataset_id>/index`
- `GET/PUT /datasets/<dataset_id>/metadata/config`
- `DELETE/GET/HEAD /datasets/<dataset_id>/skills`
- `DELETE/GET /datasets/<dataset_id>/skills/<path:skill_kwd>`

## agent_api.py（6 条）

**性质/前置条件**：功能级新增子系统：Webhook（配置/日志/测试）、附件（上传/下载/预览）、导出下载。可独立立项，互不阻塞。

- `POST /agents/<agent_id>/upload`
- `DELETE/GET/HEAD/PATCH/POST/PUT /agents/<agent_id>/webhook`
- `GET /agents/<agent_id>/webhook/logs`
- `DELETE/GET/HEAD/PATCH/POST/PUT /agents/<agent_id>/webhook/test`
- `GET /agents/attachments/<attachment_id>/download`
- `GET /agents/attachments/<attachment_id>/preview`

## document_api.py（5 条）

**性质/前置条件**：缩略图、文档内图片、文档产物读取需要一个「文档产物目录」的落盘约定；`/documents/upload`、`/documents/ingest` 是入口形态差异。

- `PUT /datasets/<dataset_id>/documents/<document_id>/metadata/config`
- `GET /documents/artifact/<filename>`
- `GET /documents/images/<image_id>`
- `POST /documents/upload`
- `GET /thumbnails`

## user_api.py（4 条）

**性质/前置条件**：忘记密码整体流程（验证码/OTP/校验/重置）需要邮件或 OTP 通道，属于独立功能。

- `POST /auth/password/forgot/captcha`
- `POST /auth/password/forgot/otp`
- `POST /auth/password/forgot/otp/verify`
- `POST /auth/password/reset`

## connector_api.py（4 条）

**性质/前置条件**：连接器实例面（`/connectors/{id}` 及其 logs/rebuild/test）与 RayRAG 的 `/data_sources/*` 语义不同，需要先决定是适配还是移植。

- `DELETE/GET/PATCH /connectors/<connector_id>`
- `GET /connectors/<connector_id>/logs`
- `POST /connectors/<connector_id>/rebuild`
- `POST /connectors/<connector_id>/test`

## chat_api.py（2 条）

**性质/前置条件**：TTS 语音合成；独立 ASR 转写接口（现有 ASR 只服务解析流水线）。

- `POST /chat/audio/speech`
- `POST /chat/audio/transcription`

## chunk_api.py（1 条）

**性质/前置条件**：需要逐条确认是接线还是功能。

- `DELETE/GET /datasets/<dataset_id>/documents/<document_id>/structure/graph`

## file2document_api.py（1 条）

**性质/前置条件**：需要逐条确认是接线还是功能。

- `POST /files/link-to-datasets`

## plugin_api.py（1 条）

**性质/前置条件**：上游 pluginlib 插件注册表（LLM 工具元数据）未移植；RayRAG 的画布工具目录在 `/api/v1/components`。

- `GET /plugin/tools`

## bot_api.py（1 条）

**性质/前置条件**：`/searchbots/ask` 的 SSE 帧格式与既有搜索补全不同，需要单独的流式处理器。

- `POST /searchbots/ask`

## file_commit_api.py（1 条）

**性质/前置条件**：需要逐条确认是接线还是功能。

- `GET /workspace-files/<file_id>/versions`

## 方法缺口（路径已实现，缺方法）

- `/datasets/<dataset_id>/navigation` — dataset_api.py [POST]

## 建议顺序

1. **生成任务**（`POST /datasets/{id}/navigation` 与产物生成）：解锁 UI 上 Generate 按钮与 artifacts/navigation 的**数据来源**，是其余产物类接口的前置条件。
2. **Agent Webhook**（3 条）：独立子系统，边界清晰，可一次做完。
3. **Agent 附件**（3 条）：与 Webhook 同属画布侧，共用上传/下载路径。
4. **忘记密码流程**（4 条）：独立且用户可感知；需要先定邮件/OTP 通道的配置项。
5. **文档产物族**（缩略图/图片/产物/上传，5 条）：先定落盘约定，再暴露接口。
6. 其余（连接器实例面、TTS/独立 ASR、搜索机器人流式问答、文件版本历史、插件工具元数据）：按需立项。


## 数据集侧栏对标结论（本轮从上游源码确认）

来源：`web/src/pages/dataset/sidebar/index.tsx`（`items` 列表）与 `web/src/routes.tsx`（路由表）。

上游 v0.27.2 数据集侧栏**恰好 5 项**：

| # | 上游 key | 标签 | 图标 |
|---|---|---|---|
| 1 | `Routes.Files` | `knowledgeDetails.subbarFiles` | FolderOpen |
| 2 | `Routes.DatasetTesting` | `knowledgeDetails.testing` | TextSearch |
| 3 | `Routes.DataSetOverview` | **`knowledgeDetails.overview`** | **Logs** |
| 4 | `Routes.DataSetSetting` | `knowledgeDetails.configuration` | Settings |
| 5 | `Routes.Compilation` | `Artifacts` | BookText |

**知识图谱（`Routes.KnowledgeGraph` = `/knowledge-graph`）在 v0.27.2 里是一条独立路由，不在数据集侧栏**（`routes.tsx:214`），从概览页等处进入。

**RayRAG 现状与差距**：侧栏为 文件列表 / **日志** / 检索测试 / 配置 / Artifacts —— 第 3 项被实现成 **Logs**，而上游这一项的**标签是 Overview（概览）只是复用了 Logs 图标**。

**因此用户「想不起来的那个功能」= 数据集概览（Dataset Overview）**，对应上游 `pages/dataset/dataset-overview/overview-table.tsx`（该文件里同时引用了 knowledgeGraph，概览页是进入知识图谱的入口之一）。

**待办**：
1. 把第 3 项从「日志」改为「概览」，面板内容按上游 overview 表（文档数/分块数/存储/最近活动）实现，日志移入概览页内的入口（上游同样是 Logs 图标 + overview 标签）。
2. 知识图谱：保持不在侧栏（与上游一致），但要给出**可达入口**（上游在概览页/解析配置里的图谱开关），并确认 `#tab4` 的 Build Graph 在接上 `/datasets/{id}/graph` 后真正可用。
3. 用户报障「配置页排版溢出」已在 0.6.9 修复（页面横向溢出 0）。

## 唯一剩下的方法缺口：`POST /datasets/{id}/navigation`（下一轮的施工单）

**上游契约**（`api/apps/restful_apis/dataset_api.py::generate_dataset_nav`，行 1171 起）：
- 方法/路径：`POST /datasets/<dataset_id>/navigation`
- 请求体：`{"documents": [...]}`（可选；缺省表示该数据集全部文档）
- 成功：`get_result(data=result)`——即 `{"code":0,"data":<service 的返回值>}`
- 语义：对该数据集的文档做一遍导航树生成（LLM 聚类 + 嵌入），已存在的树按文档增量更新

**RayRAG 侧已具备的零件**（都已移植且有单测，位于 `src/knowlege_dataset_nav.rs`）：
- `upsert_dataset_nav_doc(store, embed, chat, lock, tenant_id, kb_id, doc_id, summary_or_tree)` —— **单文档入口**（async）：内部完成摘要/图谱文本 → 嵌入 → 找最佳簇 `find_best_cluster` → 必要时 `maybe_split_cluster` → 落行
- `nav_embed(embed, text)`、`llm_merge`、`llm_create_summary`、`readable_cluster_name`、`nav_keywords(_limited)`、`nav_entities(_limited)`
- 行写入用 `store_upsert` / `store_delete`，索引为 `knowlege_dataset_nav::index_name(tenant)`
- 存储已接通：`AppState.doc_store`（`DocStorePool`），**必须用 `spawn_blocking`**（`with_store` 模式，见 `src/api/dataset_artifacts.rs`）

**实现要点（下一轮照此施工）**：
1. 解析请求体 `documents`（缺省 = `state.docs.list(&kb_id)` 的全部文档 id）；
2. 对每个文档取已解析分块文本（`state.engine` 中的索引分块，按 `doc_id` 过滤）拼成 `summary_or_tree`；
3. 取租户默认嵌入与对话模型（`state.tenant_models` + `state.embedder`/`state.llm`）；**缺失时返回 `code 102` 并说明缺哪一个**，不要静默跳过——这正是 UI 上用户需要看到的提示；
4. 逐个文档调用 `upsert_dataset_nav_doc`，**按需、流水线式**（不要一次性把所有文档读进内存，遵循用户的"避免内存泄漏"要求）；
5. 返回值带 `generated: <文档数>`、`skipped: [...]`（无分块/无摘要的文档及其原因），让部分失败可见；
6. 测试：无嵌入模型 → 102 且文案指明缺什么；有分块 → 生成后 `GET /navigation` 能读到节点；无分块的文档 → 计入 `skipped` 而不是静默略过。

**UI 联动**：`/dataset/compilation`（Artifacts）与数据集导航视图在生成后应能立即读到新树（读取接口已实现）。

### 施工前必须先补的一块：`HarnessChat` 的生产适配器（本轮查明的缺件）

**证据**：`grep -rn "impl .*HarnessChat" src/` 的结果**只有测试用的 `FakeChat`**（`knowlege_wiki_incremental.rs` 的 5 处 `#[cfg(test)]`）。也就是说 `crate::harness::HarnessChat` 这个 trait 在**生产路径上没有任何实现**——而导航生成（`upsert_dataset_nav_doc` / `llm_create_summary` / `llm_merge`）与 wiki 生成都需要它。

**因此下一轮的施工顺序应为**：
1. 写 `HarnessChat` 的生产适配器：包住 RayRAG 真实的对话客户端（`state.llm` / `state.tenant_models` 解析出的租户默认对话模型），把 `chat(system, history, gen_conf) -> Result<String,String>` 映射到现有 LLM 调用；**错误要原样带出**（返回 `Err(String)` 而不是空串），否则生成会"成功但什么都没产生"。
2. 视需要在 `AppState` 上暴露一个返回该适配器的访问点（`Arc<dyn HarnessChat>`），或按请求构造。
3. 然后才是 `POST /datasets/{id}/navigation` 本体（本文件上一节的 6 条要点）。

**顺带收益**：`HarnessChat` 一旦有了生产实现，wiki（Artifacts）生成与结构编译的同类缺口也一并解除——这几条一直挂在"引擎已移植但无处调用"的状态。

### 施工配方（本轮把最后几个未知点也查清了，下一轮照抄即可）

四块零件全部就位，且都有可参照的现有代码：

1. **按数据集取分块**（照抄 `src/server.rs:2901` 的图谱构建写法）：
   ```rust
   let chunks: Vec<crate::search::IndexedChunk> = state.engine.read().unwrap().to_vec()
       .into_iter()
       .filter(|c| c.metadata.get("kb_id").map(String::as_str) == Some(kb_id.as_str()))
       .collect();
   ```
   空则按上游语义返回明确错误（图谱那边是 `"No chunks in this knowledge base"`；导航可返回 `code 102` + `skipped` 说明）。
2. **嵌入**：`state.embedder: Option<SharedEmbedder>`（`Arc<dyn EmbeddingBackend>`），直接传给 `upsert_dataset_nav_doc(store, Some(&*embedder), …)`。
3. **对话**：`state.llm: Option<Arc<LlmClient>>` → 包一层本轮新增的 `LlmHarnessChat::new(llm)` 即得到 `dyn HarnessChat`。
4. **锁**：`knowlege_dataset_nav::NavKbLock` 只有测试实现（`OpenLock`，`knowlege_dataset_nav.rs:2934`）。生产实现写一个极小的 `Mutex<HashSet<String>>` 版本：`acquire` 插入成功即 true，`release` 移除即可（对应上游 `lock.spin_acquire/release`）。

**端点骨架**：所有权检查 → 前置条件检查（缺嵌入/对话 → `code 102` **分别指明缺哪一个**）→ 解析 `documents`（缺省=数据集全部文档）→ 取分块 → 逐文档 `upsert_dataset_nav_doc`（顺序执行、用完即丢，避免一次性把整个数据集读进内存）→ 返回 `{generated, skipped:[{doc_id, reason}], total}`。

**验收**：① 无嵌入或对话模型 → `code 102` 且文案指明缺哪个；② 有分块且有模型 → 生成后 `GET /datasets/{id}/navigation` 能读到节点；③ 无分块的文档 → 出现在 `skipped` 里并带原因，不静默略过；④ 越权数据集 → 400（沿用 `kb_accessible`）。

### 第五块零件（本轮新发现）：`Embedder` → `EmbeddingBackend` 也需要适配器

尝试实现端点时编译报错，暴露了最后一处不匹配：

- `AppState.embedder: Option<SharedEmbedder>`，而 `SharedEmbedder = crate::embed::SharedEmbedder`（即 `Arc<dyn crate::embed::Embedder>`）；
- 导航引擎要的是 `&dyn crate::structure_compile::EmbeddingBackend`；
- **两者是不同的 trait**，`impl EmbeddingBackend for …` 目前**只存在于测试**（`knowlege_dataset_nav.rs:2961` 的 `MockEmbed`、`structure_compile.rs:7866/7875`）。

即：和 `HarnessChat` 完全一样的缺件——**需要一个把 `dyn Embedder` 接到 `EmbeddingBackend` 上的生产适配器**（把 `EmbeddingBackend` 的方法映射到 `Embedder` 的嵌入调用；错误同样要 `Err` 带出，不能返回空向量，否则会写出"没有嵌入的行"）。

**同源收益**：这个适配器一旦存在，`structure_compile`（结构编译）与 wiki 生成里所有依赖 `EmbeddingBackend` 的路径一并解除。

> 施工提示：本轮已写好端点主体但**因这一处不匹配而回退**（未提交半成品，仓库保持可编译）。下一轮顺序：① 先写 `Embedder`→`EmbeddingBackend` 适配器（+单测），② 再落端点主体（骨架与四条验收已在本文件中，代码可几乎照抄本轮回退版本）。

## wiki / Artifacts 生成（下一块的施工配方：编排入口已找到）

**编排入口**：`knowlege_wiki_incremental::wiki_compile_incremental`（`src/knowlege_wiki_incremental.rs:10191`）——

```rust
pub async fn wiki_compile_incremental(
    store: &dyn DocStore,          // 用 OffThreadDocStore（本轮已具备，同导航）
    chat: &dyn HarnessChat,        // LlmHarnessChat（0.7.5 已具备）
    embd: Option<&dyn Embedder>,   // state.embedder —— 注意这里要的是 embed::Embedder，无需 EmbedderBridge
    tenant_id: &str, kb_id: &str, mode: &str,
    chunk_delta: &Map<String, Value>,        // 本批要编译的分块
    previous_chunk_state: &Map<String, Value>,
    current_chunk_state: &Map<String, Value>,
    incremental: bool,
    deleted_doc_ids: Option<&BTreeSet<String>>,
    disabled_doc_ids: &BTreeSet<String>,
    callback: Option<&(dyn Fn(f64, &str) + Send + Sync)>,
) -> Value
```

**三块状态从哪来（已定位）**：
- `current_chunk_state` ← `knowlege_wiki::scan_current_chunk_state(store, tenant, kb, &doc_ids)`（`src/knowlege_wiki.rs:1343`）；
- 历史版本/上一批状态 ← `knowlege_wiki::load_map_versions(store, tenant, kb, &doc_set, requested)`（`src/knowlege_wiki.rs:1214`）；
- `chunk_delta` 需由本批变更的分块构造（`doc_id → chunks`），可参照 `wiki_map_from_chunks` 的入参形状。

**端点/UI**：`POST /datasets/{id}/wiki/compile`（或复用上游路径）+ Artifacts 面板 Generate 按钮调用它；返回值需含 `pages_created/updated/skipped` 与**逐文档跳过原因**。

**验收**：① 缺 chat 模型 → `code 102` 指明；② 有分块 → 生成后 `GET /datasets/{id}/artifacts` 能读到页面（0.5.5 已实现该读取面）；③ 无分块文档 → 计入 skipped 带原因；④ **必须实测"会真正生成"的主路径**（导航那次就是这条路径 panic 的）。

### 关键结构发现（本轮）：wiki 生成是**两阶段**，不能只调编排入口

读 `wiki_compile_incremental` 的实现发现：它**不接收分块正文**，而是从存储里取 **MAP 阶段产出的 extracts**：

```rust
let changed_chunks = chunk_id_set(chunk_delta, "changed_chunk_ids");
let deleted_chunks = chunk_id_set(chunk_delta, "deleted_chunk_ids");
let new_chunks     = chunk_id_set(chunk_delta, "new_chunk_ids");
...
let map_results = crate::harness::knowlege_wiki::load_map_extracts_for_state(...);
```

也就是说 `chunk_delta` 只是一组**分块 id 集合**（三个键），真正的输入是**已经写进存储的 MAP extracts**。

**因此端点必须按顺序做两件事**：
1. **MAP**：对每个文档调用 `knowlege_wiki::wiki_map_from_chunks(store, chat, chunks, doc_id, tenant, kb, language, max_workers, llm_timeout, parser_config, batch_size_cap, window_fraction, target_chunk_ids, callback)`，把 extracts 写入存储（分块正文在这里才被真正使用）；
2. **COMPILE/REDUCE**：再调 `wiki_compile_incremental(..., chunk_delta = {"new_chunk_ids": [...], "changed_chunk_ids": [], "deleted_chunk_ids": []}, previous_chunk_state = load_map_versions(...), current_chunk_state = scan_current_chunk_state(...), incremental = false, ...)` 产出页面。

**这解释了为什么不能"只接编排入口"**：少了 MAP，`load_map_extracts_for_state` 取不到东西，`wiki_compile_incremental` 会安静地什么都不产出——正是本项目反复要避免的"静默成功"。
