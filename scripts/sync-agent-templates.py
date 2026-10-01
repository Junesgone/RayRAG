#!/usr/bin/env python3
"""Sync the agent/ingestion pipeline template fixtures from a pinned RAGFlow checkout.

Usage: sync-agent-templates.py [TREE_DIR] [--check|--write]

Sources (both must carry the wrapper keys title/description/dsl; DSL-only files
such as `compiler.json` are skipped):
  * `agent/templates/*.json` — agent canvases + the in-agent ingestion pipelines
  * `internal/ingestion/pipeline/template/*.json` — the moved ingestion pipelines

Generated artifacts:
  * `src/api/fixtures/agent_templates/<slug>.json` — the compact DSL, byte-for-byte
    the upstream `dsl` value (no trailing newline), stale fixtures removed
  * `src/api/fixtures/agent_templates/meta.json` — id/title/description/canvas metadata
    (en + zh) in slug order
  * `src/agent_templates.rs` — AGENT_TEMPLATES rows, the dataflow_canvas slug list
    and the template-count doc comments
"""

import argparse
import json
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "src" / "api" / "fixtures" / "agent_templates"
META_PATH = FIXTURES / "meta.json"
AGENT_RS = ROOT / "src" / "agent_templates.rs"
DATAFLOW = "dataflow_canvas"


def load_json(path):
    try:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    except OSError as error:
        sys.exit(f"cannot read {path}: {error}")
    except json.JSONDecodeError as error:
        sys.exit(f"invalid JSON in {path}: {error}")


def template_wrappers(tree):
    rows = {}
    for directory in (Path(tree) / "agent" / "templates", Path(tree) / "internal" / "ingestion" / "pipeline" / "template"):
        if not directory.is_dir():
            continue
        for path in sorted(directory.glob("*.json")):
            data = load_json(path)
            if not all(key in data for key in ("title", "description", "dsl")):
                continue
            slug = path.stem
            if slug in rows:
                sys.exit(f"duplicate template slug across upstream dirs: {slug}")
            rows[slug] = data
    return rows


def localized(value, lang):
    if isinstance(value, dict):
        text = value.get(lang)
        if isinstance(text, str) and text:
            return text
        english = value.get("en")
        if isinstance(english, str) and english:
            return english
    return ""


def fixtures_from(rows):
    return {slug: json.dumps(data["dsl"], ensure_ascii=False, separators=(",", ":")) for slug, data in rows.items()}


def meta_from(rows):
    meta = []
    for slug in sorted(rows):
        data = rows[slug]
        meta.append(
            {
                "id": slug,
                "title": {"en": localized(data.get("title"), "en"), "zh": localized(data.get("title"), "zh")},
                "description": {
                    "en": localized(data.get("description"), "en"),
                    "zh": localized(data.get("description"), "zh"),
                },
                "canvas_type": data.get("canvas_type", ""),
                "canvas_category": data.get("canvas_category") or "agent_canvas",
                "avatar": data.get("avatar", ""),
            }
        )
    return meta


def rust_string(value):
    return value.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")


def rust_array(rows):
    lines = []
    for slug in sorted(rows):
        data = rows[slug]
        lines.append("    (")
        lines.append(f'        "{rust_string(slug)}",')
        lines.append(f'        "{rust_string(localized(data.get("title"), "en"))}",')
        lines.append(f'        "{rust_string(localized(data.get("description"), "en"))}",')
        lines.append(f'        include_str!("api/fixtures/agent_templates/{slug}.json"),')
        lines.append("    ),")
    return "\n".join(lines)


def rust_match_arms(rows):
    dataflow = sorted(slug for slug, data in rows.items() if (data.get("canvas_category") or "agent_canvas") == DATAFLOW)
    lines = []
    for index, slug in enumerate(dataflow):
        prefix = "        " if index == 0 else "        | "
        lines.append(f'{prefix}"{slug}"' if index == 0 else f'{prefix}"{slug}"')
    body = "\n".join(lines)
    return f"{body} => \"{DATAFLOW}\","


def current_fixture_slugs():
    return {path.stem: path.read_text(encoding="utf-8") for path in FIXTURES.glob("*.json") if path.name != "meta.json"}


def current_rust_entries():
    text = AGENT_RS.read_text(encoding="utf-8")
    start = text.find("pub const AGENT_TEMPLATES")
    if start < 0:
        sys.exit("AGENT_TEMPLATES not found in " + str(AGENT_RS))
    end = text.find("\n];", start)
    if end < 0:
        sys.exit("AGENT_TEMPLATES end not found")
    entries = re.findall(r'\(\s*"([^"]+)",\s*"((?:[^"\\]|\\.)*)",\s*"((?:[^"\\]|\\.)*)",\s*include_str!\("([^"]+)"\),\s*\)', text[start:end])
    return [(slug, name, description, path) for slug, name, description, path in entries]


def current_dataflow_slugs():
    text = AGENT_RS.read_text(encoding="utf-8")
    match = re.search(r"match slug \{(.*?)_ => \"agent_canvas\",", text, re.S)
    if not match:
        sys.exit("dataflow_canvas match block not found")
    arms = match.group(1)
    arms = arms[: arms.rfind("=>")] if "=>" in arms else arms
    return re.findall(r'"((?:[^"\\]|\\.)*)"', arms)


def apply_rust(rows):
    text = AGENT_RS.read_text(encoding="utf-8")
    start = text.find("pub const AGENT_TEMPLATES")
    end = text.find("\n];", start)
    if start < 0 or end < 0:
        sys.exit("AGENT_TEMPLATES block not found")
    head = text[:start]
    tail = text[end + len("\n];"):]
    const_line_end = text.find("\n", start)
    const_line = text[start:const_line_end]
    new_const = const_line + "\n" + rust_array(rows) + "\n];"
    text = head + new_const + tail

    match = re.search(r"match slug \{(.*?)(_ => \"agent_canvas\",)", text, re.S)
    if not match:
        sys.exit("dataflow_canvas match block not found")
    text = text[: match.start(1)] + "\n" + rust_match_arms(rows) + "\n        " + text[match.start(2):]

    total = len(rows)
    text = re.sub(r"\(\d+ templates\)", f"({total} templates)", text)
    text = re.sub(r"\d+ seeded RAGFlow workflow templates", f"{total} seeded RAGFlow workflow templates", text)
    AGENT_RS.write_text(text, encoding="utf-8")


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

    rows = template_wrappers(args.tree)
    fixtures = fixtures_from(rows)
    meta = meta_from(rows)

    if args.write:
        FIXTURES.mkdir(parents=True, exist_ok=True)
        for slug, body in fixtures.items():
            (FIXTURES / f"{slug}.json").write_text(body, encoding="utf-8")
        stale = set(current_fixture_slugs()) - set(fixtures)
        for slug in stale:
            (FIXTURES / f"{slug}.json").unlink()
        META_PATH.write_text(json.dumps(meta, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
        apply_rust(rows)
        print(
            f"wrote {len(fixtures)} templates (removed {len(stale)} stale); "
            f"{len([m for m in meta if m['canvas_category'] == DATAFLOW])} dataflow pipelines"
        )
        return 0

    failures = []
    current = current_fixture_slugs()
    if set(current) != set(fixtures):
        failures.append(
            "fixture set differs: missing=" + str(sorted(set(fixtures) - set(current))) + " stale=" + str(sorted(set(current) - set(fixtures)))
        )
    for slug in sorted(set(current) & set(fixtures)):
        try:
            if json.loads(current[slug]) != json.loads(fixtures[slug]):
                failures.append(f"fixture DSL differs: {slug}")
        except json.JSONDecodeError as error:
            failures.append(f"fixture is not valid JSON: {slug} ({error})")
    if load_json(META_PATH) != meta:
        failures.append("meta.json differs")
    expected_entries = [
        (slug, localized(rows[slug].get("title"), "en"), localized(rows[slug].get("description"), "en"), f"api/fixtures/agent_templates/{slug}.json")
        for slug in sorted(rows)
    ]
    if current_rust_entries() != expected_entries:
        failures.append("AGENT_TEMPLATES rows differ")
    expected_dataflow = sorted(slug for slug, data in rows.items() if (data.get("canvas_category") or "agent_canvas") == DATAFLOW)
    if current_dataflow_slugs() != expected_dataflow:
        failures.append("dataflow_canvas slug list differs")
    if failures:
        print("check FAILED")
        for failure in failures:
            print("  - " + failure)
        return 1
    print(f"check ok: {len(fixtures)} templates")
    return 0


if __name__ == "__main__":
    sys.exit(main())
