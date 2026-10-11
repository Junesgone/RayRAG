# Document surface — distilled (RAGFlow v0.27.2)

Upstream artifact: `api/apps/restful_apis/document_api.py` @ `a024bea0cd93f39e6652a42bf84dd20c55bc560b` — 2,257 lines, **19 routes**.

Method: full static read + **39 read-only probes** against the live instance (2026-10-12, zero writes). Raw evidence: `~/.cache/rea/ragflow-v0.27.2/round10-forensics/` (`round10-summary.tsv`, 39 `*.body`).

Envelope: application errors always ride HTTP **200** with a numeric `code` (101 ARGUMENT_ERROR / 102 DATA_ERROR); only unmatched routes fall through to a framework 404 (e.g. `..%2F`-encoded traversal never reaches the app handler). Two source-only mixed envelopes carry `code=500` with HTTP 200 (partial local upload; partial batch status).

## Route contracts (probe-verified unless marked *source*)

| Route | Contract highlights |
|---|---|
| `GET /datasets/{id}/documents` | invalid `orderby` → 101 `invalid orderby field: {v}`; invalid `types` → 102 `Invalid filter conditions: {v} type`; invalid `run` → 102 `Invalid filter run status conditions: {v}`; `page`/`page_size` invalid **silently fall back to defaults**; `type=filter` → `{total, filter}`; `id`+`ids` together → 102 `Should not provide both 'id':{id} and 'ids'{[...]}`; foreign `id` → 102 `you don't own the document {id}`; docs map uses `dataset_id/chunk_count/...` keys, thumbnail rewritten to `/api/v1/documents/images/{kb_id}-{thumb}` |
| `GET /thumbnails` | no `doc_ids` → 101 + `data=false` + `Lack of "Document ID"`; bogus ids → `code:0, data:{}` (empty map **is** success) |
| `GET /documents/images/{image_id}` | split on first hyphen only; malformed or missing → 102 `Image not found.` (indistinguishable) |
| `GET /documents/artifact/{filename}` | bad ext → 102 `invalid file type`; missing/unauthorized → 102 `Artifact not found.`; access = session/message reference + canvas accessibility |
| `GET /documents/{doc_id}/preview` • `GET /documents/{doc_id}` • `GET /datasets/{id}/documents/{doc_id}` | all three → 102 `document not found` for foreign/missing ids (anti-enumeration parity); empty storage → `This file is empty.` (*source*) |
| `DELETE /datasets/{id}/documents` | `{}` → 102 `should either provide doc ids or set delete_all(true), dataset: {id}. `; both → 102 `should not provide both doc ids and delete_all(true), dataset: {id}. `; unknown ids → 102 `These documents do not belong to dataset {id} or Document not found: {ids}`; success → `{deleted: n}` (*source*) |
| `POST …/documents/parse` • `…/stop` | `{}` → 102 `` `document_ids` is required ``; foreign ids → 102 `Documents not found: ['{id}']`; success `{success_count, errors?}`; stop on non-running appends `Can't stop parsing document that has not started or already completed` into `errors` (*source*) |
| `POST …/documents/batch-update-status` | bad status → 101 `"Status" must be either 0 or 1:{v}!`; empty list → 101 `"doc_ids" must be a non-empty list.`; partial failure → HTTP 200 + code 500 + `Partial failure` + per-doc map (*source*) |
| `GET /datasets/{id}/metadata/summary` | foreign ds → 102 `You don't own the dataset {id}. ` (trailing space); success `{summary: {...}}` |
| `POST /datasets/{id}/metadata/update` • `PATCH …/documents/metadatas` | duplicate handlers, same shape; `{}` **succeeds** with `{updated:0, matched_docs:0}`; falsy `selector` (incl. `[]`) coerced via `or {}` — **no error for empty selector** |
| `POST /documents/upload` | exclusive inputs: both → 101 `Provide either multipart file(s) or ?url=..., not both.`; neither → 101 `Missing input: provide multipart file(s) or url`; unsafe URL → 101 `URL resolves to a non-public address ({ip}), which is not allowed.` |
| `PATCH /datasets/{id}/documents/{doc_id}` | `{name:null}` → 102 `Field: <name> - Message: <Input should be a valid string> - Value: <None>` (**before** ownership checks); foreign doc → 102 `the dataset doesn't own the document`; legacy RAPTOR fields silently dropped (*source*); `Not supported yet!` for visual/ppt parser mismatch (*source*) |
| `PUT …/documents/{doc_id}/metadata/config` | requires `metadata`; foreign doc → 102 `document {doc} not found in dataset {ds}`; missing key → 101 `metadata is required` (*source*) |
| `POST /datasets/{id}/documents` (upload) | `type=local|web|empty`; local: `No file part!` / `No file selected!` (101); all-failed → 102 `There seems to be an issue with your file format...`; **partial success → HTTP 200 + code 500 + data** (*source*); web: `Lack of "name"` / `Lack of "url"` / `The URL format is invalid` (101); empty: `File name can't be empty.` (101) / `Duplicated document name in the same dataset.` (102) (*source*) |
| `POST /documents/ingest` | `{doc_ids, run, delete, apply_kb, user_id}`; success `data=true`; error → `get_json_result(error_code, message)` e.g. 109 `no authorization` (*source*) |

## Notes for the application-side crosscheck (next round)

Crosscheck list against `src/server.rs` handlers (one row per item, to be recorded in `apply-crosscheck-02.md`):
1. exact strings & codes for all 39 probe-verified shapes above;
2. list silent-pagination-fallback; `type=filter` shape; thumbnail rewrite path;
3. empty-selector meta updates succeed (both routes); trailing-space messages;
4. `id`+`ids` conflict message's odd formatting;
5. mixed envelopes (partial upload / partial batch status) — same family as the file-commit envelope item;
6. images composite-id split rule + content-type sniffing parity;
7. artifact access gate behavior (session or message reference).
