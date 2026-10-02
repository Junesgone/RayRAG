#!/usr/bin/env python3
"""Audit RayRAG's HTTP surface against the fixed RAGFlow reference tree.

The parity ledger (`docs/ragflow-v0.27.2-coverage.md`) tracks *files*; this tool tracks
*endpoints*. A file can be "aligned" while a handful of its routes are still missing or
spelled differently, and a client written against RAGFlow only ever sees the routes.

Three outcomes per upstream route:

* **exact** — RayRAG registers the upstream spelling.
* **alias** — RayRAG answers the upstream spelling through a route it also exposes under
  its own name (recorded in `ALIASES`). The alias must exist in both directions, or
  `--check` fails: that is how the two spellings stay in step.
* **missing** — neither. Every one of these must be listed in `KNOWN_MISSING` with a note
  saying what it is, so the remaining work is a number rather than a feeling. A route that
  becomes implemented while still listed as missing also fails `--check` (stale entry),
  which keeps the list honest in both directions.

Usage:
    api-parity-audit.py [--check] [--source DIR] [--write]

    --check   fail on a broken alias, a stale KNOWN_MISSING entry, or a new upstream route
    --write   refresh docs/api-parity-worklist.md (internal document, never published)
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SERVER_RS = ROOT / "src" / "server.rs"
WORKLIST = ROOT / "docs" / "api-parity-worklist.md"
DEFAULT_SOURCE = os.environ.get(
    "RAGFLOW_SOURCE_DIR", "/home/junesgone/.cache/ragflow-v0.27.2"
)

# Upstream path -> the RayRAG route that answers it. Both spellings are registered; the
# entry documents *why* the names differ (upstream uses hyphens, RayRAG's own names use
# underscores) and lets `--check` prove neither side was dropped.
ALIASES: dict[str, str] = {
    "/compilation-templates/builtins": "/api/v1/compilation_templates/builtins",
    "/compilation-templates/wiki-presets": "/api/v1/compilation_templates/wiki_presets",
    "/compilation-template-groups": "/api/v1/compilation_template_groups",
    "/compilation-template-groups/<group_id>": "/api/v1/compilation_template_groups/{id}",
    "/system/stats": "/api/v1/stats",
    "/searches/<search_id>/completions": "/api/v1/searches/{search_id}/completions",
}

# Upstream routes RayRAG does not answer yet, with the reason they are still open.
# Deleting an entry here is what "implementing" one means; the checker flags entries that
# have become reachable so the list cannot drift behind the code.
# Upstream paths answered by a RayRAG route whose *shape* differs (a separator inside one
# segment, for example). Verified at request time: `/api/v1/messages/aa:bb` matches
# `/api/v1/messages/{message_ref}` because a route parameter consumes the whole segment.
COVERED: dict[str, str] = {
    "/messages/<memory_id>:<message_id>": "/api/v1/messages/{message_ref}",
    "/messages/<memory_id>:<message_id>/content": "/api/v1/messages/{message_ref}/content",
}

# Upstream methods on paths RayRAG already answers with other methods. A path-only audit
# cannot see these: `/users/me` answered 405 for GET until it was wired up.
KNOWN_METHOD_GAPS: dict[str, str] = {
    "/agents/<agent_id>/sessions": "DELETE 删除 Agent 会话",
    "/providers/<provider_id_or_name>/instances/<instance_id_or_name>": "PUT 更新模型实例",
    "/providers/<provider_id_or_name>/instances/<instance_id_or_name>/models": "DELETE 移除实例内模型",
    "/providers/<provider_id_or_name>/instances/<instance_id_or_name>/models/<path:model_name>": "POST 新增实例内模型",
}

KNOWN_MISSING: dict[str, str] = {
    "/agents/<agent_id>/upload": "Agent 附件上传（画布内文件输入）",
    "/agents/<agent_id>/webhook": "Agent Webhook 配置（含 logs/test）",
    "/agents/<agent_id>/webhook/logs": "Agent Webhook 调用日志",
    "/agents/<agent_id>/webhook/test": "Agent Webhook 连通性测试",
    "/agents/attachments/<attachment_id>/download": "Agent 附件下载",
    "/agents/attachments/<attachment_id>/preview": "Agent 附件预览",
    "/agents/download": "Agent DSL/画布导出下载",
    "/agents/prompts": "Agent 提示词模板列表",
    "/agents/test_db_connection": "Agent 数据库组件连接测试",
    "/auth/password/forgot/captcha": "忘记密码-图形验证码",
    "/auth/password/forgot/otp": "忘记密码-发送 OTP",
    "/auth/password/forgot/otp/verify": "忘记密码-校验 OTP",
    "/auth/password/reset": "忘记密码-重置",
    "/chat/audio/speech": "TTS 语音合成",
    "/chat/audio/transcription": "ASR 语音转写（当前仅有解析流水线内的 ASR）",
    "/chats/<chat_id>/sessions": "会话（session）列表",
    "/chats/<chat_id>/sessions/<session_id>": "会话（session）详情/删除",
    "/connectors/<connector_id>": "连接器实例读写（RayRAG 用 data_sources 面）",
    "/connectors/<connector_id>/logs": "连接器同步日志",
    "/connectors/<connector_id>/rebuild": "连接器重建索引",
    "/connectors/<connector_id>/test": "连接器连通性测试",
    "/datasets/<dataset_id>/<index_type>": "按索引类型读取数据集",
    "/datasets/<dataset_id>/artifacts": "数据集 Artifacts（wiki/topics/structure/graph）",
    "/datasets/<dataset_id>/artifacts/<page_type>/<path:slug>": "Artifacts 单页内容",
    "/datasets/<dataset_id>/artifacts/alteration": "Artifacts 变更记录",
    "/datasets/<dataset_id>/artifacts/graph": "Artifacts 图视图",
    "/datasets/<dataset_id>/artifacts/structure": "Artifacts 结构视图",
    "/datasets/<dataset_id>/artifacts/topics": "Artifacts 主题视图",
    "/datasets/<dataset_id>/chunks": "数据集级分块列表",
    "/datasets/<dataset_id>/documents/<document_id>/metadata/config": "文档级元数据配置",
    "/datasets/<dataset_id>/documents/<document_id>/structure/graph": "文档结构图（编译产物）",
    "/datasets/<dataset_id>/embedding/check": "嵌入模型可用性检查",
    "/datasets/<dataset_id>/graph": "数据集知识图谱读取",
    "/datasets/<dataset_id>/index": "数据集索引重建",
    "/datasets/<dataset_id>/metadata/config": "数据集元数据配置",
    "/datasets/<dataset_id>/navigation": "数据集导航树",
    "/datasets/<dataset_id>/navigation/<path:name>": "导航树单节点",
    "/datasets/<dataset_id>/navigation/<path:name>/children": "导航树子节点",
    "/datasets/<dataset_id>/navigation/search": "导航树检索",
    "/datasets/<dataset_id>/skills": "数据集 Skills 列表",
    "/datasets/<dataset_id>/skills/<path:skill_kwd>": "数据集 Skill 详情/更新",
    "/documents/artifact/<filename>": "文档产物文件读取",
    "/documents/images/<image_id>": "文档内图片读取",
    "/documents/upload": "文档上传（RayRAG 用 /datasets/{id}/documents 与 /files/upload）",
    "/files/link-to-datasets": "文件关联数据集",
    "/plugin/tools": "插件工具列表",
    "/searchbots/ask": "搜索应用问答机器人（上游 bot_api 的公开问答面）",
    "/searchbots/detail": "搜索应用机器人详情",
    "/searchbots/mindmap": "搜索应用脑图",
    "/searchbots/retrieval_test": "搜索应用检索测试",
    "/thumbnails": "缩略图服务",
    "/users/me/models": "当前用户可用模型列表",
    "/workspace-files/<file_id>/versions": "文件版本历史（file_commit）",
}


def upstream_routes(source: Path) -> dict[str, tuple[str, frozenset[str]]]:
    """Upstream route -> (source file, HTTP methods)."""
    directory = source / "api" / "apps" / "restful_apis"
    if not directory.is_dir():
        sys.exit(f"reference tree not found: {directory}")
    routes: dict[str, tuple[str, frozenset[str]]] = {}
    for path in sorted(directory.glob("*.py")):
        text = path.read_text(encoding="utf-8", errors="replace")
        pattern = re.compile(
            r'@manager\.route\(\s*"([^"]+)"(?:\s*,\s*methods\s*=\s*\[([^\]]*)\])?'
        )
        for match in pattern.finditer(text):
            methods = frozenset(
                part.strip().strip('"\'').upper()
                for part in (match.group(2) or '"GET"').split(",")
                if part.strip()
            )
            previous = routes.get(match.group(1))
            if previous:
                methods = previous[1] | methods
            routes[match.group(1)] = (path.name, methods)
    return routes


def rayrgg_routes() -> dict[str, tuple[str, frozenset[str]]]:
    """Every `/api/v1/...` path RayRAG registers, with the methods it answers.

    The route call is scanned with bracket matching so `get(a).post(b)` and a nested
    `axum::routing::post(c)` are both read; a handler name is not a method name, so only
    the method builders themselves count.
    """
    text = SERVER_RS.read_text(encoding="utf-8", errors="replace")
    routes: dict[str, tuple[str, frozenset[str]]] = {}
    # A route's path may sit under a comment inside the `.route(...)` call — three
    # endpoints were reported missing because of exactly that.
    for match in re.finditer(
        r'\.route\(\s*(?://[^\n]*\n\s*)*"([^"]+)"\s*,', text
    ):
        path = match.group(1)
        if not path.startswith("/api/v1"):
            continue
        depth = 1
        index = match.end()
        while index < len(text) and depth > 0:
            char = text[index]
            if char == "(":
                depth += 1
            elif char == ")":
                depth -= 1
            index += 1
        body = text[match.end() : index]
        methods = frozenset(
            found.group(1).upper()
            for found in re.finditer(
                r"(?<![A-Za-z0-9_])(get|post|put|delete|patch|head|options)\s*\(", body
            )
        ) or frozenset({"ANY"})
        previous = routes.get(path)
        if previous:
            methods = previous[1] | methods
        routes[path] = (path, methods)
    return routes


def upstream_spelling(path: str) -> str:
    """Upstream path rewritten in the brace syntax RayRAG registers."""
    return re.sub(r"<(?:[^:>]+:)?([^>]+)>", r"{\1}", path)


def normalise(path: str) -> str:
    """Path shape: `/api/v1` prefix, parameter names and separators all erased.

    Parameter *names* are not part of the contract a client sees — upstream says
    `<dataset_id>`, RayRAG says `{id}`, and both answer the same URL shape. Erasing them
    is what separates "spelled exactly like upstream" from "same endpoint, different
    spelling".
    """
    if path.startswith("/api/v1"):
        path = path[len("/api/v1") :]
    path = re.sub(r"<(?:[^:>]+:)?[^>]+>", "{}", path)
    path = re.sub(r"\{[^}]+\}", "{}", path)
    return path.replace("-", "_").rstrip("/").lower()


def classify(
    upstream: dict[str, tuple[str, frozenset[str]]],
    own: dict[str, tuple[str, frozenset[str]]],
) -> dict[str, list[tuple[str, str]]]:
    """Route-level and method-level classification.

    A path can exist while a method on it does not — upstream's `GET /users/me` answered
    405 in RayRAG for exactly that reason, and a path-only audit cannot see it.
    """
    by_norm: dict[str, dict[str, frozenset[str]]] = {}
    for route, (_, methods) in own.items():
        by_norm.setdefault(normalise(route), {})[route] = methods

    result: dict[str, list[tuple[str, str]]] = {
        "exact": [],
        "alias": [],
        "missing": [],
        "covered": [],
        "method": [],
        "mismatch": [],
    }
    for path, (origin, methods) in sorted(upstream.items()):
        spelled = f"/api/v1{upstream_spelling(path)}"
        candidates: dict[str, frozenset[str]] = {}
        if spelled in own:
            result["exact"].append((path, origin))
            candidates = by_norm.get(normalise(spelled), {})
        elif spelled in ALIASES.values():
            result["alias"].append((path, origin))
            candidates = by_norm.get(normalise(spelled), {})
        elif normalise(path) in by_norm:
            result["alias"].append((path, origin))
            candidates = by_norm[normalise(path)]
        elif path in COVERED:
            result["covered"].append((path, origin))
            candidates = by_norm.get(normalise(COVERED[path]), {})
        elif path in KNOWN_MISSING:
            result["missing"].append((path, origin))
            continue
        else:
            result["mismatch"].append((path, origin))
            continue

        answered = set().union(*candidates.values()) if candidates else set()
        answered |= {"GET"} if "HEAD" in answered else set()
        missing_methods = {
            method
            for method in methods
            if method not in answered and "ANY" not in answered and not (method == "HEAD" and "GET" in answered)
        }
        if missing_methods:
            result["method"].append(
                (path, f"{origin} [{','.join(sorted(missing_methods))}]")
            )
    return result


def render(
    upstream: dict[str, tuple[str, frozenset[str]]],
    own: dict[str, tuple[str, frozenset[str]]],
    result: dict[str, list[tuple[str, str]]],
) -> str:
    out = [
        "# RayRAG ↔ RAGFlow HTTP 接口对照工作清单（内部文档，禁止发布）",
        "",
        "> 生成：`scripts/api-parity-audit.py --write`；数据源 = 固定 RAGFlow v0.27.2 参考树 "
        "`api/apps/restful_apis/*.py` 与 `src/server.rs` 的 `.route(...)` 注册表。",
        "> 用途：把「每个上游接口 RayRAG 是否应答」变成可复核的数字，并按批收敛。",
        "",
        "## 现状",
        "",
        f"- 上游路由：**{len(upstream)}**；RayRAG `/api/v1` 路由：**{len(own)}**",
        f"- 同名应答（exact）：**{len(result['exact'])}**",
        f"- 别名应答（alias，上游拼写已注册但走 RayRAG 自己的命名）：**{len(result['alias'])}**",
        f"- 形状不同但已被应答（covered）：**{len(result['covered'])}**",
        f"- 尚未实现（missing，已登记）：**{len(result['missing'])}**",
        f"- 路径已有但方法缺失（method）：**{len(result['method'])}**",
        f"- 未登记的缺口（mismatch，`--check` 视为失败）：**{len(result['mismatch'])}**",
        "",
        "## 未实现清单（按上游文件分组）",
        "",
        "| 上游路径 | 来源文件 | 说明 |",
        "| --- | --- | --- |",
    ]
    grouped: dict[str, list[str]] = {}
    for path, origin in result["missing"]:
        grouped.setdefault(origin, []).append(path)
    for origin in sorted(grouped):
        for path in grouped[origin]:
            note = KNOWN_MISSING.get(path, "")
            out.append(f"| `{path}` | `{origin}` | {note} |")
    if not grouped:
        out.append("| — | — | 无 |")

    out += ["", "## 别名对照（上游拼写 → RayRAG 路由）", "", "| 上游路径 | RayRAG 路由 |", "| --- | --- |"]
    for upstream_path, own_path in sorted(ALIASES.items()):
        out.append(f"| `{upstream_path}` | `{own_path}` |")
    out += ["", "## 方法级缺口（路径已应答，但上游方法未实现）", "", "| 上游路径 | 来源与缺失方法 |", "| --- | --- |"]
    for path, detail in result["method"]:
        out.append(f"| `{path}` | {detail} |")
    if not result["method"]:
        out.append("| — | — |")
    out.append("")
    return "\n".join(out)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--source", default=DEFAULT_SOURCE)
    args = parser.parse_args()

    upstream = upstream_routes(Path(args.source))
    own = rayrgg_routes()
    result = classify(upstream, own)

    problems: list[str] = []
    for upstream_path, own_path in ALIASES.items():
        if own_path not in own:
            problems.append(f"alias target missing from src/server.rs: {own_path}")
        # A client sends the upstream URL; what has to be true is that some registered
        # route matches it. Literal paths must be registered verbatim; parameterised ones
        # must match the alias target's shape (axum matches any segment into `{name}`, so
        # `{group_id}` and `{id}` answer the same request).
        spelled = f"/api/v1{upstream_path}"
        if "<" not in upstream_path:
            if spelled not in own:
                problems.append(f"upstream spelling not registered: {spelled}")
        elif normalise(upstream_path) != normalise(own_path):
            problems.append(
                f"alias target shape differs from the upstream path: {upstream_path} -> {own_path}"
            )
        if upstream_path not in upstream:
            problems.append(f"alias recorded for an upstream route that no longer exists: {upstream_path}")

    for upstream_path, own_path in COVERED.items():
        if own_path not in own:
            problems.append(f"covered route target missing from src/server.rs: {own_path}")
        if upstream_path not in upstream:
            problems.append(f"covered route recorded for an upstream path that no longer exists: {upstream_path}")

    reachable = {f"/api/v1{path}" for path, _ in result["exact"] + result["alias"]}
    for path in KNOWN_MISSING:
        if f"/api/v1{path}" in reachable:
            problems.append(f"stale KNOWN_MISSING entry (now implemented): {path}")

    for path, origin in result["mismatch"]:
        problems.append(f"unclassified upstream route (add an alias or a KNOWN_MISSING note): {path} ({origin})")

    for path, detail in result["method"]:
        if path not in KNOWN_METHOD_GAPS:
            problems.append(f"method gap without a note: {path} {detail}")
    implemented_gaps = {f"/api/v1{path}" for path in KNOWN_METHOD_GAPS}
    for path, _ in result["exact"] + result["alias"]:
        if f"/api/v1{path}" in implemented_gaps and path not in {p for p, _ in result["method"]}:
            problems.append(f"stale KNOWN_METHOD_GAPS entry (now answered): {path}")

    if result["missing"]:
        for path, origin in result["missing"]:
            if path not in KNOWN_MISSING:
                problems.append(f"missing route without a note: {path} ({origin})")

    if args.write:
        WORKLIST.write_text(render(upstream, own, result), encoding="utf-8")
        print(f"worklist written: {WORKLIST}")

    print(
        "api parity: upstream={u} rayrgg={r} exact={e} alias={a} covered={c} missing={m} method={mm} mismatch={x}".format(
            u=len(upstream),
            r=len(own),
            e=len(result["exact"]),
            a=len(result["alias"]),
            c=len(result["covered"]),
            m=len(result["missing"]),
            mm=len(result["method"]),
            x=len(result["mismatch"]),
        )
    )
    if args.check and problems:
        for problem in problems:
            print(f"  ! {problem}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
