# REA 逆向分析产物（RAGFlow v0.27.2）

> 本目录为 **内部资产**（发布时硬拒排除，不入公开快照）。
> 来源：`morluto/rea` 对 v0.27.2 参考树（commit `a024bea0cd93`）+ 镜像前端产物（web-dist 707 JS）的逆向分析。
> 工作笔记与原始证据：`~/.cache/rea/ragflow-v0.27.2/`（notes/01–10、round3–10-forensics/、round8-ui/、web-dist/）。

## 模块分析（REA 轮次 → 蒸馏件）

| # | 模块 | 蒸馏件 | 核心结论 |
| --- | --- | --- | --- |
| 01 | 登录链 | （见运维记录续记③） | `/api/v1/auth/login`；未注册邮箱 109 |
| 02 | 接口面与迁移 | `api-surface-v0.27.2.md` | 三源 87/86/19；新面家族确认 |
| 03 | 检索与对话链 | `runtime-surface-v0.27.2.md` | 6 端点 200 形状；默认模型悬挂实证 |
| 04 | OpenAI 兼容面 | `openai-surface-v0.27.2.md` | 两段式错误；`model:"model"` 占位符；SSE 细节 |
| 05 | Search App | `search-surface-v0.27.2.md` | 净零生命周期；searchbots=AUTH_BETA |
| 06 | Memory 家族 | `memory-surface-v0.27.2.md` | 四类记忆；**实例检索缺陷**；提示词资产 |
| 07 | 文件提交面 | `file-commit-surface-v0.27.2.md` | 双域语义；错误三态；**RetCode 全表** |
| 08 | UI 面 | `ui-map-v0.27.2.md` | 侧栏 7 项；65 路由/55 块；en·zh 标签 |
| 09 | sessions/audio | `sessions-audio-surface-v0.27.2.md` | 109+data:false 统一形；双风格定型 |

| 10 | 应用交叉核对 | `apply-crosscheck-01.md` | 九模块契约 × Rust 实现对照；台账 7 行回写 + 陈旧注记修正 |
| 11 | UI 路由对账 | `ui-route-crosscheck-v0.27.2.md` | 上游 3 形式浏览器实测 + RayRAG 51 探针；遍历增补建议 4+1 项 |
| 12 | 文档面（document_api） | `document-surface-v0.27.2.md` | 19 路由全文 + 39 探针（全只读）；混合信封再确认；**应用侧交叉核对待做** |
| 13 | 文档面交叉核对 | `apply-crosscheck-02.md` | 40 行逐项对照四级分类（✓/T/C/S/F）；修正批①–④ 计划在案（批① 文案、批② 码值·状态、批③ 形状已落地 ✓） |
| 14 | 编译模板面交叉核对 | `apply-crosscheck-03.md` | **发现 v0.26.4 陈旧种子**：kind `artifacts`→`wiki`、新增 `mode`、wiki presets 用 `example` 键、`empty` 模板移除；8 builtins / 6 presets 基线刷新；编辑器深层表单 + 上游 HTTP 信封（200/101/102）落地 |
| 16 | 搜索设置头像 + admin 复核 | `apply-crosscheck-05.md` | **发现 avatar 列缺失**（抽屉提交被静默忽略）与共享裁剪对话框未复用；`SearchAppSeed.avatar` 列 + 全局 layout 共享 avatar-upload 对话框 + 移除按钮/双语校验落地；CDP 23/23；admin 9 脚本批次全绿，8 行 partial→aligned |
| 15 | 数据集 Artifacts/Wiki 编译面 | `apply-crosscheck-04.md` | **发现门控永关**（缺 `Knowledgebase.pipeline_id` 列）与**新解析数据集 compile 静默 0 页**（current state 未并入 engine chunk 视图）；5 模式空态 + Contents/Topics→Pages + markdown 详情 + Update/Clear 落地；mock model + CDP 26/26 端到端 |

## 资产

- `memory-default-system-prompt.txt` — 记忆默认提取系统提示词（3604 字符，源自实例真实配置）。
- 编译模板 v0.27.2 种子基线（v0.3.10aj）：`api/db/init_data/compilation_templates/*.yaml` → 内嵌 fixture `src/api/fixtures/compilation_template_builtins.v0.27.2.json`（8 条）与 `..._wiki_presets.v0.27.2.json`（6 条，`example` 键）；`cargo test --locked --lib api::compilation_templates::tests::reference_palette_matches_v0272_seed_set` 守护。

## 方法（已成范式）

逐模块四步：**静态地图 → 接口面提取 → 语义追踪 → 运行探针**；补齐三源（前端 API 表 / 官方文档 / 实例存活探针）+ 服务端日志取证；每轮产出工作笔记（~/.cache）+ 蒸馏件（本目录）+ 台账（运维记录.md）。

## 未决总表

- chat 成功形状（默认模型 MiniMax-M3 上游不可达）；
- ~~记忆语义检索（实例记录 `embd_id` 陈旧格式，待 Boss 确认 DB 修复）~~ **2026-10-12 用户决定不需要修复，事项关闭**；
- 音频成功形状（实例未配置默认 TTS/ASR）；
- 文件提交 create/diff/tree 成功形状（需真实提交，未做写操作）；
- 文档面写路径成功形状（上传/解析/停止/删除、web/empty、ingest）——零污染政策下未取，需要时以临时文档净零一次；
- 文档面四级差异修正批①–④（文案 / 码值·状态 / 形状 / 功能）——见 `apply-crosscheck-02.md`，按序执行（批① 文案、批② 码值·状态、批③ 形状已落地 ✓；批④ 功能在途）。
