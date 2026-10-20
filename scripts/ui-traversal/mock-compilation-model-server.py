#!/usr/bin/env python3
"""Deterministic OpenAI-compatible model stub for the compilation E2E check.

Serves /v1/embeddings (384-dim vectors) and /v1/chat/completions (MAP JSON,
page synthesis markdown, partition/route JSON) so the wiki compile pipeline can
run end to end without a real model. Dev/verification only.
"""
import json
import re
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

DIM = 384
PORT = 18089


def map_reply(user):
    chunk = "C1"
    match = re.search(r"^- (C\d+)$", user, re.MULTILINE)
    if match:
        chunk = match.group(1)
    return json.dumps(
        {
            "entities": [
                {
                    "name": "Alpha",
                    "type": "other",
                    "aliases": [],
                    "source_chunk_id": chunk,
                }
            ],
            "concepts": [
                {
                    "term": "Alpha",
                    "definition_excerpt": "Alpha is the first subject of the source text.",
                    "source_chunk_id": chunk,
                }
            ],
            "claims": [
                {
                    "statement": "Alpha is the first subject of the source text.",
                    "subject": "Alpha",
                    "confidence": "explicit",
                    "source_chunk_id": chunk,
                }
            ],
            "relations": [],
            "topics": ["General"],
        },
        ensure_ascii=False,
    )


def page_title(user):
    match = re.search(r"^- Title:\s*(.+)$", user, re.MULTILINE)
    if match:
        title = match.group(1).strip()
        if title:
            return title
    match = re.search(r"^- Page ID:\s*([^\s]+)$", user, re.MULTILINE)
    if match:
        return match.group(1).split("/")[-1].replace("_", " ").title()
    return "Mock Page"


def page_reply(user):
    title = page_title(user)
    return (
        "SUMMARY: A concise encyclopedic summary of " + title + ".\n"
        "TITLE: " + title + "\n"
        "TOPIC: General\n\n"
        "# " + title + "\n\n"
        + title
        + " is described by the source text as the first subject of the document.\n\n"
        "## Overview\n\n"
        "The source explains " + title + " in one grounded paragraph.\n"
    )


def groups_reply(user):
    match = re.search(r"through (\d+) must appear", user)
    if not match:
        return "[]"
    last = int(match.group(1))
    groups = [[index] for index in range(last + 1)]
    return json.dumps(groups)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        pass

    def _send(self, payload, status=200):
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b"{}"
        try:
            payload = json.loads(raw or b"{}")
        except Exception:
            payload = {}
        with open("/tmp/mock-model.log", "a", encoding="utf-8") as log:
            log.write(self.path + "\n")
        if self.path.endswith("/embeddings"):
            inputs = payload.get("input")
            if isinstance(inputs, str):
                inputs = [inputs]
            data = [
                {"object": "embedding", "index": index, "embedding": [0.01] * DIM}
                for index, _ in enumerate(inputs or [""])
            ]
            self._send(
                {
                    "object": "list",
                    "data": data,
                    "model": payload.get("model", "mock-embed"),
                    "usage": {"prompt_tokens": 1, "total_tokens": 1},
                }
            )
            return
        if self.path.endswith("/chat/completions"):
            messages = payload.get("messages") or []
            system = ""
            user = ""
            for message in messages:
                role = message.get("role")
                content = message.get("content")
                if not isinstance(content, str):
                    content = json.dumps(content)
                if role == "system" and not system:
                    system = content
                if role == "user":
                    user = content
            if "knowledge extraction engine" in system:
                reply = map_reply(user)
            elif "wiki COMPILER" in system or "wiki editor" in system:
                reply = page_reply(user)
            elif "plan concise, semantically coherent encyclopedia pages" in system:
                reply = groups_reply(user)
            elif "route entities to semantically appropriate encyclopedia pages" in system:
                reply = "[]"
            elif "dedup assistant" in system:
                reply = "[]"
            else:
                reply = "{}"
            self._send(
                {
                    "id": "chatcmpl-mock",
                    "object": "chat.completion",
                    "created": 0,
                    "model": payload.get("model", "mock-model"),
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": reply},
                            "finish_reason": "stop",
                        }
                    ],
                    "usage": {
                        "prompt_tokens": 10,
                        "completion_tokens": 10,
                        "total_tokens": 20,
                    },
                }
            )
            return
        self._send({"error": "unsupported"}, status=404)


if __name__ == "__main__":
    open("/tmp/mock-model.log", "w").close()
    server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    print("mock model server on", PORT, flush=True)
    server.serve_forever()
