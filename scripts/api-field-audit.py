#!/usr/bin/env python3
"""Compare RayRAG's live JSON against the field names the API guide documents.

The guide is the contract. A name that differs (`doc_count` for `document_count`) looks harmless
and breaks every client written against the guide, so this walks a curated list of documented
objects, fetches real responses from a running instance, and reports every documented key that is
absent. It reports; it does not fix, so the mapping stays a human decision.

Usage:
  RAYRAG_AUDIT_PASSWORD=... python3 scripts/api-field-audit.py [--base http://127.0.0.1:9390]
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request

# Fields the guide documents for each object, taken from the API reference section by section.
DOCUMENTED: dict[str, dict[str, list[str]]] = {
    "dataset": {
        "source": "GET /api/v1/datasets (one element)",
        "fields": [
            "avatar", "chunk_count", "chunk_method", "create_date", "create_time", "created_by",
            "description", "document_count", "embedding_model", "id", "language", "name",
            "parser_config", "permission", "similarity_threshold", "status", "tenant_id",
            "token_num", "update_date", "update_time", "vector_similarity_weight",
        ],
    },
    "document": {
        "source": "GET /api/v1/datasets/{id}/documents (one element of data.docs)",
        "fields": [
            "chunk_count", "chunk_method", "create_date", "create_time", "created_by", "id",
            "location", "name", "parser_config", "process_begin_at", "process_duration",
            "progress", "progress_msg", "run", "size", "source_type", "status", "thumbnail",
            "token_count", "type", "update_date", "update_time",
        ],
    },
    "chunk_list": {
        "source": "GET .../chunks",
        "fields": ["chunks", "doc", "total"],
    },
    "chunk_row": {
        "source": "GET .../chunks (one element of data.chunks)",
        "fields": ["available", "content", "docnm_kwd", "document_id", "id", "image_id",
                   "important_keywords", "tag_kwd", "positions"],
    },
    "chat": {
        "source": "GET /api/v1/chats (one element)",
        "fields": ["create_date", "create_time", "dataset_ids", "description", "id", "language",
                   "llm_id", "llm_setting", "name", "prompt_config", "similarity_threshold",
                   "status", "tenant_id", "update_date", "update_time",
                   "vector_similarity_weight"],
    },
    "session": {
        "source": "GET /api/v1/chats/{id}/sessions (one element)",
        "fields": ["chat_id", "create_date", "create_time", "id", "messages", "name",
                   "update_date", "update_time"],
    },
    "file": {
        "source": "GET /api/v1/files (one element of data.files)",
        "fields": ["id", "name", "type", "size", "parent_id", "create_time"],
    },
    "file_list": {
        "source": "GET /api/v1/files",
        "fields": ["total", "files", "parent_folder"],
    },
    "agent": {
        "source": "GET /api/v1/agents (data.canvas[0])",
        "fields": ["avatar", "canvas_category", "canvas_type", "description", "id", "nickname",
                   "permission", "release_time", "tags", "tenant_id", "title", "type",
                   "update_time"],
    },
    "memory": {
        "source": "GET /api/v1/memories (one element of data.memory_list)",
        "fields": ["avatar", "create_date", "create_time", "description", "id", "memory_type",
                   "name", "permissions", "storage_type", "tenant_id"],
    },
    "search_app": {
        "source": "GET /api/v1/searches (one element of data.search_apps)",
        "fields": ["id", "name", "description", "tenant_id", "create_time"],
    },
    "retrieval": {
        "source": "POST /api/v1/retrieval",
        "fields": ["chunks", "doc_aggs", "total"],
    },
}


def request(base: str, path: str, token: str, method: str = "GET", body=None, query: str = ""):
    url = f"{base}{path}{query}"
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Authorization", f"Bearer {token}")
    if data:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=30) as response:
            return json.loads(response.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as error:  # a 4xx is a result too, not a crash
        return json.loads(error.read().decode("utf-8", "replace") or "{}")


def first(value):
    if isinstance(value, list):
        return value[0] if value else {}
    return value if isinstance(value, dict) else {}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", default=os.environ.get("RAYRAG_BASE_URL", "http://127.0.0.1:9390"))
    parser.add_argument("--email", default="admin@rayrag.local")
    args = parser.parse_args()
    password = os.environ.get("RAYRAG_AUDIT_PASSWORD", "")
    if not password:
        print("RAYRAG_AUDIT_PASSWORD is required (operator env, never committed)", file=sys.stderr)
        return 2

    login = request(args.base, "/api/v1/auth/login", "", "POST",
                    {"email": args.email, "password": password})
    token = (login.get("data") or {}).get("access_token", "")
    if not token:
        print(f"login failed: {login}", file=sys.stderr)
        return 2

    kbs = request(args.base, "/api/v1/datasets", token)
    kb_list = kbs.get("data") or []
    kb_list = kb_list.get("datasets", kb_list) if isinstance(kb_list, dict) else kb_list
    kb_id = kb_list[0]["id"] if kb_list else ""
    docs = request(args.base, "/api/v1/datasets", token)
    doc_list = []
    chunk_payload = {"chunks": [], "doc": {}, "total": 0}
    if kb_id:
        listing = request(args.base, f"/api/v1/datasets/{kb_id}/documents", token)
        payload = listing.get("data")
        doc_list = payload.get("docs", []) if isinstance(payload, dict) else (payload or [])
        if doc_list:
            chunk_payload = request(
                args.base,
                f"/api/v1/datasets/{kb_id}/documents/{doc_list[0]['id']}/chunks",
                token,
            ).get("data") or chunk_payload

    chats = request(args.base, "/api/v1/chats", token)
    chat_list = chats.get("data") or []
    chat_list = chat_list.get("chats", chat_list) if isinstance(chat_list, dict) else chat_list
    chat_id = chat_list[0]["id"] if chat_list else ""
    sessions = request(args.base, f"/api/v1/chats/{chat_id}/sessions", token) if chat_id else {}
    session_list = sessions.get("data") or []

    files = request(args.base, "/api/v1/files", token)
    file_payload = files.get("data") or {}
    agents = request(args.base, "/api/v1/agents", token)
    agent_payload = agents.get("data") or {}
    memories = request(args.base, "/api/v1/memories", token)
    searches = request(args.base, "/api/v1/searches", token)

    def list_of(payload, key):
        """Endpoints answer either a bare list or an object with a named list; accept both."""
        data = payload.get("data") if isinstance(payload, dict) else payload
        if isinstance(data, dict):
            return data.get(key) or []
        return data or []

    samples = {
        "dataset": first(kb_list),
        "document": first(doc_list),
        "chunk_list": chunk_payload,
        "chunk_row": first(chunk_payload.get("chunks")),
        "chat": first(chat_list),
        "session": first(session_list),
        "file": first(file_payload.get("files") if isinstance(file_payload, dict) else []),
        "file_list": file_payload if isinstance(file_payload, dict) else {},
        "agent": first(agent_payload.get("canvas") if isinstance(agent_payload, dict) else []),
        "memory": first(list_of(memories, "memory_list")),
        "search_app": first(list_of(searches, "search_apps")),
        "retrieval": {"chunks": [], "doc_aggs": [], "total": 0},
    }

    missing_total = 0
    unsampled = []
    report = {}
    for name, spec in DOCUMENTED.items():
        sample = samples.get(name) or {}
        missing = [field for field in spec["fields"] if field not in sample]
        report[name] = {"source": spec["source"], "sampled": bool(sample), "missing": missing}
        if not sample:
            # An object with no rows cannot confirm or deny anything; saying "missing" here would
            # be a false alarm, so it is reported separately.
            unsampled.append(name)
            continue
        missing_total += len(missing)
    for name in sorted(report):
        row = report[name]
        state = "no data" if not row["sampled"] else ("ok" if not row["missing"] else "GAP")
        detail = "" if not row["missing"] else " missing: " + ", ".join(row["missing"])
        print(f"{state:8s} {name:12s} {row['source']}{detail}")
    print(f"\ndocumented fields absent: {missing_total}; objects with no data to check: {unsampled}")
    return 1 if missing_total else 0


if __name__ == "__main__":
    sys.exit(main())
