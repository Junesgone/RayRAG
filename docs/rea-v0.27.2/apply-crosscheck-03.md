# 编译模板面交叉核对（REA 轮次 14，v0.27.2）

> 轮次：2026-10-20（v0.47.0 切片）
> 基线：RAGFlow v0.27.2 固定树 `~/.cache/ragflow-v0.27.2`（commit `a024bea0cd93`）
> 产物：本文件 + `docs/parameter-interface-registry.md` 的「编译模板参数面」节 + 台账 11 行 UI residual 转 aligned

## 1. 本轮发现（证据优先）

### 1.1 内嵌种子数据是 v0.26.4 的陈旧副本（重要）

- **对照对象**：`api/db/init_data/compilation_templates/*.yaml`（8 个 builtin）与 `.../wiki/*.yaml`（6 个 preset）。
- **RayRAG 侧**：`src/api/fixtures/compilation_template_builtins.v0.26.4.json`、`..._wiki_presets.v0.26.4.json`，由旧轮次编译嵌入。
- **逐项差异**（机器对比 YAML vs JSON）：

| 维度 | v0.26.4 内嵌副本 | v0.27.2 上游 | 结论 |
|---|---|---|---|
| graph-wiki 的 kind | `artifacts` | `wiki`（`web/src/constants/compilation.ts`：`Artifacts = 'wiki'`） | **改名** |
| wiki config 键 | 无 `mode` | `mode: entity`（entity/topic） | **新增** |
| wiki presets 键 | `page_example` | `example`（后端 `load_wiki_presets_from_files` 逐字映射） | **改名** |
| builtin 数量 | 9（含 `empty`） | 8（`docs/ragflow-v0.27.2-delta.md` 明列 removed upstream：`api/db/init_data/compilation_templates/empty.yaml`） | **移除 empty** |
| 排序 | display_name（empty 最后） | `_sort_builtins`：empty 优先，否则 display_name；无 empty 时即 display_name 升序 | 顺序随数据刷新而变 |
| wiki presets 文本 | brand 等 instruction 243/238 字、general 446/450 字 | v0.27.2 文件逐字（brand instruction 238 字等） | **内容刷新** |

结论：台账 `api/db/init_data/compilation_templates/*.yaml` 行的旧注记 "Fixed v0.26.4 builtin..." 与 "v0.27.2 re-audit: 复核通过（映射在库）" **只证明了映射存在，没证明内容一致**；本轮以内容级 diff 纠偏。

### 1.2 组/模板 HTTP 信封与上游不一致

- 上游契约（静态全文）：`compilation_template_group_api.py` 的成功路径走 `get_json_result`（HTTP 200、`{code:0,message:"success",data:...}`），失败路径走 `get_data_error_result`（HTTP 200、`{code:102,message}`、无 data）；`@validate_request("name","templates")` 缺参走 `get_json_result(code=101,message="required argument are missing: ...; ",data=None)`（HTTP 200）。
- RayRAG 旧实现（探针实测）：成功 POST 返回 **HTTP 201** 且无 message；校验失败返回 **HTTP 400 + code 400**；不存在返回 **HTTP 404 + code 404**。
- 本轮按上游三函数逐字复刻：200 + 成功信封 / 200 + 102 data-error / 200 + 101 缺参信封；detail/update/delete 的 not-found 同族。

### 1.3 编辑器残留

- v0.3.10c 只落了 edit-next 的 Layout-less 外壳与 name/kind/global_rules/rechunk 字段；上游 `TemplateConfiguration` / `BlueprintSection` / `SectionFieldGrid` / `AddFieldModal` 的深层行为缺失（11 行 UI residual）。
- 上游 `ExcludedBuiltinKinds = [session_essence, session_graph]`：编辑器 kind 选项只有 6 个。
- 字段/分节模型：`FieldKeyOrders`（type/description/rule、statement/subject、definition_excerpt/term）、`SectionCardFieldMap`（entity·relation=type/description，claim=statement/subject，concept=definition_excerpt/term）、`SectionPriority=[entity,relation]`。
- 保存变换（`transformFormToPayload`）：组名取 templates[0].name；wiki 仅在 `use_blueprint` 时写入 trim 后的 instruction/example，否则写空串；tree 只保留 raptor。

## 2. 本轮落地

1. **fixtures 刷新**：`src/api/fixtures/compilation_template_builtins.v0.27.2.json`（8 条，字节级源自 YAML）与 `..._wiki_presets.v0.27.2.json`（6 条，`example` 键）；启动校验增加「上游排序」断言；旧 v0.26.4 fixture 删除。
2. **kind 词表**：校验、scope 推导、错误文案全部 `artifacts`→`wiki`；混组文案 "A wiki template cannot be combined with other templates in the same group."。
3. **校验文案逐字**：`Wiki mode must be either 'entity' or 'topic'.`、`Claim statement is required.`/`Claim subject is required.`、`Concept term is required.`/`Concept definition excerpt is required.`（替换旧的 `Statement is required.` 族）。
4. **旧数据迁移**：载入时 `artifacts`→`wiki` + 补 `mode=entity`；写路径（POST/PUT）同样归一化，旧缓存前端提交的 artifacts 模板落库即 wiki。
5. **编辑器深层表单**（web.rs + 新 CDP 脚本）：
   - kind 选项过滤 session 类、按上游 formatKindLabel 命名；
   - wiki：mode 选择 + Blueprint 折叠区（preset select + instruction textarea + example textarea，Custom 恢复 builtin 默认）；
   - tree：RAPTOR 提示词/max_token/聚类阈值/聚类比例/按叶重分块；
   - 其余 kind：rechunk 开关 + 规则；
   - 分节 tabs（Entity/Relation/Claim/Concept）与字段卡片网格、Add/Edit field 弹层（type datalist + 去重校验 + 必填校验）、删除；
   - 模板名 + JSON 预览 sheet；保存走上游变换并回跳 /agents；编辑回填含 blueprint preset 匹配。
6. **HTTP 信封**：上述 200 + {0/101/102} 契约；builtins/wiki-presets 成功体补 `message:"success"`。

## 3. 证据

- **单元/集成测试**：`cargo test --locked --lib compilation_template` → 18 passed / 0 failed（含 fixtures 基线、逐字文案、迁移、HTTP 信封）；全量见 v0.47.0 运维记录。
- **CDP 真点**（`scripts/ui-traversal/compilation-template-editor-cdp.mjs`，fnOS Chrome / CDP，debug 实例 :9490）：59/59 checks、0 pageerror。覆盖 palette（8/排序/wiki kind/mode）、presets（6/example）、kind 切换、tabs/卡片数（entity 8、relation 7）、add/edit/delete field、preset 填充与 Custom 回落、JSON 预览、wiki 保存（dataset scope、mode topic、instruction、sections）、编辑回填、tree 保存（raptor max_token/rechunk、无 sections、file scope）、空字段拒绝、清理。
- **HTTP 探针**（debug 实例）：`/tmp/ct-probes.json` 记录 builtins、presets、bad_mode、mixed、missing_claim、two_rechunk_trees 的形状与逐字 message；本轮新版改写为 200 信封后由 `compilation_template_group_errors_answer_upstream_envelopes` 单元测试固化。
- **静态契约**：`api/apps/restful_apis/compilation_template_group_api.py`、`..._api.py`、`utils/compilation_template_validation.py`、`api/utils/api_utils.py`（`get_json_result/get_data_error_result/validate_request`）、`api/db/services/compilation_template*_service.py`、`web/src/pages/user-setting/compilation-templates/edit-next/**`。

## 4. 在案差异（未随批）

- 编辑器 kind/mode/blueprint 选择用原生 `<select>`/textarea 呈现（组件文件在台账为 reference；行为与上游 SelectWithSearch/MarkdownEditor 等价，交互装饰层不复刻）。
- update 路由的畸形 JSON 体仍由 axum 提取器拒绝（上游未包装 validate_request，走通用 500）；正常/缺字段/空体路径已对齐。
- `POST .../execute` 是 RayRAG 扩展路由（上游无此路径），维持 `{code:0,data:{results,templates}}` 形状。
