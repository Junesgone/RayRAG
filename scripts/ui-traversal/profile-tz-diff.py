#!/usr/bin/env python3
"""Verify the deployed profile timezone list against upstream expectations.

Usage: profile-tz-diff.py <en.html> <zh.html>
Upstream (ragflow v0.26.4 profile/index.tsx + user-setting/constants.tsx):
  - list computed at runtime: Intl.supportedValuesOf('timeZone') + longOffset
    GMT names ('GMT+08:00 Asia/Shanghai'), sorted by offset;
  - edit dialog: searchable input, placeholder 'Select a timeZone', option
    value/label = the full name, save validation message 'select your timezone';
  - display row: find by stored name, else empty (no fallback).
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
    if len(argv) != 3:
        print("usage: profile-tz-diff.py <en.html> <zh.html>")
        return 2
    en = read(Path(argv[1]))
    zh = read(Path(argv[2]))
    if not en or not zh:
        print("empty page input")
        return 2
    markers = [
        "Intl.supportedValuesOf('timeZone')",
        "timeZoneName:'longOffset'",
        "offsetString+' '+tz",
        "a.offset-b.offset",
        "placeholder='Select a timeZone'",
        "tz.name===storedTz",
        "'select your timezone'",
        "provider-tz-list",
        "tzRenderList",
    ]
    stale = [
        "['Asia/Shanghai','Asia/Hong_Kong'",
        "||'Asia/Shanghai'",
        "America/Los_Angeles','Pacific/Auckland'",
    ]
    failures = []
    for page, html in [("en", en), ("zh", zh)]:
        for m in markers:
            if m not in html:
                failures.append(f"{page}: missing marker {m}")
        for s in stale:
            if s in html:
                failures.append(f"{page}: stale content {s}")
    print(
        f"en markers={'all' if all(m in en for m in markers) else 'partial'} "
        f"stale={'none' if not any(s in en for s in stale) else 'present'}"
    )
    print(
        f"zh markers={'all' if all(m in zh for m in markers) else 'partial'} "
        f"stale={'none' if not any(s in zh for s in stale) else 'present'}"
    )
    print(f"failures={len(failures)}")
    for f in failures:
        print("FAIL:", f)
    verdict = "PASS" if not failures else "FAIL"
    print(f"RESULT={verdict}")
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
