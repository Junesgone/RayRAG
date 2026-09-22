#!/usr/bin/env python3
"""End-to-end check of the LLM-driven metadata filter (upstream
`common/metadata_utils.apply_meta_data_filter` + `rag/prompts/generator.gen_meta_filter`).

The script stands up an OpenAI-compatible stub (chat + embeddings), then drives a
running RayRAG instance through the whole chain:

  1. configure a tenant provider instance whose base URL is the stub,
  2. create a knowledge base, upload two documents and parse them,
  3. give each document its own metadata value,
  4. call `/api/v1/retrieval` with a `semi_auto` and then an `auto` metadata filter,
  5. assert the retrieval only returned the document the stub's filter selected and
     that the stub actually received the upstream `meta_filter` prompt.

Usage: RAYRAG_BASE=http://127.0.0.1:9498 RAYRAG_ADMIN_PASSWORD=... \
       python3 scripts/ui-traversal/metadata-filter-e2e.py
"""

import json
import os
import sys
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BASE = os.environ.get("RAYRAG_BASE", "http://127.0.0.1:9498")
PASSWORD = os.environ.get("RAYRAG_ADMIN_PASSWORD")
EMAIL = os.environ.get("RAYRAG_ADMIN_EMAIL", "admin@rayrag.local")
STUB_PORT = int(os.environ.get("RAYRAG_STUB_PORT", "18081"))
# The stub binds localhost by default; when RayRAG runs in a container, bind the
# bridge gateway and hand that address to the provider instance instead.
STUB_BIND = os.environ.get("RAYRAG_STUB_BIND", "127.0.0.1")
STUB_BASE = os.environ.get("RAYRAG_STUB_BASE", f"http://127.0.0.1:{STUB_PORT}/v1")
DIMENSION = 384
if not PASSWORD:
    print("RAYRAG_ADMIN_PASSWORD is required", file=sys.stderr)
    sys.exit(2)

# What the stub answers for the metadata-filter prompt: only Farm A's documents.
FILTER_ANSWER = {
    "logic": "and",
    "conditions": [{"key": "owner", "value": "Farm A", "op": "="}],
}
prompts = []


class StubHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass

    def _send(self, payload):
        body = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.endswith("/models"):
            self._send(
                {
                    "object": "list",
                    "data": [
                        {"id": "probe-chat", "object": "model"},
                        {"id": "probe-embed", "object": "model"},
                    ],
                }
            )
            return
        self._send({"ok": True})

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b"{}"
        try:
            payload = json.loads(raw or b"{}")
        except json.JSONDecodeError:
            payload = {}
        if self.path.endswith("/chat/completions"):
            system = ""
            messages = payload.get("messages") or []
            if messages:
                system = messages[0].get("content") or ""
            prompts.append({"path": self.path, "system": system, "messages": messages})
            self._send(
                {
                    "id": "stub-chat",
                    "object": "chat.completion",
                    "model": payload.get("model", "stub"),
                    "choices": [
                        {
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": json.dumps(FILTER_ANSWER),
                            },
                            "finish_reason": "stop",
                        }
                    ],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
                }
            )
            return
        if self.path.endswith("/embeddings"):
            inputs = payload.get("input")
            if isinstance(inputs, str):
                inputs = [inputs]
            self._send(
                {
                    "object": "list",
                    "model": payload.get("model", "stub"),
                    "data": [
                        {
                            "object": "embedding",
                            "index": index,
                            "embedding": [0.05] * DIMENSION,
                        }
                        for index, _ in enumerate(inputs or [""])
                    ],
                    "usage": {"prompt_tokens": 1, "total_tokens": 1},
                }
            )
            return
        self._send({"ok": True})


def call(method, path, token=None, body=None, raw=None, content_type="application/json"):
    url = BASE + path
    data = raw
    headers = {}
    if body is not None:
        data = json.dumps(body).encode()
    if data is not None:
        headers["Content-Type"] = content_type
    if token:
        headers["Authorization"] = "Bearer " + token
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            return response.status, json.loads(response.read() or b"{}")
    except urllib.error.HTTPError as error:
        raw = error.read()
        try:
            return error.code, json.loads(raw or b"{}")
        except json.JSONDecodeError:
            return error.code, {"raw": raw[:200].decode(errors="replace")}


def multipart(field, filename, content, content_type="text/plain"):
    boundary = "----rayragmetadatafilter"
    body = b"".join(
        [
            f"--{boundary}\r\n".encode(),
            f'Content-Disposition: form-data; name="{field}"; filename="{filename}"\r\n'.encode(),
            f"Content-Type: {content_type}\r\n\r\n".encode(),
            content.encode(),
            f"\r\n--{boundary}--\r\n".encode(),
        ]
    )
    return body, f"multipart/form-data; boundary={boundary}"


def main():
    results = {"errors": [], "checks": {}}

    def check(ok, label, detail=None):
        results["checks"][label] = "ok" if ok else "FAIL"
        if not ok:
            results["errors"].append(label + (f" ({detail})" if detail else ""))

    server = ThreadingHTTPServer((STUB_BIND, STUB_PORT), StubHandler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    results["stub"] = STUB_BASE

    status, body = call("POST", "/api/v1/auth/login", body={"email": EMAIL, "password": PASSWORD})
    token = (body.get("data") or {}).get("access_token")
    check(status == 200 and bool(token), "admin_login", f"{status} {body}")
    if not token:
        print(json.dumps(results, indent=1))
        return 1

    # 1. Provider instance pointing at the stub (leftovers from an earlier run
    # are removed first so the instance name stays exact).
    status, body = call("GET", "/api/v1/providers/OpenAI/instances", token)
    leftovers = [
        item["instance_name"]
        for item in (body.get("data") or [])
        if str(item.get("instance_name", "")).startswith("Metadata Probe")
    ]
    if leftovers:
        call(
            "DELETE",
            "/api/v1/providers/OpenAI/instances",
            token,
            body={"instances": leftovers},
        )
    status, body = call(
        "POST",
        "/api/v1/providers/OpenAI/instances",
        token,
        body={
            "instance_name": "Metadata Probe",
            "api_key": "probe-key",
            "base_url": results["stub"],
            "model_info": [
                {"model_name": "probe-chat", "model_type": "chat", "max_tokens": 4096},
                {
                    "model_name": "probe-embed",
                    "model_type": "embedding",
                    "max_tokens": DIMENSION,
                },
            ],
        },
    )
    check(status in (200, 201) and body.get("code") == 0, "provider_instance_created", f"{status} {body}")

    # `model_info` is what the instance stores, so the probe binds those two
    # names through RAGFlow's composite selector form.
    chat_model, embed_model = "probe-chat", "probe-embed"
    status, body = call(
        "PUT",
        "/api/v1/tenant/default-chat-model",
        token,
        # RAGFlow's composite form, exactly what the upstream web client sends.
        body={"selector": f"{chat_model}@Metadata Probe@OpenAI"},
    )
    check(
        status in (200, 201) and body.get("code") == 0,
        "default_chat_model_set_via_ragflow_composite",
        f"{status} {body}",
    )

    # 2. Knowledge base + two documents + parse.
    kb_name = f"Metadata filter probe {int(time.time())}"
    status, body = call("POST", "/api/v1/datasets", token, body={"name": kb_name})
    kb_id = (body.get("data") or {}).get("id")
    check(status in (200, 201) and bool(kb_id), "dataset_created", f"{status} {body}")
    if not kb_id:
        print(json.dumps(results, indent=1))
        return 1

    status, body = call(
        "PUT",
        f"/api/v1/datasets/{kb_id}",
        token,
        body={"embd_id": f"{embed_model}@Metadata Probe@OpenAI"},
    )
    check(status in (200, 201) and body.get("code") == 0, "dataset_embedding_bound", f"{status} {body}")

    doc_ids = []
    for name, text in (
        ("probe-farm-a.txt", "Farm A raises fish in oxygen rich ponds.\n\nSecond paragraph."),
        ("probe-farm-b.txt", "Farm B grows rice on terraces.\n\nSecond paragraph."),
    ):
        payload, content_type = multipart("file", name, text)
        status, body = call(
            "POST",
            f"/api/v1/datasets/{kb_id}/documents",
            token,
            raw=payload,
            content_type=content_type,
        )
        created = body.get("data")
        if isinstance(created, list) and created:
            created = created[0]
        doc_id = (created or {}).get("id")
        check(status in (200, 201, 202) and bool(doc_id), f"document_uploaded_{name}", f"{status} {body}")
        if doc_id:
            doc_ids.append(doc_id)

    parsed = True
    for doc_id in doc_ids:
        status, body = call(
            "POST", f"/api/v1/datasets/{kb_id}/documents/{doc_id}/reparse", token
        )
        parsed = parsed and body.get("code") == 0
    for _ in range(60):
        status, body = call("GET", f"/api/v1/datasets/{kb_id}/documents", token)
        docs = body.get("data") or []
        if docs and all(doc.get("run") == "DONE" for doc in docs):
            break
        time.sleep(1)
    chunk_counts = [doc.get("chunk_count") or doc.get("chunks") or 0 for doc in docs]
    check(parsed and all(count for count in chunk_counts), "documents_parsed", str(chunk_counts))

    # 3. One metadata value per document through the upstream selector endpoint.
    for doc_id, owner in zip(doc_ids, ["Farm A", "Farm B"]):
        status, body = call(
            "PATCH",
            f"/api/v1/datasets/{kb_id}/documents/metadatas",
            token,
            body={
                "selector": {"document_ids": [doc_id]},
                "updates": [{"key": "owner", "value": owner}],
            },
        )
        check(
            status == 200 and body.get("data", {}).get("matched_docs") == 1,
            f"metadata_written_{owner}",
            f"{status} {body}",
        )

    # 4. Retrieval with the LLM-driven filters.
    def retrieve(filter_body):
        return call(
            "POST",
            "/api/v1/retrieval",
            token,
            body={
                "question": "Which farm raises fish?",
                "kb_ids": [kb_id],
                "top_k": 10,
                "page_size": 10,
                "similarity_threshold": 0.0,
                "vector_similarity_weight": 1.0,
                "include_metadata": True,
                "reference_metadata": {"include": True, "fields": ["owner"]},
                "meta_data_filter": filter_body,
            },
        )

    status, body = retrieve({"method": "semi_auto", "semi_auto": [{"key": "owner", "op": "="}]})
    chunks = (body.get("data") or {}).get("chunks") or []
    owners = sorted({(chunk.get("document_metadata") or {}).get("owner") for chunk in chunks})
    check(status == 200 and body.get("code") == 0, "semi_auto_retrieval_ok", f"{status} {body}")
    check(chunks and owners == ["Farm A"], "semi_auto_filters_to_the_generated_condition", str(owners))

    status, body = retrieve({"method": "auto"})
    chunks = (body.get("data") or {}).get("chunks") or []
    owners = sorted({(chunk.get("document_metadata") or {}).get("owner") for chunk in chunks})
    check(status == 200 and body.get("code") == 0, "auto_retrieval_ok", f"{status} {body}")
    check(chunks and owners == ["Farm A"], "auto_filters_to_the_generated_condition", str(owners))

    # 5. The stub must have received the upstream prompt.
    check(len(prompts) >= 2, "stub_received_chat_requests", str(len(prompts)))
    results["prompt_heads"] = [item["system"][:60] for item in prompts]
    semi = next((item for item in prompts if "Operator constraints" in item["system"]), None)
    auto = next(
        (
            item
            for item in prompts
            if "Available metadata keys" in item["system"]
            and "Operator constraints" not in item["system"]
        ),
        None,
    )
    check(
        semi is not None
        and "Available metadata keys" in semi["system"]
        and '"owner"' in semi["system"]
        and "- Operator constraints:" in semi["system"]
        and 'Which farm raises fish?' in semi["system"]
        and semi["messages"][-1]["content"] == "Generate filters:",
        "semi_auto_prompt_shape",
        (semi or {}).get("system", "")[-260:],
    )
    check(
        auto is not None
        and "Available metadata keys" in auto["system"]
        and "Operator constraints" not in auto["system"],
        "auto_prompt_shape",
        (auto or {}).get("system", "")[-260:],
    )
    results["prompt_tail"] = (semi or {}).get("system", "")[-320:]

    # Clean up: the probe dataset and the probe provider instance.
    status, body = call("DELETE", "/api/v1/datasets", token, body={"ids": [kb_id]})
    check(status == 200, "dataset_removed", f"{status} {body}")
    # The default chat selector has to go first: deleting an instance that is
    # still the tenant default is (correctly) refused with 409.
    call("PUT", "/api/v1/tenant/default-chat-model", token, body={"selector": None})
    status, body = call(
        "DELETE",
        "/api/v1/providers/OpenAI/instances",
        token,
        body={"instances": ["Metadata Probe"]},
    )
    results["cleanup_provider"] = f"{status} {body}"
    check(status == 200 and body.get("code") == 0, "provider_instance_removed", f"{status} {body}")

    results["stub_prompts"] = len(prompts)
    print(json.dumps(results, indent=1, ensure_ascii=False))
    return 1 if results["errors"] else 0


if __name__ == "__main__":
    sys.exit(main())
