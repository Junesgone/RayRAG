#!/usr/bin/env python3
"""Generate/refresh docs/parameter-interface-registry.md (internal, never published).

The registry is the single lookup surface for parameter names and functional
interfaces so new code cannot drift from RAGFlow naming or reuse a parameter
with a different meaning. Generated tables live between explicit markers; the
hand-written contract sections around them are preserved.

Usage: gen-interface-registry.py [--check|--write]
"""

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOC = ROOT / "docs" / "parameter-interface-registry.md"
ENV_EXAMPLE = ROOT / ".env.example"
SERVER_RS = ROOT / "src" / "server.rs"

ENV_BEGIN = "<!-- BEGIN GENERATED ENV -->"
ENV_END = "<!-- END GENERATED ENV -->"
ROUTE_BEGIN = "<!-- BEGIN GENERATED ROUTES -->"
ROUTE_END = "<!-- END GENERATED ROUTES -->"

PREAMBLE = """# RayRAG 参数与功能接口登记表（内部文档，禁止发布）

> 用途：统一参数名称与功能接口，避免参数名写错、接口形状不一致、错误只静默不提示。
> 兼容目标：与 RAGFlow v0.27.2 的参数名/接口名保持一致；RayRAG 扩展单独标注。
> 本文件是内部工程资产：`scripts/publish-github.sh` 会把它从发布快照中剔除，禁止发布到 GitHub。
>
> 维护规则：新增/修改环境变量、HTTP 接口、模型选择器格式或错误契约时，必须同步更新本文件
> （env 与路由表由 `scripts/gen-interface-registry.py --write` 刷新，契约段手动维护）。

## 1. 环境变量登记表（与 RAGFlow 命名兼容）

- 读取位置：项目根目录 `.env`（首次登录引导页写回同一文件）；`.env.example` 是默认模板。
- 统一超时：所有对外命令/子进程必须读取 `RAYRAG_CMD_TIMEOUT`（默认 7200 秒 = 2 小时，硬上限）。
- 空值约定：值为空表示“未配置”，走代码默认或明确报错；不允许静默回退到错误端点。

<!-- BEGIN GENERATED ENV -->
<!-- END GENERATED ENV -->

## 2. HTTP 接口登记表

- 统一错误契约：`{ "code": <int>, "message": <string>, "data": <object|null> }`；`code=0` 成功。
- 不允许静默错误：失败必须返回非 0 code 与可读 message（前端就地展示），禁止只写日志不出错。
- 鉴权：`Authorization: <token>`（登录返回）；`/api/v1/auth/login` 与公开分享接口除外。

<!-- BEGIN GENERATED ROUTES -->
<!-- END GENERATED ROUTES -->

## 3. 命名与数据契约

| 契约 | 规则 | 示例 |
| --- | --- | --- |
| 模型选择器 | `<model>@<provider>@<instance>`（缺省实例用 `default`） | `qwen3.8-max@Tongyi-Qianwen@default` |
| 数据集解析配置 | `parser_config` 内使用 RAGFlow camelCase 键（`chunk_token_num`/auto_keywords/raptorEn/graphEn…） | `{"chunk_token_num":2048,"raptorEn":true}` |
| 文档/切片状态 | `UNSTARTED / RUNNING / DONE / FAILED`；文档启用位 `status` 为 `"0"/"1"` | `{"run":"DONE","status":"1"}` |
| 运行形态 | 纯 Linux 与纯 Docker 等价；元数据 PostgreSQL，向量 zvec（不允许 MySQL） | `RAYRAG_VECTOR_BACKEND=zvec` |
| 超时 | 命令/HTTP 客户端统一读取 `RAYRAG_CMD_TIMEOUT` | `timeout: 7200` |

## 4. 变更流程

1. 新增参数/接口 → 先在 RAGFlow v0.27.2 源码中找同名或最接近的参数/接口，保持命名与形状一致。
2. 同步更新本文件（env/路由表用生成脚本刷新，契约段手改），并在 `CHANGELOG.md` 与 `运维记录.md` 记录。
3. 加回归测试：参数默认值、接口成功/失败形状、错误 message 可读（不允许静默）。
4. 发布前运行 `python3 scripts/gen-interface-registry.py --check`，确保登记表与代码一致。
"""


def is_decorative(text):
    """A comment that only draws a line (==== or ----) carries no meaning."""
    return not text or set(text) <= set("=-#*/ ")


def load_env_rows():
    """Every key the template declares, active or commented out.

    The template comments out values whose absence is meaningful (an auto-tuned worker
    count, an endpoint with a built-in default), so a commented `#KEY=value` line is a
    documented key too. Treating those as plain comments used to fold them into the note
    of whatever key came next, which hid real parameters from this registry.
    """
    rows = []
    comments = []
    try:
        lines = ENV_EXAMPLE.read_text(encoding="utf-8").split("\n")
    except OSError as error:
        sys.exit(f"cannot read {ENV_EXAMPLE}: {error}")
    for line in lines:
        stripped = line.strip()
        if not stripped:
            comments = []
            continue
        assignment = re.fullmatch(r"(#\s*)?([A-Z][A-Z0-9_]*)=(.*)", stripped)
        if assignment:
            commented, key, value = assignment.groups()
            note = " ".join(part for part in comments if not is_decorative(part))
            if commented:
                marker = "（模板中默认注释，取消注释即生效）"
                note = f"{note} {marker}".strip() if note else marker
            rows.append((key, value.strip(), note))
            comments = []
            continue
        if stripped.startswith("#"):
            comment = stripped.lstrip("#").strip()
            if not is_decorative(comment):
                comments.append(comment)
            continue
        comments = []
    return rows


def env_table(rows):
    out = ["| 变量 | 默认模板值 | 说明 |", "| --- | --- | --- |"]
    for key, value, note in rows:
        clean = note.replace("|", "\\|")
        out.append(f"| `{key}` | `{value}` | {clean} |")
    return "\n".join(out)


def route_rows():
    try:
        text = SERVER_RS.read_text(encoding="utf-8")
    except OSError as error:
        sys.exit(f"cannot read {SERVER_RS}: {error}")
    rows = []
    pattern = re.compile(
        r'\.route\(\s*"([^"]+)"\s*,\s*(?:(get|post|put|delete|patch)\s*\(|([a-z_]+::[a-z_]+))',
        re.S,
    )
    for path, method, bare in pattern.findall(text):
        rows.append((method.upper() if method else "ANY", path, bare or "-"))
    return rows


def route_table(rows):
    out = ["| 方法 | 路径 | 处理器 |", "| --- | --- | --- |"]
    for method, path, handler in rows:
        out.append(f"| {method} | `{path}` | `{handler}` |")
    return "\n".join(out)


def replace_block(text, begin, end, content):
    start = text.find(begin)
    stop = text.find(end)
    if start < 0 or stop < 0:
        sys.exit(f"marker missing: {begin} / {end}")
    return text[: start + len(begin)] + "\n" + content + "\n" + text[stop:]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()

    generated_env = env_table(load_env_rows())
    generated_routes = route_table(route_rows())
    if args.write or not DOC.exists():
        base = DOC.read_text(encoding="utf-8") if DOC.exists() else PREAMBLE
        updated = replace_block(base, ENV_BEGIN, ENV_END, generated_env)
        updated = replace_block(updated, ROUTE_BEGIN, ROUTE_END, generated_routes)
        DOC.write_text(updated, encoding="utf-8")
        print(f"registry written: {DOC} ({len(load_env_rows())} env vars, {len(route_rows())} routes)")
        return 0

    text = DOC.read_text(encoding="utf-8")
    expected = replace_block(text, ENV_BEGIN, ENV_END, generated_env)
    expected = replace_block(expected, ROUTE_BEGIN, ROUTE_END, generated_routes)
    if expected != text:
        print("registry is stale; run: python3 scripts/gen-interface-registry.py --write")
        return 1
    print("registry check ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
