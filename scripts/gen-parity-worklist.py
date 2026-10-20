#!/usr/bin/env python3
"""Generate the internal v0.27.2 parity worklist from the coverage ledger.

Reads docs/ragflow-v0.27.2-coverage.md and writes docs/ragflow-v0.27.2-worklist.md
(ordered work queues: W-1..W-6 audit queues, UI residuals, re-audit rows).
Run after any intentional ledger status change.
"""
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LEDGER = ROOT / "docs/ragflow-v0.27.2-coverage.md"
OUT = ROOT / "docs/ragflow-v0.27.2-worklist.md"

rows = []
suspicious = 0
for line in LEDGER.read_text(encoding="utf-8").splitlines():
    if not line.startswith("| `"):
        continue
    # Mirror the coverage tool: splitn(7, '|'), then split the remainder's
    # first '|' into mapping/note (notes may themselves contain ' | ').
    fields = line.split("|")
    if len(fields) < 8:
        suspicious += 1
        continue
    path = fields[1].strip().strip("`")
    lines = fields[2].strip()
    status = fields[5].strip()
    rest = "|".join(fields[6:]).strip()
    if rest.endswith("|"):
        rest = rest[:-1].rstrip()
    if "|" in rest:
        mapping, note = rest.split("|", 1)
    else:
        mapping, note = rest, ""
    mapping = mapping.strip().strip("`")
    note = note.strip()
    rows.append(
        {
            "path": path,
            "lines": lines,
            "status": status,
            "mapping": mapping,
            "note": note,
        }
    )

wq = {f"W-{i}": [] for i in range(1, 7)}
ui = []
reaudit = []
for r in rows:
    m = re.search(r"worklist (W-[1-6])", r["note"])
    if m:
        wq[m.group(1)].append(r)
    elif r["status"] == "partial" and r["path"].startswith("web/src/"):
        ui.append(r)
    if "content changed: re-audit" in r["note"]:
        reaudit.append(r)

counts = {}
for r in rows:
    counts[r["status"]] = counts.get(r["status"], 0) + 1

TITLES = {
    "W-1": "搜索/提供商接入",
    "W-2": "数据源接入",
    "W-3": "OCR/视觉解析器",
    "W-4": "Agentic harness 移植（最大队列）",
    "W-5": "结构图/编译器/切分",
    "W-6": "沙箱提供商与鉴权",
}

out = []
out.append("# RAGFlow v0.27.2 Parity Worklist（内部工作清单，禁止发布）")
out.append("")
out.append("> 来源：`docs/ragflow-v0.27.2-coverage.md`（固定 commit `a024bea0cd93f39e6652a42bf84dd20c55bc560b`，5977 文件）。")
out.append("> 生成：`scripts/gen-parity-worklist.py`（v0.3.10g 起；含人工维护的「方法任务」区）；台账状态变更后重跑刷新。")
out.append("> 用途：v0.27.2 对标收敛的有序工作队列；每批完成后回写台账并刷新本清单。")
out.append("")
out.append("## 现状摘要")
out.append("")
out.append(f"- 文件总数：{len(rows)}；unmapped={counts.get('unmapped', 0)}（v0.3.10g 全量分类完成）")
out.append(f"- partial：{counts.get('partial', 0)}（UI 残留 {len(ui)} + 实施队列 {sum(len(v) for v in wq.values())}）")
out.append(f"- re-audit：{len(reaudit)}（上游内容变更、需复核对齐的已审计行）")
out.append("- replaced / N/A / reference：无逐文件动作（基础设施替代与审计参考材料）")
out.append("")
out.append("## 方法任务（人工维护）")
out.append("")
out.append("| 任务 | 状态 | 说明 |")
out.append("|---|---|---|")
out.append("| **REA 逆向分析（RAGFlow v0.27.2 全量行为与代码）** | **进行中**（2026-10-11 完成安装 / 接入与首轮执行；逐模块分析进行中——01 登录链 ✓、02 接口面与迁移 ✓） | 需求来源：项目便签（RayRAG项目）。使用 `morluto/rea`（`rea-agents@6.4.0`，面向二进制 / JS / Electron / .NET / 网站行为的 MCP 逆向工具；Hermes MCP 已注册、170 工具、实测 ✓ Connected）对 v0.27.2 参考树（固定 commit `a024bea0cd93`）与镜像前端产物做逆向分析，把分析结果、方法、行为应用到 RayRAG；产出逐模块分析记录并回写台账与实施队列。进展（2026-10-11）：安装 / Hermes 接入 / 连通 ✓；web-dist 导出（707 JS / 39.1MB）✓；参考树导入（6754 条）✓；静态重构（707 解析 / 7.78M AST 节点 / 25,160 findings）✓；source↔bundle 对比两轮跑通（无 sourcemap，逐文件映射仅 2 项；转语义图 + 运行观测路径，见运维记录 10-11 续记②；续记③：MCP 直达工具全量上线、登录链逐模块分析 ✓（`notes/01-登录链.md`）、`/api/v1/*` 新 REST 面对标增量在案；续记④：API 面三源提取（前端 87 路径 / 官方文档 86 端点 / 实例探针）+ 官方迁移表 19 行 + 新面家族确认（`docs/rea-v0.27.2/api-surface-v0.27.2.md`）；续记⑤：检索与对话链运行取证 ✓（6 端点 200 响应形状 + `chat/completions` 默认模型悬挂实证 → `docs/rea-v0.27.2/runtime-surface-v0.27.2.md`）；续记⑥：OpenAI 兼容面运行取证 ✓（10 探针矩阵 + 源码契约六文件；两段式错误哲学 → `docs/rea-v0.27.2/openai-surface-v0.27.2.md`）；续记⑦：Search App（searches/*）运行取证 ✓（16 探针含净零生命周期 + searchbots 认证族发现 → `docs/rea-v0.27.2/search-surface-v0.27.2.md`）；续记⑧：Memory 家族与 workspace 提交面运行取证 ✓（18 探针全只读 + **实例记忆检索缺陷查实** + 默认提取提示词资产 → `docs/rea-v0.27.2/memory-surface-v0.27.2.md`）；续记⑨：文件提交面运行取证 ✓（源码全文 + 12 探针 + **错误三态与 RetCode 全表修正** → `docs/rea-v0.27.2/file-commit-surface-v0.27.2.md`）；续记⑩：UI 面对照全量提取 ✓（侧栏 7 项/65 路由/55 懒加载块/en·zh 标签资产 → `docs/rea-v0.27.2/ui-map-v0.27.2.md`）；续记⑪：sessions/audio/mindmap/recommendation 运行取证 ✓（源码段全文 + 15 探针 + `get_data_error_result` 双风格定型 → `docs/rea-v0.27.2/sessions-audio-surface-v0.27.2.md`）；续记⑫：**分析阶段 01–09 完成**（2026-10-12 01:0x）＋应用交叉核对首轮 ✓（7 行台账回写 REA 交叉引用、agent_api 行陈旧「remain open」修正（v0.3.8n 286d586）、信封边界注记 → `docs/rea-v0.27.2/apply-crosscheck-01.md`）；续记⑬：UI 路由对账 ✓（上游 3 形式浏览器实测 + RayRAG 51 探针：37×200 / 8×307 / 6×404 全部语义正确；`//` 形式两侧 404 一致；遍历增补建议 4 条 + locale 裁决项 → `docs/rea-v0.27.2/ui-route-crosscheck-v0.27.2.md`）；续记⑭：文档面（document_api.py）静态全文 + 39 探针 ✓（19 路由契约逐条实测、信封/码值、混合信封再确认（部分上传/批状态 = HTTP 200 + code=500）、空选择器宽容、静默分页回退；探针零写 → `docs/rea-v0.27.2/document-surface-v0.27.2.md`）；续记⑮：文档面应用交叉核对 ✓（40 行逐项对照四级分类——✓7 等价 / 含 T19 / 含 C18 / 含 S6 / 含 F4 / 备注 3；修正批①–④ 计划在案；ledger document_api 行注记精细化并修复一处 stray 管道符 → `docs/rea-v0.27.2/apply-crosscheck-02.md`）；续记⑯：文档面修正批①（文案级）落地 ✓（30 处对照编辑——list/batch/delete/parse/stop/rename/preview/download/images/summary/metadata-config 文案族；`selector` falsy 归一；env_surface 接线缺陷修复；`cargo test --locked` 2271/0；批②已落地（续记⑰）：文档面码值/状态级 —— 体内 400→101/102、404→102，34 处编辑、5 文件；create-route 鉴权 102/109 与 No file part! 101；images、doc 级 metadata-config 200+102；upload 业务错误 HTTP 200；ingest 四处经 round11 探针取证后冻结；测试同步 7 断言；`cargo test --locked` 2271/0；批③已落地（续记⑱）：文档面形状级 —— list `total` 双键+message、delete `{deleted,success_count}`、upload 成功=数组/失败=500 换行 message/部分=混合信封、ingest 成功/取消 `data:true`、thumbnails 省略键+message、batch 成功 message；新增 `api_error_result` 无-data 错误形态并切文档面错误族；42 处编辑、5 文件；`cargo test --locked` 2271/0；批④已落地（续记⑲）：功能级——④a 列表查询面/分页（b379c03）、④b ingest 语义（2608e78）、④d file-commit 信封逐字复刻（31815fe，owner 决策：逐字复刻）；④c metadata/config raw-body 校验落地（726d9c9，round12 实测归证）；④e PATCH 字段面落地（21e994f，round13 实测 26 键成功体 + 17 校验臂、构造性零写；已审计差异三项在案）；批④ 全清 → 合并 v0.46.0 发版）；续记⑳（2026-10-20）：v0.27.2 编译模板参考数据复核——发现 v0.26.4 陈旧种子并刷新为 v0.27.2（kind artifacts→wiki、mode/example 字段、empty 模板移除、wiki presets example 键；8 builtins / 6 presets）；edit-next 深层表单（模式/RAPTOR/分节字段/蓝图/JSON 预览）落地，11 行 UI residual 转 aligned；v0.47.0 发版）；续记㉑（2026-10-20）：数据集 Artifacts/Wiki 编译面 UI 落地（5 视图模式空态、Contents/Topics→Pages、markdown 详情与 wiki link、Update/Clear、真实 MAP→COMPILE 端到端生成），Knowledgebase.pipeline_id 顶层列落地，compile 并入 engine chunk 视图修复新解析数据集 pages_created=0；CDP 26/26；v0.48.0 发版）；续记㉒（2026-10-21）：搜索设置抽屉头像改用共享 avatar-upload 裁剪弹窗（挂全局 layout）+ SearchAppSeed.avatar 列持久化（POST/PUT）+ 移除按钮与双语校验；admin 控制台 9 个 CDP 脚本批次复核实点（8 行 partial→aligned）；search-setting 行 aligned；CDP 23/23；v0.49.0 发版）；续记㉓（2026-10-21）：上游 svgIcons 固定 provider 图片资产补齐（29 个 SVG 全量入库 + mapping 覆盖 AIMLAPI/Astraflow/FastEmbed/FunASR/FuturMix/GreenPT/MWS/Synthorai/Xiaomi，单测断言资产存在）；profile-page CDP 对齐共享裁剪弹窗并补姓名规则断言；login-next/skills/chat-channel/setting-model/dataset-testing/profile 批次复核，8 行 partial→aligned；v0.50.0 发版）；续记㉔（2026-10-21）：路由表复核（/chunk 四视图、/chats/widget、agentbots logs 均已注册）；user-setting 壳改 upstream 精确 303/64、data-source 网格改 viewport 断点（lg=2/2xl=4）；修复搜索抽屉 touched 别名 bug（thresh→threshold、vWeight→weight，滑块编辑才会进请求体）；新增 3 个 CDP 探针，5 行 partial→aligned；v0.51.0 发版）；续记㉕（2026-10-21）：search_view 端到端（mock chat model + search-answer CDP：AI summary SSE 101 字符/1 citation、mind map 7 节点、related 3、0 pageError）；修复 search_complete 流式桥 bounded blocking_send panic（改 unbounded）与 ?sapp= applySapp 提前调用 searchRerankToggle 的 ReferenceError；tree-select 旧 Radix focus residual 撤回（上游行为普通 div）；search-view + tree-select 两行 aligned；v0.52.0 发版）。 |")
out.append("")
out.append("## 实施队列")
out.append("")
for code in ["W-1", "W-2", "W-3", "W-4", "W-5", "W-6"]:
    items = wq[code]
    out.append(f"### {code} {TITLES[code]}（{len(items)} 项）")
    out.append("")
    out.append("| 文件 | 行数 | RayRAG 对应 | 台账注记 |")
    out.append("|---|---:|---|---|")
    for r in items:
        note = r["note"].replace("|", "\\|")
        out.append(f"| `{r['path']}` | {r['lines']} | `{r['mapping']}` | {note} |")
    out.append("")

out.append(f"## UI 对齐残留（{len(ui)} 项，任务-3 的 CDP 实点批次收敛）")
out.append("")
out.append("| 文件 | 行数 | RayRAG 对应 | 台账注记 |")
out.append("|---|---:|---|---|")
for r in ui:
    note = r["note"].replace("|", "\\|")
    out.append(f"| `{r['path']}` | {r['lines']} | `{r['mapping']}` | {note} |")
out.append("")

out.append(f"## 复核对账队列（re-audit，{len(reaudit)} 项）")
out.append("")
out.append("上游 v0.27.2 相对 v0.26.4 变更过的已审计行；逐行复核后移除 `[v0.27.2 content changed: re-audit]` 标记并更新注记。")
out.append("")
out.append("| 文件 | 状态 | RayRAG 对应 |")
out.append("|---|---|---|")
for r in reaudit:
    out.append(f"| `{r['path']}` | {r['status']} | `{r['mapping']}` |")
out.append("")

out.append("## 执行顺序与证据标准")
out.append("")
out.append("- 顺序建议：W-3（小、独立）→ W-1 → W-6 → W-2 → W-5 → W-4（最大，按子模块分批）。UI 残留随任务-3 的实点批次；re-audit 队列按状态分组插入 W 批次之间。")
out.append("- 每批证据：`cargo fmt --check` + `cargo check --locked --features postgres-backend,zvec-backend` + 定向单测；涉及部署跑 `docker compose build/up` 健康检查；台账行更新为 aligned/partial 并写明批次；`运维记录.md` 追加；每轮发布新版本号（GitHub Release）。")
out.append("- 纪律：先查证据再下结论；不得无证据翻正；reranker 停机期间不做其深度重测；内部资产（本清单、台账、运维记录、参数登记表）禁止发布。")
out.append("")

OUT.write_text("\n".join(out) + "\n", encoding="utf-8")
print(f"wrote {OUT.name}: rows={len(rows)} suspicious={suspicious} wq={ {k: len(v) for k, v in wq.items()} } ui={len(ui)} reaudit={len(reaudit)}")
