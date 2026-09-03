#!/usr/bin/env python3
"""Fetch a deployed RayRAG page and node --check every <script> block.

Usage: python3 scripts/ui-traversal/page-script-syntax-scan.py [path] [lang]
  path defaults to the first /api/v1/datasets id rendered at /kbs/<id>;
  lang defaults to zh-CN (cookie). Login uses RAYRAG_ADMIN_PASSWORD from .env.
Exits nonzero when any block fails to parse.
"""

import http.client
import json
import os
import re
import subprocess
import sys
import tempfile

HOST = "127.0.0.1"
PORT = 9390
ENV_PATH = "/home/junesgone/.openclaw/workspace/RayRAG/.env"


def load_password():
    try:
        with open(ENV_PATH, encoding="utf-8") as fh:
            for line in fh:
                if line.startswith("RAYRAG_ADMIN_PASSWORD="):
                    return line.split("=", 1)[1].strip()
    except OSError as exc:
        print("cannot read .env:", exc)
    return ""


def in_site_path(path):
    if not path.startswith("/") or ".." in path or "\\" in path:
        raise ValueError("absolute in-site path without traversal required")
    return path


def request_json(method, path, data=None, headers=None):
    in_site_path(path)
    conn = http.client.HTTPConnection(HOST, PORT, timeout=20)
    try:
        body = json.dumps(data).encode() if data is not None else None
        hdrs = dict(headers or {})
        if body is not None:
            hdrs.setdefault("Content-Type", "application/json")
        conn.request(method, path, body=body, headers=hdrs)
        resp = conn.getresponse()
        raw = resp.read()
        status = resp.status
    finally:
        conn.close()
    if status >= 400:
        raise OSError(f"HTTP {status} for {path}")
    try:
        return json.loads(raw.decode("utf-8"))
    except json.JSONDecodeError as exc:
        raise OSError(f"invalid JSON from {path}: {exc}") from exc


def fetch_page(path, headers):
    in_site_path(path)
    conn = http.client.HTTPConnection(HOST, PORT, timeout=20)
    try:
        conn.request("GET", path, headers=dict(headers))
        resp = conn.getresponse()
        raw = resp.read()
        status = resp.status
    finally:
        conn.close()
    if status >= 400:
        raise OSError(f"HTTP {status} for {path}")
    return raw.decode("utf-8")


def check_block(block, tmpdir, index):
    file_path = os.path.join(tmpdir, f"scanblk{index}.js")
    try:
        with open(file_path, "w", encoding="utf-8") as fh:
            fh.write(block)
    except OSError as exc:
        print("BAD block", index, "write failed:", exc)
        return False
    try:
        proc = subprocess.run(
            ["node", "--check", file_path], capture_output=True, text=True, timeout=30
        )
    except (OSError, subprocess.SubprocessError) as exc:
        print("BAD block", index, "node failed:", exc)
        return False
    if proc.returncode == 0:
        return True
    print("BAD block", index, "len", len(block))
    print("HEAD:", repr(block[:240]))
    print("TAIL:", repr(block[-240:]))
    print(proc.stderr[:600])
    return False


def main():
    argv = sys.argv[1:]
    path = argv[0] if len(argv) > 0 else None
    lang = argv[1] if len(argv) > 1 else "zh-CN"
    password = load_password()
    if not password:
        print("RAYRAG_ADMIN_PASSWORD missing")
        return 1
    try:
        login = request_json(
            "POST",
            "/api/v1/auth/login",
            {"email": "admin@rayrag.local", "password": password},
            {"Content-Type": "application/json"},
        )
        tok = login["data"]["access_token"]
        auth = {"Authorization": "Bearer " + tok}
        if not path or path.startswith("/kbs/auto"):
            items = (
                request_json("GET", "/api/v1/datasets", headers=auth).get("data") or []
            )
            if not items:
                print("no datasets to scan")
                return 1
            path = in_site_path("/kbs/" + str(items[0]["id"]))
        html = fetch_page(path, dict(auth, Cookie="lang=" + lang))
        blocks = re.findall(r"<script[^>]*>(.*?)</script>", html, re.S)
    except (ValueError, OSError, KeyError, json.JSONDecodeError) as exc:
        print("scan failed:", exc)
        return 1
    print("page", path, "blocks:", len(blocks))
    try:
        with tempfile.TemporaryDirectory(prefix="rayrag-pagescan-") as tmpdir:
            results = [check_block(block, tmpdir, i) for i, block in enumerate(blocks)]
    except (OSError, subprocess.SubprocessError) as exc:
        print("scan failed:", exc)
        return 1
    bad = sum(1 for ok in results if not ok)
    print("bad blocks:", bad)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
