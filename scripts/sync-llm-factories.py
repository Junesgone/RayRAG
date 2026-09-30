#!/usr/bin/env python3
"""Regenerate the RayRAG provider catalogs from a pinned RAGFlow checkout.

Usage:
  sync-llm-factories.py [TREE_DIR] [--check|--write]   (default: ~/.cache/ragflow-v0.27.2 --check)

Two artifacts are kept in sync with upstream `conf/llm_factories.json`:

1. `src/data/providers/llm_factories.json` — the compact discovery catalog:
   SoMark is dropped (wired from providers.rs presets), FastEmbed / Builtin /
   siliconflow_intl stay (filtered at render time), and every upstream `llm`
   entry becomes {n, t, mx, tools}. A name already present keeps its curated
   `t` (multimodal rows are tagged chat or image2text per provider) and only
   new upstream models use the default rule (chat when the type list contains
   it, otherwise the first type).

2. `src/api/tenant_models.rs::RAGFLOW_LLM_FACTORIES_JSON` — the upstream-shaped
   array the Available-models grid and factory fallback parse: every factory as
   {name, rank?, llm: [{n, t: [types], mx}]}, preserving multi-types verbatim.

Exit code 1 on --check mismatch. Deterministic output follows upstream order.
"""

import argparse
import json
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CATALOG_PATH = ROOT / "src" / "data" / "providers" / "llm_factories.json"
TENANT_PATH = ROOT / "src" / "api" / "tenant_models.rs"
INLINE_PREFIX = 'const RAGFLOW_LLM_FACTORIES_JSON: &str = r#"'
INLINE_SUFFIX = '"#;'
EXCLUDED = {"SoMark"}


def load_json(path):
    try:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    except OSError as error:
        sys.exit(f"cannot read {path}: {error}")
    except json.JSONDecodeError as error:
        sys.exit(f"invalid JSON in {path}: {error}")


def max_tokens(entry):
    try:
        return int(entry.get("max_tokens") or 0)
    except (TypeError, ValueError):
        return 0


def model_type(entry):
    types = entry.get("model_type")
    if not isinstance(types, list):
        types = [types] if types else []
    types = [value for value in types if value]
    if "chat" in types:
        return "chat"
    return types[0] if types else "chat"


def upstream_factories(tree):
    raw = load_json(Path(tree) / "conf" / "llm_factories.json")
    factories = raw.get("factory_llm_infos", raw)
    if not isinstance(factories, list):
        sys.exit(f"unexpected factory list in {tree}/conf/llm_factories.json")
    return factories


def compact_catalog(factories, existing):
    catalog = {}
    for factory in factories:
        name = factory["name"]
        if name in EXCLUDED:
            continue
        url = factory.get("url")
        if isinstance(url, dict):
            url = url.get("default")
        existing_types = {}
        for row in existing.get(name, {}).get("models", []):
            existing_types.setdefault(row.get("n"), []).append(row.get("t"))
        models = []
        for entry in factory.get("llm", []):
            model_name = entry.get("llm_name", "")
            curated = existing_types.get(model_name) or []
            curated_type = curated.pop(0) if curated else None
            models.append(
                {
                    "n": model_name,
                    "t": curated_type or model_type(entry),
                    "mx": max_tokens(entry),
                    "tools": bool(entry.get("is_tools")),
                }
            )
        entry = {
            "name": name,
            "logo": factory.get("logo", ""),
            "tags": factory.get("tags", ""),
            "status": str(factory.get("status", "1")),
        }
        # A few providers carry a curated rank in the bundled artifact; keep it so
        # the card order keeps the verified baseline, and take upstream for new ones.
        existing_rank = existing.get(name, {}).get("rank")
        if existing_rank is not None:
            entry["rank"] = str(existing_rank)
        elif "rank" in factory:
            entry["rank"] = str(factory["rank"])
        if url:
            entry["url"] = url
        entry["models"] = models
        catalog[name] = entry
    return catalog


def inline_catalog(factories):
    rows = []
    for factory in factories:
        row = {"name": factory["name"]}
        rank = factory.get("rank")
        row["rank"] = str(rank) if rank is not None else None
        models = []
        for entry in factory.get("llm", []):
            types = entry.get("model_type")
            if not isinstance(types, list):
                types = [types] if types else []
            models.append(
                {
                    "n": entry.get("llm_name", ""),
                    "t": [value for value in types if value],
                    "mx": max_tokens(entry),
                }
            )
        row["llm"] = models
        rows.append(row)
    return rows


def inline_line(rows):
    body = json.dumps(rows, ensure_ascii=False, separators=(",", ":"))
    return INLINE_PREFIX + body + INLINE_SUFFIX


def current_inline_rows():
    text = TENANT_PATH.read_text(encoding="utf-8")
    for line in text.splitlines():
        if line.startswith("const RAGFLOW_LLM_FACTORIES_JSON"):
            if INLINE_PREFIX not in line or not line.rstrip().endswith(INLINE_SUFFIX):
                sys.exit("inline factories const has an unexpected shape")
            body = line.split(INLINE_PREFIX, 1)[1].rsplit(INLINE_SUFFIX, 1)[0]
            try:
                return json.loads(body)
            except json.JSONDecodeError as error:
                sys.exit(f"inline factories const is not valid JSON: {error}")
    sys.exit("inline factories const not found in " + str(TENANT_PATH))


def write_inline(rows):
    text = TENANT_PATH.read_text(encoding="utf-8")
    lines = text.splitlines(keepends=True)
    for index, line in enumerate(lines):
        if line.startswith("const RAGFLOW_LLM_FACTORIES_JSON"):
            lines[index] = inline_line(rows) + "\n"
            TENANT_PATH.write_text("".join(lines), encoding="utf-8")
            return
    sys.exit("inline factories const not found in " + str(TENANT_PATH))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "tree",
        nargs="?",
        default=os.path.join(os.environ.get("HOME", "."), ".cache", "ragflow-v0.27.2"),
    )
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    if args.check and args.write:
        sys.exit("choose either --check or --write")

    factories = upstream_factories(args.tree)
    current = load_json(CATALOG_PATH) if CATALOG_PATH.exists() else {}
    catalog = compact_catalog(factories, current)
    inline = inline_catalog(factories)

    if args.write:
        CATALOG_PATH.write_text(
            json.dumps(catalog, ensure_ascii=False, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
        write_inline(inline)
        print(
            f"wrote {CATALOG_PATH}: {len(catalog)} providers, "
            f"{sum(len(v['models']) for v in catalog.values())} models; "
            f"inline const: {len(inline)} factories, "
            f"{sum(len(v['llm']) for v in inline)} rows"
        )
        return 0

    failures = []
    if catalog != current:
        failures.append(
            "compact catalog differs: "
            f"current={len(current)} providers/{sum(len(v.get('models', [])) for v in current.values())} models "
            f"generated={len(catalog)} providers/{sum(len(v['models']) for v in catalog.values())} models"
        )
    if inline != current_inline_rows():
        failures.append("inline factories const differs")
    if failures:
        print("check FAILED")
        for failure in failures:
            print("  - " + failure)
        return 1
    print(
        f"check ok: compact {len(catalog)} providers/"
        f"{sum(len(v['models']) for v in catalog.values())} models, "
        f"inline {len(inline)} factories"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
