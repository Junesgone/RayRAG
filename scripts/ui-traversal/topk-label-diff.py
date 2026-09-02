#!/usr/bin/env python3
"""Verify the deployed topK label split against upstream expectations.

Usage: topk-label-diff.py <dir-with-pages>
Expected files in <dir>:
  kb_en.html kb_zh.html ds_en.html ds_zh.html sp_en.html sp_zh.html
  sk_en.html sk_zh.html sa_en.html sa_zh.html
Upstream mapping (ragflow v0.26.4):
  knowledgeDetails.topK = 'Top-K' (zh 'Top-K') -> kb/dataset/search-app pages
  skillSearch.topK     = 'Top K Results' (zh '返回结果数量') -> skills modal
  next-search/search-setting.tsx hard-codes 'Top K' -> search sidebar
"""

import sys
from pathlib import Path


def read(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError as exc:
        print(f"cannot read {path}: {exc}")
        return ""


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: topk-label-diff.py <dir-with-pages>")
        return 2
    base = Path(argv[1])
    pages = {}
    for key in [
        "kb_en",
        "kb_zh",
        "ds_en",
        "ds_zh",
        "sp_en",
        "sp_zh",
        "sk_en",
        "sk_zh",
        "sa_en",
        "sa_zh",
    ]:
        pages[key] = read(base / (key + ".html"))
        if not pages[key]:
            print(f"missing page file: {key}.html")
            return 2

    failures = []
    for key in ["kb_en", "kb_zh", "ds_en", "ds_zh", "sa_en", "sa_zh"]:
        if ">Top-K<" not in pages[key]:
            failures.append(f"{key}: upstream 'Top-K' label missing")
        if ">Top K<" in pages[key]:
            failures.append(f"{key}: stale merged 'Top K' label present")
        if "召回条数" in pages[key]:
            failures.append(f"{key}: stale zh '召回条数' present")
    for key in ["sp_en", "sp_zh"]:
        if ">Top K<" not in pages[key]:
            failures.append(f"{key}: literal 'Top K' sidebar label missing")
        if "召回条数" in pages[key]:
            failures.append(f"{key}: stale zh '召回条数' present")
    if ">Top K Results<" not in pages["sk_en"]:
        failures.append("sk_en: 'Top K Results' label missing")
    if ">返回结果数量<" not in pages["sk_zh"]:
        failures.append("sk_zh: '返回结果数量' label missing")
    if ">Top K<" in pages["sk_en"]:
        failures.append("sk_en: stale merged 'Top K' label present")

    for key, html in sorted(pages.items()):
        print(
            f"{key}: Top-K={'Top-K' in html} TopKLabel={'>Top K<' in html} Results={'>Top K Results<' in html} zhResults={'>返回结果数量<' in html} staleZh={'召回条数' in html}"
        )
    print(f"failures={len(failures)}")
    for f in failures:
        print("FAIL:", f)
    verdict = "PASS" if not failures else "FAIL"
    print(f"RESULT={verdict}")
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
