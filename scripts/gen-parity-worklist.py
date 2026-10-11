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
out.append("| **REA 逆向分析（RAGFlow v0.27.2 全量行为与代码）** | **进行中**（2026-10-11 同日完成安装与 Hermes 接入；首轮分析在跑） | 需求来源：项目便签（RayRAG项目）。使用 `morluto/rea`（`rea-agents@6.4.0`，面向二进制 / JS / Electron / .NET / 网站行为的 MCP 逆向工具；Hermes MCP 已注册、170 工具、实测 ✓ Connected）对 v0.27.2 参考树（固定 commit `a024bea0cd93`）与镜像前端产物做逆向分析，把分析结果、方法、行为应用到 RayRAG；产出逐模块分析记录并回写台账与实施队列。进展（2026-10-11）：安装 / Hermes 接入 / 连通 ✓；web-dist 导出（707 JS / 39.1MB）✓；参考树导入已完成（entries 6754 / relationships 10025）；首轮静态分析运行中。 |")
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
