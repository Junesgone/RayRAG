#!/usr/bin/env python3
"""Machine-diff RayRAG chat-channel descriptions against upstream locales.

Usage: cc-zh-diff.py <en.html> <zh.html> <upstream-web-src-dir>
Exits 0 when all checks pass; prints per-check counts.
"""

import re
import sys
from pathlib import Path


def read_text(path: Path) -> str:
    """Read a UTF-8 file, reporting IO problems instead of raising."""
    try:
        return path.read_text(encoding="utf-8")
    except OSError as exc:
        print(f"cannot read {path}: {exc}")
        return ""


def locale_block(text: str, key: str) -> dict[str, str]:
    """Extract a nested 'key: { ... },' locale block as key->value dict."""
    pat = re.compile(key + r": {(.*?)" + r"\n      },", re.S)
    match = pat.search(text)
    if match is None:
        return {}
    item = re.compile(r"(\w+):\s*'((?:[^'\\]|\\.)*)'")
    out: dict[str, str] = {}
    for m in item.finditer(match.group(1)):
        out[m.group(1)] = m.group(2).replace("\\'", "'")
    return out


def page_descs(html: str) -> dict[str, str]:
    return dict(re.findall(r"(\w+):\{name:'[^']*',desc:'([^']*)'", html))


def page_zh_map(html: str) -> dict[str, str]:
    match = re.search(r"var CC_DESC_ZH=\{([^}]*)\}", html)
    if match is None:
        return {}
    return dict(re.findall(r"(\w+):'([^']*)'", match.group(1)))


def main(argv: list[str]) -> int:
    if len(argv) != 4:
        print("usage: cc-zh-diff.py <en.html> <zh.html> <upstream-web-src-dir>")
        return 2
    try:
        en_path, zh_path, websrc = (Path(a) for a in argv[1:4])
        inputs = [en_path, zh_path, websrc / "locales/en.ts", websrc / "locales/zh.ts"]
        for p in inputs:
            if not p.is_file():
                print(f"missing input file: {p}")
                return 2
        en_block = locale_block(read_text(websrc / "locales/en.ts"), "chatChannelDesc")
        zh_block = locale_block(read_text(websrc / "locales/zh.ts"), "chatChannelDesc")
        en_html = read_text(en_path)
        zh_html = read_text(zh_path)
    except OSError as exc:
        print(f"input error: {exc}")
        return 2
    if not en_block or not zh_block or not en_html or not zh_html:
        print("empty parse result (bad locale block or empty page)")
        return 2
    en_descs = page_descs(en_html)
    zh_map = page_zh_map(zh_html)

    diff_en = sorted(k for k in en_block if en_descs.get(k) != en_block[k])
    diff_zh = sorted(k for k in zh_block if zh_map.get(k) != zh_block[k])
    missing_en = sorted(set(en_block) - set(en_descs))
    missing_zh = sorted(set(zh_block) - set(zh_map))
    en_injects = "var CC_DESC_ZH={" in en_html
    zh_injects = "var CC_DESC_ZH={" in zh_html

    print(
        f"en desc vs upstream en.ts: matched={len(en_block) - len(diff_en)}/{len(en_block)} missing={missing_en} diff={diff_en}"
    )
    print(
        f"zh CC_DESC_ZH vs upstream zh.ts: matched={len(zh_block) - len(diff_zh)}/{len(zh_block)} missing={missing_zh} diff={diff_zh}"
    )
    print(
        f"en injects CC_DESC_ZH: {en_injects} (want False); zh injects: {zh_injects} (want True)"
    )
    ok = (
        not diff_en
        and not missing_en
        and not diff_zh
        and not missing_zh
        and not en_injects
        and zh_injects
    )
    verdict = "PASS" if ok else "FAIL"
    print(f"RESULT={verdict}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
