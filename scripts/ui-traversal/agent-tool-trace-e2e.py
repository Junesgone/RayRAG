#!/usr/bin/env python3
"""End-to-end check of the agent tool timeline (upstream
`pages/agent/log-sheet/tool-timeline-item.tsx` + `tool_use_callback`).

The script stands up an OpenAI-compatible stub that answers the Agent component's
first request with a tool call and the second one with the final answer, then
drives a running RayRAG instance through the whole chain:

  1. configure a tenant provider instance whose base URL is the stub,
  2. create a knowledge base, upload a document and parse it,
  3. create an agent whose Agent component owns a Retrieval tool,
  4. run the agent (non-streaming) and read the run trace,
  5. assert the trace carries a `tool_use_callback` record (tool_name, arguments,
     result, elapsed_time) next to the progress samples,
  6. open the try-out page in Chrome (CDP), open the run-log drawer and expand
     the tool row.

Usage: RAYRAG_BASE=http://127.0.0.1:9390 RAYRAG_ADMIN_PASSWORD=... \
       python3 scripts/ui-traversal/agent-tool-trace-e2e.py
"""

import json
import os
import sys
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BASE = os.environ.get("RAYRAG_BASE", "http://127.0.0.1:9390")
PASSWORD = os.environ.get("RAYRAG_ADMIN_PASSWORD")
EMAIL = os.environ.get("RAYRAG_ADMIN_EMAIL", "admin@rayrag.local")
STUB_PORT = int(os.environ.get("RAYRAG_STUB_PORT", "18082"))
STUB_BIND = os.environ.get("RAYRAG_STUB_BIND", "127.0.0.1")
STUB_BASE = os.environ.get("RAYRAG_STUB_BASE", f"http://127.0.0.1:{STUB_PORT}/v1")
DIMENSION = 384
CDP = os.environ.get("RAYRAG_CDP", "http://127.0.0.1:16002")
SHOTS = "docs/ui-traversal/rayrag-audit"
if not PASSWORD:
    print("RAYRAG_ADMIN_PASSWORD is required", file=sys.stderr)
    sys.exit(2)

requests_seen = []
STAMP = int(time.time())


class ProbeAbort(Exception):
    """Raised when a UI assertion already recorded its failure."""


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
            requests_seen.append(payload)
            tools = payload.get("tools") or []
            messages = payload.get("messages") or []
            answered = any(message.get("role") == "tool" for message in messages)
            if tools and not answered:
                name = tools[0]["function"]["name"]
                message = {
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [
                        {
                            "id": "call_probe_1",
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": json.dumps({"query": "rust ownership"}),
                            },
                        }
                    ],
                }
            else:
                message = {"role": "assistant", "content": "tool trace answer"}
            self._send(
                {
                    "id": "stub-chat",
                    "object": "chat.completion",
                    "model": payload.get("model", "stub"),
                    "choices": [{"index": 0, "message": message, "finish_reason": "stop"}],
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
                        {"object": "embedding", "index": index, "embedding": [0.05] * DIMENSION}
                        for index, _ in enumerate(inputs or [""])
                    ],
                    "usage": {"prompt_tokens": 1, "total_tokens": 1},
                }
            )
            return
        self._send({"ok": True})


def call(method, path, token=None, body=None, raw=None, content_type="application/json"):
    data = raw
    headers = {}
    if body is not None:
        data = json.dumps(body).encode()
    if data is not None:
        headers["Content-Type"] = content_type
    if token:
        headers["Authorization"] = "Bearer " + token
    request = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            return response.status, json.loads(response.read() or b"{}")
    except urllib.error.HTTPError as error:
        raw_body = error.read()
        try:
            return error.code, json.loads(raw_body or b"{}")
        except json.JSONDecodeError:
            return error.code, {"raw": raw_body[:200].decode(errors="replace")}


def multipart(field, filename, content, content_type="text/plain"):
    boundary = "----rayragtooltrace"
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



def run_ui_checks(results, check, token):
    """Drive Chrome through the try-out page and assert the tool timeline."""
    del token
    from playwright.sync_api import sync_playwright

    agent_id = results["run"]["agent_id"]
    with sync_playwright() as playwright:
        browser = playwright.chromium.connect_over_cdp(CDP)
        context = browser.contexts[0]
        page = context.new_page()
        page.set_viewport_size({"width": 1440, "height": 1000})
        page.goto(BASE + "/login", wait_until="domcontentloaded", timeout=40000)
        page.evaluate(
            """async ({ password }) => {
                const res = await fetch('/api/v1/auth/login', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ email: 'admin@rayrag.local', password }),
                });
                const data = await res.json();
                localStorage.setItem('rayrag_token', data.data.access_token);
                document.cookie = `rayrag_token=${data.data.access_token};path=/`;
            }""",
            {"password": PASSWORD},
        )
        page.goto(f"{BASE}/agent/{agent_id}/explore", wait_until="domcontentloaded", timeout=30000)
        try:
            page.wait_for_function(
                "() => document.querySelectorAll(\"[data-testid='explore-session-card']\").length > 0",
                timeout=20000,
            )
            page.click("[data-testid='explore-session-card']")
            page.wait_for_function(
                "() => document.querySelectorAll(\"[data-testid='explore-message-log']\").length > 0",
                timeout=20000,
            )
            page.click("[data-testid='explore-message-log']")
            page.wait_for_function(
                "() => document.querySelectorAll(\"[data-testid='explore-tool-row']\").length > 0",
                timeout=20000,
            )
            tool_rows = page.evaluate(
                """() => Array.from(document.querySelectorAll("[data-testid='explore-tool-row']")).map((node) => ({
                    name: node.querySelector('summary .agent-explore-log-text').textContent,
                    elapsed: node.querySelector('summary .agent-explore-log-elapsed').textContent,
                }))"""
            )
            check(bool(tool_rows), "ui_tool_row", tool_rows)
            check(
                bool(tool_rows) and tool_rows[0]["name"].startswith("Agent Search"),
                "ui_tool_name_pretty",
                tool_rows,
            )
            page.click("[data-testid='explore-tool-row'] summary")
            page.wait_for_function(
                "() => document.querySelectorAll(\"[data-testid='explore-tool-row'] .agent-explore-tool-value\").length >= 2",
                timeout=10000,
            )
            fields = page.evaluate(
                """() => Array.from(document.querySelectorAll("[data-testid='explore-tool-row'] .agent-explore-tool-field")).map((node) => ({
                    label: node.querySelector('.agent-explore-tool-label').textContent,
                    value: node.querySelector('.agent-explore-tool-value').textContent.slice(0, 80),
                }))"""
            )
            results["ui"] = {"rows": tool_rows, "fields": fields}
            check(
                [field["label"] for field in fields][:2] == ["Arguments", "Result"],
                "ui_tool_fields",
                fields,
            )
            os.makedirs(SHOTS, exist_ok=True)
            page.screenshot(path=f"{SHOTS}/agent-tool-trace-{STAMP}.png")
        except Exception as error:  # noqa: BLE001 - record the page state
            state = page.evaluate(
                """() => ({
                    url: location.href,
                    cards: document.querySelectorAll("[data-testid='explore-session-card']").length,
                    logs: document.querySelectorAll("[data-testid='explore-message-log']").length,
                    tools: document.querySelectorAll("[data-testid='explore-tool-row']").length,
                    sheet: document.getElementById('exploreLogSheet')?.className,
                    body: document.getElementById('exploreLogBody')?.innerHTML.slice(0, 400),
                    status: document.getElementById('exploreStatus')?.textContent,
                })"""
            )
            results["ui_state"] = state
            check(False, "ui_tool_timeline", f"{error} | {state}")
            page.close()
            raise ProbeAbort() from error
        page.close()


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

    created = {"provider": None, "kb": None, "doc": None, "agent": None}
    try:
        # 1. Provider instance pointing at the stub.
        status, body = call("GET", "/api/v1/providers/OpenAI/instances", token)
        for instance in body.get("data") or []:
            if instance.get("instance_name") == "tool-trace-probe":
                call(
                    "DELETE",
                    f"/api/v1/providers/OpenAI/instances/{instance.get('instance_name')}",
                    token,
                )
        payload = {
            "instance_name": "tool-trace-probe",
            "api_key": "stub-key",
            "base_url": STUB_BASE,
            "model_info": [
                {"model_name": "probe-chat", "model_type": ["chat"], "max_tokens": 4096},
                {"model_name": "probe-embed", "model_type": ["embedding"], "max_tokens": 4096},
            ],
        }
        status, body = call("POST", "/api/v1/providers/OpenAI/instances", token, payload)
        check(status == 200 and body.get("code") == 0, "provider_instance", f"{status} {body}")
        created["provider"] = "tool-trace-probe"

        # 2. Tenant default chat model (the Agent component resolves through it).
        status, body = call(
            "PATCH",
            "/api/v1/models/default",
            token,
            {
                "model_type": "chat",
                "model_provider": "OpenAI",
                "model_instance": "tool-trace-probe",
                "model_name": "probe-chat",
            },
        )
        check(status == 200 and body.get("code") == 0, "default_chat_model", f"{status} {body}")

        # 3. Knowledge base bound to the stub embedding, one parsed document.
        status, body = call(
            "POST",
            "/api/v1/datasets",
            token,
            {
                "name": f"tool-trace-kb-{STAMP}",
                "chunk_method": "naive",
                # Upstream composite selector: model@instance@provider.
                "embd_id": "probe-embed@tool-trace-probe@OpenAI",
            },
        )
        check(status == 200 and body.get("code") == 0, "dataset_create", f"{status} {body}")
        kb_id = (body.get("data") or {}).get("id")
        created["kb"] = kb_id
        raw, content_type = multipart(
            "file",
            f"tool-trace-{STAMP}.txt",
            "Rust ownership guide: values are moved unless they implement Copy.",
        )
        status, body = call(
            "POST", f"/api/v1/datasets/{kb_id}/documents", token, raw=raw, content_type=content_type
        )
        check(status == 200 and body.get("code") == 0, "document_upload", f"{status} {body}")
        doc_id = (body.get("data") or {}).get("doc_id") or (body.get("data") or {}).get("id")
        created["doc"] = doc_id
        deadline = time.time() + 120
        state = ""
        while time.time() < deadline:
            _, listing = call("GET", f"/api/v1/datasets/{kb_id}/documents", token)
            docs = listing.get("data") or []
            doc = next((item for item in docs if item.get("id") == doc_id), None)
            state = str((doc or {}).get("run") or "")
            if state.upper() in {"DONE", "FAILED"}:
                break
            time.sleep(2)
        check(state.upper() == "DONE", "document_parsed", state)

        # 4. Agent with a Retrieval tool.
        dsl = {
            "graph": {"nodes": [{"id": "begin", "type": "beginNode"}], "edges": []},
            "components": {
                "begin": {
                    "obj": {"component_name": "Begin", "params": {}, "mode": "conversational"},
                    "upstream": [],
                    "downstream": ["agent"],
                },
                "agent": {
                    "obj": {
                        "component_name": "Agent",
                        "params": {
                            "max_rounds": 3,
                            "prompts": "Use the retrieval tool to answer.",
                            "tools": [
                                {
                                    "component_name": "Retrieval",
                                    "name": "search_docs",
                                    "params": {"kb_ids": [kb_id]},
                                }
                            ],
                        },
                    },
                    "upstream": ["begin"],
                    "downstream": [],
                },
            },
        }
        status, body = call(
            "POST",
            "/api/v1/agents",
            token,
            {"title": f"tool-trace-agent-{STAMP}", "dsl": dsl, "kb_ids": [kb_id]},
        )
        check(status == 200 and body.get("code") == 0, "agent_create", f"{status} {body}")
        agent_id = (body.get("data") or {}).get("id")
        created["agent"] = agent_id

        status, body = call(
            "POST",
            f"/api/v1/agents/{agent_id}/completions",
            token,
            {"question": "What is rust ownership?", "stream": False},
        )
        check(status == 200 and body.get("code") == 0, "agent_run", f"{status} {body}")
        answer = ((body.get("data") or {}).get("answer") or "")
        check("tool trace answer" in answer, "agent_answer", answer[:80])
        message_id = (body.get("data") or {}).get("message_id")
        results["run"] = {
            "message_id": message_id,
            "answer": answer[:60],
            "agent_id": agent_id,
        }

        # 5. The run trace carries the tool record next to the progress samples.
        status, body = call("GET", f"/api/v1/agents/{agent_id}/logs/{message_id}", token)
        check(status == 200 and body.get("code") == 0, "trace_endpoint", f"{status} {body}")
        components = body.get("data") or []
        check(isinstance(components, list) and bool(components), "trace_components", components)
        agent_trace = next(
            (item for item in components if item.get("component_id") == "agent"), None
        )
        samples = (agent_trace or {}).get("trace") or []
        tool = next((sample for sample in samples if sample.get("tool_name")), None)
        check(tool is not None, "tool_record_present", samples)
        if tool:
            results["tool"] = tool
            check(tool.get("tool_name", "").startswith("search_"), "tool_name", tool)
            check(
                (tool.get("arguments") or {}).get("query") == "rust ownership",
                "tool_arguments",
                tool,
            )
            check(
                "ownership" in str(tool.get("result") or ""),
                "tool_result",
                str(tool.get("result"))[:120],
            )
            check(float(tool.get("elapsed_time") or 0) >= 0.0, "tool_elapsed", tool)
        progress = next(
            (sample for sample in samples if sample.get("message") == "Agent"), None
        )
        check(progress is not None, "progress_sample_present", samples)

        # 6. The UI renders it: try-out page → session → run log → tool row.
        try:
            run_ui_checks(results, check, token)
        except ProbeAbort:
            pass

    finally:
        for key, path in (
            ("agent", lambda value: f"/api/v1/agents/{value}"),
            ("kb", lambda value: f"/api/v1/datasets/{value}"),
            ("provider", lambda value: f"/api/v1/providers/OpenAI/instances/{value}"),
        ):
            value = created.get(key)
            if value:
                call("DELETE", path(value), token)
        call(
            "PATCH",
            "/api/v1/models/default",
            token,
            {"model_type": "chat", "model_provider": "", "model_instance": "", "model_name": ""},
        )
        results["prompts"] = len(requests_seen)

    print(json.dumps(results, indent=1))
    return 1 if results["errors"] else 0


if __name__ == "__main__":
    sys.exit(main())
