#!/usr/bin/env python3
"""Catch the bug class behind the reported model-page wall.

A page once emitted `</tbody></table></p></details>` with **no** matching opener: the
browser inferred a table and rendered every provider and model as one permanently
expanded block (15,515 px tall). Unbalanced `<details>`/`<table>` are a precise signal
of that mistake, so this walks the main pages and reports any page where the counts
disagree — including the "closing tag without an opener" case, which is the exact one
that looks fine to a human reading the source.

Usage: RAYRAG_ORIGIN=http://127.0.0.1:9390 RAYRAG_LOGIN_EMAIL=... RAYRAG_LOGIN_PASSWORD=... \
       python3 scripts/ui-traversal/tag-balance-check.py
"""
import json
import os
import re
import sys
import urllib.request

ORIGIN = os.environ.get("RAYRAG_ORIGIN", "http://127.0.0.1:9390")
EMAIL = os.environ.get("RAYRAG_LOGIN_EMAIL", "admin@rayrag.local")
PASSWORD = os.environ.get("RAYRAG_LOGIN_PASSWORD", "")
PAGES = [
    "/dashboard", "/datasets", "/chat", "/agents", "/search", "/files",
    "/user-setting/model", "/user-setting/profile", "/user-setting/api",
    "/user-setting/team", "/user-setting/data-source", "/user-setting/chat-channel",
    "/user-setting/mcp",
    "/admin/services", "/admin/users",
]
TAGS = ("details", "table", "thead", "tbody", "tr", "section", "aside", "summary")


def request(path, token=None, payload=None):
    data = json.dumps(payload).encode() if payload is not None else None
    req = urllib.request.Request(f"{ORIGIN}{path}", data=data)
    if payload is not None:
        req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=60) as response:
            return response.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as error:
        # A page that genuinely does not exist has no markup to balance; report it as a
        # finding so a broken path is visible rather than skipped.
        raise SystemExit(f"{path} answered HTTP {error.code}; fix the page path first")


def main():
    token = ""
    if PASSWORD:
        body = json.loads(request("/api/v1/auth/login", payload={"email": EMAIL, "password": PASSWORD}))
        token = body.get("data", {}).get("access_token", "")
    failures = []
    for page in PAGES:
        html = request(page, token)
        # Inline scripts build markup as strings, `<style>` blocks carry upstream class
        # names inside comments (a CSS comment quoting `<aside class="…">` was the first
        # false positive this guard produced), and HTML comments can quote markup too.
        # Strip all three before counting.
        stripped = re.sub(r"<script\b.*?</script>", "", html, flags=re.S | re.I)
        stripped = re.sub(r"<style\b.*?</style>", "", stripped, flags=re.S | re.I)
        stripped = re.sub(r"<!--.*?-->", "", stripped, flags=re.S)
        for tag in TAGS:
            opened = len(re.findall(rf"<{tag}\b", stripped, re.I))
            closed = len(re.findall(rf"</{tag}>", stripped, re.I))
            if opened != closed:
                failures.append(f"{page}: <{tag}> opened {opened} closed {closed}")
    if failures:
        for line in failures:
            print(f"UNBALANCED {line}")
        print(f"tag balance: FAILED ({len(failures)} finding(s))")
        return 1
    print(f"tag balance: OK ({len(PAGES)} pages, tags={'/'.join(TAGS)})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
