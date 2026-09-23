#!/usr/bin/env python3
"""End-to-end probe for PDF chunk positions (DeepDOC `@@page\\tx0\\tx1\\ttop\\tbottom##`).

RayRAG's PDF parser now reads the content stream with coordinates and tags every
line box exactly like `deepdoc/parser/pdf_parser.py::_line_tag`, so the chunker
projects them into `position_int` / `page_num_int` / `top_int` and the chunk API
returns real `positions` — the geometry the web previewer overlays on a rendered
page (upstream `useGetChunkHighlights` + `pdf-preview.tsx`).

The probe generates a two-paragraph PDF whose geometry is known to the point,
uploads it to a throwaway dataset, parses it, and checks the chunk API:

* every chunk carries `positions`, each entry a 5-element
  `[page, x0, x1, top, bottom]` list with 1-based pages and positive coordinates,
* the visible content has no `@@…##` tags left in it,
* the first paragraph's box starts at the x/y the generator wrote,
* `page_num_int` / `top_int` agree with `positions`.

Usage: RAYRAG_BASE=http://127.0.0.1:9390 RAYRAG_LOGIN_PASSWORD=... \\
       python3 scripts/ui-traversal/pdf-positions-e2e.py
"""

from __future__ import annotations

import json
import os
import sys
import time
import urllib.error
import urllib.request

BASE = os.environ.get("RAYRAG_BASE", "http://127.0.0.1:9390")
PASSWORD = os.environ.get("RAYRAG_LOGIN_PASSWORD")
EMAIL = os.environ.get("RAYRAG_LOGIN_EMAIL", "admin@rayrag.local")
TIMEOUT = float(os.environ.get("RAYRAG_PROBE_WAIT", "180"))

STAMP = int(time.time() * 1000)
DATASET_NAME = f"pdf-positions-probe-{STAMP}"
# Paragraph one at baseline y=700 (12 pt), paragraph two 200 pt lower, so they
# stay separate boxes. Left margin 72 pt, nine 500/1000-em glyphs + a 250 space
# → 51 pt wide.
FIRST_TEXT = "Positioned line one"
SECOND_TEXT = "Positioned line two"
FIRST_X0 = 72.0
FIRST_TOP = 792.0 - (700.0 + 0.75 * 12.0)  # ascent 0.75 em above the baseline

checks: list[str] = []


def check(condition: bool, label: str, detail: object = "") -> None:
    if condition:
        checks.append(label)
        print(f"  ok  {label}")
        return
    raise AssertionError(f"{label} — {detail}")


def request(
    method: str,
    path: str,
    *,
    token: str | None = None,
    body: bytes | None = None,
    content_type: str = "application/json",
) -> tuple[int, object]:
    url = f"{BASE}{path}"
    headers = {"Content-Type": content_type}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as response:
            raw = response.read()
            status = response.status
    except urllib.error.HTTPError as error:  # pragma: no cover - surfaced below
        raw = error.read()
        status = error.code
    try:
        return status, json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return status, raw


def build_pdf() -> bytes:
    """A one-page PDF with two paragraphs at known coordinates."""
    content = (
        "BT /F1 12 Tf "
        f"{FIRST_X0:.1f} 700 Td ({FIRST_TEXT}) Tj "
        f"0 -200 Td ({SECOND_TEXT}) Tj "
        "ET"
    ).encode("ascii")
    widths = " ".join("250" if code == 32 else "500" for code in range(32, 127))
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R "
        b"/Resources << /Font << /F1 5 0 R >> >> >>",
        b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream",
        (
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 "
            f"/LastChar 126 /Widths [{widths}] /Encoding /WinAnsiEncoding >>"
        ).encode("ascii"),
    ]
    pdf = bytearray(b"%PDF-1.7\n")
    offsets: list[int] = []
    for number, body in enumerate(objects, start=1):
        offsets.append(len(pdf))
        pdf += b"%d 0 obj\n" % number + body + b"\nendobj\n"
    xref_offset = len(pdf)
    pdf += b"xref\n0 %d\n" % (len(objects) + 1)
    pdf += b"0000000000 65535 f \n"
    for offset in offsets:
        pdf += b"%010d 00000 n \n" % offset
    pdf += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (
        len(objects) + 1,
        xref_offset,
    )
    return bytes(pdf)


def multipart(fields: dict[str, str], filename: str, payload: bytes) -> tuple[bytes, str]:
    boundary = f"----rayragprobe{STAMP}"
    chunks: list[bytes] = []
    for name, value in fields.items():
        chunks.append(
            f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n{value}\r\n'.encode()
        )
    chunks.append(
        (
            f'--{boundary}\r\nContent-Disposition: form-data; name="file"; '
            f'filename="{filename}"\r\nContent-Type: application/pdf\r\n\r\n'
        ).encode()
    )
    chunks.append(payload)
    chunks.append(f"\r\n--{boundary}--\r\n".encode())
    return b"".join(chunks), f"multipart/form-data; boundary={boundary}"


def main() -> int:
    if not PASSWORD:
        print("RAYRAG_LOGIN_PASSWORD is required", file=sys.stderr)
        return 2
    status, login = request(
        "POST",
        "/api/v1/auth/login",
        body=json.dumps({"email": EMAIL, "password": PASSWORD}).encode(),
    )
    check(status == 200 and login.get("code") == 0, "login", login)
    token = login["data"]["access_token"]

    dataset_id = None
    document_id = None
    try:
        status, created = request(
            "POST",
            "/api/v1/datasets",
            token=token,
            body=json.dumps({"name": DATASET_NAME, "chunk_method": "naive"}).encode(),
        )
        check(200 <= status < 300 and created.get("code") == 0, "dataset created", (status, created))
        dataset_id = created["data"]["id"]

        pdf = build_pdf()
        body, content_type = multipart({"name": "positions.pdf"}, "positions.pdf", pdf)
        status, uploaded = request(
            "POST",
            f"/api/v1/datasets/{dataset_id}/documents",
            token=token,
            body=body,
            content_type=content_type,
        )
        check(200 <= status < 300 and uploaded.get("code") == 0, "document uploaded", (status, uploaded))
        documents = uploaded["data"]
        document_id = documents[0]["id"] if isinstance(documents, list) else documents["id"]

        status, queued = request(
            "POST",
            f"/api/v1/datasets/{dataset_id}/documents/{document_id}/reparse",
            token=token,
            body=b"{}",
        )
        check(status == 200 and queued.get("code") == 0, "parse queued", queued)

        deadline = time.time() + TIMEOUT
        document: dict = {}
        chunks: list[dict] = []
        while time.time() < deadline:
            status, listed = request(
                "GET", f"/api/v1/datasets/{dataset_id}/documents/{document_id}", token=token
            )
            document = listed.get("data") or {}
            status, payload = request(
                "GET",
                f"/api/v1/datasets/{dataset_id}/documents/{document_id}/chunks",
                token=token,
            )
            chunks = (payload.get("data") or {}).get("chunks") or []
            if chunks and document.get("run") in ("DONE", "3") and document.get("progress", 0) >= 1:
                break
            if document.get("run") in ("FAIL", "4"):
                raise AssertionError(f"parse failed: {document}")
            time.sleep(1.0)

        check(bool(chunks), "document produced chunks", document)
        reported = document.get("chunk_count", document.get("chunk_num", 0))
        check(reported >= len(chunks), "document reports its chunk count", document)

        positions_found = 0
        for chunk in chunks:
            positions = chunk.get("positions")
            check(
                isinstance(positions, list) and positions,
                f"chunk {chunk.get('id', '?')[:8]} carries positions",
                chunk,
            )
            for position in positions:
                check(
                    isinstance(position, list) and len(position) == 5,
                    "position has five fields",
                    position,
                )
                page, x0, x1, top, bottom = position
                check(page >= 1, "page numbers are 1-based", position)
                check(
                    0 <= x0 < x1 and 0 <= top < bottom,
                    "position box is well formed",
                    position,
                )
                positions_found += 1
            check("@@" not in chunk.get("content", ""), "tags are stripped from content", chunk)
            check(
                not chunk.get("content", "").endswith("##"),
                "no tag tail in content",
                chunk,
            )

        check(positions_found >= 2, "both paragraphs are positioned", positions_found)

        first = chunks[0]
        position = first["positions"][0]
        check(
            abs(position[1] - FIRST_X0) <= 1,
            f"first box x0 is the generated margin ({FIRST_X0})",
            position,
        )
        check(
            abs(position[3] - FIRST_TOP) <= 1,
            f"first box top is the generated baseline ({FIRST_TOP:.1f})",
            position,
        )
        check(FIRST_TEXT in first.get("content", ""), "first paragraph text survives", first)
        check(
            SECOND_TEXT in "\n".join(chunk.get("content", "") for chunk in chunks),
            "second paragraph text survives",
            [chunk.get("content") for chunk in chunks],
        )

        pages = [position[0] for chunk in chunks for position in chunk["positions"]]
        tops = [position[3] for chunk in chunks for position in chunk["positions"]]
        check(
            first.get("page_num_int") in (None, pages),
            "page_num_int mirrors positions",
            first.get("page_num_int"),
        )
        check(
            first.get("top_int") in (None, tops),
            "top_int mirrors positions",
            first.get("top_int"),
        )
    finally:
        if dataset_id:
            request(
                "DELETE",
                f"/api/v1/datasets/{dataset_id}",
                token=token,
                body=json.dumps({"ids": [dataset_id]}).encode(),
            )

    print(json.dumps({"checks": checks, "count": len(checks)}, indent=2))
    print(f"pdf-positions-e2e: {len(checks)} checks ok")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except AssertionError as error:
        print(f"PROBE FAILED: {error}", file=sys.stderr)
        sys.exit(1)
