# Changelog

All notable changes to RayRAG are documented in this file.

## [0.19.0] - 2026-10-03

### One real tool catalogue, and a capability report that describes this deployment

Three surfaces answered from invented data:

* `GET /api/v1/mcp/tools` returned two hard-coded tools — `search_knowledge`, `list_datasets` — that no
  code in the deployment could execute, while `mcp_server::ToolRegistry` held the real catalogue;
* `GET /api/v1/plugins` returned a fixed list naming `PaddleOCR` and `mxbai-rerank` whatever the
  deployment was actually configured with;
* `POST /api/v1/plugins/{id}/toggle` answered `"Plugin X toggled"` for an operation that changed
  nothing at all.

All three now read one source of truth. `GET /api/v1/plugin/tools` (previously missing) and
`/api/v1/mcp/tools` both build from `ToolRegistry`, enriched with **the caller's own datasets** — the
retrieval tool is told which dataset ids exist, because a model cannot pass an id it has never seen.
`GET /api/v1/plugins` reports, per capability, whether this deployment has it and which model provides
it, read from the running state:

```json
{"id":"embedding","name":"Embedding","available":true,"model":"Qwen3-Embedding-4B-Q4_K_M.gguf",
 "configured_by":"EMBED_API_BASE / EMBED_MODEL"}
```

Every line says **where a deployment changes it**, and a capability that is *absent* carries the reason
rather than a bare `false` — a distinction the test suite enforces by failing on any unavailable
capability without a `detail`. (It failed on the first run for exactly that reason: `llm` and
`embedding` reported `false` with nothing to act on, so both gained one.)

The payload also states what `available` means: **configuration, not liveness**. On this host the rerank
endpoint is configured while the reranker itself is stopped, so a report that read as "reranking works"
would be wrong; the response carries that sentence, the count is labelled `configured`, and the UI says
*configured* rather than *available*.

`toggle` now answers `103` with the reason — these capabilities are provider-backed and configured
through the wizard or the environment, so a runtime switch would be a claim the deployment cannot
honour — and names the capability when the id is unknown.

The model page gained a **Capabilities** card that renders the report with the missing capabilities
explained underneath.

**2218 tests pass, 0 failures**, including the catalogue being the registry's own, the live dataset
listing reaching the retrieval tool, both listings agreeing, and the toggle and unknown-id refusals. The
parity ledger moved from 7 to **6** remaining gaps (195 upstream routes, 342 of ours, 142 exact).

## [0.18.0] - 2026-10-03

### The connector family becomes real: instances, history, a probe and a rebuild

The connector endpoints answered from a **hard-coded array of three invented connectors**
(`local`/`file`, `web`, `database`) and `create` echoed its body without storing anything — a user could
"add" a data source and find it gone on reload. Four documented endpoints were missing entirely. All of
it is now backed by one persisted, owner-scoped store:

| endpoint | behaviour |
|---|---|
| `POST /api/v1/connectors` | validates the name and the type against the connector registry, then stores the instance |
| `GET /api/v1/connectors/{id}` | the instance, `404`/`102 Connector not found.` otherwise |
| `PATCH` / `DELETE /api/v1/connectors/{id}` | update name/enabled/config, or remove it with its history |
| `GET /api/v1/connectors/{id}/logs` | the instance's history, newest first |
| `POST /api/v1/connectors/{id}/test` | **really builds the connector and lists the source** |
| `POST /api/v1/connectors/{id}/rebuild` | reports what the source offers, and says it wrote nothing |

The refusals are the point of several of these. `create` names the unknown type **and lists the known
ones**, because a connector type comes from `ConnectorRegistry::kinds()` rather than a second list that
could drift; a malformed body is answered as a business code (`400`/`101`) instead of axum's plain-text
rejection; and **`rebuild` never claims to have imported anything** — it reports
`{"scanned": N, "imported": 0}` and says in the message that importing is the parse pipeline's job.
Saying "rebuilt 12 documents" while writing nothing is exactly the silent lie this project keeps
removing.

`test` probes for real: it builds the connector through the registry, loads credentials and lists the
source under the deployment's command timeout, then records the outcome as the instance's `last_test`
**and** as a history line. A source with no builder says so rather than reporting success. The history
is bounded (2000 lines kept, 500 returned per request) so a connector someone retries in a loop cannot
grow without end.

Configuration is read under both the upstream and the friendly key: `url` is the source location and
`path` is accepted as its alias, because `LocalConnector` reads its root directory from `url` and a user
who typed a path should not be told "root directory URL is required".

The data-source settings page gained a **Connectors** card that lists the instances with Test, Rebuild,
Logs and Delete per row, plus a create form whose type list is rendered from the registry.

**2216 tests pass, 0 failures**, including the owner scoping, the bounded history, the registry-derived
type list, both configuration key spellings, and a router test that walks create → read → probe a real
two-file directory → rebuild → history → patch → delete → refusals. The write verbs keep the
management-write guard they always had: a non-admin still gets `403`. The parity ledger moved from 11 to
**7** remaining gaps (195 upstream routes, 341 of ours, 141 exact).

## [0.17.0] - 2026-10-03

### The embedding check: do this dataset's stored vectors still agree with its model?

Switching an embedding model on a dataset that already holds vectors is the kind of change that ruins
retrieval without raising an error: the query vector comes from the new model, the stored vectors came
from the old one, and the similarity scores become meaningless. Upstream answers that question with
`POST /api/v1/datasets/{id}/embedding/check`, and RayRAG now answers it too — re-embedding a sample of
the dataset's own chunk text and comparing against the vectors already stored:

```json
{"summary": {"kb_id", "model", "sampled", "valid", "avg_cos_sim", "min_cos_sim", "max_cos_sim",
             "match_mode", "threshold"},
 "results": [{"chunk_id", "doc_id", "doc_name", "vector_field", "vector_dim",
              "stored_vector_dim", "cos_sim"}]}
```

An average at or above **0.9** is a success; below it the answer carries business code **10**
(`NOT_EFFECTIVE`) with upstream's message and the same data attached, so a caller cannot mistake a
worrying number for a passing check. Refusals are explicit: `` `embd_id` is required. ``,
`no authorization`, and `No embedded chunks are available to compare.` — the last one because reporting
a similarity of zero for a dataset with nothing embedded would read as a failed check instead of an
absent one.

Two deliberate choices: the sample is taken by **even spacing** over the dataset's embedded chunks
rather than at random, so the same dataset gives the same answer twice — this number decides whether a
migration is safe; and a dimension mismatch is reported as `cos_sim: 0.0` **with both widths visible**
(`vector_dim` and `stored_vector_dim`) instead of as a generic failure. `embd_id` also accepts the
literal `default`, which resolves to this deployment's configured `EMBED_MODEL`; without that alias a
client would have to invent a label and print it next to real numbers.

The dataset configuration tab gained an **Embedding check** button that runs it and prints the average,
the range, how many chunks were compared and which model answered — including the code 10 verdict. The
button asks for four chunks rather than the API default of eight, because eight measured **54.9 s**
against this host's embedding endpoint; the default stays useful for automation and the button stays
responsive for a person who just pressed it. The `code 10` branch is covered by the tests (threshold and
dimension mismatch) rather than by the live probe — producing genuinely mismatched vectors live would
mean corrupting the dataset being checked.

**2210 tests pass, 0 failures**, including the cosine edge cases (identical, orthogonal, opposite,
mismatched widths), the sampling being reproducible and in range, the threshold, the alias resolving to
the configured model, and a router test that walks all three refusals. The parity ledger moved from 12
to **11** remaining gaps (195 upstream routes, 337 of ours, 137 exact).

## [0.16.0] - 2026-10-03

### Speech: text to speech and speech to text

Upstream's chat surface has two audio endpoints; RayRAG had clients for both providers in `audio.rs`
but no route that used them. This release adds the routes and makes an unconfigured deployment say so:

| endpoint | behaviour |
|---|---|
| `POST /api/v1/chat/audio/speech` | `{"text": …}` in, `audio/mpeg` out, with `Cache-Control: no-cache`, `Connection: keep-alive` and `X-Accel-Buffering: no` |
| `POST /api/v1/chat/audio/transcription` | multipart `file` plus `stream`; `{"text": …, "tokens": …}` or an SSE stream |

The wording is upstream's, because clients match on it: `102 No default TTS model is set`,
`102 No default ASR model is set`, `102 Missing 'file' in multipart form-data`, and
`102 Unsupported audio format: .mp4. Allowed: .aac, .flac, .m4a, .mp3, .ogg, .opus, .wav, .webm, .wma`.
A missing `text` is a `101` naming the field rather than upstream's unhandled lookup.

Two deliberate differences, both in the direction of not hiding a failure:

* **speech synthesizes before answering**, so a provider error is an explicit `105` carrying the
  provider message instead of a truncated audio stream whose headers already promised `200 OK`;
* **`stream=true` on transcription emits one `partial` event carrying the whole transcript** and then
  `[DONE]`, because the transcription call underneath is not itself streaming. Inventing partial
  boundaries would look more faithful and tell the client less.

The interface reaches both: the model page gained a **Speech** card — type text and hear it, or choose an
audio file and read the transcript. Its failures are printed as text, so an unconfigured deployment
shows the sentence naming the missing model instead of doing nothing. Walking it in a browser as a user
(CDP) reported the card, both buttons and the file picker's accept list (exactly the allowlist above);
*Speak* with empty text said `Type something first`, *Speak* with text on this unconfigured deployment
said `No default TTS model is set`, *Transcribe* with no file said `Choose an audio file first`, and the
page logged **0 errors**. The chat composer's own microphone control — where upstream puts it — is still
pending; this card is a working surface, not a claim that the composer is finished.

Configuration is `RAYRAG_TTS_API_BASE` / `RAYRAG_TTS_API_KEY` / `RAYRAG_TTS_MODEL` /
`RAYRAG_TTS_VOICE` (defaults `tts-1`, `alloy`) alongside the existing `RAYRAG_ASR_*`. The first-login
wizard and `.env.example` gained the four TTS fields in the same release, so the guide, the template
and the registry cannot drift apart.

**2205 tests pass, 0 failures**, including the suffix parsing, the exact refusal sentence, the
allowlist as the documented set, and a router test that walks the documented refusals over a real
multipart body. The parity ledger moved from 14 to **12** remaining gaps (195 upstream routes, 336 of
ours, 136 exact).

## [0.15.0] - 2026-10-03

### Auto-metadata schema: the dataset's field configuration and the per-document override

Upstream documents three endpoints for the schema a dataset defines over its documents' metadata, and
RayRAG answered none of them:

| endpoint | behaviour |
|---|---|
| `GET /api/v1/datasets/{id}/metadata/config` | the dataset's schema, with the deployment's built-in fields alongside it |
| `PUT /api/v1/datasets/{id}/metadata/config` | replaces the user list and answers with the stored schema |
| `PUT /api/v1/datasets/{id}/documents/{doc_id}/metadata/config` | a per-document override |

The shape is upstream's `AutoMetadataConfig`: `metadata` is what the user configured and
`built_in_metadata` is what this deployment offers on top — the keys the extraction pipeline really
writes (`author`, `tags`, `url`, `language`, `published_at`). A client shows both and edits only the
first; a body that tries to set the built-ins is overridden rather than trusted, because they describe
the deployment, not the request.

**Validation is real**: a key must be non-empty after trimming and at most 255 characters, `type` must
be one of `string`/`list`/`time`/`number`, a duplicate key is refused (it would make every later filter
ambiguous), and an empty `enum` value is refused. A rejected body is a `101` naming the field, and the
stored schema is left untouched — silently keeping a malformed schema would break filtering later.

The interface can reach it too: the dataset configuration tab gained a *Metadata schema* card that
lists the fields, adds and removes rows, saves through the endpoint, and shows the built-ins it cannot
edit. It is a self-contained block (its own markup and its own `<script>`), which is the pattern that
avoids the string-literal trap the composer control fell into in 0.13.0.

Deleting a dataset now clears its schema and its documents' overrides with it — leaving them behind
would resurrect a schema if a new dataset were ever given the same id.

**2201 tests pass, 0 failures**, including every validation rule, the built-ins being the deployment's,
the per-document scoping, and a full router round trip (read the defaults → save → read back → three
refused bodies → the stored schema unchanged → override a document → refuse an unknown dataset on both
verbs). The parity ledger moved from 16 to **14** remaining gaps (195 upstream routes, 334 of ours, 134
exact).

## [0.14.0] - 2026-10-03

### Document thumbnails, and the image endpoint they point at

Two more documented endpoints, as one feature:

| endpoint | behaviour |
|---|---|
| `GET /api/v1/thumbnails?doc_ids=…` | a map from document id to its thumbnail url, or `null` when it has none; `doc_ids` is repeatable |
| `GET /api/v1/documents/images/{image_id}` | serves the thumbnail (`.jpg`) or the document's own image (`{id}.{ext}`) |

RayRAG has no pre-rendered thumbnails, so it makes them: an image document is decoded, scaled to fit
200px on its longest side (aspect ratio kept) and encoded as JPEG on first request, then cached beside
the other uploads. A document whose format needs a renderer — a PDF, a Word file — has **no**
thumbnail, and the map says `null` for it rather than inventing a placeholder, which is what the guide's
own client expects. A missing `doc_ids` is answered exactly as upstream does: HTTP 200 with business
code `101 Lack of "Document ID"`.

Two bugs the tests caught, both worth naming because they were silent:

* **Repeated query parameters collapsed.** The handler read `doc_ids` through a map extractor, and a
  map keeps only the *last* value — so a request for three documents returned one. It reads the raw
  query now, so repeats and the comma-separated spelling both work.
* **The image id could not be split on the first dash.** Upstream's ids are 32 hex characters with no
  dashes; RayRAG's are dashed UUIDs, where splitting at the first dash cuts the id in half. The
  knowledge base is now found by longest matching prefix, so `{kb_id}-{name}.jpg` resolves for a
  dashed UUID and a name that itself contains dashes. And the document id is the name without *any*
  extension — trimming only `.jpg` asked for a document called `photo-1.png`.

**2197 tests pass, 0 failures**, including the generation itself (a 600×300 PNG becomes a 200×100
JPEG), the map for an image document, a text document and an unknown id, and both served forms. The
parity ledger moved from 18 to **16** remaining gaps (195 upstream routes, 332 of ours, 132 exact).

## [0.13.0] - 2026-10-03

### Runtime attachments: the upload endpoint, and the two that stream them back

Four documented endpoints were missing, all part of one feature — a file (or a crawled page) that is
kept as an attachment and later fetched, previewed or handed to an agent:

| endpoint | behaviour |
|---|---|
| `POST /api/v1/documents/upload` | multipart `file`(s) **or** `?url=`; one file answers with an object, several with an array |
| `POST /api/v1/agents/{agent_id}/upload` | the same, stored against a canvas (`102 canvas not found.` when it does not exist) |
| `GET /api/v1/agents/attachments/{id}/download` | streams the bytes; `ext`, `mime_type`, `filename` and `disposition=inline` choose the headers |
| `GET /api/v1/agents/attachments/{id}/preview` | the same, always inline |

Two behaviours are worth naming, because both were wrong in the first draft and are now checked:

* **A `?url=` upload is fetched by the server, so it is validated first.** Only `http`/`https`, no
  loopback or private address, no `.local`/`.internal` host, at most ten redirects, and the response is
  capped at the configured upload limit. A test asserts that `127.0.0.1`, `localhost`, `10.0.0.5`,
  `192.168.1.10`, `172.16.4.4`, `nas.local` and `file://` are all refused before anything is fetched.
  Upstream validates its crawl URLs the same way; an upload endpoint must not be a proxy into the
  host's own network.
* **Validation happens before storage.** The first draft stored a multipart file and *then* noticed that
  a `url` had been passed too, leaving an orphan attachment behind for a request that was refused. The
  parts are now read first, then the "not both" rule is applied, so a refused request stores nothing.

The guide's two invalid-input answers are honoured exactly — `101 Provide either multipart file(s) or
?url=..., not both.` and `101 Missing input: provide multipart file(s) or url` — which needed the route
to take the raw request rather than a `Multipart` extractor: axum rejects a bodyless request with a 415
before a handler runs, and the documented answer is a business code, not a transport error.

**The composer control is not shipped.** I added a 📎 button to the chat composer, and the first
insertion landed *inside* the Send button — the composer is one concatenated markup string, and a
nested `<button>` plus a duplicate id is worse than no button at all. A second attempt put the helpers
outside the page's `<script>`, and the review caught both. The control is removed again rather than
half-fixed; the endpoints below are complete, tested and reachable from any client, and the composer
needs its markup split into proper elements before a control belongs in it. (The knowledge-base page's
*Version history* card from 0.12.0 is unaffected: it was added to markup that already had element
boundaries.)

**2191 tests pass, 0 failures**, including the upload→download→preview round trip, the `ext` header
switch, both invalid-input cases, the internal-address refusal, and the canvas-scoped upload against
both an unknown and a real canvas. The parity ledger moved from 22 to **18** remaining gaps
(195 upstream routes, 330 of ours, 130 exact).

## [0.12.0] - 2026-10-03

### The workspace commit family: snapshots, diffs and version history

RAGFlow keeps a snapshot history over its file workspaces, and the API guide documents nine endpoints
for it — `POST/GET /commits`, `GET /commits/{id}`, `GET /commits/{id}/files`, `GET /commits/diff`,
`GET /changes`, `GET /commits/{id}/tree`, `GET /commits/{id}/files/{file_id}/content`, and
`GET /workspace-files/{file_id}/versions`. RayRAG answered **none** of them, and the parity audit could
not see them either: upstream registers them from a helper with an f-string prefix
(`f"{prefix}/commits"`), so the literal-path scan skipped the whole family. Both are fixed here.

* **The audit sees them now.** It expands the helper's prefixes from their call sites
  (`/datasets/<entity_id>`, `/workspaces/<entity_id>`), which moved the ledger from 181 to **195
  upstream routes** — fourteen routes that had been invisible — and from 311 to 326 of RayRAG's.
* **All nine endpoints are implemented** in `src/api/workspaces.rs`, with real storage
  (`workspace_commits.json`, restored at boot like every other store): a commit records the files it
  touched and the resulting state, the list is paginated, the diff compares two snapshots, `changes`
  reports what is not committed yet, the tree and the file content are served from the snapshot, and a
  file's version history spans the snapshots that touched it.
* **Two scopes, as upstream has.** A `/workspaces/{workspace_id}` scope is a folder in the file store,
  and its state is the files below it. A `/datasets/{dataset_id}` scope is a knowledge base — upstream
  writes artifact commits with `folder_id = kb_id` — so its state is the dataset's documents. The two
  never mix: a commit records which kind of scope it belongs to.
* **The interface can reach it.** The knowledge-base page gained a *Version history* card: a button
  lists the snapshots, and *Create snapshot* asks for a message, reads the actual uncommitted changes
  from `GET /changes`, and commits exactly those — it does not invent a change when nothing has moved.

Measured against the running container (`/api/v1/workspaces/root/commits` and
`/api/v1/datasets/{id}/commits`), with failures named per the guide:

```
POST /api/v1/workspaces/root/commits     {"message":"first","files":[{"file_id":"f1","file_name":"a.txt","operation":"add","content":"hello"}]}
  -> {"code":0,"data":{"id":…,"file_count":1,"parent_id":null,"tree_state":"{…f1…}"}}
GET  /api/v1/workspaces/root/commits      -> {"total":1,"page":1,"page_size":5,"commits":[…]}
GET  …/commits/{id}                       -> the commit plus its files[]
GET  …/commits/{id}/files                 -> [{"operation":"add","old_hash":null,"new_hash":…}]
GET  …/commits/{id}/tree                  -> the tree as it looked then
GET  …/commits/{id}/files/f1/content       -> {"content":"hello"}
GET  …/commits/diff?from=…&to=…           -> [{"operation":"modify","old_hash":…,"new_hash":…}]
GET  …/changes                            -> what is not committed yet
GET  /api/v1/workspace-files/f1/versions  -> two entries, newest first
POST …/commits {}                          -> 400 {"code":101,"message":"required argument are missing: message"}
GET  /api/v1/workspaces/nope/commits       -> 404 {"code":102,"message":"Could not resolve folder 'nope'"}
GET  …/commits/does-not-exist              -> 404 {"code":102,"message":"Commit not found in workspace"}
```

Two smaller things surfaced while verifying it, both fixed: the dataset page's document list had no
horizontal scroll container, so a wide table pushed the page sideways on a phone width (the
knowledge-base page's own list has had one for a while), and cleaning up test data in a deployed
instance means clearing **both** the JSON state file and its PostgreSQL snapshot — the mirror restores a
deleted file at boot, which is exactly what it is for.

**2187 tests pass, 0 failures**, including an end-to-end test of the family (create → list → get →
files → tree → content → second commit → diff → versions → the three failure paths) and a
dataset-scope test whose state comes from the document store.

## [0.11.9] - 2026-10-03

### Sidebars now follow the window (reported by the user)

Reported: some sidebars do not grow, shrink or wrap when the window changes — the admin console's in
particular. Measured before the fix, at widths 1600 → 390: the admin sidebar was **288px at every width
down to 768px** (it had no breakpoint at all), the user-setting sidebar was **303px**, and the dataset
and template sidebars were **220px**. They now use `clamp()` columns, so they grow and shrink with the
viewport with sane bounds:

| page | before | after |
|---|---|---|
| `/admin/*`, `/admin/users` | 288px fixed | 288px @1600 → 198px @900, stacked full width ≤768px |
| `/user-setting` | 303px fixed | 336px @1600 → 200px @900, 64px icon rail ≤768px |
| `/dataset/*` | 220px fixed | 272px @1600 → 187px @900, stacked ≤768px |
| `/agent-templates` | 220px fixed | 256px @1600 → 173px @900, stacked ≤600px |

Sidebar labels wrap (`overflow-wrap:anywhere`) instead of running past the edge, and a new probe
(`sidebar-responsive-cdp.mjs`) walks five pages through nine widths and fails if a
sidebar stops following the window, if a label overflows without ellipsis, or if the page scrolls
sideways. It is part of `verify-all.sh`, as is the page audit below.

### Every page, at a desktop and a phone width, audited for what a user would notice

`responsive-cdp.mjs` walks the app's known routes plus deep links to real records —
34 pages — at 1280px and 480px, recording per page: HTTP status, **page errors**, horizontal overflow,
elements wider than the viewport outside a scroll container, and whether the page uses card containers.
It found three things a passing test suite had not:

- **`/search` threw `TypeError: (list || []).forEach is not a function`.** It still assumed the documents
  endpoint answers a bare array; since 0.11.x it answers the documented `{docs, total_datasets}`. It now
  accepts both.
- **`/kbs/{id}` threw `ReferenceError: dsTagLoad is not defined`.** The knowledge-base page renders the
  tag-set tab and calls `dsTagLoad()`, but the tag script lived only in the dataset page's inline
  JavaScript. It is now one shared block (`DS_TAG_SCRIPT`) that both pages push.
- **`/dataset/configuration/{id}` overflowed a 480px viewport by 148px.** The configuration tabs are
  label/field tables with a 220px label column; a `max-width:600px` rule now stacks those cells. The
  dataset tab's two tables got the scroll wrapper their siblings already had, which also fixes a
  document-name-dependent overflow.

The search page also had a silently failing call: it asked for metadata keys without `dataset_ids`, so
the endpoint answered `102 Lack of dataset_ids in query parameters` and the code ignored the business
code, leaving the key list empty.

### One flaky test, fixed at its cause

`default_backend_follows_the_compiled_features` set `RAYRAG_VECTOR_BACKEND` in the process environment,
which is global: a parallel test opening a store at that moment read the other test's value. Backend
resolution is now a named function (`requested_backend`) that both call sites share and the test asserts
directly, with no environment mutation.

Three mistakes of my own were caught on the way, each by a check that exists for exactly this: two edits
that landed **inside** a Rust string literal (injecting `b.push_str(...)` as literal page text — caught by
the rendered-page and browser checks) and a stray `}` in the stylesheet (caught by the parser test).

**2183 tests pass**, 0 failures; `verify-all.sh` passes nine suites, two of them new.

## [0.11.9] - 2026-10-03

### Every page, at a desktop and a phone width, audited for what a user would notice

A new audit (`responsive-cdp.mjs`) walks the app's known routes plus deep links to
real records — 34 pages — at 1280px and 480px, and records per page: HTTP status, **page errors**,
horizontal overflow, elements wider than the viewport outside a scroll container, and whether the page
uses card containers at all. It found three things a passing test suite had not:

- **`/search` threw `TypeError: (list || []).forEach is not a function`.** It still assumed the
  documents endpoint answers a bare array; since 0.11.x it answers the documented
  `{docs, total_datasets}`, so the page broke on a shape change that its own tests could not see. It now
  accepts both.
- **`/kbs/{id}` threw `ReferenceError: dsTagLoad is not defined`.** The knowledge-base page renders the
  tag-set tab and calls `dsTagLoad()`, but the tag script lived only in the dataset page's inline
  JavaScript — a copy of the markup without its script. The script is now one shared block
  (`DS_TAG_SCRIPT`) that both pages push.
- **`/dataset/configuration/{id}` overflowed a 480px viewport by 148px.** The configuration tabs are
  label/field tables with a 220px label column, which with their inputs is wider than a narrow card. A
  `@media (max-width:600px)` rule now stacks those cells, so every field stays reachable without a
  horizontal scrollbar.

The search page also had a silently failing call: it asked for metadata keys without `dataset_ids`, so
the endpoint answered `102 Lack of dataset_ids in query parameters` and the code ignored the business
code, leaving the key list empty. It now scopes the request to the selected datasets and honours the code.

Two mistakes of my own were caught on the way, both by tests that exist for exactly this: an edit that
landed **inside** a Rust string literal (injecting `b.push_str(...)` as literal page text, which the
rendered-page assertion rejected), and a stray `}` in the stylesheet (rejected by the stylesheet parser
test).

**2181 tests pass**, 0 failures.

## [0.11.8] - 2026-10-03

### `POST /api/v1/chat/completions` is the guide's endpoint again

The guide points every newcomer at `POST /api/v1/chat/completions`: three modes, and a stream of
`{"code":0,"message":"","data":{"answer":…,"reference":…,"final":false,"session_id":…}}` frames ending
with `{"code":0,"message":"","data":true}`. This project had **that path bound to its OpenAI-shaped
handler**, so a client written against the guide received `chat.completion.chunk` objects and a `[DONE]`
sentinel instead. The OpenAI handler keeps serving OpenAI clients — it already has its own path,
`/v1/chat/completions`.

The endpoint now implements the guide's three modes:

| body | behaviour |
|---|---|
| no `chat_id`, no `session_id` | answer with the tenant's default chat model; `session_id` is `""` |
| `chat_id` only | use that assistant's datasets and model, and **create a session** for it |
| `chat_id` + `session_id` (or `session_id` alone) | continue that session |

plus the documented body (`messages` or `question`, `stream` defaulting to **true**, `llm_id`,
`pass_all_history_messages`, `legacy`) and failure (`102 Please input your question.`). `legacy: true`
returns the whole answer so far and keeps thinking tags literal, as v0.23.0 did; otherwise the tags are
stripped and signalled with `start_to_think` / `end_to_think`. A failed generation sends an error frame
instead of a stream that ends in `data: true` over an empty answer.

### Reference chunks now carry document identity

Building the guide's `reference` object exposed a real gap: `ChunkReference` held no document id or name,
so `doc_aggs` could not be aggregated at all. It now carries both — filled at the retrieval path from the
chunk's own `doc_id` and the document store — and `reference_payload` builds `chunks` plus `doc_aggs` from
them. The other 24 construction sites were seeded with `None`, so nothing else changes behaviour.

### A failed generation is no longer dressed as an answer

`generate_chat_answer` turned a failed model call into the *answer text* — `"Error: error sending request
for url (…)"` — and the response around it still carried business code `0`. A client that checks the code,
which the guide tells clients to do, read that as a successful answer. The failure now propagates: the
stream sends an error frame and the non-stream responses carry a business code (`105 Connection error`
here) with the transport message.

**2181 tests pass**, 0 failures, including a regression for the endpoint's documented contract: a missing or
blank question is `102 Please input your question.`, an unknown `chat_id` and an unknown `session_id` are
both refused, a generation that cannot happen reports why rather than answering with an empty success, and
the stream still ends with the documented terminal frame.

## [0.11.7] - 2026-10-03

### The last two id conventions on the chat paths, separated

0.11.5 and 0.11.6 aligned the chat-assistant collection and its single-item verbs. Two paths still carried
RayRAG's session id where the guide documents an assistant id:

- **`POST /api/v1/chats/{chat_id}/completions`** now takes the **assistant**. It resolves the assistant
  (business code `102` when unknown), then follows the guide's rule: with `session_id` it continues that
  session (`102 Session not found!` when it does not exist), and with **no** `session_id` it **creates one
  bound to that assistant**, seeded from the assistant's own `dataset_ids` and `llm_id`.
- The session-scoped verbs moved to the endpoint that names them: `POST /api/v1/conversations/{id}/completions`
  and `DELETE`/`POST /api/v1/conversations/{id}/messages/{msg_id}` (+ `/regenerate`, `/feedback`).

The request body also accepts the guide's names: `session_id` (this project's `conversation_id`) and
`chat_id` (this project's `app_id`), so a client written against the guide does not have to translate.

The Chat page was moved onto the new endpoints and re-checked in the browser (0 page errors).

### Recorded, not quietly left

`POST /api/v1/chat/completions` — the endpoint the guide points newcomers at, with its three modes and its
`{code, message, data:{answer, reference}}` stream — is currently bound to the OpenAI-shaped handler in this
project. That is a **shape** difference, not a naming one, and it needs its own change rather than a rename,
so this release records it instead of half-touching it.

**2176 tests pass**, 0 failures, including a regression that proves an unknown assistant and an unknown
session are both refused with `102`, and that a request without a session creates one bound to the assistant.

## [0.11.6] - 2026-10-03

### The single-item chat verbs, one level below the listing

0.11.5 made `GET/POST/DELETE /api/v1/chats` mean chat assistants. The verbs on
`/api/v1/chats/{id}` were still answering RayRAG's **sessions**, so an id taken from the new listing
answered `404 Chat not found` at its own detail endpoint — the mirror image of the bug the previous
release fixed.

`GET`, `PUT`, `PATCH` and `DELETE` on `/api/v1/chats/{chat_id}` now address assistants, and the guide's
two update verbs are honoured as the guide describes them:

| verb | behaviour |
|---|---|
| `GET` | the assistant, with `dataset_ids`, `kb_names`, `llm_id`, `llm_setting`, `prompt_config`; a missing or unowned one answers business code `102` |
| `PATCH` | partial update; nested `llm_setting` / `prompt_config` are deep-merged and everything the body omits is preserved |
| `PUT` | full overwrite; fields the body omits reset to their defaults |
| `DELETE` | `{"code": 0, "data": true}`, as documented |

Session detail operations moved to `/api/v1/conversations/{id}`, which now serves `GET` (fetch),
`PATCH` (rename) and `DELETE` alongside the create and bulk-delete on the collection.

**2175 tests pass**, 0 failures. The assistant regression now walks the whole documented lifecycle:
create, list, fetch one, patch one field and check the rest survived, replace it and check the omitted
fields reset, refuse an unknown id with `102`, delete one, then bulk-delete the rest.

## [0.11.5] - 2026-10-03

### `/api/v1/chats` now means what the guide means: chat assistants

The guide defines `/api/v1/chats` as the **chat assistant** collection, with sessions under
`/api/v1/chats/{chat_id}/sessions`. RayRAG had its **session** records on that path for every verb, so the
two collections shared one URL. The consequences were not cosmetic:

- a guide client that created an assistant with `POST /api/v1/chats` never saw it in `GET /api/v1/chats`;
- the ids that listing returned 404'd at their own `/sessions` endpoint (fixed for the symptom in 0.11.3,
  whose note said the collection semantics were still not aligned — this is that).

All three verbs on the path now speak about assistants, and sessions moved to the endpoint that names them:

| endpoint | meaning |
|---|---|
| `GET /api/v1/chats` | assistants, `data: {chats, total}`, with `dataset_ids`, `kb_names`, `llm_id`, `llm_setting`, `prompt_config`, `rerank_id`, `top_k`, plus the documented `id` / `name` / `keywords` / `page` / `page_size` / `desc` filters |
| `POST /api/v1/chats` | create an assistant; empty name answers business code `101`, a duplicate answers `102 duplicated chat name` |
| `DELETE /api/v1/chats` | delete assistants by `ids`, `delete_all`, or the legacy `chat_id` |
| `POST` / `DELETE /api/v1/conversations` | create and bulk-delete **sessions** (what the old chats path did) |

A partial delete is reported — `102` with the per-id `errors` — rather than answering success over ids that
were not removed. The Chat page creates its sessions through the new endpoint, so the UI keeps working.

Still not aligned, and now written down where it can be found: the path parameter of
`/api/v1/chats/{id}/completions` is a session id in RayRAG and an assistant id in the guide, and RayRAG's own
assistant-management path `/api/v1/chatapps*` remains alongside the documented one.

**2175 tests pass**, 0 failures, including a regression that walks the documented lifecycle: create an
assistant with `dataset_ids`, see `kb_names` and `create_time` in the reply, be refused a duplicate with
`102` and an empty name with `101`, find it in `{chats, total}`, resolve its id at `/sessions`, filter it by
`keywords`, delete it, and see the listing empty afterwards.

## [0.11.4] - 2026-10-03

### The two remaining documented shapes, migrated in one piece

The previous release recorded two list endpoints that answered a different shape from the API guide and
explained they needed migrating together with their consumers. That migration is done:

- **`GET /api/v1/files`** now answers `data: {total, files, parent_folder}` as documented, and — the part
  that was missing entirely — it **honours its documented query**: `parent_id` (with `404 Folder not
  found!` for an unknown folder), `keywords`, `page`, `page_size` and `desc`. Before this it ignored all of
  them and always returned the root folder as a bare array.
- **`GET /api/v1/chats/{chat_id}/sessions`** now answers `data` as the list itself, as documented, instead
  of wrapping it in `{items, total, page, page_size}`. Pagination still applies to the list.

The Files page reads whichever shape arrives, so it keeps working either way, and the tests that consumed
the old wrapper were updated to the documented one rather than the other way round.

The audit from 0.11.2 also picked up two more names the guide uses: a file record's `type` (the project
stored `file_type`, with the same values) and a session's `chat_id` plus `create_time`/`update_time`. They
are now filled in the same additive way; a session with no assistant keeps `chat_id` absent rather than
inventing one.

**2175 tests pass**, 0 failures, including a new regression that checks the files list's shape, its
`parent_id` navigation, its keyword filter, that `total` counts every match rather than the page, and that
an unknown folder is a 404.

## [0.11.3] - 2026-10-03

### Every chat id the listing returns now works at its sessions endpoint

`GET /api/v1/chats` answers with this project's conversation records, while
`GET /api/v1/chats/{chat_id}/sessions` looked only in the chat-app store. The two id spaces did not
meet, so a client that listed chats and then asked for a session got `404 Chat not found` for **every
entry it had just been given**. The sessions handler now falls back to the conversation store, so an id
from the listing resolves to that conversation.

This closes the visible symptom. The deeper difference is recorded, not papered over: the guide's
`/api/v1/chats` lists chat *assistants*, while RayRAG returns chat *sessions* — the collection semantics
are still not aligned and the internal worklist says so.

### Also in this release

- The remaining documented-shape deviations are written down with the reason they are not fixed in one
  go: `GET /api/v1/files` answers a bare array where the guide documents
  `{total, files, parent_folder}`, and the sessions list answers `{items, total, page, page_size}` where
  the guide documents an array. Both are consumed as-is by the pages and by tests, so they need the
  migration done in one piece rather than half-way.

The audit from 0.11.2 stands at 22 remaining gaps out of the guide's documented fields, all of them
concepts this project keeps elsewhere (`llm_id`, `llm_setting`, `prompt_config`) or values it does not
track (`token_num`, `thumbnail`).

**2174 tests pass**, 0 failures.

## [0.11.2] - 2026-10-03

### The API guide's field names, added systematically

`document_count` in the last release was found by accident. To stop relying on accidents, there is now
内部辅助脚本: it holds the field names the guide documents for each object — dataset,
document, chunk, chat, session, file, agent, memory, search app, retrieval — fetches real responses from a
running instance, and reports every documented key that is absent. It reports rather than fixes, so the
mapping stays a human decision, and an object with no rows is listed as "no data" instead of a false alarm.

Its first run found that the differences were systematic, not isolated:

| the guide says | RayRAG stored |
|---|---|
| `embedding_model` | `embd_id` |
| `dataset_ids` | `kb_ids` |
| `created_by`, `tenant_id` | `owner_id` |
| `create_time`, `update_time` | `created_at`, `updated_at` |
| `location` | `storage_name` |
| `title` (agents) | `name` |
| `parser_config` (object) | `parser_config` (JSON **string**) |

`api::compat` now adds the documented name beside the original one on every list and detail endpoint, and
parses `parser_config` into the object the guide documents (lifting `chunk_method` out as the sibling field
the guide has). **Everything is additive** — the original keys stay, so no existing caller changes
behaviour.

Two rules kept this honest:

- a documented name is only filled from a field that already holds that fact; a real value is never
  overwritten, and nothing is invented to make a payload look complete;
- where the project genuinely has no value, the field is `null` — which is what the guide's own examples
  show for those fields — rather than a plausible-looking placeholder.

The audit is a report, not a gate, so it is deliberately **not** wired into `verify-all` yet: the remaining
gaps it lists (chat `llm_id`/`llm_setting`/`prompt_config` and friends, which this project keeps in its own
model tables) need a decision each, not a rename.

**2173 tests pass**, 0 failures, including six new compat regressions.
## [0.11.1] - 2026-10-03

### The Files page is grouped by dataset

The top level of **Files** now shows one folder per dataset, with its name and file count, instead of a
single flat table. Opening a folder lists that dataset's files and offers a way back; the workspace file
browser below keeps working exactly as before. The folder grid reflows with the screen
(`repeat(auto-fill, minmax(190px, 1fr))`), so the same page works on a narrow laptop or a wide monitor.

### `document_count`, the name the guide uses

The dataset payload called the document count `doc_count`, while the API guide documents
`document_count`. A client written against the guide would read `null`. Both names are now emitted —
the documented one added, the original kept so nothing existing breaks — and the Files page's folder
cards use whichever arrives. The assertion that caught this was first written into the wrong test (a
navigation-tree response, which the guide does not document); it now lives in the dataset-list test.

### One unexplained test failure, explained

The previous release recorded that `oauth_callback_flow_matches_upstream` had failed once in the full
parallel run while passing on its own, and said the cause was unknown. It was found: the test starts a
local fake OAuth provider that read the request with **a single `read()`** and then asserted on its
contents. Under parallel load a request can arrive in two segments, so the assertions saw half a request,
the thread panicked, and the client side failed with an unrelated-looking error. The fake provider now
reads the headers, then the body those headers announce, so it always sees the whole request. Two
consecutive full runs pass with 2167 tests.

## [0.11.0] - 2026-10-03

### The three console pages that answered 404

`/admin/whitelist`, `/admin/roles` and `/admin/monitoring` existed in the console's route map but were
never registered, so every visit returned 404. They now exist, they sit in the console sidebar beside
the other sections, and each one is backed by real data rather than a placeholder:

- **`/admin/whitelist`** is a real registration whitelist, matching RAGFlow's `enable_whitelist`
  variable. It is **off by default**, so an existing deployment behaves exactly as before. Turn it on
  and only the listed addresses may register; an entry may be a full address or an `@domain` so a whole
  company can be admitted at once. The list is persisted (`RAYRAG_WHITELIST_FILE`) and survives a restart.
  `POST /api/v1/users` consults it and refuses a stranger with business code `103` and a message naming
  the page to fix it — not a silent rejection.
- **`/admin/roles`** lists every account with its role and grants or revokes administrator through the
  existing admin API.
- **`/admin/monitoring`** reports host metrics, the service table and the running version.

Measured on the running container: all three pages load with **0 console errors**, the roles table shows
4 accounts, monitoring shows 5 services and 9 metrics, and the whitelist round trip is verified —
a non-listed address is refused while a listed one registers, after which the deployment is restored
to its previous state.

### The chat page's own script was broken

`/chat` served `innerHTML='<option value=''>…'`: the HTML attributes were written with single quotes
inside a single-quoted JavaScript string, so the string ended early and the whole script block failed to
parse. Every console error on that page came from this one line, and the features it defines — new chat,
create chat, delete, rename, share — were silently undefined. The attributes are double-quoted now.

To stop this class of bug from reaching a release again, a new check parses **every inline script on
every page** with the same parser the browser uses. It found this defect, and it runs in `verify-all`
from now on.

### Also fixed

- The **built-in pipeline explainer** was emitted after the configuration tab's closing tags, so it was
  painted on *every* tab — including Artifacts and the file list. It now lives inside the tab that
  chooses a chunk method, and the file list no longer carries it at all.
- `GET /api/v1/datasets/{id}/documents` now answers the **documented** shape
  `data: {"docs": [...], "total_datasets": n}`. It used to return a bare array, so a client written
  against the API guide read `data.docs` and got null. The pages accept both shapes.
- `/datasets/new` and `/knowledge` were 404s; they now redirect to the datasets page.

**2167 tests pass**, 0 failures.
## [0.10.3] - 2026-10-02

- **补上 API 说明书里最后一条可落地的废弃别名：`POST /api/v1/file/convert` → `POST /api/v1/files/link-to-datasets`**。两条路径指向**同一个处理函数**，因此不可能各自漂移。
- **顺带查出一个"说谎"的接口**：`POST /api/v1/file2document` 此前**只返回 `code:0` 加一句 "N file(s) queued for conversion"，实际上什么都没做**——调用方看到成功，却永远等不到文档。这正是本项目明令避免的静默成功。现在它走真实的转换逻辑。
- **`link-to-datasets` 的真实行为**：
  - 目录 ID 按说明书**展开为目录内的文件**（说明书原文："If a folder ID is provided, all files within that folder will be converted"），目录本身不会变成一个文档；
  - 目标数据集先全部校验再动手，避免"一半成功"让调用方无从判断；
  - 文件不存在或不属于当前用户 → **HTTP 404 + 业务码 102 `File not found!`**；数据集不可管理 → **404 + 102 `Can't find this dataset!`**；
  - **一个都没转换成功时不再返回成功**，而是 **HTTP 500 + 业务码 103**，并把每个失败原因列在消息里；部分成功时成功列表照常返回、失败原因放在 `errors` 里。
- **测试**：新增 3 条回归（空选择不算成功、目录展开为其子文件且目录本身不成为文档、不存在/他人文件必须被报告而不是跳过）。全量 **2162 passed / 0 failed**。

## [0.10.2] - 2026-10-02

- **按 API 说明书落实 OpenAI 兼容接口的 `extra_body`**（说明书第三处落地）。说明书在 `POST /api/v1/openai/{chat_id}/chat/completions` 下定义三个键，此前 RayRAG **一个都没解析**：`extra_body` 里的内容被 `#[serde(flatten)]` 静默吞掉，调用方写了等于没写。
  - **`reference`**：`false` 时不再返回 `references` —— 之前无论调用方要不要引用都硬塞一个 `references` 数组。
  - **`reference_metadata.include` + `fields`**：为每个引用附带 `document_metadata`；`fields` 是白名单，**省略=全部键，空数组=一个都不要**（说明书把这两种极端都写明了），已按此实现并测试。为此 `ChunkReference` 新增可选字段 `document_metadata`，只在被请求时填充。
  - **`metadata_condition`**：作为**真正的检索过滤**（而不是只过滤引用列表）。过滤发生在上下文组装之前——只过滤引用会让被排除的文档继续影响答案，却对调用方不可见，那是最坏的一种"看似生效"。过滤条数记入日志（`metadata_condition filtered retrieval results`），不静默丢弃。运算符复用 `metadata_filter` 的既有实现，语义与检索接口一致。
- **测试**：新增 2 条契约回归（白名单两端行为、`metadata_condition` 确实编译成真实过滤条件）。全量 **2159 passed / 0 failed**。
- **未验证部分如实说明**：答案生成链路仍受本机 chat 端点不可达（`LLM_API_BASE=http://172.29.0.1:8088` 无监听）限制，因此"带 `extra_body` 的完整答案+引用"未取得端到端实机证据；本轮实机验证的是**参数已被真正解析**（错误类型校验返回 422，说明字段进入了契约而非被忽略）。

## [0.10.1] - 2026-10-02

- **按 API 说明书的「Error codes」章节落实业务错误码契约**（说明书第二处落地）。说明书明确：**HTTP 状态码与响应体里的业务码是两件事，必须分别检查**。此前 RayRAG 的 `api_error` 把业务码直接等于 HTTP 码（400/403/404…），而另一些地方（我最近写的端点）又返回 **HTTP 200 + body code 102**——**两种都是错的**：只看状态码的调用方会把失败当成功，只看 body code 的调用方拿不到 101/102/108/109 这些上游定义的码。
- **新增契约构造器**：`api_error_code(status, code, message)` 与 `invalid_argument`(101) / `invalid_or_missing`(102) / `no_permission`(108) / `unauthenticated`(109) / `conflict`(409)，并按说明书给出 `code` 常量表（0/101/102/103/105/106/108/109）。
- **已迁移**：产物（artifacts）与导航（navigation）两条路径的"数据缺失/无效"响应从 **HTTP 200 + code 102** 改为**HTTP 400 + code 102**（说明书里 400 就该是 400）。对应回归断言也改为同时校验两个信号。
- **未迁移的如实标注**：历史端点仍在用 `api_error`（业务码=HTTP 码），已在内部登记表里写明"逐个迁移中，未迁移的不会被伪装成已迁移"。
- **测试**：新增一条契约回归（五个构造器必须成对返回说明书里的 HTTP 状态与业务码，且错误必须自带说明文字）。全量 **2157 passed / 0 failed**。

## [0.10.0] - 2026-10-02

- **按 API 说明书的「Deprecated API Aliases」表补齐 v0.24.0 兼容路径**（用户要求，用说明书优化 RayRAG）。说明书列出 20 条旧路径→新路径的映射，RayRAG 原本只覆盖 2 条，本版把**目标端点已存在**的那些全部接上：`/api/v1/file/upload`、`/file/create`、`/file/list`、`/file/root_folder`、`/file/parent_folder`、`/file/all_parent_folder`、`/file/rm`、`/file/rename`、`/file/mv`、`/file/get/{file_id}`、`/api/v1/chats_openai/{id}/chat/completions`、`/v1/system/healthz`（上游在 `/api` 之外也提供这一个）。
- **每条别名都指向与替代端点*同一个*处理函数**，不是第二份实现——否则两边迟早走样。两条"查询参数形态"的旧路径（`file/parent_folder?file_id=`、`file/all_parent_folder?file_id=`）新增了薄适配器：**缺少 `file_id` 时返回 `code 102` 并点名缺哪个参数**，而不是一个莫名其妙的 404。
- **两条刻意不接**：`POST /api/v1/file/convert` 与 `POST /v1/document/upload_info` 的替代端点（`/api/v1/files/link-to-datasets`、`/api/v1/documents/upload`）在 RayRAG **尚未实现**——把旧路径指到一个不存在的功能上只会制造新的"看起来能用"的假象，故留在未实现清单里。
- **测试**：新增一条覆盖全部别名的回归（含 `/v1/system/healthz` 无需鉴权、两个查询参数形态缺参必须 `code 102` 且点名参数、其余旧路径必须"已注册"而不是 404）。全量 **2156 passed / 0 failed**。

## [0.9.10] - 2026-10-02

- **首次登录账号改为 `admin` / `admin` / `admin`（用户名、昵称、密码），不再生成随机密码、也不再因"密码不够 12 位"而拒绝启动**（用户要求）。此前 bootstrap 要求必须设置 `RAYRAG_ADMIN_PASSWORD` 且长度 ≥12，否则**直接启动失败**；现在未配置时就用 `admin`，`RAYRAG_ADMIN_EMAIL` 默认也从 `admin@rayrag.local` 改为 **`admin`**，并新增 `RAYRAG_ADMIN_NICKNAME`（默认 `admin`）三个都可被环境变量覆盖。
- **仍然"不静默"**：启动时会用一行 WARN 说明"首个账号用的是默认密码 admin，请在登录后修改（Profile → password），或在首次启动前设置 RAYRAG_ADMIN_PASSWORD"；`RAYRAG_ADMIN_PASSWORD` 被显式设置为 **空字符串**时**报错拒绝**（避免出现空密码账号）。
- **验证（端到端，非单测）**：新增的隔离自检 内部辅助脚本 现在**不预设任何管理员变量**，跑的正是全新部署的默认路径——实跑结果：`fresh login: OK (default admin/admin account)`；同一次运行里 env 写回也仍然通过。
- 文档同步：README 的登录说明与 `.env.example` 的「First login」段落都改为 admin/admin/admin，并说明可覆盖。

## [0.9.9] - 2026-10-02

- **修复两处"页面 JS 不生效"的真 bug（都是浏览器实测抓到的）**：
  1. `dsParseTypeChange()` 被我改写时**丢掉语句分隔符并多了一个括号**，整段是语法错误，导致选择 `Pipeline` 时毫无反应（控制台 `Unexpected token 'var'`、`dsParseTypeChange is not defined`）——现在函数恢复正确，**选择 Pipeline 会显示 Pipeline 选择框与提示**；
  2. 滑块联动函数 `syncSliderToNumber` / `syncNumberToSlider` 只加进了**两个脚本段中的一个**，而渲染滑块的页面用的是另一段 → 控制台 `... is not defined`、拖动无效。现在**两个脚本段都定义**。
- 教训（本阶段第三次同源）：**Rust 字符串里内嵌 JS，编译通过 ≠ 浏览器能跑**。因此每次界面改动都必须在浏览器里点一遍并**读控制台错误**——本轮正是靠 `page.on('pageerror')` 抓到这两处。
- 测试：全量 **2155 passed / 0 failed**。

## [0.9.8] - 2026-10-02

- **修复滑块与数值框的联动**（浏览器实测发现的真 bug）：上一版生成的属性是 `oninput='syncSliderToNumber(this,\'cfgThrV\')'`——**单引号包裹的 HTML 属性里 `\'` 不是转义**，属性在 `\'` 处就结束了，浏览器拿到的是一个语法错误的处理函数，于是**拖动滑块数字不变、输入数字滑块也不动**（页面控制台报 `SyntaxError: Unexpected end of input`）。改为属性内使用双引号（`oninput='syncSliderToNumber(this,"cfgThrV")'`）后双向联动生效。
- **这条是"部署后实测"抓到的**：编译、单测、fmt 全绿，但浏览器里两个方向都不工作——再次印证本阶段定下的规矩：**界面改动必须在浏览器里点一遍**。
- 测试：全量 **2155 passed / 0 failed**。

## [0.9.7] - 2026-10-02

- **创建数据集时选择自定义 Pipeline 不再是一个空壳**（用户报告）：选择 `Pipeline` 后会**显示真正的 Pipeline 选择框**（此前对话框里根本没有这个字段，选了也只有一个空标题），并在框下**静态写明下一步该做什么**——"自定义 Pipeline 需先在 **Agents** 页面创建，创建后再回到这里选择"，并附上直达 Agents 的链接；未选择 Pipeline 就提交时**给出明确原因**（把错误显示在对话框的错误位），而不是静默失败或创建一个无法解析的数据集。
- 实现上刻意**不引入动态注入的 JS**：提示文案由服务端直接渲染（本阶段已因"把 JS 字符串渲染成页面文本"踩过一次坑），切换显示只用一行属性赋值。
- 测试：全量 **2155 passed / 0 failed**。

## [0.9.6] - 2026-10-02

- **数据集 Configuration 页新增固定的「Built-in pipeline introduction / 内置流水线说明」栏目**（用户要求）。栏目逐个列出上游的 12 种内置分块方法（General/naive、Book、Email、Laws、Manual、One、Paper、Picture、Presentation、Q&A、Table、Tag），**说明文字取自 RAGFlow 自身的 `knowledgeConfiguration` 文案**（`web/src/locales/en.ts`，由 `category-panel.tsx` 的 `DescriptionKeyMap` 映射），包含各自支持的**文件格式**与分块行为要点，以可折叠条目呈现。
- 该栏目同时出现在两个渲染该配置表的页面（数据集配置页与数据集设置页），不会因页面不同而内容不一致。
- 测试：全量 **2155 passed / 0 failed**。

## [0.9.5] - 2026-10-02

**用户报告的两处设置界面问题：**

- **user-setting 左侧栏顺序按用户要求排列**：**Model providers → Data sources → Chat channels → MCP → Team → Profile → API**（原来 Data sources 在最前、Model providers 排第三）。对应测试断言也改为跟随这个顺序。
- **容器日志不再"固定写死某个模型"**：启动日志现在明确写出**值来自哪里**（`source=EMBED_MODEL` / `source=LLM_MODEL`），并且**没有了内置的默认提供商与默认模型**——原先 `LLM_API_BASE` 缺省会指向第三方 `api.minimaxi.com`、`LLM_MODEL` 缺省为 `MiniMax-M3`，意味着**全新安装可能在用户从未选择的情况下把请求发给第三方**。现在必须由用户指定端点与模型；未配置时打印中性的引导（"尚未配置：请在 .env 设置…或完成首次引导的模型步骤"），**不出现任何具体模型名**。API key 仍可留空（本地 llama.cpp / Ollama 这类服务通常不需要）。
- 测试：全量 **2155 passed / 0 failed**。

## [0.9.4] - 2026-10-02

**用户报告的四个界面缺陷，全部修复：**

- **Languages 一栏出现"乱码"**：那一行把**构造语言按钮的 JavaScript 源码当成文本输出**到了单元格里（所以既显示源码、又根本没有按钮可选）。改为**服务端直接渲染** 11 个语言按钮（`data-lang` + `toggleLang`），源码不再泄漏到页面。
- **概览页错位**：导航树卡片被插进了**统计行内部**，把 `Total Docs` 挤进表格里（用户截图正是这个现象）。现在导航树有**自己的卡片**，统计行保持完整。
- **相似度阈值 / 向量权重的滑块与数值框不联动**：数值显示原本是个只读 `<span>`，现在换成真正的 **number 输入框**，并与滑块**双向联动**（拖滑块→数字变；输入数字→滑块动，且自动夹在 min/max 之间）；`loadConfig` 回填也改为写 `.value`（原来写 `textContent`，对输入框无效）。
- **多个下拉框文字白底白字看不见**：下拉弹出层由**操作系统绘制、不跟随页面主题**，而深色主题的文字色接近纯白 → 弹出层里白底白字。现在为 `select option/optgroup` **显式指定深色字 + 白底**，并为禁用项指定灰色。
- 测试：全量 **2155 passed / 0 failed**。

## [0.9.3] - 2026-10-02

- **忘记密码流程落地**（上游 `user_api.py`）：`POST /api/v1/auth/password/forgot/otp`、`…/otp/verify`、`POST /api/v1/auth/password/reset`。
  - **一次性验证码**：10 分钟有效、**单次使用**（重置后立即失效）、**最多 5 次错误尝试**（超出即作废），验证码替换旧码；错误分别提示"没申请过/已过期/尝试过多/还剩几次"，**每一种失败都有自己的说法**。
  - **不枚举账号**：请求验证码的响应与地址是否存在无关。
  - **没有邮件服务器时不撒谎**：RayRAG 目前不发送邮件，因此验证码**写入服务端日志**，响应里 `delivered_by` 明确写 `server_log`，文案是"请向管理员索取"——而不是"请查收邮箱"。配置 `EMAIL_SMTP_HOST` 后 `delivered_by` 变为 `email`。
- **账面**：同路径应答 **114 → 117**，未实现 **27 → 24**，不匹配 0。
- **测试**：新增 4 条存储层测试（单次使用、错误尝试倒数并作废、未知地址如实报告、新码替换旧码）。全量 **2155 passed / 0 failed**。

## [0.9.2] - 2026-10-02

- **Agent 页面新增 Webhook 面板**：上一版把 webhook 接口做好了，但页面上没有入口（本阶段同类问题的第 5 次）。Agent 详情页现在有：URL 输入框、Enabled 勾选、**Save webhook / Send test delivery / Show delivery log / Remove** 四个按钮，以及状态行与投递日志区。
  - 打开页面**自动读取**已注册的 webhook（没有则显示"还没有注册"的原话）；
  - **Test** 之后自动刷新日志，成功与否用颜色区分（失败为红），并逐条显示 `时间 - 事件 - OK/FAILED - HTTP 状态或传输错误 - 耗时`；
  - 保存/删除都把接口返回的**原话**显示出来，不吞错误。
- **测试**：全量 **2151 passed / 0 failed**（kb_ui 123 条全过）。

## [0.9.1] - 2026-10-02

- **Agent Webhook 子系统落地**（上游 `agent_api.py` 的三条路径）：`GET/POST/PUT/PATCH/DELETE /api/v1/agents/{id}/webhook`、`GET .../webhook/logs`、`POST .../webhook/test`。
  - 每个 Agent 一个 webhook，**落盘为 JSON 并原子写入**（`RAYRAG_WEBHOOKS_FILE`，默认在静态目录旁的 `webhooks.json`）；更新时**保留 id**（是同一个 webhook，不是新建）。
  - **测试投递是真的发出去**：真的 HTTP POST（带 token 时用 Bearer），记录状态码或传输错误与耗时，返回文案分别是 `Delivered: HTTP 200 OK` / `Delivery failed: …`；未注册 webhook 或已停用时**明确拒绝并说明**要先做什么。
  - 投递历史**每个 Agent 各看各的**，整体**上限 50 条**（避免文件无限增长），空历史时提示用 test 发一条。
- **账面**：同路径应答 **111 → 114**，未实现 **30 → 27**，不匹配 0。
- **一处刻意不做**：上游在 `/webhook/test` 上还注册了 `DELETE/GET/HEAD/PATCH/PUT`。该动作**有副作用**（会真的发出投递），因此 RayRAG 只接受 `POST`，其余按 HTTP 语义返回 405——**不为了对齐数字把有副作用的动作挂到 `GET`/`HEAD` 上**；原因写入方法缺口备注。
- **测试**：新增 3 条存储层测试（更新保留 id 且重开文件仍在、历史按 Agent 过滤且整体封顶、删除如实报告是否删到）。全量 **2151 passed / 0 failed**。

## [0.9.0] - 2026-10-02

- **补上上一版刻意留下的那一条**：`DELETE /api/v1/datasets/{id}/artifacts/structure` —— 上游只清**结构类**产物，而 RayRAG 原有的 `DELETE /datasets/{id}/artifacts` 会清空**全部**产物页。上一版因为"没摸清产物行的形状"而**拒绝交付半成品**；本版按行上的 `page_type_kwd` 过滤（与列表过滤同一个字段、与 `clear_artifacts` 同一种条件写法），并返回删除条数与说明（0 条时明确写 "No structure artifacts were stored for this dataset."）。
- **账面**：**方法缺口 1 → 0**，同路径应答 **111**，未实现 **30**，不匹配 0。
- **测试**：新增一条（空数据集清空必须说明原因）。全量 **2148 passed / 0 failed**。

## [0.8.9] - 2026-10-02

- **产物读取面对齐上游的三条字面路径**：`GET /api/v1/datasets/{id}/artifacts/{structure,graph,alteration}`。三条路径都复用同一份列表逻辑（把路径段折进查询），**两个形状不可能各走各的**；未知产物类型会**列出支持的类型**，而不是回一个空列表让人误以为"这个数据集没有"。
- **账面**：同路径应答 **108 → 111**，未实现 **33 → 30**，不匹配 0。
- **一处**没有交付**（记在账本里，不藏）**：上游对 `/artifacts/structure` 还有 `DELETE`（只清结构类产物）。RayRAG 现有的 `DELETE /datasets/{id}/artifacts` 会清空**全部**产物页；要按 `page_type` 过滤删除，需要先摸清产物行在存储里的真实形状——本轮查证时发现 `search_existing_pages` 的返回并不是我假设的 `Vec`，**在预算内没能改对，就没有把半成品塞进去**，而是在方法缺口里写明原因与下一步。**一个"看起来实现了但删错东西"的接口比没有更糟。**
- **测试**：全量 **2147 passed / 0 failed**；登记表 **353 条路由**。

## [0.8.8] - 2026-10-02

- **索引维护接口对齐上游**：新增 `GET / POST / DELETE /api/v1/datasets/{id}/index` 与 `DELETE /api/v1/datasets/{id}/{index_type}`。RayRAG 本来就有这两项能力（内部修复巡检用的 `vector_mirror`），但**上游路径下取不到**。现在：
  - `GET` 报告该数据集索引现状（类型、是否启用、分块数）；
  - `POST` 重建向量索引，返回**分块数与是否真的修复了**；数据集没有已解析分块时返回 `chunks: 0` 并说明"nothing to index"，而不是一个看不出问题的成功；
  - `DELETE` 支持 `vector`；**其他索引类型被明确拒绝并列出支持项**（`Supported: vector`），不会回一个"已删除"却没删任何东西的成功。
- **账面变化**：同路径应答 **106 → 108**，未实现 **35 → 33**，**方法缺口 0**，不匹配 0。
- **测试**：新增一条覆盖 GET/POST/DELETE 三态（含"空数据集必须说明原因"与"不支持的索引类型必须列支持项"）。全量 **2147 passed / 0 failed**。

## [0.8.7] - 2026-10-02

- **模型设置页新增「Endpoint health / Check model endpoints」按钮**：调用 `GET /api/v1/models/endpoints/check`，用表格列出**每个能力**（chat / embedding / rerank / asr / vision）的环境变量、端点、是否可达、失败详情；不可达时在末尾用红字汇总 `Unreachable: …`，并附上那句关键提醒（嵌入可达 ≠ 对话可达）。无配置或服务未启动时，**页面上直接看得出来**，而不是去猜为什么生成结果为空。
- 这补上了用户要求里的硬规则：「每个功能都要有交互按钮」——上一版刚加的接口如果没有入口，用户依然看不到它。
- **测试**：全量 **2146 passed / 0 failed**（kb_ui 123 条全过）。

## [0.8.6] - 2026-10-02

- **新增 `GET /api/v1/models/endpoints/check`：一次调用逐个探测所有模型端点**。起因是一次真实故障——产物抽取一直空转，因为 `LLM_API_BASE` 指向的服务**根本没在运行**，而唯一的查法是登录宿主机手工 `curl`。现在按能力（chat / embedding / rerank / asr / vision）分别报告：环境变量名、base 地址、是否可达、失败详情；并返回 `unreachable` 列表。
- **响应里带一句关键提醒**：*"嵌入端点可达 ≠ 对话端点可达，而且嵌入服务会对对话请求回 200 但内容乱码——请逐个能力分别确认。"* 这正是本轮实测踩到的坑（把 `LLM_API_BASE` 指到嵌入端口会伪装成"模型没抽到实体"）。
- **测试**：全量 **2145 passed / 0 failed**；登记表随之更新。

## [0.8.5] - 2026-10-02

- **README 新增一节「产物/导航生成为空怎么办」**，把这一轮追出来的完整判断链写给用户：①端点是否**真的在运行**（`curl $LLM_API_BASE/v1/models` 返回 `000` 就是没服务）；②容器内**不能写 `127.0.0.1`**，要用 docker 网关地址；③**绝不要把 `LLM_API_BASE` 指向嵌入端口**——嵌入服务对 `/v1/chat/completions` 也会回 200，但返回乱码，于是故障会伪装成"模型没抽到实体"。
- 背景：本机实测 `EMBED_API_BASE` 的 `:8888` 从容器内可达（200），而 `LLM_API_BASE` 的 `:8088` **宿主上没有任何进程监听**——即产物抽取失败的真因是**对话模型服务没有运行**，与 RayRAG 代码无关。上一版已让接口把这种情况报告为 `failed: n` 并写明原因，本版把"怎么排查"补进文档。
- 无代码改动（文档），版本号随之递增以保持 tag/Release 一致。

## [0.8.4] - 2026-10-02

- **更正一处"把失败说成数据结果"的错误**：上一版让产物编译返回抽取计数（`extracts`），于是 `0` 变得可见——但我随后把它**解读成"这个数据集没有实体"**。上传一个**含人名/机构名**的文档后仍是 `extracts: 0`，**这个解读当场被推翻**；容器日志里写着真因：`WARN wiki_map: batch extraction failed`。也就是说：**MAP 阶段的模型抽取失败了，而接口把它报告成"模型没找到内容"**。
- **两处修复**：
  1. **引擎不再吞错误**：`wiki_gen_json` 原先用 `.ok()?` 丢掉对话调用的错误，导致"模型调用失败"和"模型返回了散文"两种情况在日志里长得一样。现在**分开记录**——调用失败记 error，JSON 解析失败记 error + 回答开头 160 字符，便于直接看出模型到底返回了什么。
  2. **端点区分"失败"与"空"**：`_meta.requested > 0` 却没有抽到东西 → 判定为**抽取失败**并计数（响应新增 `failed` 字段），理由文案改为"抽取没有产出：模型调用或其 JSON 回答失败（见服务端日志）"；只有确实没有分块正文时才说"没有内容可抽"。
- **教训**：让一个数字可见 ≠ 理解这个数字。报告计数为 0 时，必须问清"这是业务事实，还是上游失败被吞掉的结果"。
- **测试**：全量 **2145 passed / 0 failed**。

## [0.8.3] - 2026-10-02

- **产物面板的 Generate 按钮真的会编译了**：此前它只显示"产物生成尚未实现"；现在调用 `POST /datasets/{id}/artifacts/compile`，并把**两阶段的结果如实说出来**——`Mapped N of M document(s), extracts E, pages created P, modified Q`；有文档抽不出东西时补一句有多少个；**当 `extracts = 0` 且确有文档时**，明确写出"模型没有从该数据集里抽到实体/概念/断言/关系/主题，因此没有产出页面——这是数据结果，不是失败"。失败则红字显示原因。
- 与上一版配套：**接口负责把"为什么是 0"讲清楚，按钮负责把它显示给用户**，而不是留一句"排队中"。
- **测试**：全量 **2145 passed / 0 failed**（kb_ui 123 条全过）。

## [0.8.1] - 2026-10-02

- **新增 `POST /datasets/{id}/artifacts/compile`——产物（Wiki）生成终于能跑了**：这是**两阶段**作业，缺任何一步都会"静默空转"——MAP 阶段逐文档让模型抽取并**把 extracts 写进存储**，COMPILE 阶段再**从存储读回 extracts** 合成页面。本实现按此顺序执行，并返回 `{mapped, total, compile}` 让两次都可见；数据集没有已解析分块时返回 `generated: 0` 加说明，而不是看不出问题的成功；缺对话模型时返回 `code 102` 并指明。
- **为什么现在才做**：它依赖三个此前缺失的零件——`LlmHarnessChat`（0.7.5，对话适配器）、`OffThreadDocStore`（0.7.7，解决"同步存储被 async 引擎调用即 panic"）、以及把两阶段结构读清楚（上一轮）。**零件不齐就动手，只会得到一个 panic 或一个什么都不生成的端点。**
- **测试**：全量 **2145 passed / 0 failed**；登记表 **348 条路由**。

## [0.8.0] - 2026-10-02

- **导航树在界面上可读了**：上一版加了"Build navigation tree"按钮，但建出来的树**没有地方看**——等于做了一半。数据集「概览」面板现在会渲染导航树表格（Cluster / Type / Documents / 操作），每行可展开查看该簇的直接子节点（调用 `GET /datasets/{id}/navigation/{name}/children`）；构建完成后**自动刷新**表格，无需手动reload。没有树时给出明确空态（"No navigation tree yet. Use Build navigation tree above."）而不是一片空白。
- **测试**：全量 **2145 passed / 0 failed**（kb_ui 123 条全过；页面启动脚本断言随之更新）。
- 过程中一次自我纠错：往里塞 JS 时用了双引号，直接**破坏了 Rust 字符串字面量导致编译失败**；改为只用单引号的写法并用 `addEventListener` 取代内联 `onclick`，重来后通过。

## [0.7.9] - 2026-10-02

- **接口有了自家按钮**：数据集「概览」面板新增 **Build navigation tree** 按钮，调用上一版落地的 `POST /datasets/{id}/navigation`，并把结果表明出来——`Generated N of M document(s)`，若有文档没有已解析分块，追加 `skipped K without parsed chunks`；失败时红字显示具体原因，并在 `data.message` 存在时一并展示（例如"该数据集没有分块"）。**不留一句"完成"就了事**。
- 这补齐了用户要求里的一条硬规则：**每个功能在界面上都要有对应的交互按钮**——否则接口做得再好，用户也点不到。经验上这正是"功能看起来不存在"的来源（本阶段已遇到三次：概览标签、知识图谱入口、以及本次的导航生成）。
- **测试**：全量 **2145 passed / 0 failed**（kb_ui 123 条全过）。

## [0.7.8] - 2026-10-02

- **`POST /datasets/{id}/navigation` 重新落地——方法缺口清零**：这次跑在上一版新增的 `OffThreadDocStore` 之上（引擎从 async 函数内部调用同步存储时不再 panic）。语义与施工单一致：所有权检查 → **缺嵌入模型或缺对话模型分别 `code 102` 并指明缺哪一个** → `documents` 作用域（缺省=数据集全部文档）→ 按 `kb_id` 取分块、**按文档聚合且每文档限 4000 字符**（用完即丢）→ 顺序调用引擎逐文档生成 → 返回 `{generated, skipped, total}`。
- **不静默**：指定了却没有已解析分块的文档进入 `skipped` 并带原因；数据集整体无分块时返回 `generated: 0` 加说明，而不是一个看不出问题的成功。
- **数字**：同名 **106**、未实现 **35**、**方法缺口 1 → 0**、不匹配 0——接口层面已无"路径在、方法缺"的条目。
- **测试**：全量 **2145 passed / 0 failed**（含缺模型的 102 回归）。

## [0.7.7] - 2026-10-02

- **修掉"同步存储 vs 异步引擎"这个反复出现的坑**：新增 `OffThreadDocStore` —— 一个把**每一次**存储调用都放到**独立 OS 线程**执行的 `DocStore` 实现（`std::thread::scope` + join）。原因是：存储后端从头到尾是同步的（PostgreSQL 驱动自带 `block_on`），在 tokio worker 上调用会 panic（`Cannot start a runtime from within a runtime`）；处理器可以用 `spawn_blocking` 规避，但**移植过来的引擎是在 async 函数内部直接调存储**，外层包不住。OS 线程没有 tokio 上下文，驱动在那儿就合法——**引擎代码一行都不用改**。
- **这是上一轮导航生成端点 panic 的根因修复**。它同时解锁 wiki/Artifacts 生成与结构编译——那两条路径的引擎结构与导航完全一致。
- **测试**：新增两条（在 tokio 任务里通过该适配器写入并读回、在 tokio 任务里搜索并取回文档 id）——**这两条正是在"直接用同步存储"时会 panic 的调用形态**。全量 **2145 passed / 0 failed**。

## [0.7.6] - 2026-10-02

- **补上嵌入侧的生产适配器 `EmbedderBridge`**：导航生成、wiki/Artifacts 生成、结构编译都依赖 `structure_compile::EmbeddingBackend`，而它和 `HarnessChat` 一样**只有测试替身**（`MockEmbed`、`FailingEmbed`）——上一轮写端点时正是被这处不匹配挡住的。`EmbedderBridge` 把运行期已持有的 `AppState.embedder`（`Arc<dyn Embedder>`）接到该 trait 上。
- **错误语义（与前一个适配器一致）**：provider 失败**原样返回 `Err(String)`，绝不返回空向量**——调用方若收到 `Ok(vec![])` 会照样写出"没有嵌入的行"并报告成功。另外校验**返回向量数必须等于输入文本数**，数量不符也报错（这正是"静默产出错位数据"的典型来源）。
- **测试**：新增三条 —— 批量映射（一次调用返回等量向量）、空输入短路（不调用 provider）、**provider 失败必须是错误而不是空向量**。全量 **2142 passed / 0 failed**。
- **下一步**：两个适配器都已就位，下一轮直接落 `POST /datasets/{id}/navigation` 端点主体（骨架与四条验收在 内部对标计划）。

## [0.7.5] - 2026-10-02

- **补上 `HarnessChat` 的生产适配器（长期缺件）**：`crate::harness::HarnessChat` 是导航生成、wiki/Artifacts 生成、结构编译共同依赖的对话模型接口，但此前**只有测试用的 `FakeChat`**——这正是这些"引擎已移植、单测完备"的功能一直无法上线的原因：不是缺端点，而是**缺一个把真实对话客户端接到 trait 上的适配器**。新增 `LlmHarnessChat`：把 OpenAI 形状的 `history`（`{"role","content"}`）映射为 `ChatMessage`，空 system 不发送，实现 `max_length`（取模型 `max_tokens`，下限 1024）与 `model_name`。
- **错误语义**：空输入返回 `Err("harness chat called with no system prompt and no history")`，**绝不返回空 `Ok`**——否则生成器会"成功"却什么都没产生。尚未转发的 `gen_conf`（temperature/top_p 等）以 debug 日志**明确说明使用模型默认值**，而不是静默忽略。
- **测试**：新增两条（空输入必须报错且错误信息说明缺什么；`max_length`/`model_name` 来自客户端）。全量 **2139 passed / 0 failed**。
- **下一步**：适配器已具备，下一轮接 `POST /datasets/{id}/navigation` 本体（施工单在内部对标计划里）。

## [0.7.4] - 2026-10-02

- **知识图谱面板开始消费 `GET /datasets/{id}/graph`**：此前面板只在点击 Build Graph 之后显示一次构建摘要（且只有一行统计 + 环形图）。现在打开 `/dataset/knowledge-graph/{id}` 时会**直接读出已构建的图谱**并渲染成两张表 —— 实体（Entity / Type / Degree / Documents）与关系（Source / Target / Weight），只在有边时显示关系表；没有实体时给出**解释性**文案（"Build Graph 从已索引分块抽取实体，文本很少的文档自然图谱很小"），而不是一片空白。
- **未构建与读取失败分开**：`built:false` 时面板顶部明确写"尚未为该数据集构建图谱"；接口读取失败则红字显示具体错误——**两种情况不会混为一谈**。
- **测试**：全量 **2137 passed / 0 failed**（`dataset_detail_paths_follow_upstream_sidebar_routes` 的启动脚本断言同步更新）。

## [0.7.3] - 2026-10-02

- **`GET /datasets/{id}/graph` 落地（上游 `get_knowledge_graph`）**：把该数据集**已经构建**的图谱按上游形状返回 `{graph: {nodes, edges}, mind_map: {}}`——节点带 id/标签/实体类型/描述/来源文档/度数，边带权重；按度数排序（上游按 pagerank）取前 N（默认 256，可 `max_nodes` 调整），**并且只保留两端都入选的边**，不会出现悬空连线。同时给 `EntityGraph` 补上了公开的序列化出口——此前它的字段全是 `#[allow(dead_code)]`，图谱构建得出来却没有对外读取的路径。
- **未构建时说清楚**：没有图谱的数据集返回空图并带 `built: false`，而不是 404、也不是假装有一张图——UI 因此能如实显示"尚未构建"。
- 数字：同名 **105→106**、未实现 **36→35**、方法缺口 1（生成类，已登记）。
- **测试**：新增回归（未构建 → 200 + `built:false` + 空 nodes + 存在 mind_map）。全量 **2137 passed / 0 failed**。

## [0.7.2] - 2026-10-02

- **修掉数据集页面的 JS 报错（并确认知识图谱路由可用）**：切标签的函数在最后两行**没有做空值保护**——`document.getElementById('tab'+n).style.display=...` 与 `document.getElementById('nav'+n).classList.add('active')`。而**知识图谱按上游设计不是侧栏项，`nav4` 根本不存在**，于是在 `/dataset/knowledge-graph/{id}` 上必然抛 `TypeError: Cannot read properties of null (reading 'classList')`，并且**中断后面的激活逻辑**（tab5 Artifacts 因此可能不被正确收起）。两处都已加空值保护。
- **知识图谱路由实测可用**：在 `/dataset/knowledge-graph/{kb}` 上，2/4/7 秒三次取样均读到 `#tab4` 处于 `display:block`，面板内容为 Knowledge Graph 说明与 **Build Graph** 按钮。上一轮"点击后 2 秒未见面板"是**取样时机/跳转方式造成的假象**，不是功能问题——本轮据实纠正。
- **测试**：全量 **2136 passed / 0 failed**（kb_ui 123 条全过）。

## [0.7.1] - 2026-10-02

- **知识图谱终于有入口了**：知识图谱面板（含 Build Graph）一直存在，按上游 v0.27.2 它**不是数据集侧栏项**而是独立路由 `/knowledge-graph/:id`——但 RayRAG 里**没有任何地方链接到它**，所以用户从界面上根本进不去，表现为"数据集没有知识图谱功能"。现在**数据集概览面板顶部**多了一个 `Knowledge graph` 按钮，指向 `/dataset/knowledge-graph/{id}`（该路由本就映射到图谱面板），与上游"从数据集进入图谱"的行为一致。
- 这与上一轮改标签是同一类问题的两个面：**功能在、入口不在**，不会报错也不会 404，只能靠对着上游逐项核对才能发现。
- **测试**：全量 **2136 passed / 0 failed**（kb_ui 123 条全过；插入时特意保持 div 配对，`dataset_panels_stay_siblings_of_the_overview_tab` 通过）。

## [0.7.0] - 2026-10-02

- **数据集侧栏第 3 项由「日志」改为「概览」（对齐上游语义）**：读上游 v0.27.2 源码确认，`pages/dataset/sidebar/index.tsx` 里这一项的**标签是 `knowledgeDetails.overview`（概览），只是复用了 Logs 图标**，路由同样是 `/dataset/logs`。RayRAG 之前直接用了图标的名字显示成「日志」，用户因此感觉"少了一个功能"。现在英文显示 **Overview**、中文显示 **概览**，深链接与面板内容不变。
- 这类"图标名当功能名"的偏差不会报错、不会 404，只会让人以为功能不存在——**发现它只能靠对着上游源码逐项核对**，本轮就是这么查出来的（含上游文件与行号，已记入内部计划文档）。
- **测试**：全量 **2136 passed / 0 failed**（kb_ui 123 条全过，侧栏标签断言同步更新）。

## [0.6.9] - 2026-10-02

- **修用户报障：数据集「配置」页排版溢出**。浏览器实测取证：配置面板内 **142 个元素越过面板右边界**（典型是权限表的一行从 x=609 拉到 **x=1818**，宽 1209px）——表格既没有宽度约束，也没有横向滚动容器，于是把**整个页面**撑出横向滚动。现在把**标签面板本身**作为滚动容器（`overflow-x:auto` + `min-width:0` + 表格 `max-width:100%`）：内容全部可见、不被裁切也不被压扁，超宽表格在面板内部滚动，页面不再横向漂移。
- **关于「数据集没有知识图谱功能」**：知识图谱**面板已实现**（`#tab4`，含 Build Graph），按 RAGFlow v0.27.2 的侧栏顺序（文件列表 / 检索测试 / 日志 / 配置 / Artifacts）它**本来就不是侧栏入口**，只保留深链接——这是既有代码里写明的对标决定。因此本轮**没有**把它塞回侧栏（那会破坏对标），而是把"面板存在但入口不在侧栏"这件事记入待办：下一轮对着 ragflow 源码确认 v0.27.2 中 KG 的正确入口位置（配置页内的开关 / 数据集卡片菜单 / 其他），再按上游方式接上。
- **测试**：全量 **2136 passed / 0 failed**（kb_ui 123 条全过），`cargo fmt --check` 干净。

## [0.6.8] - 2026-10-02

- **禁用的按钮必须说明原因**：浏览器实测发现数据集产物页的 Generate 按钮在**没有分配分块方法（pipeline）**时是灰的，而旁边**一个字都没有**——用户只会看到一个点不动的按钮，不知道缺什么。现在会直接写明：`This dataset has no chunk method assigned yet, so artifact generation cannot start.`（没有已解析分块时则写 `has no parsed chunks yet`），并指出**当前可用**的操作（读取列表 / 单页 / 主题 / 清空产物）。
- 这与上一轮同一条原则：**界面不要说含糊话**——上一轮改的是"排队中"的假承诺，这一轮补的是"按钮灰着但不解释"的死胡同。
- **测试**：全量 **2136 passed / 0 failed**（kb_ui 123 条全过）。

## [0.6.7] - 2026-10-02

- **UI 不再说"排队中"这种含糊话**：数据集产物（Wiki）面板的 Generate 按钮原先只显示 "Wiki generation is queued for a later slice."，视图切换器则提示另一种视图"稍后切片会有"。现在两者都**如实反映后端状态**：点击 Generate 会真的去读 `GET /datasets/{id}/artifacts`，然后告诉你这个数据集当前有几页编译产物；没有产物时明确说明「产物生成尚未实现，因此这个按钮不能编译新页面；列表 / 单页读取 / 编辑 / 主题 / 清空这些接口是可用的」。
- 这样做而不是继续留一句占位话，是因为占位话会让用户以为**功能即将可用**，而真实状态是**读取面已上线、生成面未实现**——据实说明才方便用户判断能不能用。
- **测试**：全量 **2136 passed / 0 failed**（kb_ui 123 条全过）。

## [0.6.6] - 2026-10-02

- **把「9380 也是 RAGFlow 的端口」写在用户第一次会看的地方**：默认端口沿用 9380（与 RAGFlow 一致），但这台机器上同时跑 RAGFlow 就必然撞端口——这正是此前纯 Linux 启动阻塞的同一个坑，只是发生在**默认值**上而不是命令行参数上。README 快速开始与 `.env.example` 现在都直接说明：改 `.env` 里的一行（如 `RAYRAG_PORT=9390`）再 `docker compose up -d`；启动横幅会打印**实际生效**的地址，`docker compose port rayrag 8080` 可随时查询。
- 全量 `cargo test --locked --lib` 通过；环境变量守卫六条全过（模板键不重复、每个文档化的键都被代码读取）。

## [0.6.5] - 2026-10-02

- **模板里最后两处「活的占位凭据」改为注释态**：`RAYRAG_POSTGRES_PASSWORD=change-me` 与 `RAYRAG_TEST_RERANKER_KEY=mxbai-rerank-key`。照抄 `.env.example` 的部署原本会共用同一个数据库口令；现在留空，`docker compose` 会因缺少该变量**直接报错退出**（`${VAR:?set ...}`），而不是静默用弱口令建库。这是上一轮修掉管理员口令占位值后的同类收尾：**模板里不应带任何可直接使用的凭据**。
- 全量 `cargo test --locked --lib` **2136 passed / 0 failed**；环境变量守卫（每个被文档化的键都要被代码读取、模板键不得重复）六条全过。

## [0.6.4] - 2026-10-02

- **纯 Linux 文档路径端到端复验通过**：按 README 步骤用 `ZVEC_LIB_DIR` 重新构建 release 后，`readelf -d` 显示 RUNPATH 已烘焙进二进制；**不带任何命令行参数、不设 `LD_LIBRARY_PATH`** 运行 `rayrag serve`，端口取自 `.env` 里的 `RAYRAG_PORT`（9398），`/api/v1/version` 返回 **200**。「照文档做就能跑」这件事本身现在有证据。
- **修掉一个静默弱默认**：`.env.example` 里的 `RAYRAG_ADMIN_PASSWORD=replace-with-a-strong-admin-password` 是**未注释的占位值**——照抄模板的部署会把管理员口令设成这个人人可猜的字符串。现在该行已注释掉（留空则走首次运行的引导界面设置），并且**服务启动时若发现口令仍是占位值会输出 WARN**，而不是默默带着弱口令运行。
- README 非 Docker 章节补充：端口优先级（**`--port` > `.env` 的 `RAYRAG_PORT` > 9380**）、zvec 共享库找不到时的症状与两种处理方式（重新导出 `ZVEC_LIB_DIR` 重建，或运行时设 `LD_LIBRARY_PATH`），并把示例 zvec 版本校准为 0.7.2。
- **测试**：全量 **2136 passed / 0 failed**。

## [0.6.3] - 2026-10-02

- **修掉纯 Linux 启动阻塞**：`rayrag serve` 的 `--port` 硬默认 9380（RAGFlow 自己的端口），**完全不读 `RAYRAG_PORT`**。在已经跑着 RAGFlow 的机器上，用 `.env` 里写好端口再运行，仍然会撞端口并以 `Address already in use` 退出——这正是上一轮纯 Linux 实测卡住的地方。现在命令行遵循统一优先级：**`--port` 参数 > env 文件里的 `RAYRAG_PORT` > 内置默认 9380**（`--static-dir` 同理接受 `RAYRAG_STATIC_DIR`）。
- **实测证据（非 Docker，直接跑二进制）**：`RAYRAG_PORT=9398` 写进 env 文件、**不带任何命令行参数**运行 `rayrag serve`，日志为 `RayRAG v0.6.3 … on http://0.0.0.0:9398`，`/api/v1/version` 返回 **200**，并成功连上现有 PostgreSQL（未连上时给出的仍是我此前加的可操作提示）。试跑进程与临时数据目录已清理，仓库无污染。
- **测试**：全量 **2136 passed / 0 failed**。

## [0.6.2] - 2026-10-02

- **补上 `PATCH /users/me/models`（上游 `set_tenant_info`）**，把上一轮登记的字段适配缺口真正关掉：上游的请求体是"每个能力一个选择器"（`llm_id` / `embd_id` / `rerank_id` / `asr_id` / `img2txt_id` / `tts_id` / `ocr_id`），而 RayRAG 内部是拆开的 `{provider, instance, name, model_type}`。适配层同时接受**两种写法**：RayRAG 的三段式 `model@instance@provider` 与上游的两段式 `model@provider`（此时实例名取 provider 名）。只应用请求里出现的字段，并在 `data.applied` 里回报实际生效的字段——**部分更新不会静默**。
- 格式错误的选择器返回带字段名的 `code 102`（例如 `'llm_id' must be '<model>@<instance>@<provider>' or '<model>@<provider>'`）；空请求体是合法的空操作并回报 `applied: []`。
- 数字：同名 **105**、未实现 **36**、方法缺口 1（仅剩生成类，已登记）。
- **测试**：新增 PATCH 回归（格式错误 → 带字段名的 102、未知 provider 有明确回应、空体 → `applied` 为空）。全量 **2136 passed / 0 failed**。

## [0.5.9] - 2026-10-02

- **移植逐主机 SSRF 守卫**：`common/ssrf_guard.py::assert_host_is_safe` 是上游给「接受原始 host/port 而非 URL」的连接准备的防线（画布数据库组件就走这条）。RayRAG 此前只有 URL 版本（`assert_url_is_safe`），本轮补齐：裸 IP、`[::1]` 形式的 IPv6 字面量、空主机、以及解析到非公网地址的域名全部拒绝，并补充了不依赖网络的守卫单测。
- **`POST /agents/test_db_connection`**：画布数据库组件保存前先验证连接。两条硬规则：① 主机必须通过上面那条 SSRF 守卫，否则返回带原因的 `code 102` 而不是跳过检查；② **PostgreSQL 做真实的连接与认证**——只探测端口会在"端口通、密码错"时报告成功，那正是画布最不该出现的静默假成功。本构建中没有客户端的引擎返回明确的 unsupported 文案，绝不猜测。返回的错误串会把密码替换为 `***`。
- 数字：同名 **103→104**、未实现 **38→37**、方法缺口 1。
- **测试**：新增守卫单测（私网/回环/链路本地/CGNAT/映射地址全部拒绝、公网字面量通过）+ 端到端回归（三类私网主机被拒且原因含 non-public、mysql 明确 unsupported、缺字段 400、端口为 0 拒绝）。全量 **2133 passed / 0 failed**。

## [0.5.8] - 2026-10-02

- **画布工具接口 2 条**：`GET /agents/prompts`（返回画布展示的四段提示词：任务分析 / 规划 / 反思 / 引用规范，**直接引用运行期常量**而不是复制一份，页面与运行时不可能不一致）、`GET /agents/download?id=`（按 id 下载已存文件，复用文件存储）。
- **修掉一处静默错误**：文件下载/删除在记录不存在时返回 **HTTP 200 + `{"code":404}` 正文**——下载客户端（浏览器、预览器、画布）先看状态码，于是"下载了一个空"看起来像成功。现在状态码与正文一致（404）。这类"状态码与正文互相矛盾"的问题此前只被当作样式问题，本轮由新回归测试发现。
- 数字：同名 **101→103**、未实现 **40→38**、方法缺口 1。
- **测试**：新增画布工具回归（四段提示词逐个与常量比对、缺 `id` 返回 400、未知文件不是 200）。全量 **2131 passed / 0 failed**。

## [0.5.7] - 2026-10-02

- **嵌入搜索机器人面 3 条落地**（上游 `bot_api.py`，beta token 客户端）：`GET /searchbots/detail`（返回应用记录，未知应用/非本人返回上游文案的 `code 102`）、`POST /searchbots/retrieval_test`（把嵌入端用的**单数 `kb_id`** 与 `size` 翻译成共享检索处理器认识的 `kb_ids`/`page_size`，一个检索实现三个入口）、`POST /searchbots/mindmap`（与聊天脑图共用同一实现，`kb_ids` 被接受而不报错）。数字：同名 **98→101**、未实现 **43→40**。
- **明确登记的剩余缺口**：`POST /searchbots/ask` 使用上游**自己的** SSE 帧格式（`code/message/data` 包封），与既有搜索补全的 `chat.completion` 帧不同，需要单独的流式处理器——已在工作清单写明，而不是用现有帧格式冒充。
- **测试**：新增嵌入端形态回归（未知应用 → 102、缺少范围 → 上游文案的 dataset 提示、单数 `kb_id` 被正确翻译、空脑图请求 → 400）。全量 **2130 passed / 0 failed**。

## [0.5.6] - 2026-10-02

- **数据集导航树接口 5 条落地**（复用上一轮接通的产物存储）：`GET /datasets/{id}/navigation`（顶层簇，支持 `keywords`/`top_k`）、`GET /datasets/{id}/navigation/{name}/children`（直接子节点：子簇与文档叶子）、`GET /datasets/{id}/navigation/search`（词法检索，返回 `mode` 与带 `score` 的节点）、`DELETE /datasets/{id}/navigation`（清空整棵树，返回删除条数）、`DELETE /datasets/{id}/navigation/{name}`（**连同子树**删除——子节点只按名字引用父节点，所以逐层收集后一次删除）。数字：同名 **94→98**、未实现 **47→43**。
- **跨后端一致的过滤口径（本轮踩坑）**：上游用 `*_kwd` 列表条件过滤（`type_kwd: ["nav_cluster"]`、`parent_kwd: ["root"]`），但内存后端按**值全等**比较，Postgres/ES 按列表成员比较——条件下推到存储会让同一份数据在不同后端返回不同结果。现在存储层只过滤 `compile_kwd`（每行都是普通字符串），`type_kwd`/`parent_kwd` 在取回后统一过滤（导航树规模有限）；同时把 `parent_kwd` 加入查询字段（上游靠存储过滤，所以它不在 `_NAV_SEARCH_FIELDS` 里）。
- **逐条差异说明**：`POST /datasets/{id}/navigation`（生成导航树）需要租户嵌入与 LLM 编译任务，已登记为方法级缺口并在 `--check` 中显式豁免；`search` 的 `chunk` / `navigation_tree` 模式同样需要嵌入模型，当前返回明确的 `code 102` 提示，而不是返回半个答案。
- **测试**：新增导航全链路回归（顶层簇 → 子节点 → 叶子（`type=doc`、`doc_id` 正确）→ 词法检索命中 → 未实现模式返回 102 → 删除节点连同子树 → 清空后归零）。全量 **2129 passed / 0 failed**。

## [0.5.5] - 2026-10-02

- **接上编译产物存储（解锁 17 条接口的前置条件）**：`DocStorePool` 此前在自身模块之外没有任何调用点——移植好的 wiki / 数据集导航 / 结构编译引擎（合计约 1.9 万行、单测完备）因此**读不到也写不进运行期数据**。现在它作为共享池挂在 `AppState` 上（有 `RAYRAG_POSTGRES_URL` 时走 PostgreSQL，否则内存后端），引擎与接口看到的是同一份数据。
- **数据集产物（Wiki）接口 5 条落地**：`HEAD /datasets/{id}/artifacts`（上游侧栏探测：有产物 200、无 404）、`GET /datasets/{id}/artifacts`（分页列表 + `page_type`/`keywords`/`topic` 过滤，按类型再按标题排序，返回上游的 `{total, items:[{slug,title,page_type}]}`）、`GET /datasets/{id}/artifacts/{page_type}/{slug}`（单页，含正文/摘要/实体/来源）、`PUT` 同路径（上游 `update_wiki_page`：按行 id 更新正文与标题，避免误改同目录其它页）、`GET /datasets/{id}/artifacts/topics`（主题树）、`DELETE /datasets/{id}/artifacts`（清空产物，返回删除条数）。
- **不假装做到的两处书面差异**：上游 PUT 会做 `[[slug]]` 链接渲染并写一条 `file_commit` 编辑记录；RayRAG 按原文保存且不写 commit（该服务未移植）——差异写在代码注释与接口工作清单里，而不是静默省略。
- **测试**：新增产物全链路回归（往共享存储写入两页 → HEAD 200 → 列表排序与过滤 → 单页读取 → 类型不符返回 404 → 主题列表 → PUT 编辑后重读 → DELETE 清空 → HEAD 变 404）。全量 **2128 passed / 0 failed**。

## [0.5.4] - 2026-10-02

- **接口对照收口，并给出剩余 50 条的真实性质**：同名应答 90 → **91**，未实现 51 → **50**，方法缺口 **0**。补齐 `/datasets/{id}/chunks` 的 `POST`/`DELETE`（上游把与 `/documents/parse`、`/documents/stop` **完全相同**的处理器挂在这条路径上，因此两条路径行为一致）。
- **重要架构发现（已写入工作清单）**：产物（Wiki）与数据集导航引擎（`knowlege_wiki*.rs`、`knowlege_dataset_nav.rs`，合计约 1.9 万行、有完整单测）**已移植但未接入运行期存储**——`DocStorePool` 在 `doc_store.rs` 之外没有任何调用点，运行中的服务用的是另一套检索存储。因此上游 `dataset_api` 的 artifacts/navigation/skills 共 17 条接口属于**功能级缺口**（先接线存储与生成任务，再暴露接口），而不是接线缺口。
- **工作清单逐条改写为「是什么功能缺什么」**：剩余 50 条全部写明性质——Agent 附件/Webhook/导出/提示词/数据库测试（9）、忘记密码流程（4）、连接器实例面（4）、搜索应用公开机器人面（4）、文档缩略图/图片/产物/上传（5）、TTS 与独立 ASR（2）、租户默认模型信息（1）、数据集产物与导航与索引（14）、其余各 1。**接线缺口已基本清零，剩下的都是新功能。**
- **测试**：数据集级回归新增 chunks 路径断言（未知文档 → 102）。全量 **2127 passed / 0 failed**。

## [0.5.3] - 2026-10-02

- **接口对标第四批（会话簇 + 功能级缺口登记）**：同名应答 88 → **90**，未实现 53 → **51**，方法缺口保持 **0**。
- **对话应用会话（上游 `chat_api` 会话簇，5 条）**：`POST/GET/DELETE /chats/{chat_id}/sessions`（新建、分页列表、按 `ids` 或 `delete_all` 删除）与 `GET/PATCH /chats/{chat_id}/sessions/{session_id}`（读取、重命名）。会话严格绑定其所属对话应用：跨应用读取返回 404，删除只作用于本应用的会话，未知应用一律 404。
- **功能级缺口如实分类**：工作清单里原先含义模糊的三条已改写为明确的功能缺口——`/plugin/tools`（上游 pluginlib 插件注册表未移植；RayRAG 的画布工具目录在 `/api/v1/components`）、`/workspace-files/{id}/versions`（文件版本历史 FileCommitService 未移植，`/api/v1/file/commits` 目前为空实现）、`/files/link-to-datasets`（file2document 为占位实现）。**把"接线缺口"和"功能缺口"分开记账，避免用接线的进度掩盖功能尚未实现。**
- **测试**：新增会话作用域回归（跨应用不可见/不可改、未知应用 404、分页包封、`delete_all` 清空后归零）。全量 **2127 passed / 0 failed**。

## [0.5.2] - 2026-10-02

- **方法级缺口清零**：接口对照的「路径已存在但上游方法缺失」由 **4 → 0**。至此 RayRAG 应答的每条路径都覆盖上游注册的全部方法。本轮补齐：
  - `PUT /providers/{p}/instances/{i}`（上游按名字更新实例：显示名/凭据/基址/模型列表；请求体省略 `model_info` 时保留原有模型，不清空实例）
  - `DELETE /providers/{p}/instances/{i}/models`（按 `model_name` 移除单个模型；模型不存在返回 404 并给出名字）
  - `POST /providers/{p}/instances/{i}/models/{m}`（上游 `chat_to_model`：直接与某个已配置模型对话，非流式返回 `{answer, reasoning_content}`，`stream:true` 时按上游帧格式 `data: [MESSAGE]<chunk>` + `data: [DONE]` 输出 SSE）
  - `DELETE /agents/{id}/sessions`（按 `ids` 选择或 `delete_all` 清空该智能体会话；只会删属于该智能体的会话，空体为幂等成功）
- **测试**：新增方法路由任务级回归，逐条断言这些方法返回的是处理器结果（404/400）而不是路由器的 405，并验证空体删除会话的成功包封。全量 **2126 passed / 0 failed**。

## [0.5.1] - 2026-10-02

- **接口对标第三批（文件树 + 会话批量删除 + 4 处方法缺口）**：数字由「同名 85 / 别名 38 / 未实现 56 / 方法缺 8」推进到「同名 **88** / 别名 **38** / 未实现 **53** / 方法缺 **4**」。
- **文件管理器对齐上游**：`POST /files`（multipart 上传 / JSON 建文件夹，一个入口两种面孔，上传复用同一处理器）、`DELETE /files`（按 `ids` 批量删除，返回上游的 `success_count` + `errors`）、`POST /files/move`（改父目录并可改名）、`GET /files/{id}/parent`、`GET /files/{id}/ancestors`（从文件自身向上走到根，含环路保护）。
- **会话删除**：`DELETE /chats` 支持上游的三种请求体——`{"ids":[…]}` 批量、`{"delete_all":true}` 全清、以及向后兼容的 `{"chat_id":"…"}` 单删——并返回上游的 `{}` / `{success_count, errors}` 包封；`PUT /chats/{id}` 与既有 PATCH 同处理器。
- **Dify 探测**：`GET /dify/retrieval` 补齐（上游 GET 探测 + POST 检索同路径），配置页用 GET 探测不会再收到 405。
- **测试**：新增文件树与会话批量删除任务级回归（建文件夹→建子文件→parent/ancestors 链→move 改名→批量删除部分成功；`delete_all=false` 空体返回 `{}`；Dify GET 探测）。全量 **2125 passed / 0 failed**。

## [0.5.0] - 2026-10-02

- **接口对标推进一批（9 条落地）**：接口对照数字由「同名 77 / 别名 37 / 未实现 65 / 方法缺 9」推进到「同名 **86** / 别名 **37** / 未实现 **56** / 方法缺 **8**」。本轮落地：数据集级批量解析 `POST /datasets/{id}/documents/parse`、批量停止 `POST /datasets/{id}/documents/stop`（均返回上游的 `{"success_count": n}`，未知文档按上游返回 `code 102` 并列出 id）、元数据汇总 `GET /datasets/{id}/metadata/summary`（上游 GET + `doc_ids` 查询，返回 `{"summary": {字段: {type, values}}}`）、元数据批量更新 `POST /datasets/{id}/metadata/update`、元数据扁平表 `GET /datasets/metadata/flattened`、标签聚合 `GET /datasets/tags/aggregation`。
- **检索三路径同源**：上游把 `retrieval_test` 同时挂在 `/retrieval`、`/datasets/search`、`/datasets/{id}/search` 上；RayRAG 现在同样三条路径共用同一处理器，路径带 id 的那条把 id 注入 `dataset_ids`，因此三种调用不会各自漂移。
- **工具自身的第三处缺陷（记录在案）**：接口对照工具的路由提取不接受 `.route(` 与路径字符串之间的注释行，导致 3 个**已经实现**的接口被误报为缺失（`/documents/ingest`、`/datasets/{id}/documents/batch-update-status`）。修正后重新测量并同步登记表，避免用错误数字排期。
- **请求体字段名（第三类兼容缺口）**：上游 `retrieval_test` 用 `dataset_ids` / `document_ids` 指定检索范围，RayRAG 原来只认 `kb_ids` / `doc_ids`——按上游字段名调用会被判为"没有可访问的知识库"。现在两套字段名都接受（路径带 id 的作用域检索也走同一处理器）。
- **测试**：新增数据集级接口的任务级回归（未知文档 → 102 且列出 id、空选择 → 400、缺 `dataset_ids` → 102、GET 汇总返回上游包封、作用域检索可达）。全量 **2124 passed / 0 failed**。

## [0.4.99] - 2026-10-02

- **HTTP 接口对照（新维度）**：新增可复现的接口对照工具，把「上游 181 个 REST 路由 RayRAG 是否应答」变成可复核的数字——同名应答 **77**、别名应答 **37**、形状不同但已应答 **2**、尚未实现 **65**（逐条登记说明）、方法级缺口 **9**。工具在 `--check` 下会把「别名目标被删掉」「已实现却仍挂在未实现清单」「出现未登记的新上游路由」判为失败，防止两个方向漂移。
- **上游拼写兼容**：RAGFlow 的 `compilation-templates*` / `compilation-template-groups` 用连字符，RayRAG 自用下划线命名——现在两种拼写同时注册，写死上游路径的第三方客户端不再 404（`/searches/{id}/completion` 单数拼写同样补上）。
- **补齐上游小接口**：`/system/ping`（未认证，返回字面量 `pong`）、`/language`（未认证的前端兼容值）、`/auth/logout`（吊销当前令牌）、`/system/stats`（与 `/stats` 同一处理器）、`/dify/retrieval/health`（Dify 外部知识库健康检查）、`/system/gaussdb/status` 与 `/system/oceanbase/status`（RayRAG 的文档引擎是 PostgreSQL，按上游「未配置」语义返回，而不是 404）。
- **方法级缺口（本轮新发现）**：接口对照工具升级为「路径 + 方法」双维度。上游在同一路径上注册的方法 RayRAG 若缺失，会返回 405——例如 `GET /users/me`（上游读个人资料，RayRAG 原先只有 PATCH）：实测 405，现已接上并返回与 `/user/info` 一致的资料（不含口令哈希）。共发现 **9 处**方法级缺口（会话删除/重命名、文件批量增删、模型实例更新与模型增删、数据集元数据汇总、Dify 探测等）已逐条登记。
- **回归测试**：新增路由对照测试，逐条断言上游拼写与 RayRAG 拼写返回相同状态码与相同载荷，断言个人资料接口不泄露口令哈希，并验证退出登录后原令牌立即失效。

## [0.4.98] - 2026-10-02

- **静默错误治理（第二批，收口）**：上一轮审计共 37 项，本轮把余下 20 项全部处理完毕——任务取消/重试/恢复路径的落库失败、文档删除与重命名过程中的补偿写入（影子删除、计数回滚、元数据恢复）、结构合并的元行写入与文档清单收缩、数据集导航行的增删改、wiki 回退 KNN 与主题向量、EPUB 条目与 XML 流、Agent 会话版本标题与运行记录、CSV 逐行解码失败、原生 zvec 检索失败回退内存打分、ASR 返回空文本、文档 redo 的分块计数重置。
- **不再「排了队但永远不跑」**：文档解析任务提交给执行器失败时，接口不再回答“已排队等待解析”，而是明确报错（此前文档会永远停在未开始状态，且没有任何提示）。
- **CSV 语义钉死**：新增回归测试固定“未闭合引号会吞掉文件其余部分”的标准 CSV 行为（这正是损坏文件看起来“表很短”的原因），并保证格式良好的行照常抽取。

## [0.4.97] - 2026-10-02

- **配置面收口**：`.env.example` 由 92 个键扩到 **176 个**，补齐此前只在代码里存在、用户无法发现的配置项（模型端点 `LLM_/EMBED_/RERANK_/FALLBACK_LLM_/TEI_`、`RAYRAG_ASR_*`、资源与流水线上限（并发/上传/HTTP/连接器/PDF 流/zvec 写缓冲）、网络与服务（CORS/对外主机端口/日志级别/索引前缀/ExeSQL 端口）、通知渠道（Webhook/飞书/会话默认/SMTP）、代码执行沙箱（本地与上游同名变量）、MCP 服务端（上游同名变量）、国内数据源与翻译凭据、OAuth 端点覆盖）。新增键一律以注释形式给出：直接复制模板不会改变现有运行行为，也不会覆盖按本机 CPU/内存/GPU 自动推导的运行参数。
- **配置面守卫测试**（`src/env_surface.rs`，6 项）：① 代码读取的每个环境变量必须在模板中登记，或带理由豁免；② 模板中的每个键必须真的被代码或容器工具读取（杜绝“设了却没用的键”）；③ 模板不得重复声明同名键（dotenv 中后者会静默覆盖前者）；④ 首次登录引导页写入的每个键都必须在模板中有据可查；⑤ 豁免项必须写明理由。任何一处漂移都会让 `cargo test` 失败。
- **静默错误治理（第一批）**：① 数据源 / 对话应用的 JSON 状态文件损坏或来自旧 schema 时，不再静默读成空存储（否则下一次保存会覆盖真实数据）：损坏文件被改名保留为 `<name>.json.corrupt-<时间戳>` 并在日志中给出原因，服务照常启动——既不销毁数据，也不静默。（这一改动在部署验证中当场抓到一次真实问题：容器内旧 schema 的 `chat_apps.json` 内容为 `[]`，旧路径会静默读空并覆盖。）② 文件重命名 / 复制失败不再返回成功（此前界面会列出一个永远打不开的文件）；③ OCR 健康探测失败记录原因（此前扫描版 PDF 静默变成 “DONE, 0 chunks”）；④ 引导页写回 `.env` 前若读取失败不再从空文件重建（此前会抹掉用户其它配置）；⑤ Agent 流式回答生成失败时补发错误事件（此前以 `finish_reason: stop` + `[DONE]` 正常结束，用户看到“成功但空”的回答）；⑥ 查询改写、检索上下文嵌入、文档分块拉取、模型 SSE 帧解析、评测失败落库、取消/重试状态落库、结构编译与结构合并的存储写入失败均补上日志。
- **可诊断的启动失败**：把模板原样复制成 `.env` 后以纯 Linux 方式启动，此前只打印 `Error: db error`；现在会打印脱敏后的数据库地址与三条具体处置建议。
- **登记表工具**：参数登记表生成器现在把模板中“默认注释”的键也纳入登记（此前会被并进相邻键的说明里而消失），登记表由 92 行增至 176 行；新增的配置面守卫测试会阻止模板与代码再次漂移。

## [0.4.96] - 2026-10-02

- **品牌**：RayRAG 专属图标（棱镜分束·三色知识块）上线：`web/static/logo.svg`、站点头部内联徽标、全站 favicon；界面可见文本统一为 RayRAG（保留上游兼容的环境变量/协议标识符）。
- **路由**：新增 `/memory` 父壳（SideBar + 内容面板，子页 `memory-message` / `memory-setting` 嵌套其内），对齐 RAGFlow v0.27.2 `routes.tsx` 的嵌套路由结构并修复该路径 404。
- **管理台**：服务详情（定义列表）与 Extra 信息（JsonView 树）弹窗、用户详情/改密/新建/删除流程、沙箱提供商卡片与 SSH 连通性测试等与对标页逐项对齐；新增全站排版审计脚本（溢出/不可见/低对比/截断=0）。
- **质量**：CDP 真实点击遍历（admin 控制台 11/11 脚本全绿，0 pageerror）；`cargo fmt --check` 干净、`cargo test --locked` 全绿。

## [0.4.95] — 2026-10-02（对标批次 v0.3.10ah，task-6 收敛轮）

> - zvec-rust 升级至 **v0.7.2**（最新 tag；预编译 libzvec_c_api.so 装机；check/定向测/doc_store 全绿）。
> - docker compose 双容器 Up (healthy)（9390→8080 / 55432→5432）；docker 构建 EXIT=0（国内镜像）。
> - 纯 Linux debug 冒烟多实例实测（setup/登录/注册/写回）；内存采样 RSS 稳定 ~75-77MB。
> - 按需流水线式加载解析证据（src/parser/pdf_stream.rs 流式限额与保护流拒绝，4 测）。

**构建**：`cargo build --release --locked`；全量测试见发布时段。

## [0.4.94] — 2026-10-02（对标批次 v0.3.10ah，W-4 收尾轮）

> - runner wiki synth 契约接线完成（`src/harness/wiki_synth.rs`：`WikiSynthHost` 绑定 `knowlege_wiki` 的 PLAN/REFINE；端到端测试绿）。
> - 复核对账队列 435 项九批复核清零（台账标记摘除；每批 `coverage --check` 通过）。
> - GPU 探测补全（`nvidia-smi` 尽力而为 + 启动日志 `gpus=` 字段）。
> - task-4 实机证据：全新引导管理员、向导 .env 写回、开放注册与权限门、资源探测日志。
> - 提供商对照归零：73/73 词根覆盖，补齐 LeptonAI/PerfXCloud presets 与 Jina 多向量别名（等价别名书面化）。

**构建**：`cargo build --release --locked`；全量测试见发布时段。

## [0.4.93] — 2026-10-01（对标批次 v0.3.10ah，W-4 收口）

> wiki.py（4154 行 / 68 定义）与 wiki_incremental.py（5004 行 / 73 定义）全量收敛为 \`full\`：
> - \`src/knowlege_wiki.rs\`（61 模块单测）：MAP/REDUCE/PLAN/REFINE 四相（提示词逐字、状态代数、KNN 对账、计划校验、逐页写出、链接/证据/草稿持久化）。
> - \`src/knowlege_wiki_incremental.rs\`（模块单测全绿）：双模式增量编译（实体逐页/主题分组）、规范实体索引与匹配、页面路由（嵌入候选+LLM 决策+NEW 二次确认）、容量受限球面 k-means、页移动对账与不稳定页拆分、FINALIZE 死链清理与自动链接、文档删除清理。
> - 台账两文件 → full；coverage --check 常绿。

## [0.4.92] — 2026-10-01（对标批次 v0.3.10ah，中间版）

> dataset_nav.py（1967 行 / 42 定义）全量收敛为 `full`（`src/knowlege_dataset_nav.rs`，18 项单测绿）：命名/存储包装/行构造/分词与关键词/条件与范围/文本与向量检索（过滤回退扫描）/分层簇定位/LLM 合并与标题/图文本/upsert 主流程（锁契约注入）/移除与清理级联/两均值分裂/扁平混合检索 `search_dataset_nav`/带束树检索 `search_nav_tree_descent`（词法种子、逐层融合、叶子门控、覆盖兜底）。书面差异：KNN/束搜索顺序执行、行 id 用 xxh3（上游 xxh64）、`MatchExpr::Dense` 无 extra_options、无 sync wrapper。W-4 余 2（wiki.py、wiki_incremental.py）。

**构建**：`cargo build --release --locked`（默认 feature：`postgres-backend`、`zvec-backend`）。

## [0.4.91] — 2026-10-01（对标批次 v0.3.10ah，中间版）

> 本中间版在 0.4.90 基础上又关闭一个 W-4 文件并推进第三个：`structure.py` 全文件收敛为 `full`；`dataset_nav.py` 完成前三批移植；台账对剩余三文件完成机器化缺口测绘（逐项书面说明）。

- `structure.py` 收敛为 `full`（3430 行 / 61+4 定义）：在本地既有提取/合并/链校验之上补齐 24 项 v0.27.2 缺口——全量自适应 `LlmCallPool`+`ModelPoolState`+`PooledChatModel`（替换直通 stub）、别名解析与关系改写/重建、精确实体合并、doc-storage 去重条件/KNN 候选/组判定与合并批次（三条提示词逐字）/单组合并/批量编排 `dedup_batch`、`entity_candidate_groups`、`dataset_struct_graph_row_id`、`RechunkedDocs` 等；structure_compile 套件 **72 项单测绿**。书面差异：嵌套批次并发顺序化、行 id 替代 Python `id()`、`MatchExpr::Dense` 无 extra_options、`ensure_llm_bundle` 不适用。
- `dataset_nav.py` 前三批（`src/knowlege_dataset_nav.rs`，13 项单测绿）：
  - part 1：常量（阈值/扇出/锁/KNN/混合权重/停用词）、`nav_doc_id`/`nav_cluster_id`/`nav_lock_key`、`extract_root_summary_from_tree`、`index_name`/`vec_field`、`store_get`/`store_search`；
  - part 2：`store_upsert`/`store_delete`、`vector_len`/`cosine_sim`、`make_nav_doc_row`/`make_nav_cluster_row`（摘要哈希命名与 `q_N_vec` 字段）、`tokenize`/`fine_tokenize`（crate 分词器承接 rag_tokenizer）、`nav_keywords`/`nav_entities`、`as_str_list`/`matches_condition`/`in_nav_scope`；
  - part 3a：`store_text_search`/`store_knn`（过滤回退扫描）、分层 `find_best_cluster`、`llm_merge`/`llm_create_summary`/`clean_title`/`fallback_title`/`readable_cluster_name`、`build_nav_graph_text`。
- 台账：`structure.py` → `full`；`dataset_nav.py`/`wiki.py`/`wiki_incremental.py` 保留 `partial` 并各附机器测绘说明（缺 39 / 64 / 71 项，含计划落点）——W-4 余 3。coverage --check 常绿（5977、unmapped=0）。

**构建**：`cargo build --release --locked`（默认 feature：`postgres-backend`、`zvec-backend`）。

## [0.4.90] — 2026-10-01（对标批次 v0.3.10ah，进行中）

> W-4 续 + 台账证据化重定级：本轮完成 mind_map_extractor、_common、runner、knowlege_compile/raptor 四个文件/单元的 1:1 移植；全递归核查发现六个此前标记 `full` 的 knowlege_compile 条目存在 v0.27.2 符号缺口，已凭证据回退为 `partial` 并附缺口说明（W-4 队列 2→8，本轮消化 4 → 余 4）。

- `src/mind_map.rs`（新，经 `#[path]` 挂载；上游 `knowlege_compile/mind_map_extractor.py`，6 测绿）：`MindMapResult`/`MindMapExtractor`（分组预算、变量替换、think/围栏剥离、`_merge`/`_todict`/`_list_to_kv`/`_be_children`/`_key`、单根/多根输出形状）；`markdown_to_json.dictify` 以「ATX 标题 + 缩进列表 + 段落」子集实现（书面说明）。
- `src/structure_compile.rs`（`_common.py` 与 `runner.py`，套件 42 测绿）：
  - `_common`：`env_int`/`env_float`、`knowledge_compile_gen_conf`（deepseek-v4/qwen3-*preview/其它三分支）、`EmbeddingBackend` 契约 + `encode`、`make_input_budget`、`doc_store_index_name`、`doc_storage_search/insert/delete/upsert_one`；`ensure_llm_bundle` 书面说明未移植（类型系统）。
  - `runner`：env 常量（LazyLock）、`resolve_template_ids_from_groups`/`load_active_templates`/`split_tree_templates`、page_index 摘要与 dataset-nav 前缀、批次并发核心（FuturesUnordered + 有序提交 + 动态 token 预算 + merge 上限冲刷）、合成阶段（wiki PLAN/REFINE + compile_kwd 覆写）、dataset 图重建、timeline 清理；宿主服务经 `StructureCompileBackend` 契约注入（v0.27.2 参数面逐字）。
- `src/knowlege_raptor.rs`（新，挂载；上游 `knowlege_compile/raptor.py`，6 测绿）：1D 分水岭聚类（手写 `np.percentile` 线性插值、比率上限二次调阈、退化聚类守卫）、`clustering` 密集重标、摘要（think/截断清理、标题首行、嵌入、错误预算）、递归层构建（小层塌缩）与层树物化；与旧 `src/raptor.rs`（上游 `rag/raptor.py`）为不同文件。
- 台账：mind_map/_common/runner/raptor → `full`；structure.py/dataset_nav.py/wiki.py/`_common` 之外的其余两个（wiki_incremental.py、structure.py/dataset_nav.py/wiki.py 缺口）保持 `partial` 并写入证据化缺口说明（W-4 余 4）；coverage --check 常绿（5977、unmapped=0）。

**构建**：`cargo build --release --locked`（默认 feature：`postgres-backend`、`zvec-backend`）。

## [0.4.89] — 2026-10-01（对标批次 v0.3.10ag）

> W-4 续：Agentic-RAG 能力层（`agentic_rag.py`，995 行）+ 五阶段编排图（`agentic_rag_graph.py`，1766 行）全文件 1:1 移植完成。W-4 队列 4 → 2（余 runner.py、wiki_incremental.py）。

- `src/advanced_rag/agentic_rag.rs`（对齐上游 995 行；52 项单测绿）：
  - 问题层：`question_keywords`/`cache_similar`（0.6 重叠 + ≥2 共享词 + 数字集必须一致）/`resolve_effective_question`（同轮 ≥2 关键词才采用原始问题）；
  - `RagTools` 数据模型：KB 分组（field_map 键存在性→结构化）、doc_scope 去重、`_settings` 读取、has_* 旗标、`scoped_doc_ids` 矩阵、`fit_messages`、引用准则（用户覆盖优先 + 说明尾注）；
  - 能力层：`sys_prompt` 路由、`formalize`（单轮直取 + 多轮一次 LLM 出 question/keywords，think/围栏剥离与 JSON 修复）、`extract_keywords`（四加权面并集压实）；
  - 检索：`RagRetrievalBackend` 注入契约 + `retrieve`（-999 哨兵、未知文档回退/阻断、向量权重、knn/rerank 解析）、`web_retrieve`、`structured_retrieve`（use_sql）；
  - 证据与判定：`fit_evidence`（8000 token 固定预算）、`judge_sufficiency`/`gen_followups`（`gen_json` 移植：think/围栏剥离 + 一次纠正重试）、四 DB 助手、`fetch_full_document`（128 分页 + token 预算外层中断）、忠实 `kb_prompt`（ID 行/元数据/预算）、`summarize_document`（start_idx 尾部 + 引用头）；
  - `rag` 工具：`run_agentic_rag` 接线 + `ThinkSplitter`（think/answer）、ID 归一 `(ID:n)`→`[ID:n]`、近重复答复缓存（不足裁决跳过缓存）、连续不足护栏（≥2 轮 STOP 提示）；
  - `pick_documents` 上游即 `return None`（忠实保留）；`_filter_by_metadata` 依赖元数据下推服务且上游不可达——书面说明；`select_by_titles` 已移植。
- `src/advanced_rag/agentic_rag_graph.rs`（对齐上游 1766 行；22 项单测绿）：
  - 预算常量（180/50/120/90/60/60/45/60）、`snip`/`safe_list`/`is_poisoned`、`select_sca_view`（相关度+覆盖+新鲜度、稳定身份哈希）、`view_terms`、`remaining_s`、`bounded`；
  - `AgenticState`（含 slot_evidence）、`ThinkSplitter`（含未配对关闭标签与部分标签保持）；
  - SCA 缺口提取、fanout 形状守卫/解析/扩展（严格重试）、双通道 `fanout_search`（BM25+narrow 与 hybrid 旁路、全局池上限 60）；
  - 节点：formalize/planner/prefetch/rag_agent（槽位研究回合，2 路并发）/draft（回退草稿）/sca（视图换入、槽证据定位、早停）/query_rewrite（账本上下文、槽线索折入、饱和早停）/formalize_answer；路由含池满/无进展/预算守卫；
  - `run_agentic_graph`（显式循环等价 LangGraph，迭代上限 60）/`run_low_graph`/`run_agentic_rag`（NAIVE 回退）；`AgenticRuntime` 注入契约；
  - 支撑件：`Kbinfos.pre_summary` 字段 + `HarnessChat::chat_streamly_delta` 默认桥。
- 台账：`agentic_rag.py`、`agentic_rag_graph.py` → `full`；worklist W-4 4 → 2；`ragflow-coverage --check` 常绿（5977、unmapped=0）。

**构建**：`cargo build --release --locked`（默认 feature：`postgres-backend`、`zvec-backend`）。

## [0.4.88] — 2026-10-01（对标批次 v0.3.10af 正式）

> W-4 收口：动作会话（`action_session.py`，2142 行）全文件 1:1 移植完成。0.4.87 的中间发布已含前四分区；本版补齐会话节点循环、导航前缀、会话内 ladder 续跑、槽位构建器与入口。

- 会话节点循环：`ActionLlmBackend` 注入式契约（`complete_with_tools`/`complete_plain`；超时由调用方给定）；`run_action_node`（原生工具调用归一化透传 / 终态解析 / 每回合一次 nudge）、`finalize_node`（预算耗尽 salvage：去配对 `tool_calls` + 无工具补全 + 松散线索兜底 `narrative:`）、`route`/`route_after_tool`（pending 先于预算、attempts≥4、`skipped_dup≥2` 早收敛）与等价会话图的显式循环 `run_session_loop`。
- 导航前缀：`NavRule`/`NavContext` + 三条逐字规则（locate → drill → global，状态路由表逐字）＋ `run_drill_merge`（全库检索 + `nav_hint` 软提升 + 各路由文档 `navigate_structure` 路径按 chunk_id 合并 + `nav_rank` 稳定重排）＋ `run_nav_chain`/`run_nav_prefix`（AUTO 执行、LLM 交还、预算 `max(5, 35%)`、单次上限 25s、提示截 600 字符）＋ `emit_nav_pair`（成对消息、payload 至多截到 800）＋ `extract_relevant_evidence`。
- 会话内 ladder 续跑（`_tool_node` 的 ladder 块）：每个已执行调用后按 `nav_rule_id` 的 verdict 继续余下阶梯，`ladder_<rung>` 成对消息与 outcomes 追加。
- 槽位构建器与入口：`initialize_state`（`slots`/`first_queries` 解析、clues≤4、queries≤3、fanout 兜底→answer 槽、`init_retry_timeout` floor/ceiling/deadline）、`run_action_session`（`action_run` 系统提示 + Direction/State/已检索/上轮摘要种子、导航前缀种入、共享 `tool_cache`/`search_queries`、deadline 扣减、返回 `SessionResult`）、`ACTION_RUN_PROMPT`/`ACTION_INITIALIZE_STATE_PROMPT` 逐字。
- 架构性差异（已在 `ActionRuntime` 文档注明）：`_inject_nav_tools_ref`/`_kb_ids`/`_seed_evidence` 为 Python 宿主管线，Rust 侧由显式 backend/`Kbinfos` 参数替代。
- 单测：action_session 42 项全绿；台账置 `full`；worklist W-4 5→4；`ragflow-coverage --check` 常绿（5977、unmapped=0）。

**构建**：`cargo build --release --locked`（默认 feature：`postgres-backend`、`zvec-backend`）。

## [0.4.87] — 2026-10-01（对标批次 v0.3.10af，进行中）

> W-4 续：动作会话（`action_session.py`，2142 行）前四分区落地（纯 Rust）；文件级收尾（导航前缀/会话入口/提示词）随下一分区。

- `src/harness/action_session.rs`：
  - 槽位模型 `Variable`/`SlotState`/`SessionResult`；`ToolOutcome` 状态契约（ok/empty/miss/poor/redundant/error 与 no_structure/no_doc/infra/bad_args 路由）；
  - 近重复检索拦截（0.8 Jaccard；`_RETRIEVAL_TOOLS` 恰为 search_chunks/grep_chunks/grep_search，不含 retrieve——与上游一致）与证据准入（表块整文、正文 1200 截断、证据池 60 上限早停）；
  - 执行器与分发：`exec_retrieve|search_chunks|web_search|list_chunks`（`ActionSearchBackend` 注入）与 `exec_navigate_tree|navigate_structure|calculate|graph_explore`（`ActionToolBackend` 注入）+ `execute_tool` 全分发（禁用短路/未知 bad_args）；
  - 终态解析：`extract_tag`/`extract_json`/`apply_patch`（仅既有 id；strength 0..1 钳制；线索取尾 4 条各截 160 字符）/`parse_terminal`（state/answer 两态）与 `parse_tool_calls`（缺 id 合成 `call_N`、未知名保留 `unknown`）；
  - 工具面：8 个逐字 spec schema + `tool_spec`/`active_tool_schemas`/`active_tool_specs`（模式工具面、无 web 提供方隐藏、禁用过滤）；
  - `tool_node_policy`：近重复跳过并计数、未知工具修正提示、同会话缓存、EMPTY_STRIKES=2 禁用、redundant 显式注记、outcomes 记录。
- 台账：`action_session.py` 记录分区进度（保持 partial）；worklist W-4=5；`ragflow-coverage --check` 常绿（5977 文件、unmapped=0）。
- 单测：action_session 16 项全绿；fmt/check 干净。

**构建**：`cargo build --release --locked`（默认 feature：`postgres-backend`、`zvec-backend`）。

## [0.4.86] — 2026-10-01（对标批次 v0.3.10ae）

> W-4 续：编译产物扩展（零 LLM）全量移植——W-4 队列 6→5。

- `src/harness/tools/compiled_expansion.rs`（上游 `tools/compiled_expansion.py`，514 行）：`expand_compiled_strategy`（实体种子 → 双拼写端点 → from/to 双向关系（≤50/向）→ 1 跳邻居 → `name_kwd` 查询（≤100）→ 按文档分组 → 块加载去重，≤5/策略）、`expand_wiki_page_strategy`（合成页直搜 → 源块加载、`similarity` 缺省 0.9 优先排序）、`expand_with_compiled`（4 模板 kind + tree + wiki_page/artifact_page/essence 三策略，`seen` 全局去重，按 similarity 稳定重排，返回增量）；`VECTOR_NUM_CANDIDATES=256`/`VECTOR_SIMILARITY=0.1` 常量；store/嵌入/块加载经 `CompiledExpansionHost` 注入契约。
- 台账：`tools/compiled_expansion.py` → full；worklist W-4 6 → 5；`ragflow-coverage --check` 常绿（5977 文件、unmapped=0）。
- 单测：3 项全绿；fmt/check 干净。

**构建**：`cargo build --release --locked`（默认 feature：`postgres-backend`、`zvec-backend`）。

## [0.4.85] — 2026-10-01（对标批次 v0.3.10ad）

> W-4 续：导航工具全量移植（nav-tree 路由 + 结构导航/向量束钻取；纯 Rust）——W-4 队列 7→6。

- `src/harness/tools/navigation.rs`（上游 `tools/navigation.py`，1936 行；切片 1+2a+2b 完成）：常量与 `NavResult`、`normalize_kind`、`load_chunks_by_ids`、`doc_aggs`；nav-tree 路由（编号列表 LLM 选择与解析、叶节点 BFS（深度 6/叶 300/文档 scope 过滤）、内容召回兜底、`dataset_navigation_by_tree`）；层搜索（分数门槛 0.2、`_nav` 摘要标注、每文档取最高分、12 上限、`dataset_navigation_search`）；结构导航（`structure_kinds_for`、TOC 树构建、向量束钻取（深度 3/K=2/相对比 0.5）与关键词回退、三选择器渲染（chunk 检索序/LLM TOC/束）、root→node 路径映射与统计、`render_outline` 兜底、`build_toc_items`/`select_toc_nodes`（默认关闭）、`read_structures`、`navigate_tree_impl`/`navigate_structure_impl`（XML + `empty_reason`）、`expand_related_via_structure`）；store/LLM/向量读取经 `NavRouterHost`/`NavStructureHost` 注入契约。
- 台账：`tools/navigation.py` → full；worklist W-4 7 → 6；`ragflow-coverage --check` 常绿（5977 文件、unmapped=0）。
- 单测：导航 11 项全绿（基础 2 + router 4 + structure 5）；fmt/check 干净。

**构建**：`cargo build --release --locked`（默认 feature：`postgres-backend`、`zvec-backend`）。

## [0.4.84] — 2026-10-01（对标批次 v0.3.10ac）

> W-4 续：探索工具（知识图谱漫游 + wiki 查询）全量移植；导航工具共享层就位（纯 Rust）。

- `src/harness/tools/exploration.rs`（上游 `tools/exploration.py`，468 行）：KG 常量（种子 2 / 候选池 64 / 相似度 0.8 / 跳数 2 / 邻居 128 / 关系 32）、注入式检索契约、`kg_parse_entity`/`kg_parse_relation`、`endpoint_terms`、`collect_evidence_ids`、`graph_explore`（种子 → 2 跳 BFS → outline 判定；不足时按相关实体/关系回取证据块并关键词窄化）与 `wiki_query`（`wiki_page_draft` 页面 markdown 抽取 + 窄化 + doc_aggs）。
- `src/harness/tools/navigation.rs`（切片 1）：`CATALOG_KINDS`/`MINDMAP_KINDS`/`MAX_EVIDENCE_CHUNKS`/`MAX_ENTITIES`、`NavResult`、`normalize_kind`、注入式 `load_chunks_by_ids`、`doc_aggs`；导航树路由与结构导航留待 v0.3.10ad。
- `src/harness/structure_qa.rs`：新增 `ask_structure`（outline 判定调用，exploration/导航复用）。
- 台账：`tools/exploration.py` → full；`tools/navigation.py` 记录切片进度；worklist W-4 8 → 7。
- 单测：新增 7 项（exploration 4 + navigation 2 + structure_qa 1）；fmt/clippy 干净。

**构建**：`cargo build --release --locked`（默认 feature：`postgres-backend`、`zvec-backend`）。

## [0.4.83] — 2026-10-01（对标批次 v0.3.10ab）

> W-4 续：agentic harness 检索腿与 SCA 统一审查移植（纯 Rust）。

- `src/harness/tools/search.rs`（上游 `tools/search.py`，685 行）：设置解析（`top_n`/`top_k`/`rerank` 下限）、`effective_query` 400 字符、检索缓存键、注入式 `HarnessRetriever`（hybrid 权重 0.3 / vector 1.0 / BM25 0 三条腿）+ 关键词窄化、grep 词抽取（10 上限）与查询转词、表块整体保留的 `grep_search`、`list_chunks`/`load_specific_chunks`/`rank_chunks_by_terms`、`TOOL_NAME_BY_FUNC`/`COMPILED_TOOL_MODES`。
- `src/harness/orchestrator/sufficient_context.rs`（上游 `orchestrator/sufficient_context.py`，419 行）：`sca_select` 提示词原文、claims 渲染（草稿 + 证据锚定 ≤300 字符、表格整文）、overall draft、`coerce_dict` 格式容错、`sub_queries`/`claims` 解析与 `_global` 兜底（顶层 missing → 草稿降级）、`to_boost`/`to_grounded` 适配器。
- 台账 2 行 partial → full；worklist W-4 队列 10 → 8；`ragflow-coverage --check` 常绿（5977 文件、unmapped=0）。
- 单测：新增 9 项（search 5 + sufficient_context 4）；fmt/clippy 干净。

**构建**：`cargo build --release --locked`（默认 feature：`postgres-backend`、`zvec-backend`）。

## [0.4.82] — 2026-10-01（对标批次 v0.3.10aa）

> W-4 续：agentic harness 确定性工具层移植（文本窄化 + 记忆检索 + 安全算术；纯 Rust）。

- `src/harness/tools/text_processing.rs`：句切分（块级 HTML 标签栈与 markdown 表格原子化）、词干匹配（上游回退词干器路径）、关键词窄化与高亮、fact-dense 句子保留。
- `src/harness/grep_sed_narrow.rs`：Claude-Code 式内存 grep+sed 窄化（行定位 + 上下文扩窗 + 字符预算钳制、兜底词机械重试、总预算按块分摊）。
- `src/harness/memory.rs`：原始检索记忆（无损 add、grep 前缀容忍 + 句窗上下文、语言无关 significance 检索复用 + 重叠比门槛）。
- `src/harness/arithmetic.rs`：白名单解析/求值器（Python 语义的地板除/取模符号/幂指数上限/链式比较短路/and-or 返回操作数/惰性三元），letters/digit_sum/date_diff，`compute` 与 `compute_from_facts`（`COMPUTE_SYSTEM` 原文）。
- 台账 4 行 partial → full；worklist W-4 队列 14 → 10；`ragflow-coverage --check` 常绿（5977 文件、unmapped=0）。
- 单测：harness 全量 33 项全绿；fmt/clippy 干净。

**构建**：`cargo build --release --locked`（默认 feature：`postgres-backend`、`zvec-backend`）。

## [0.4.81] — 2026-10-01（对标批次 v0.3.10z）

> W-4 续：agentic harness orchestrator 策略移植（`direct` + `query_rewriter`，纯 Rust）。

- `src/harness/orchestrator/direct.rs`：`direct_search`（`Kbinfos` 合并去重、`_chunk_key` 真值链与匿名回退、doc_aggs 的 None 语义、entity 加权检索查询注入、`empty_result` 分支、`direct` 阶段作用域）+ `DirectTools` 注入契约（`extract_keywords_weighted` / `hybrid_search`）。
- `src/harness/orchestrator/query_rewriter.rs`：`sca_query_rewrite` 模板原文 + 渲染（gaps / bridge / research_context）、`parse_rewrite_queries`（对象与裸字符串、去重保序、think/围栏清洗）、`rewrite_gap_to_query`（缺 chat / 空 gaps / 调用失败 → 空、`rewrite` 阶段作用域）。
- 台账 2 行 partial → full；worklist W-4 队列 16 → 14；`ragflow-coverage --check` 常绿（5977 文件、unmapped=0）。
- 单测：orchestrator 5 项全绿；fmt/clippy 干净。

**构建**：`cargo build --release --locked`（默认 feature：`postgres-backend`、`zvec-backend`）。

## [0.4.80] — 2026-10-01（对标批次 v0.3.10y）

> W-4 起步：RAGFlow v0.27.2 agentic harness 基础层移植（`src/harness/`，纯 Rust）。

- `src/harness/`：`HarnessChat` 契约 + `form_message`/`message_fit_in` 提示装配；chunk 字段访问器（`chunk_utils`）；思考模式表 `low/medium/high/ultra` + `naive` 回退（`config`）；四面向关键词加权抽取（`keywords`，`KEYWORDS_SYSTEM` 原文 + entity/qualifier ×3 + 400 字符上限）；LLM 用量计账（`stats`：phase 作用域、round/round-phase、tokens、日志表）；结构 outline QA（`structure_qa`）；报告提示词（`report_prompt`）；think 日志桥（`think_log`）；orchestrator/tools 包布局文档。
- 台账 11 行 partial → full；worklist W-4 队列 27 → 16；`ragflow-coverage --check` 常绿（5977 文件、unmapped=0）。
- 单测：`cargo test --lib harness::` 13 项全绿；fmt/clippy 干净。

**构建**：`cargo build --release --locked`（默认 feature：`postgres-backend`、`zvec-backend`）。

## [0.4.79] — 2026-10-01（对标批次 v0.3.10x）

> W-5 收尾：结构图采样服务 + 数据集级结构图合并任务全量移植（纯 Rust）——worklist **W-5 队列清零**。

- `src/api/structure_graph_common.rs`（上游 `api/apps/services/structure_graph_common.py`，598 行）：5 个采样常量、实体/关系字段表、`graph_search`、`project_entity`/`project_relation`（复用结构投影）、`dedup_entities`、`normalize_relation_endpoints`（歧义键丢弃）、`filter_entities_with_relations`、`row_has_enabled_source`（JSON 编码源列表与 doc_id 回退）、`build_bucket`（小桶全量；大桶 top-256 mention 采样 + 关系源出 + 目标实体）与 `keyword_subgraph`（精确名 → BM25 名称包含过滤 → KNN 单例回退；模板桶归并；树/分页祖先回溯；邻居实体；树内关系过滤）。
- `src/dataset_structure_merger.rs`（上游 `rag/svr/task_executor_refactor/dataset_structure_merger.py`，1027 行）：256 哈希桶（500 条冲洗）、增量时间窗、`record_doc_deletion` + 两遍幽灵清理（共享行回写 `doc_ids_kwd`）、`consume_deletion_markers`、`merge_bucket`/`merge_relations`（分组、去重、最长描述/原名、mention 求和、分批嵌入）、`kg_build_meta` 构建时间戳、`run_structure_merge`（模板 kind 过滤、disabled→全量重建、进度/取消）。
- 台账：两行 partial → full；worklist **W-5 → 0**（W-1/W-2/W-3/W-6 均 0，余 W-4 27 项 agentic harness）；`ragflow-coverage --check` 通过（5977 文件、unmapped=0）。
- 单测：`structure_graph_common` 8 项 + `dataset_structure_merger` 7 项全绿；fmt/clippy 干净。

**构建**：`cargo build --release --locked`（默认 feature：`postgres-backend`、`zvec-backend`）。

## [0.4.78] — 2026-10-01（对标批次 v0.3.10w）

> W-5 续：RAGFlow v0.27.2 Compiler 流程包（`rag/flow/compiler/*`）移植 + `agent/templates/compiler.json` 判定（纯 Rust）。

- `src/compiler.rs`：`CompilerFromUpstream`（serde 别名 `_created_time`/`_elapsed_time`/`json`/`markdown`/`text`/`html` + `deny_unknown_fields` ↔ 上游 `populate_by_name=True, extra="forbid"`）与 `CompilerOutputFormat` 五值枚举；切分/归一化全套（slumber 两级切分、markdown 原子块、原子 JSON 记录、rechunk 组形式化、小块合并、五路上游归一化）；`CompilerParam.check`（单数优先 + 传统复数回退，空组报错文案 1:1）、`compile_language` 三级回退、`compile_progress`；共 11 项单测。
- `agent/templates/compiler.json`：判定为上游永不入库的 DSL-only 残留（容器内复刻 `IntegrityError: NOT NULL constraint failed: canvas_template.id`），台账记 replaced，等价能力由 compilation templates + Compiler 组件承载。
- 台账 4 行收敛（full×3 / replaced×1），`worklist` W-5 队列 6→2；`ragflow-coverage --check` 通过（5977 文件、unmapped=0）。

**构建**：`cargo build --release --locked`（默认 feature：`postgres-backend`、`zvec-backend`）。

## [0.4.77] — 2026-10-01（对标批次 v0.3.10v）

> W-5 续：RAGFlow v0.27.2 的重解析计数器释放（`api/db/services/document_counter_service.py`）移植（纯 Rust）。

### Added

- **重解析计数器释放**（`document_counter_service.py` → `src/api/document.rs`）：`reparse_counter_release` 按文档当前 `chunk_count` 产出负 chunk 增量与其 KB 归属，`reparse_doc` 在入队前应用（`KbStore::update_counts` + `DocStore::set_chunk_count(id, 0)`），使重解析从零开始——文档缺失报错、全零跳过，与上游 `LookupError`/空值语义一致。
- 有界差异（台账注记）：本地存储没有单事务，两次写由处理器按顺序执行；KB 缺失路径经前置 `kb_manageable` 校验不可达。

### Tests

- 1 项单测（释放并双端归零、二次调用 no-op、缺失文档报错）；`cargo fmt --check`、`cargo check --locked --features postgres-backend,zvec-backend` 全绿。
## [0.4.76] — 2026-10-01（对标批次 v0.3.10u）

> W-5 续：RAGFlow v0.27.2 的渠道智能体目标校验（`api/channels/targets.py`）移植（纯 Rust）。

### Added

- **渠道智能体目标校验**（`api/channels/targets.py` → `src/api/features.rs`）：`validate_agent_target` 在 `AgentStore` 上按 id 查找并校验 `canvas_category == "agent_canvas"`（否则 `not_found`），再经 `agent_accessible`（built_in / owner / team 成员 / 管理员空归属）判定租户可访问性（否则 `forbidden`），有效则返回 `None`。

### Tests

- 1 项单测覆盖缺失 id、类别不符、owner 放行、team 非成员 forbidden、成员放行；`cargo fmt --check`、`cargo check --locked --features postgres-backend,zvec-backend` 全绿。
## [0.4.75] — 2026-10-01（对标批次 v0.3.10t）

> W-5 续：RAGFlow v0.27.2 的共享句界定义（`rag/flow/chunker/_sentence_boundary.py`）移植（纯 Rust）。

### Added

- **共享句界定义**（`_sentence_boundary.py` → `src/chunk/sentence.rs`）：`SENTENCE_BOUNDARY_PATTERN`（中文/英文句末标点 + 换行，捕获组保留边界归属前一子句）、DOTALL 等价编译、`split_sentences_keep_boundaries` 句级拆分。
- **token 分块器接线**：超过 `chunk_token_num` 的单个 section 先按句界重拆再合并（对齐上游 `token_chunker` 消费点）。

### Tests

- 2 项新单测（模式常量与匹配、保留边界的拆分语义）；`chunk::token` 既有 9 项测试无回归；`cargo fmt --check`、`cargo check --locked --features postgres-backend,zvec-backend` 全绿。
## [0.4.74] — 2026-10-01（对标批次 v0.3.10s）

> W-5 首批：RAGFlow v0.27.2 的 canonical delimiter 解析（`rag/nlp/delim.py`）移植（纯 Rust）。

### Added

- **canonical delimiter 解析**（`rag/nlp/delim.py` → `src/chunk/delim.rs`）：`parse_delimiter_field`（反引号 token + 裸字符、插入序去重、稳定按**字符数**最长优先）、`compile_delimiter_pattern`（`regex::escape` 交替）、`has_wrapped_delimiter`、`normalize_text_newlines`（CRLF/CR→LF）与 `DEFAULT_DELIMITER` 常量；接入 naive 与 token 分块器的 split 路径（空字段保持原行为）。
- **修复**：排序从 `String::len()`（UTF-8 字节长度）改为 `chars().count()`，避免中文标点（3 字节）错误地排在换行（1 字节）之前。

### Tests

- 4 项新单测（裸/包裹 token 最长优先、去重与换行归一、包裹检测与模式编译、默认常量 8 字符）；`cargo test --locked --lib chunk::delim` 4 passed，既有 chunk 测试无回归；`cargo fmt --check`、`cargo check --locked --features postgres-backend,zvec-backend` 全绿。
## [0.4.73] — 2026-10-01（对标批次 v0.3.10r）

> W-6 清零：RAGFlow v0.27.2 的 Tenki 与 UCloud Agent Sandbox 提供商契约移植（纯 Rust）。

### Added

- **Tenki 提供商契约**（`agent/sandbox/providers/tenki.py` → `src/sandbox.rs` + `src/api/sandbox_admin.rs`）：配置默认值与校验消息（api_key/timeout/max_lifetime/cpu_cores/memory_mb/disk_size_gb/max_output_bytes/max_artifacts/max_artifact_bytes）、语言归一化（python3→python、javascript→nodejs）、产物契约（8 扩展名/深度 16//home/tenki）、注册表条目与完整配置 schema。
- **UCloud Agent Sandbox 提供商契约**（`ucloud_agent_sandbox.py` → 同上）：region cn-wlcb/template base/insecure_http 等配置、域推导（`<region>.sandbox.ucloudai.com`，domain 可覆盖）、js/node 语言别名、产物契约（/home/user）、注册表条目与 schema。
- 沙箱注册表 5→7；有界差异：vendor SDK 执行为 Python-only，本块为契约级对齐（与 aliyun 先例一致）。

### Tests

- 3 项新单测（tenki 默认值/边界/校验、ucloud 默认值/校验/域推导、产物与 home 常量）+ 两处注册表断言更新；`cloud_provider` 3 passed、`sandbox_admin` 4 passed；`cargo fmt --check`、`cargo check --locked --features postgres-backend,zvec-backend` 全绿。
## [0.4.72] — 2026-10-01（对标批次 v0.3.10q）

> W-6 首批：RAGFlow v0.27.2 的执行器 /run 鉴权（auth.py）与预鉴权限流（preauth.py）移植（纯 Rust）。

### Added

- **执行器 /run 鉴权**（`agent/sandbox/executor_manager/services/auth.py` → `src/sandbox.rs`）：共享密钥 env、fail-closed 503、无效令牌 401、`ALLOW_UNAUTHENTICATED` 显式 opt-in 与一次性安全警告、`Bearer`/`X-Sandbox-Token` 提取、常量时间比较、启动姿态日志；`RemoteSandboxConfig.api_token`（env `SANDBOX_EXECUTOR_MANAGER_API_TOKEN`）随 `POST /run` 发送 `Authorization: Bearer`（`/healthz` 不带，对齐 self_managed.py）。
- **预鉴权限流**（`preauth.py` → `src/sandbox.rs`）：`SANDBOX_RUN_PREAUTH_RATE_LIMIT`（默认 30/minute）解析、按客户端地址滑动窗口、每窗口一次拒绝告警、超万地址清理、reset。

### Tests

- 5 项新单测（令牌提取、常量时间比较、三种鉴权姿态与 503/401、速率解析、滑动窗口行为）；`cargo test --locked --lib sandbox` 34 passed；`cargo fmt --check`、`cargo check --locked --features postgres-backend,zvec-backend` 全绿。
## [0.4.71] — 2026-10-01（对标批次 v0.3.10p）

> W-2 清零：RAGFlow v0.27.2 的 Azure DevOps 数据源（代码文件 + Pull Request）移植（纯 Rust）。

### Added

- **Azure DevOps 数据源连接器**（`common/data_source/azure_devops/{connector,utils}.py` → `src/connectors.rs`）：PAT 基本鉴权与 203/401/403/登录页映射；三种索引模式（organization / projects / repositories）；code 内容（条目列举、窗口过滤、跳过规则、1MB 流式上限、文件级一次重试）与 pull_requests 内容（`$skip` 分页、活跃恒留/完成按 `closedDate` 的窗口语义、≥400 字符描述详情补全）；6 次指数退避重试与 `Retry-After` 截断。
- `DocumentSource::AzureDevOps` 与注册表 `azure_devops` 分支；有界差异（台账记录）：无检查点持久化、无 120/min 限流器、超限文件以失败条目呈现。

### Tests

- 4 项 mock 单测（校验/URL 规则、code 映射与噪音跳过、PR 窗口与截断详情、203 鉴权映射）；`cargo fmt --check`、`cargo check --locked --features postgres-backend,zvec-backend` 全绿。
## [0.4.70] — 2026-10-01（对标批次 v0.3.10o）

> W-2 续：RAGFlow v0.27.2 的 Sitemap 数据源连接器移植（纯 Rust）。

### Added

- **Sitemap 数据源连接器**（`common/data_source/sitemap_connector.py` → `src/connectors.rs`）：`sitemap` 注册表分支与 `DocumentSource::Sitemap`；`urlset`/`sitemapindex` 递归（深度上限 5、访问去重、抓取上限 1000）；`<lastmod>` 多格式解析与 `since`/`until` 增量窗口；`url_filter`（长度上限、嵌套量词拒绝、编译错误措辞对齐）；逐跳 SSRF 校验 + 10 次重定向上限 + 64 MiB 响应上限 + 可配置 User-Agent；网页经共享 `crawler::html_to_markdown` 转为 Markdown。
- 有界差异（台账记录）：文档 id `sitemap:<sha1>`（上游 md5）、PDF 载荷走 `ConnectorDoc.blob` 字符串通道（lossy，同 Blob 连接器先例）、`follow_pdf_links` 链接发现推迟（缺 Slim docs API 对应）、Rust regex 线性时间故无需超时防护。

### Tests

- 5 项 mock 单测（校验矩阵、urlset 抓取 + Markdown、index 递归 + 自引用防护、窗口/过滤收窄、无匹配 URL 报错）；`cargo fmt --check`、`cargo check --locked --features postgres-backend,zvec-backend` 全绿。
## [0.4.69] — 2026-10-01（对标批次 v0.3.10n）

> W-2 首批：RAGFlow v0.27.2 的 Xquik 数据源连接器移植（纯 Rust）。

### Added

- **Xquik 数据源连接器**（`common/data_source/xquik_connector.py` → `src/connectors.rs`）：`xquik` 注册表分支与 `DocumentSource::Xquik`；`GET /x/tweets/search`（`q`/`queryType`/`limit`/`sinceTime`/`untilTime`，`x-api-key` 头）；`$.tweets` 游标分页（`cursor`/`next_cursor`/`has_next_page`，`max_pages`/`page_size`/`batch_size`/`request_delay` 默认 10/100/32/0.5s，重复游标停止）；上游内容模板与 metadata 字段列表逐条保留；validation 模式 `limit=1` 单页。

### Tests

- 4 项 mock 单测（校验矩阵、游标分页与鉴权/参数断言、重复游标停止、HTTP 401 透出）；`cargo fmt --check`、`cargo check --locked --features postgres-backend,zvec-backend` 全绿。
## [0.4.68] — 2026-10-01（对标批次 v0.3.10m）

> W-1 清零：RAGFlow v0.27.2 的 MWS 助手与 AIMLAPI 设备授权端点移植（纯 Rust）。

### Added

- **MWS GPT Model Hub 助手**（`rag/llm/mws_utils.py` → `src/mws.rs`）：`normalize_mws_project_url`（`projects/<project>` 根校验、拒绝凭据/查询/片段、保留 host:port）、`mws_api_url`、`require_mws_token`。
- **AIMLAPI 设备授权端点**（`api/apps/restful_apis/aimlapi_api.py` + `common/aimlapi_utils.py` → `src/api/aimlapi.rs` + `src/aimlapi.rs`）：`POST /api/v1/llm/aimlapi/authorize/start` 与 `/poll`（RFC 8628 设备码流、apiKey 多字段变体、终止态处理）；归因头 `X-AIMLAPI-Source`/`X-AIMLAPI-Partner-ID`；设备码服务端保存（进程内 TTL 存储，单实例有界差异）。
- **配置**：`AIMLAPI_APP_URL` / `AIMLAPI_SOURCE` / `AIMLAPI_PARTNER_ID` / `AIMLAPI_PARTNER_NAME` / `AIMLAPI_VERIFICATION_BASE_URL` / `AIMLAPI_REQUESTED_USD_LIMIT_MINOR`。

### Tests

- mws 3 项 + aimlapi 6 项单测（项目根校验矩阵、设备码存储 TTL/用户作用域、start/poll 成功与 pending/expired/用户不匹配、终止态清理、上游 400）；`cargo fmt --check`、`cargo check --locked --features postgres-backend,zvec-backend`、登记表门禁全绿。
## [0.4.67] — 2026-10-01（对标批次 v0.3.10l）

> W-1 续：RAGFlow v0.27.2 的 Sofya 与 You.com 智能体工具移植（纯 Rust）。

### Added

- **Sofya 画布工具**（`agent/tools/sofya.py` → `src/sofya.rs`）：`sofya_search` 的 topic/freshness/max_results 参数、`POST /v1/search`（Bearer + 固定 User-Agent）、`search_depth` 节点配置（未知回退默认并告警）、`content`→`description` 回退与空白归一、仅瞬态重试（{429,500,502,503,504} 与网络错误）。
- **You.com 画布工具**（`agent/tools/youcom.py` → `src/youcom.rs`）：`youcom_search` 的 freshness 参数；keyless `/v1/agents/search` 与 keyed `/v1/search` 端点-头部联动（API key 可选）；`count` clamp 1–100；web+news 合并按 top_n 截断；snippets 拼接优先、description 回退；全错误重试语义。
- 两者共享既有输出契约（`formalized_content`/`json`/`doc_aggs` + ChunkReference）与 12s 组件超时、错误前缀 `Sofya`/`You.com`。

### Tests

- sofya 4 项 + youcom 6 项 mock 单测（归一化、载荷、端点/头部选择、合并截断、重试与立即失败路径）；`cargo fmt --check`、`cargo check --locked --features postgres-backend,zvec-backend` 全绿。
## [0.4.66] — 2026-10-01（对标批次 v0.3.10k）

> W-1 续：RAGFlow v0.27.2 的 Querit 智能体工具（querit_search / querit_contents）移植（纯 Rust）。

### Added

- **Querit 画布工具**（`agent/tools/querit.py` → `src/querit.rs` + `src/agent.rs` 接线）：`querit_search`（count/chunks_per_doc、sites/time_range/country/language 过滤器、`{sys.query}` 默认）与 `querit_contents`（1–10 个绝对 HTTP(S) URL、format/crawl_timeout/extras_meta）；三尝试重试契约（{429,500,502,503,504} 与网络错误重试、其他立即抛出）、上游校验消息逐条、API key 脱敏、组件超时 12s/70s，错误前缀 `Querit`/`Querit contents`。
- **配置**：`QUERIT_API_KEY` 环境变量（节点 `api_key` 优先）。

### Tests

- 11 项 querit mock 单测（重试成功/穷尽/立即抛、载荷与过滤器、URL 归一化与校验、响应校验、脱敏、contents 请求）+ 工具注册与超时映射回归测试；`cargo fmt --check`、`cargo check --locked --features postgres-backend,zvec-backend` 全绿。
## [0.4.65] — 2026-10-01（对标批次 v0.3.10j）

> W-1 首批：RAGFlow v0.27.2 的 web_search 提供商层（Querit / Serply / You.com）与共享 URL 助手移植（纯 Rust）。

### Added

- **web_search 提供商层**（`rag/utils/web_search_conn.py` + `querit_conn.py` + `serply_conn.py` + `youcom_conn.py` → `src/web_search.rs`）：`has_web_search_provider` / `create_web_search_provider`（`prompt_config.web_search_provider` 默认 `tavily`，`*_api_key` 字段，You.com 免钥）与四个连接器（Tavily 复用既有 Canvas 工具客户端，advanced/6 条）；chunk/doc_agg 聚合形状与上游一致；失败按上游语义返回空并告警（不记录查询文本）。
- **共享 URL 助手**（`rag/utils/url_utils.py` → `src/url_utils.rs`）：`ensure_v1`（识别 v+数字版本段）与 `append_api_path`（幂等追加），上游 docstring 示例逐条入测试。

### Tests

- web_search 9 项 mock 单测（请求头/参数/载荷、keyless/keyed 端点选择、web+news 合并截断、失败路径、Tavily 适配器）+ url_utils 2 项；`cargo fmt --check` 与 `cargo check --locked --features postgres-backend,zvec-backend` 全绿。
## [0.4.64] — 2026-10-01（对标批次 v0.3.10i）

> W-3 完成：RAGFlow v0.27.2 的 Mistral OCR 远端 PDF 解析器全量移植（纯 Rust），OCR 解析器收敛队列清零。

### Added

- **Mistral OCR 远端解析器**（`deepdoc/parser/mistral_parser.py` + `rag/llm/ocr_model.py::MistralOcrModel` → `src/parser/mistral_ocr.rs`）：`GET /models` 就绪探测；`POST /v1/ocr`（`model`/`document`/`include_blocks`/`table_format`/`include_image_base64`，文档 ≤20 MiB 走 base64 data URI，超出走 `/files` 上传→签名 URL→OCR→删除）；`MISTRAL_TYPE_TO_RAGFLOW` 类型映射与 `keep_header_footer`；`@@页\tx0\tx1\t顶部\t底部##` 位置标签与 `<!--TABLE_START-->`/`<!--IMAGE_START-->` 标记输出。
- **配置**：`layout_recognize` 支持 `Mistral OCR` 与 `<model>@Mistral OCR`（归一化）；`MISTRAL_OCR_API_KEY` / `MISTRAL_OCR_BASE_URL` / `MISTRAL_OCR_MODEL` / `MISTRAL_OCR_TABLE_FORMAT` / `MISTRAL_OCR_KEEP_HEADER_FOOTER` 环境变量（API key 缺失或服务不可达时明确报错，不做静默降级）。

### Tests

- 9 项单测（mock `/models`+`/v1/ocr`、上传+签名 URL+删除路径、分页选择器、错误/空结果）+ pipeline 端到端 + `@mistral ocr` 归一化测试；`cargo fmt --check`、`cargo check --locked --features postgres-backend,zvec-backend` 全绿。
## [0.4.63] — 2026-10-01（对标批次 v0.3.10h）

> W-3 首批：RAGFlow v0.27.2 的 MonkeyOCRv2 远端 PDF 解析器全量移植（纯 Rust）。

### Added

- **MonkeyOCRv2 远端解析器**（`deepdoc/parser/monkeyocrv2_parser.py` → `src/parser/monkeyocrv2.rs`）：`GET /health` 就绪探测（10 秒超时）；`POST /parse` 多部分上传（`start_page_id`/`end_page_id`）并按 512 MiB 上限流式读取 ZIP；布局记录转换（`{root}/{root}.json` → `jsons/*.json` → `all_results.json` 回退；图像按 basename 索引；1 万成员 / 2 GiB 解压安全上限）；输出沿用既有 `@@页	左	右	上	下##` 位置标签与 `<!--TABLE_START-->` / `<!--IMAGE_START-->` 标记，由现有分块与图像阶段消费。
- **配置**：`layout_recognize` 支持 `MonkeyOCRv2` 与 `<model>@monkeyocrv2`（归一化）；服务地址取 `parser_config.monkeyocrv2_server_url`（按知识库覆盖），回退 `MONKEYOCRV2_SERVER_URL`；未配置或服务不可达时给出明确错误，不做静默降级。

### Tests

- 11 项新单测（含 mock `/health`+`/parse` 端到端与 pipeline `position_int` 断言）与 `@monkeyocrv2` 归一化测试；`cargo fmt --check`、`cargo check --locked --features postgres-backend,zvec-backend` 全绿。
## [0.4.62] — 2026-10-01（对标批次 v0.3.10g）

> RAGFlow v0.27.2 覆盖台账全量分类完成（5977 文件，unmapped=0）并建立对标收敛队列；本轮无功能性代码变更。

### Changed

- **v0.27.2 覆盖台账全量分类**：399 行机械归置（`web/src/**` React 源按既有规则记入参考材料、Go 模式 Docker 文件记入 replaced、样例/工具记入 reference）+ 68 行逐文件审计（GaussDB/SereneDB 存储适配 → replaced；模型类型映射 → aligned；59 项新提供商/解析器/harness 待接入项 → partial 并逐条注记）；`ragflow-coverage --check` 由 unmapped=467 收敛为 **unmapped=0**。
- **对标收敛队列（发布快照剔除）**：新增内部分类清单与生成器，覆盖搜索提供商、数据源、OCR 解析器、agentic harness、结构图/编译器、沙箱六类队列，以及 UI 残留与上游变更复核队列；发布守卫同步更新。

## [0.4.61] — 2026-10-01（对标批次 v0.3.10f）

> 建立参数/接口统一登记表与一致性门禁（用户长期要求项；登记表不发布）。

### Added

- **内部参数/接口登记表**（内部的参数/接口登记表，发布时剔除并在 publish 脚本中加守卫）：
  环境变量登记表（79 项，源自 `.env.example`）+ HTTP 接口登记表（310 条路由，从 `src/server.rs` 提取）
  由 `scripts/gen-interface-registry.py --check/--write` 生成与校验；契约段固定模型选择器格式
  （`model@provider@instance`）、`parser_config` camelCase、运行/切片状态枚举、`{code,message,data}`
  错误契约与“不允许静默错误”规则，供后续编码查询。
- **一致性门禁测试** `settings::tests::env_example_and_interface_registry_stay_in_sync`：
  `.env.example` 与登记表 env 表必须互为子集，参数只出现在一处即测试失败。

### Notes

- 浏览器：Chrome/CDP 恢复尝试后仍为 `phase:blocked`（应用提示需在 App Center 停止再启动），
  本轮 `/search/{id}` 与 chunk 详情真点遍历顺延；登记表轮不依赖浏览器。
- 全量 `cargo test --locked --offline --lib` **1639 passed / 0 failed / 27 ignored**；`cargo fmt --check` 干净。

## [0.4.60] — 2026-10-01（对标批次 v0.3.10e）

> agent / ingestion pipeline 模板 fixture 同步到 RAGFlow v0.27.2（30 模板 / 15 条 dataflow 管线）。

### Added

- **模板同步工具** 内部辅助脚本（`--check/--write`）：从固定参考树读取
  `agent/templates/*.json` 与迁移后的 `internal/ingestion/pipeline/template/*.json`，一次生成
  fixtures（上游 `dsl` 紧凑 JSON）、`meta.json`（en/zh 标题/描述/canvas 元数据）与
  `src/agent_templates.rs` 的清单、`dataflow_canvas` 分类和计数。
- **模板集合更新为 30 个**：新增 `cajal_scientific_paper_agent` 与 `audio/email/picture/presentation/qa/table`
  管线模板；移除上游已删除的 `cv_analysis_and_candidate_evaluation` 与 `ingestion_pipeline_resume`；
  共 15 条 `dataflow_canvas` 管线。
- `compiler.json`（DSL-only，无 title/description）暂不进入模板列表，记录为随 Compiler 流程后续接入。

### Notes

- 台账：16 行 `agent/templates` 再审计关闭；12 个 `internal/ingestion/pipeline/template` 行由 `reference`
  修正为 `full`（原 `internal/` 兜底规则误判）；full 569→581、reference 4452→4440，unmapped 467 不变，
  `--check` 通过。
- 全量 `cargo test --locked --offline --lib` **1638 passed / 0 failed / 27 ignored**；`cargo fmt --check` 干净。

## [0.4.59] — 2026-10-01（对标批次 v0.3.10d）

> 详情路由真实遍历补齐 + v0.27.2 数据集 Artifacts 页签落地。

### Added

- **数据集 Artifacts 页签**（`/dataset/compilation/{id}`，v0.27.2 新增）：sidebar 顺序改为上游 v0.27.2 的
  Files / Retrieval testing / Logs / Configuration / **Artifacts**（knowledge-graph 保留深链面板但不再占 sidebar）；
  页面含 Back（→ files）、"{数据集名}' dataset" 标题、视图选择器（Wiki / Tree-PageIndex / Graph / Mind map /
  Timeline，默认 Wiki）、LlmWiki 空态 "No wiki yet" 与按上游 `canGenerateWiki`（chunk_count > 0 且 pipeline_id）
  门控的 Generate 按钮。

### Notes

- 真点遍历：临时数据集上补齐 `/dataset/{files,logs,configuration,retrieval,knowledge-graph,compilation}/{id}`
  六个详情路由（分别 21/21/30/27/16/3 个交互元素），全量 UI map 与增量报告同步更新。
- wiki/tree/structure 生成后端与深层组件（版本历史、进度、导航树、编辑器）列入下一轮队列；
  Generate 在无 chunk/pipeline 时与上游一致保持禁用。
- 台账：dataset/compilation 5 个已实现文件分类（index/constants/llm-wiki-view/empty-state → partial，
  wiki-generation-eligibility → full）；unmapped 472 → 467、partial 38 → 42，`--check` 通过。
- 真点探针 内部 UI 遍历脚本（`dataset-artifacts-tab-cdp.mjs`（sidebar Artifacts 项/隐藏 knowledge-graph、
  面板视图模式/空态/门控、Back 返回 files）**PASS**；全量 `cargo test --locked --offline --lib` **1638 passed / 0 failed / 27 ignored**（首跑出现一次 oauth 并行竞态偶发失败，单跑与复跑均通过）；`cargo fmt --check` 干净。

## [0.4.58] — 2026-10-01（对标批次 v0.3.10c）

> 补齐 v0.27.2 唯一缺失的路由 `/compilation-templates/edit-next`（含 `/:id`），
> 并在真浏览器里验证创建 / 编辑回填 / 删除清理全链路。

### Added

- **知识编译模板编辑页**（Rust SSR + `/api/v1/compilation_template_groups`）：无全局壳层的
  Back + 标题头、名称 / 描述 / 模板类型（9 个内置 kind，artifacts 显示为 Wiki）/ 全局规则 /
  「重新切分 Parser 输出」开关与重分片规则、Back/Save 页脚；切换模板类型时按内置模板
  套用默认 config；`/:id` 编辑模式从 API 回填。
- **`/agents` 入口区**：Knowledge compilation 区块列出模板分组卡片（点击进编辑、删除经确认），
  「Add template」跳转编辑页。

### Notes

- 真浏览器（CDP，Chrome）探针 内部 UI 遍历脚本（`compilation-template-edit-cdp.mjs`：
  登录 → 创建（kind/global_rules/rechunk/rechunk_rules 持久化）→ 编辑回填 → 删除清理 →
  `/agents` 入口 Add/卡片/删除全链路 **PASS**。
- 台账：edit-next 的 11 个核心文件由 `unmapped` 升为 `partial`（外壳/表单/保存/回填已实现，
  Blueprint/分节字段/预览弹层/树模板等深层组件未复刻），unmapped 483 → 472、partial 27 → 38；
  `--check` 通过。
- 全量 `cargo test --locked --offline --lib` **1638 passed / 0 failed / 27 ignored**；`cargo fmt --check` 干净。

## [0.4.57] — 2026-10-01（对标批次 v0.3.10b）

> 本轮把模型提供商目录与 Available-models 网格同步到 RAGFlow v0.27.2，
> 并在真浏览器里完成 v0.27.2 的 34 路由 UI 遍历。

### Added

- **模型提供商目录同步 v0.27.2**：`src/data/providers/llm_factories.json` 重新生成为
  **73 提供商 / 1008 模型**；`src/api/tenant_models.rs` 内嵌的 `RAGFLOW_LLM_FACTORIES_JSON`
  （Available 网格与工厂回退的数据源）重新生成为 **74 工厂 / 1009 行**。新增提供商：
  FunASR、GreenPT、Hubris、MWS、Mistral OCR、Synthorai、aimlapi.com、llmman。
- **同步工具 内部辅助脚本**：从固定参考树一次生成两份产物，
  `--check` 可在 v0.26.4 快照上逐字段回归（两份产物都完全一致）；已有模型保留其类型，
  新模型按"含 chat 取 chat、否则取首类型"补齐，多类型列表原样保留。

### Fixed

- Available-models 网格与上游 v0.27.2 的差异清零：**71 张卡片全部渲染**（此前缺 8 张），
  卡片顺序、能力标签顺序、API key 文档链接、SystemSetting 行标签全部一致
  （`provider-catalog-diff.py --reference ~/.cache/ragflow-v0.27.2`：**PASS**）。

### Notes

- v0.27.2 UI 遍历（真点，**34/34 路由**）刷新 内部 UI 遍历记录（ragflow-ui-map.json`，
  增量报告 内部 UI 遍历记录（ragflow-ui-map-delta-v0.27.2.md`；v0.27 新增路由
  `/home`、`/chats/share`、`/chats/widget`、`/search/share`、`/agent`、`/agent/share`、
  `/compilation-templates/edit-next`、`/dataflow-result`。
- 台账：59 行 `conf/models` 复核关闭（unmapped 492 → 483，full 559 → 568），`--check` 通过。
- 全量 `cargo test --locked --offline --lib` **1636 passed / 0 failed / 27 ignored**；
  `cargo fmt --check` 干净。

## [0.4.56] — 2026-09-30（对标批次 v0.3.10a）

> 本轮把对标基线从 RAGFlow **v0.26.4** 升级到 **v0.27.2**（tag 提交 `a024bea0cd93`），
> 覆盖率门禁获得一次性基线迁移能力：旧台账 4441 行 → 新台账 5977 行。

### Added

- **对标基线升级到 RAGFlow v0.27.2**（固定提交 `a024bea0cd93f39e6652a42bf84dd20c55bc560b`）：
  上游新增 1845 个文件、删除 309 个，4132 个既有文件中 2222 个内容发生变化。新台账
  `内部对标台账`（v0.26.4 台账归档保留）。
- **覆盖率门禁支持 `--migrate`**：按路径继承旧状态；内容变化且已审计的行标注
  `[v0.27.2 content changed: re-audit]`（本轮 515 行）；上游新增文件按既有引用规则归档
  （内部 Go / 测试 / 文档 / 仓库自动化，共 1353 个），其余 492 个产品文件标为 `unmapped`
  等待逐文件审计；同时生成增量清单 `内部对标台账`。
- 重新生成逐文件工作清单（Python/TS/TSX 共 2722 个源文件）与文件指纹清单（5976 个文件）。

### Notes

- 门禁复核：`--check` 通过（5977 文件；`unmapped=492` 是下一阶段对标队列）；工具单测 6/6。
- 全量 `cargo test --locked --offline --lib` **1636 passed / 0 failed / 27 ignored**。
- 工作树内的 clippy 建议清理（needless borrow / `Ok(..?)` / 可折叠 if-let 等，15 个文件，
  行为等价）已并入本轮验证；`cargo fmt` 干净。

## [0.4.55] — 2026-09-24（对标批次 v0.3.9g）

> 承接上一版：把五个连接器常量（SeaFile / S3 / Bitbucket / Confluence / Jira）**逐字段**与上游重新核对
> 一遍，补齐漏掉的 tooltip 与一个真实的交互缺口（`allowCustomValue`），并把台账里这 5 行标为 `aligned`。

### Added

- **S3 地区选择器支持自定义值**（上游 `allowCustomValue: true`）：此前是写死的 35 项 `<select>`，
  AWS 新地区只能等 RayRAG 更新列表。现在渲染为 combobox（`role=combobox`、`aria-autocomplete=list`、
  隐藏的 `<datalist>` 提供 35 条建议、输入即自由文本），输入的新地区**原样入库**（真点验证
  `ap-southeast-9` 保存后读回仍是它）。新增与详情两个对话框都支持；schema 用 `allow_custom` 标记，
  目前仅 S3 地区使用（回归断言「只有它开了这个开关」）。
- **补齐 9 条缺失的 tooltip**：S3 的 `prefix` / `Role ARN` / `addressing_style` / `endpoint_url`，
  SeaFile 的 `seafile_token` / `repo_token` / `repo_id` / `sync_path` / `include_shared`。
  文案取自上游 `locales/en.ts`（这些连接器键上游没有 zh 词条，中文界面同样回退英文，与上游 i18next
  `fallbackLng` 行为一致），以悬停浮层呈现。

### Fixed

- **Segmented 控件不再自动选中第一项**：上游的 `Segmented` 没有默认值时保持未选中，RayRAG 之前会
  默认选中第一项——S3 的 `bucket_type` 首项是 `s3_compatible`，于是「没动过模式」的用户会被判成
  S3 兼容模式并索要 Access Key。现与上游一致地留空，并且规则里按上游 `config.bucket_type || 's3'`
  的写法把未选中视为普通 S3（新增回归覆盖「未选模式 = 普通 s3」与「有 key 无地区仍然被拒」）。

### Notes

- 台账：五个连接器常量由 `partial` 升为 `aligned`（partial 44 行 → 44-5，aligned 277 → 282），
  `--write` 刷新元数据、`--check` 通过（unmapped=0）。
- 新增回归 2 条（tooltip/自定义值开关、未选模式的 S3 语义）；全量 `cargo test --locked --offline --lib`
  **1636 passed / 0 failed / 27 ignored**。
- CDP 真点（内部 UI 遍历脚本（`data-source-form-cdp.mjs`，本版扩展）：SeaFile 全流程照旧；
  新增 S3 段——真点 S3 卡片 → 地区字段是 `role=combobox` 且带 35 条建议 → 悬停 `Prefix` 提示浮层显示 →
  手输 `ap-southeast-9` → 保存后从 API 读回仍是 `ap-southeast-9` → 探针结束前删除该数据源。

## [0.4.54] — 2026-09-24（对标批次 v0.3.9f）

> 对标 `pages/user-setting/data-source/constant/{seafile,bitbucket,confluence,jira,s3}-constant.tsx`
> 的 `customValidate` / `shouldRender` / `FormFieldType.Custom`：数据源对话框此前只做「必填」检查，
> 上游那些**跨字段**规则、随选项变化的说明面板和悬停提示都没有落地。

### Added

- **悬停提示（3 类源共 3 处 + 全源通用）**：上游 `Tooltip` 是鼠标悬停/聚焦才出现的浮层；RayRAG 之前
  把 tooltip 文案当常驻小字段落显示。现在每个带 tooltip 的字段渲染 `?` 触发器
  （`.ds-tip-trigger`，`tabindex=0`、`aria-label`、`role=button`）与 `role="tooltip"` 浮层，
  `mouseenter`/`focus` 显示、`mouseleave`/`blur`/`Escape` 隐藏；新增/详情两个对话框同一套实现。
- **随选项变化的说明面板**（上游 `FormFieldType.Custom`）：
  - SeaFile：`account` 显示「Syncs all libraries visible to the Account API Token below.」；
    `library`/`directory` 显示「Provide one of these authentication methods: …」（标题 + 两条要点）；
  - S3：`assume_role` + `bucket_type=s3` 显示「No credentials required. Uses the default environment role.」；
  - Confluence：`index_mode=everything` 显示「This choice will index all pages the provided credentials
    have access to.」；Bitbucket：`index_mode=workspace` 显示「This connector will index all repositories
    in the workspace.」
  面板文本随 schema 下发（首行为标题、其余为要点），并按 `shouldRender` 同样的条件显隐；面板不进入提交载荷。
- **跨字段校验规则（`customValidate`）**：规则作为数据随 schema 下发，对话框与 API 用同一套：
  - SeaFile：`account` 必须有账号 Token；`library`/`directory` 需要 **账号 Token 或库 Token 之一**、
    必须有 Library ID；`directory` 还必须有目录路径；未选 scope 时按上游 `?? 'account'` 处理；
  - S3：有 key 时（`bucket_type=s3`）地区必填、`s3_compatible` 或 `access_key` 模式必须给全 Access Key
    ID 与 Secret、`iam_role` 模式必须给 Role ARN（**上游该字段的报错文案误写成 "AWS Secret Access Key
    is required"（`s3-constant.tsx` 复制粘贴），RayRAG 改为点名 Role ARN**，并在台账注明不复刻该笔误）；
  - Confluence：`page`/`space` 模式分别要求 Page ID / Space Key；Bitbucket：`repositories`/`projects`
    模式分别要求 Repository Slugs / Projects；Jira：云端要邮箱 + API Token，Server 要用户名 + 密码。
  规则的 `when` 支持「多值任一（`any_of`）」「非空其一（`any_filled`）」「未设置时的默认值（`default`）」
  与「或条件组（`when_any`）」，正好覆盖上游 `||` / `&&` / `??` 的写法。
- **API 侧同规则兜底**：`create/update` 数据源时先跑同一套规则，不通过就 400 并回上游原句；
  绕过网页的调用方也无法存下一个连接器用不了的配置。
- **错误体改成 RAGFlow 形状 `{code, message}`**：这两个端点此前返回纯文本，浏览器按 JSON 解析会报
  「Unexpected token」——正是「出错却看不到提示」的那类问题；现改为 JSON（HTTP 状态码不变）。

### Notes

- 新增回归 4 条：SeaFile 各 scope 的规则（含「任一 Token」与默认 scope）、S3/Confluence/Bitbucket/Jira
  的条件规则、说明面板的顺序与显隐、对话框渲染 + API 兜底（含 `{code, message}` 断言）。
- 全量 `cargo test --locked --offline --lib` **1635 passed / 0 failed / 27 ignored**。
- CDP 真点（内部 UI 遍历脚本（`data-source-form-cdp.mjs`，新脚本）：真点 SeaFile 卡片的 `+ Add` →
  账号 scope 面板/字段/3 个提示触发器 → 悬停显示、移开隐藏、`role=tooltip` → 切 `library` 后换成
  Token 面板并出现 Library Token / Library ID → 缺 Token 时用上游原句拦下且**未发出请求** →
  绕过网页直接 POST 同样被 400 拒绝并回同一句 → 填全后保存成功、探针结束前删除。

## [0.4.53] — 2026-09-24（对标批次 v0.3.9e）

### Added

- **引导页预填默认值，用户只需确认**（`src/api/setup.rs::field_defaults`）：凡是代码里有默认值的项，
  页面直接给出该值并打 `default` 徽标（现取自 `cmd_timeout` 的各个常量、`pdf_stream` 的解压上限、
  向量后端/维度/目录、上传上限、注册开关等 14 项），用户看一眼点保存即可；**没有安全默认值的项
  （模型端点、密钥、数据库连接串、搜索地址）保持空白**——猜一个地址写进去比留空更糟。
  已经由环境变量或 `.env` 设定的项照旧显示实际值并标 `env`/`from file`，不会被误标为默认值。
- **引导页与 `.env` 动态同步**：新增 `GET /api/v1/setup/watch?fingerprint=`，页面每 5 秒比对
  环境文件的指纹（大小 + mtime + 内容 FNV-1a，同内容重写不算变化，只 stat + 哈希一个小文件，
  不返回内容）。文件被外部修改时：
  - **没有未保存修改** → 自动重新加载并显示「已从环境文件重新加载」提示（提示会自行消失）；
  - **有未保存修改** → 显示警示条「环境文件已在页面外被修改，重新加载会丢弃未保存的修改」并给出
    `Reload` 按钮，**绝不静默丢弃用户输入**；
  - 保存成功后页面用响应里的新指纹刷新基线，自己写的那次不会被当成外部修改。

### Notes

- 新增回归 2 条：`every_field_with_a_code_default_is_prefilled_and_labelled`（有默认值的项必须与代码
  常量逐一对齐、端点类必须没有默认值）、`the_file_fingerprint_notices_a_real_change_and_ignores_a_rewrite`
  （同内容重写不算变化、内容变化必定被发现、文件不存在时如实报告）。页面回归补充预填徽标、
  同步条与轮询代码的断言。
- CDP 真点（内部 UI 遍历脚本（`setup-wizard-cdp.mjs`，已扩展）：真点确认预填值（7200 / 33554432 /
  67108864 / 384 / zvec）、直接改写 `.env` 后页面在数秒内自动跟进新值、有未保存输入时出现警示条且
  输入保留、点 `Reload` 后才采用文件值；探针结束时把 `.env` 原样写回（实测与跑前逐字节一致）。
- 全量 `cargo test --locked --offline --lib` **1631 passed / 0 failed / 27 ignored**。

## [0.4.52] — 2026-09-24（对标批次 v0.3.9d）

### Fixed

- **`RERANK_MODEL` 此前是静默失效的参数**：README 与部署文档都让用户设置它，代码却从未读取
  （`llm.rs::rerank_model_for` 里写着 `let _ = model;`，`RemoteReranker` 的请求体也只有
  `query`/`documents`/`top_k`）。结果是：用户改了参数没有任何提示，多模型端点（TEI 多模型、
  vLLM、Xinference）直接报错，而错误信息无法让人联想到配置。现在 `RERANK_MODEL`
  一路打通：`RerankerConfig.model`（含 `reranker.json` 持久化，`#[serde(default)]` 兼容旧文件）
  → `RemoteReranker::with_model` → 请求体里的 `model` 字段；提供商实例的 rerank 模型也不再被丢弃。
  **未设置时不发送该字段**，单模型端点（不认识未知 model 名的服务）照旧可用。
- 回归 `the_request_names_the_model_only_when_one_is_configured` 用真实 HTTP mock 断言两种请求体，
  并确认空白值不算模型。

### Added

- **首次登录引导页补齐剩余的可配置项**（`src/api/setup.rs` 字段表 + `web::setup_page`）：
  - 模型组：`RERANK_API_BASE` / `RERANK_API_KEY` / `RERANK_MODEL`、
    `RAYRAG_ASR_API_BASE` / `RAYRAG_ASR_API_KEY` / `RAYRAG_ASR_MODEL`；
  - 新增 **检索** 组（面向中国大陆网络）：`SEARXNG_URL`、`TAVILY_API_KEY`、`BOCHA_API_KEY`
    —— 这三个都是**每次调用读取**，因此标 `live` 并进 `applies_live()`；
  - 新增 **访问控制** 组：`REGISTER_ENABLED`（1/0）、`DISABLE_PASSWORD_LOGIN`（0/1）、
    `RAYRAG_CORS_ORIGIN`、`RAYRAG_MAX_UPLOAD_BYTES`；
  - 分组标题中英化（模型 / 存储 / 资源 / 检索 / 访问控制），不再直接用英文分组键。
- **容器内存上限按「可复制」呈现**：新增 `FieldKind::Readonly` 与 `api::setup::is_readonly`，
  `RAYRAG_MEMORY_LIMIT` 只读展示（`RAYRAG_MEMORY_LIMIT=6g`，值来自进程环境，缺省用 compose 默认值），
  带复制按钮（`navigator.clipboard`，纯 http 局域网来源下回退到选中文本 + `execCommand`，
  再失败就提示手动 Ctrl+C），并说明它属于 `docker-compose.yml` 的 `mem_limit`；
  **写入被端点拒绝**（`belongs to the container definition and cannot be set here`），
  不会把一条没人读取的配置写进 env 文件。
- **保存后交接到模型提供商页**：`POST /api/v1/setup/complete` 返回
  `next:{needs_model_provider,url:'/user-setting/model'}`，页面在回执下方显示
  「继续添加模型提供商」按钮；尚未配置模型端点时同时给出提示句。首页的引导横幅也补了
  `Model providers` 入口，用户不必自己找路。
- 新增回归 8 条：字段表覆盖新开关与分组（含「Select 必须有选项」的守卫）、只读字段不可写且标注
  指向 compose、页面渲染只读行/复制按钮/交接按钮、rerank 请求体，以及本轮批量操作两条
  （`bulk_document_operations_follow_the_upstream_contract` 覆盖三个端点的校验/部分失败/删除边界，
  `dataset_files_bulk_bar_matches_upstream_operate_bar` 覆盖批量栏条目、对话框、三态全选与
  「状态来自服务端」），以及 env 文件位置四条（项目根优先、显式变量优先、已加载文件优先、
  绑定挂载时原地写回且不产生半截文件）。

### Added

- **数据集文件页的批量操作栏对齐上游**（`web::dataset_tab_page` 的 Files 标签页，参考
  `pages/dataset/dataset/index.tsx` + `components/bulk-operate-bar.tsx`）：勾选任意行即出现
  `role="menu"` 的批量栏，内容与上游一致 —— 已选数量（`Selected: N Files`，旁附清除图标）、
  分隔线，以及 **启用 / 禁用 / 解析 / 取消 / 元数据 / 删除** 六个条目（RayRAG 另加**重命名**，
  因为「批量重新命名」是用户明确要求的能力）。表格首列是复选框列：表头 `Select all`
  带三态（全选/半选/未选），行复选框 `Select row`。
  - **解析** 打开上游 `reparse-dialog.tsx` 的对话框：`Parse file` / `Are you sure to parse?`，
    勾选项按已选文档的 chunk 总数显示 `Do you want to clear the existing N chunks?`
    （无 chunk 时显示 `Clear existing chunks`），另有 `Apply global auto-metadata settings`。
  - **删除** 打开上游 `confirm-delete-dialog.tsx` 的确认框（`Delete files` /
    `Are you sure to delete them?` / `Selected N files`）；正在解析的文档按上游语义拒绝删除
    并给出原因。
  - **重命名** 对话框给出 查找 / 替换为 / 添加前缀 三个输入与逐条预览（未变化的行划掉），
    确认后按上游 PATCH 契约逐个改名并汇总失败原因。
  - 中英文案全部走 `t()` 表（新增 25 条，键名即上游 `en.ts` 原文）。
- **三个上游批量端点**（`src/api/document.rs`，此前只有逐文档端点，批量栏要发 N 个请求）：
  - `POST /api/v1/datasets/{id}/documents/batch-update-status`（`{doc_ids, status}`，
    逐文档结果映射，部分失败返回 500 `Partial failure`）；
  - `DELETE /api/v1/datasets/{id}/documents`（`{ids}` 或 `{delete_all}`，两者同时给出或都不给
    都按上游原文报错；不属于该数据集的 id 直接拒绝而不是静默跳过）；
  - `POST /api/v1/documents/ingest`（`{doc_ids, run:1|2, delete?, apply_kb?}`：`run=1` 入队解析、
    `run=2` 取消并标记 `CANCELLED`；`delete` 即对话框里的「清空已有 chunk」）。
- **文档启用/禁用改为服务端状态**（`DocRecord.status`，`#[serde(default)]` 兼容旧文件）：
  此前这个开关只写在浏览器 `localStorage`，禁用一份文档对其它客户端、对检索毫无影响。
  现在 `status` 随文档持久化，`GET /datasets/{id}/documents` 每行返回它，
  **检索按上游语义只使用 `status == "1"` 的文档**（有禁用文档时才收窄查询，常见路径不变）。

- **`.env` 默认落在项目根目录**（`src/api/setup.rs::project_root`/`resolve_env_file`）：引导页写入的
  就是用户能找到、能编辑、能备份的那一个文件。解析顺序为 `RAYRAG_ENV_FILE` → 启动时实际读取的文件
  → 项目根目录 `.env`（含 `docker-compose.yml`/`Cargo.toml`/`.env.example` 的目录，从工作目录、
  可执行文件目录与构建目录逐级上溯）→ 状态目录旁的旧位置；已存在「状态目录旁 .env」的部署保持原样，
  不会凭空多出第二个文件。回执里的路径也做了词法归一（`/app/web/.env` 而不是
  `/app/web/static/../.env`）。
- **容器内也能写宿主机的那一份**：`docker-compose.yml` 把 `./.env` 绑定挂载到 `/app/.env` 并显式设置
  `RAYRAG_ENV_FILE=/app/.env`（`install.sh` 保证首次启动前该文件已存在）。绑定挂载的文件**不能用
  rename 覆盖**（`Device or resource busy`），因此 `write_env_file` 在原子写失败时回退为「先写同目录
  临时文件，再原地覆盖」，失败时原文件保持不变。
- **引导页保存不再被只读项卡住**：页面不再回传只读字段（回传会被端点拒绝，实测把整次保存拦下）。
- **`DISABLE_PASSWORD_LOGIN` 选项与部署一致**：此前只列 `0/1`，而 `docker-compose.yml` 的默认值是
  `false`，于是下拉框显示为空、保存时校验失败（甚至可能把用户设置悄悄改成 `1`）。现列
  `false/true`，并接受解析器认可的全部拼写（大小写不敏感 `true/false/1/0/yes/no`）；选项表里没有的
  现值（例如手写的 `True`）会被追加进下拉框，保证「页面显示的就是保存回去的」。

### Notes

- 全量 `cargo test --locked --offline --lib` **1629 passed / 0 failed / 27 ignored**。
- CDP 真点（内部 UI 遍历脚本（`setup-wizard-cdp.mjs`）：五组卡片齐全、只读行是 `<code>`
  且无输入框、点复制有反馈、直接 POST 写 `RAYRAG_MEMORY_LIMIT` 被拒、保存回执区分
  「立即生效 / 需重启」、交接按钮指向 `/user-setting/model`、重开页面值已持久化。

## [0.4.51] — 2026-09-24（对标批次 v0.3.9c）

### Added

- **公共模型目录接入**（`https://models.agent-one.dev/list`，新模块 `src/model_catalog.rs`）：
  填自定义提供商/模型时不必再凭记忆回答「这个模型支持工具调用吗、上下文多大、多少钱一百万 token」。
  该页面是 Next.js 应用，数据藏在 React Server Component 的
  `self.__next_f.push([1, "…"])` 分片里，因此目录是从**页面载荷**里解析出来的：拼接分片 →
  按括号配对切出 `"directory":{…}` 对象 → serde 反序列化（当前 75 个提供商；每个模型带
  `features{attachment,reasoning,tool_call,structured_output}`、`pricing{input,output}`、
  `limit{context}`、`modalities{input,output}`）。解析**严格失败**：形状变了就报
  `catalogue page carried no RSC payload` / `catalogue payload had no 'directory' object`，
  绝不静默返回空目录。
  - **按需、有缓存的取用**：内存 → 磁盘（`<状态目录>/model_catalog.json`，临时文件 + rename 原子写）
    → 网络，TTL 24 小时；刷新失败**保留旧副本**并记录原因，离线部署仍可查询。页面读取上限
    `MAX_CATALOG_PAGE_BYTES` 32 MiB，走统一超时客户端与有界读取。
  - **排序照顾「你正在配的那个端点」**：先给与所填 Base-URL 同 host 的提供商的精确命中，其次是该端点**自己提供的模型**
    （按与输入名的公共前缀排序，近似拼错的名字排在字母序邻居之前），最后才是别的提供商的同名模型——
    避免在 `api.deepseek.com` 上把 `302.AI` 的 `deepseek-chat` 当成 DeepSeek 的价格套上去。
  - 端点：`GET /api/v1/model-catalog/status`（只报缓存，不触发网络）、
    `GET /api/v1/model-catalog/providers?provider=|base_url=`、
    `GET /api/v1/model-catalog/lookup?model=&base_url=`、
    `GET /api/v1/model-catalog/search?q=&base_url=`、
    `POST /api/v1/model-catalog/refresh`（管理员）。查不到是**空列表**而不是错误；目录不可达也返回
    `code 0` + 原因，外部服务失联不会让对话框瘫痪。
- **对话框里的真实查询按钮**（`web::providers_page`）：
  - 自定义模型对话框新增 `Look up model info` / `查询模型信息`：用已填的模型名 + 当前 Base-URL 查询，
    结果面板逐条给出模型名、提供商、上下文长度、能力标签、`$/M in · $/M out`、端点与官网；
    **只填空着的字段**（模型类型、Max tokens、Tool call），要覆盖手填值必须点 `Use these values` /
    `使用这些信息`；打开与关闭对话框都会清空上一次的结果。
  - `+ Add Provider` 入口此前只有 `showAddProv()` 函数、页面没有按钮（不可达代码）；现补上入口与
    `Look up provider` / `查询提供商`：填好 API Base 点一次即识别提供商、回填 ID 与名称，
    并把目录中该提供商的模型名灌进 `apModels` 的候选列表。
  - 目录相关 20 个文案键进入 `t()` 中英对照表（上下文长度 / 价格 / 工具调用 / 视觉 / 结构化输出 …）。

### Fixed

- **`json!` 宏递归上限**：把目录标签继续塞进原本那张 picker 标签表会触发
  `recursion limit reached while expanding json_internal!`；目录文案改为独立表，再用
  `as_object_mut().extend()` 合并（顺带说明为什么不能合成一张表）。

### Notes

- 新增回归 8 条：`model_catalog` 6 条（RSC 解析与能力/定价、Base-URL 优先与容忍匹配、
  已知端点的自有模型回退与排序、失败信息、落盘往返与 TTL）、`server` 1 条
  （lookup / provider 目录 / 空结果 / 缺参 400）、`web` 1 条（页面标记、中英文案与 CSS）。
  全量 `cargo test --locked --offline --lib` **1621 passed / 0 failed / 27 ignored**。
- 真点验证：内部 UI 遍历脚本（`model-catalog-lookup-cdp.mjs`（登录 → 目录状态 → lookup API →
  真点按钮填字段 → 覆盖按钮 → 取消后清空 → Add Provider 识别提供商）。

## [0.4.50] — 2026-09-24（对标批次 v0.3.9b）

### Added

- **首次登录引导页 `/setup`**（`src/api/setup.rs` + `web::setup_page`）：环境变量仍是唯一事实来源，
  但不再要求用户自己去编辑 `.env`。页面按 模型 / 存储 / 资源 三组列出可配置项（嵌入与对话端点、
  向量后端与目录、PostgreSQL、并发、超时、各类上限、OCR），每项带当前值、说明与本机推荐值；
  保存后写入**进程实际读取的那个环境文件**（启动时记住 `RAYRAG_ENV_FILE` → `./.env` →
  项目 `.env`，都没有则在状态目录旁新建），并原子写回（保留注释与其它键）。
  - **如实标注生效范围**：每项标 `live`（每次调用读取，立即生效：超时、各类包体/解压上限、
    OCR 客户端）或提示需要重启（嵌入/对话客户端在启动时构建、向量后端/维度/并发/数据库在启动时决定）。
    这一致性由回归测试守着：字段表与 `applies_live()` 必须逐项一致 —— 写这一版时它当场抓出我
    把 `RAYRAG_MODEL_TIMEOUT` 与 `EMBED_*`/`LLM_*` 错标成 live 的问题。
  - **优先级不藏着**：进程环境里已存在的变量优先于文件（加载器语义如此），因此这类字段额外打
    `env` 徽标并说明「环境变量优先于此处写入的文件」。
  - 新的公开端点 `GET /api/v1/setup/status`（未登录也可访问，供登录页/首页判断），
    受会话保护的 `GET /api/v1/setup/options`、`POST /api/v1/setup/complete`；
    未配置任何模型端点的部署会在首页出现一次性引导横幅。
- **主机资源判定进入 REST**：`GET /api/v1/system/status` 增加 `host` 段（CPU / 可用内存 /
  自动推导的入库并发），与启动日志一致。

### Notes

- 新增回归 6 条：`api::setup` 的 3 条（env 文件写回保留注释与未知键、取值校验拒绝无法兑现的值、
  live/重启标注与实现一致）、`web` 的 2 条（引导页字段与徽标齐全、首页横幅只在未配置时出现）、
  以及既有 `host_resources` 覆盖。
- 全量 `cargo test --locked --offline --lib` **1613 passed / 0 failed / 27 ignored**。
- 挂账：引导页目前覆盖环境变量类设置；模型实例（provider/instance/model）仍走既有的
  「模型提供商」页面，下一步把两者串成一条首登流程（配好端点后直接进入添加模型）。

## [0.4.49] — 2026-09-24（对标批次 v0.3.9a）

> 把上一版的「解压上限」补到 PDF：PDF 的流同样可以「几 KB 输入、几 GB 输出」，
> 而 `lopdf` 的解压 API 是无上限的。

### Fixed

- **PDF 流解压上限（`src/parser/pdf_stream.rs`，新模块）**：`lopdf` 的
  `decompressed_content()`（`get_page_content` / `get_and_decode_page_content` /
  `get_plain_content` 背后）会把流展开成多大就缓冲多大 —— 一个 256 KiB 的 PDF 可以声明
  256 MiB 的页面内容。新模块自带**有上限**的解码：`FlateDecode` 用
  `ZlibDecoder::take(limit + 1)` 边解边停，`ASCIIHexDecode` / `ASCII85Decode` 同样封顶，
  并实现 PNG/TIFF predictor（`DecodeParms`）重建；对无法在限额内解码的过滤器（LZW 等）
  **明确报错**而不是交给无界解码器。上限 `RAYRAG_PDF_STREAM_LIMIT_BYTES`（默认 64 MiB）。
  接入点：`pdf.rs`（文本路径与 OCR 探测）、`pdf_text.rs`（定位文本路径 + `/ToUnicode` CMap）、
  `mineru.rs`、`docling.rs` —— 全仓已无 `get_and_decode_page_content` / `get_plain_content` 调用。
- **被拒绝的页面必须看得见**：解析器对每一页的拒绝都打 `WARN`（带页码与原因）；若整篇
  **一个字都没解出来且确有页面被拒**，解析直接失败并给出原因（
  `PDF content could not be decoded: 1 page stream(s) exceeded the 64 MiB safety limit`），
  而不是显示「DONE、0 chunks」把问题藏起来。
- **补齐最后两处下载路径**：`data_source/blob.rs`、`data_source/gitlab.rs` 的整包下载接入连接器上限；
  `llm.rs::raw_post_bytes`（TTS 音频）接入 API 包体上限。至此生产代码中已无无界的
  `resp.bytes()` 读取（余下三处均在 `#[cfg(test)]` 的 mock 服务里）。

### Notes

- 新增回归 5 条（`parser::pdf_stream::tests`）：64 MiB 级 flate 炸弹被拒（压缩后 <200 KiB）、
  普通 flate 流完整解出、无过滤器流直通、PNG predictor 行重建（含 Sub/Up/Average/Paeth 的
  基础用例）、非常规过滤器给出明确报错。
- **实测**：现场构造 256 KiB 的 PDF（页面流展开 256 MiB）——CLI 下给出
  `Error: PDF content could not be decoded: 1 page stream(s) exceeded the 64 MiB safety limit`
  且内存平稳；正常单页 PDF 仍正确解出 `normal pdf text`。
- 全量 `cargo test --locked --offline --lib` **1609 passed / 0 failed / 27 ignored**。

## [0.4.48] — 2026-09-24（对标批次 v0.3.8z）

> 继续「不许有任何一条无界的内存路径」：这一版堵住**外部对象下载**和**压缩文档解压**两类，
> 并把最后 14 处无超时的 HTTP 客户端全部收口。

### Fixed

- **Office/EPUB 压缩成员解压上限（zip bomb）**（`src/parser/mod.rs::read_member_limited`）：
  docx/xlsx/pptx/epub 本质是 ZIP，成员头可以声明任意大小；`docx.rs` / `ppt.rs` / `epub.rs` /
  `tcadp.rs` 之前用 `read_to_end` 无上限读取，`docx.rs` 甚至还按**声明大小** `Vec::with_capacity`
  预留。现在统一走 `read_member_limited`：先按声明大小预检（文本 64 MiB / 媒体 128 MiB），再用
  `take(limit + 1)` 兜住「声明撒谎」的情况，最后复核实际长度。`docx.rs` 的成员读取改为返回
  `Result<Option<_>>`，把「存在但超限」与「不存在」区分开（不再报成 `word/document.xml missing`），
  `ppt.rs` 的超限幻灯片同样**显式报错**而不是静默跳过。
  **实测**：现场构造的 200 KiB docx（`word/document.xml` 解压后 200 MiB）现在立即返回
  `DOCX part 'word/document.xml' declares 209715384 bytes, above the 64 MiB safety limit`；
  正常 docx/xlsx/pptx/md 仍照常解析。
- **连接器下载有上限**（`RAYRAG_CONNECTOR_BODY_LIMIT_BYTES`，默认 256 MiB）：S3/WebDAV/HTTP/RSS 等
  9 处 `.bytes()` 全量下载（下游是待解析文档，所以上限比 API 响应宽松）改走有界读取，远端对象
  再大也不会把进程撑爆，超限时明确报错。
- **最后 14 处无超时客户端收口**：`llm_enhanced`、`translate`×3、`mcp_client`、`parser/somark`、
  `connectors`、`channels`×3、`main.rs` 的裸 `reqwest::Client::new()` 全部改用带超时的共享客户端
  （模型类 300s / 连接 10s）；全仓除 `common.rs` 内部的两处 builder 回退外，已无无超时客户端。

### Notes

- 新增回归 3 条（`parser::zip_member_limit_tests`）：声明超限**在读之前**被拒（不发生分配）、
  声明撒谎时按实际累计量截断、限额内成员完整返回。
- 全量 `cargo test --locked --offline --lib` **1604 passed / 0 failed / 27 ignored**。

## [0.4.47] — 2026-09-24（对标批次 v0.3.8y）

> 承接上一版的「不许再有失控内存」：这一版把**所有对外 HTTP 读取**和**索引提交流程**也纳入
> 有界、按需的轨道，并让部署**按主机资源自动决定并发**。

### Added

- **主机资源自动判定与并发自适应**（`src/host_resources.rs`，新模块）：启动时读取 CPU 数与
  `/proc/meminfo` 的 `MemAvailable`，据「半个 CPU 核数」与「可用内存的 3/4 ÷ 单文档最坏占用
  （512 MiB）」取小值，钳制在 1..=8，作为文档入库并发（`RAYRAG_MAX_CONCURRENT_TASKS` 显式设置时
  仍然优先）。启动日志打印判定结果，`GET /api/v1/system/status` 增加 `host` 段
  （`cpus` / `available_memory_bytes` / `document_tasks`），控制台可以直接看到本机的实际取值。
  小主机不再跑工作站默认值，大主机也不被固定值卡住。
- **统一的外呼超时与包体上限**：`RAYRAG_MODEL_TIMEOUT`（模型类调用，默认 300 秒，钳制 ≤1800）与
  `RAYRAG_HTTP_BODY_LIMIT_BYTES`（单次响应体，默认 32 MiB）；新增
  `common::cmd_timeout::{read_body_limited,read_json_limited,read_text_limited}`，先按
  `Content-Length` 预检、再按块累积并对总量封顶。

### Fixed

- **Embedding / 视觉 / OCR / 爬虫 / 存储读取不再无界**：这些客户端此前用 `reqwest::Client::new()`
  （**完全没有超时**）并直接 `.json()` / `.text()` / `.bytes()` 把响应体整个缓存。现在统一走
  模型超时客户端 + 有界读取：对端无论返回无限流、谎报长度还是分块传输，进程持有的内存都有上限。
  `src/crawler.rs` 里那句「限制响应体大小（防 OOM）」过去只是注释，现在真的执行。
- **一次文档提交不再克隆三份索引**：`persist_online_index` / `rollback_online_index` /
  `persist_index_change` / `restore_index` 全部改为**借用**快照（新增 `SearchEngine::chunks()`），
  只在真正回滚时才复制。此前每次提交都持有「前像 + 前像的克隆 + 当前全量克隆」三份含 embedding
  的索引；语料越大，这个瞬时放大越明显。

### Notes

- 新增回归：`host_resources` 三条（`/proc/meminfo` 解析、按 CPU/内存推导并发并钳制、
  本机探测）、`common::cmd_timeout_tests::bounded_reads_stop_at_the_limit_instead_of_following_the_peer`
  （声明长度超限直接拒绝；**无长度无限流按累计量截断**；非 JSON 报错而不是 panic）。
- 全量 `cargo test --locked --offline --lib` **1601 passed / 0 failed / 27 ignored**。

## [0.4.46] — 2026-09-23（对标批次 v0.3.8x）

> 本版是一次**内存安全**修复：RayRAG 曾把整台机器的内存吃光导致主机卡死，这一版从启动路径、
> 集合回收、索引同步和文件响应四个方向把内存占用变回有界、按需。

### Fixed

- **Markdown 解析器的死循环（主机卡死的直接原因）**（`src/parser/markdown.rs`）：逐行遍历用
  `sections.last().map(|s| s.end_line + 1)` 取下一个位置，而 `include_meta == false` 时入栈的元素
  被改写成 `end_line = 0` —— 于是 `i` 永远回到 1，**任何多行的 Markdown**（列表 / 围栏代码 / 表格）
  都会无限把段落压进结果里，内存以每秒上百 MB 增长，直到把整台机器吃光。上游用的是
  `element["end_line"] + 1`（`deepdoc/parser/markdown_parser.py:385`），现按上游改为用刚抽取出的
  元素自身位置推进；`push_section` 也不再改写行号。复现与验证：同一个 34 字节表格文件，
  `.txt` 正常、`.md` 在 3 GB 上限下 OOM；修复后 4 个 Markdown 样本（列表/代码/表格/混合）在 2 GB
  上限下毫秒级完成。
- **启动不再重写全部向量集合**（`src/store/mod.rs`）：旧实现每次启动都把整个索引克隆两份
  （含 embedding）再做 `delete_all` + 全量 `upsert` + `flush`，把 **每一个** 集合重写一遍 ——
  部署实例上就是每次启动重写 1.3 GB 原生集合，单次启动在这段里跑了数分钟、内存涨到数 GB。
  现在启动只做一件事：把索引不再引用的集合目录**直接从磁盘回收**（不打开、不加载），
  漂移检查与修复统统移出启动路径。
- **修复改为后台、按需、逐个进行**（`server::start_mirror_repair`）：启动 30 秒后开始，每 5 分钟
  一轮，**每轮只修一个 owner**，按 kb_id 排序，修完即释放；修不好的 owner 只告警一次并在本进程内
  不再重试（修复循环永远不会变成新的失控点）。
- **清空集合改为分页删除**（`ZvecCollection::clear`）：`delete_by_filter("position >= 0")` 会让
  原生层先把所有主键收集起来 —— 部署实例上单个 523 MB 集合的这一次调用就冲破了 6 GB 上限并被
  容器内 OOM 杀掉。现在用「只取主键的迭代器 + 每页 512 个 id 批量删除」，峰值内存与页大小成正比。
- **原生写缓冲加上限**（`RAYRAG_ZVEC_MAX_BUFFER_BYTES`，默认 64 MiB）：进程无法解释的内存不再无界增长。
- **索引同步不再克隆整库**（`sync_snapshot`）：分组改为借用（`owners_by_kb` 只存切片引用），
  一次提交只为「真正变化的 chunk」复制数据，而不是把 previous/current 两侧的全部 chunk
  各克隆一遍。文档越多、内存放大越明显的那个问题由此消失。
- **文件响应改为流式**（`api::common::stream_stored_file`）：预览/下载不再把整个文件读进内存
  （一个 2 GB 视频的预览曾经就是 2 GB 常驻内存），改为 64 KiB 分块按需读取；空文件仍回上游的
  `This file is empty.`。

### Added

- **容器内存上限**（`RAYRAG_MEMORY_LIMIT`，compose 默认 `6g`）：进程失控时由内核在容器内结束它，
  而不是把主机拖死。稳态占用只有几十 MB，因此这个上限对正常使用没有影响。

### Notes

- 新增回归：`online_native_mirror_reclaims_directories_the_index_dropped`（回收不再打开的目录、
  已打开的集合不动其存储）、`online_native_mirror_loads_chunks_only_for_drifted_collections`
  （与索引一致的集合一个 chunk 都不加载，只有漂移的 owner 才被读取与重写）、
  `native_collection_upserts_queries_and_reopens` 增加分页清空断言（38 行、页大小 5 全部删净）、
  `api::common::tests::stream_stored_file_delivers_large_files_and_reports_empty_ones`
  （跨多个分块的大文件字节一致 + 空文件/缺失文件的上游报文）。
- 实测（部署实例，`docker compose`）：修复前单次启动 **5.5 分钟 / 峰值 5.44 GB**；现在
  **约 9 秒 / 19 MiB**，后台修复轮次期间内存保持平稳（隔离复现中 17.9 MiB → 51 MiB 并回落）。

## [0.4.45] — 2026-09-23（对标批次 v0.3.8w）

### Added

- **文档预览器格式矩阵**（上游 `pages/document-viewer/index.tsx` +
  `components/document-preview/{index,txt-preview,csv-preview,image-preview,video-preview,md,document-header}.tsx`
  + `pages/document-viewer/file-error/index.tsx` + `components/new-document-link.tsx`）：文档页不再只认识 PDF。
  `ext` 决定预览器，`resource` 决定字节源（`document` → `/api/v1/documents/{id}/preview`，
  `files` → `/api/v1/files/{id}`），与上游**两张并不相同的分发表**逐条对齐：
  - **独立页**（`/document/{id}?ext=…&resource=…`）：图片（jpg/jpeg/png/gif/bmp/tif/tiff/webp/ico）、
    `md`/`mdx`、`txt`、`pdf`、`xlsx`/`xls`、`docx`、`ppt`/`pptx`，`ext=html` 把字节交给浏览器；
  - **共享分发器**（切片工作台 `/documents/{kb}/{doc}`）：pdf、`doc`/`docx`、txt、图片、
    **11 种视频**（mp4/avi/mov/mkv/wmv/flv/mpeg/mpg/asf/rm/rmvb）、ppt/pptx、**仅 `xlsx`**、`csv`、md/mdx。
- **文本类预览器**：`TxtPreviewer`（`<pre>` + 加载态）、`CSVFileViewer`（分隔符/引号/换行齐全的 CSV
  解析，表头 + 行表，空单元格显示 `-`）、`Md`（GFM 表格、任务列表、围栏代码、引用、粗斜体/删除线/
  链接/自动链接，`remark-breaks` 语义：段内单个换行即 `<br>`）、`ImagePreviewer`（Blob 对象 URL）、
  `VideoPreviewer`（`controls` + 80vh 滚动容器）。
- **`DocumentHeader`**（上游 `document-header.tsx`）：切片工作台预览器上方显示文档名 + `Size`
  （`formatBytes`：1024 进制、单位表 `bytes/KB/MB/…`、小于 10 才带一位小数）+ `Uploaded time`
  （`DD/MM/YYYY HH:mm:ss`）。
- **文件页 Eye 预览按钮**（上游 `action-cell.tsx` + `new-document-link.tsx`）：仅
  `SupportedPreviewDocumentTypes`（xlsx/xls/pdf/docx/md/mdx + 图片）显示，指向
  `/document/{id}?ext={ext}&resource=files` 并在新标签页打开 —— txt/csv/ppt 可预览但**不**显示该按钮，
  与上游一致。
- **上游端点**：`GET /api/v1/documents/{id}`（`document_api.py::download_document`，`attachment` +
  扩展名 MIME）与 `GET /api/v1/files/{id}`（`file_api.py::download`，`fetchPreviewBlob(..., 'files')`
  的字节源）。
- **表格/幻灯片读取端点**（RayRAG 侧 `replaced`，上游在浏览器用 `@js-preview/excel` / `pptx-preview`）：
  `GET /api/v1/documents/{id}/preview/sheets`（按工作表返回单元格网格，行/列上限并带 `truncated` 标记）、
  `GET /api/v1/documents/{id}/preview/slides`（逐页文本）。

### Fixed

- **`GET /api/v1/files/{id}` 的 MIME 一律 `application/octet-stream`**：`mime_from_extension` 取的是
  *文件名*，旧代码传裸扩展名（`"png"`）导致永远落空，图片/视频在预览器里拿不到正确类型。
- **预览器状态被脚本重置（静默失效）**：页面先赋值 `DP_STATE`，紧随其后的脚本又用
  `var DP_STATE={…}` 初始化，把页面赋值冲掉 —— 所有 fetch 预览器都不渲染且不报错。改为
  `var DP_STATE=window.DP_STATE||…`，并加回归断言锁住这次交接。
- CSV/video 在**独立页**没有分支：上游同样如此，RayRAG 现在渲染 `fileError` 红框而不是空白页。

### Notes

- 新增回归 6 条：`document_viewer_dispatches_upstream_previewer_tables`（两张分发表逐条 + 大小写/点前缀）、
  `document_header_formats_size_and_uploaded_time_like_upstream`（`formatBytes`/`formatDate` 边界）、
  `document_preview_sections_and_script_match_upstream`（面板标记 + 脚本钩子 + CSS）、
  `document_viewer_page_uses_resource_and_ext_query`（ext/resource/缺省回退/html/未知类型）、
  `document_page_selects_previewer_by_stored_type`（PDF 双栏保留、非 PDF 走分发器）、
  `files_page_preview_button_follows_supported_preview_types`；服务端 3 条：
  `document_download_route_serves_attachment_bytes`、`file_blob_route_serves_raw_bytes`、
  `preview_reader_routes_return_workbook_grid_and_deck_text`。
- 新增真点探针 内部 UI 遍历脚本（`document-preview-matrix-cdp.mjs`（14/14）：真实浏览器里上传
  txt/md/csv/png/xlsx/pptx/docx 后逐个打开，断言渲染结果与上游语义（含 `?ext=csv` 在独立页返回
  `fileError`、`resource=files` 走文件路由、文件页 Eye 链接只出现在受支持类型上）。
- PDF 分支的标记在这一批里被拆成 `document_toolbar_markup` / `pdf_viewer_markup` /
  `document_chunk_pane_markup` / `document_workbench_scripts` 四个部分，拼接结果与拆分前**逐字节相同**
  （已用脚本比对确认）。
- 挂账：docx 预览器（上游 `@extend-ai/react-docx`）、pptx 形状/主题/图片（当前只有逐页文本）、
  markdown 的 KaTeX/rehype 插件链。

## [0.4.44] — 2026-09-23（对标批次 v0.3.8v）

### Added

- **文档预览器（PDF 渲染 + 切片高亮）**（上游 `pages/document-viewer/index.tsx` +
  `components/document-preview/{index,pdf-preview,hooks}.tsx` + `components/pdf-drawer/index.tsx`）：文档页
  `/document/{id}` 从「只有切片文本」升级成上游那套**真预览**——左侧 pdf.js 逐页渲染画布，选中切片后把
  `positions` 逐框叠在页面上（悬停显示切片文本、点击固定），右侧仍是切片工作台，点行即定位。
  - **高亮几何与上游同源**：`buildChunkHighlights` 把 `positions` 当作**scale=1 视口**里的点坐标
    （`page, x0, x1, top, bottom`），react-pdf 再按缩放换算；RayRAG 用同样的换算手写叠加层
    （`left/top/width/height = position × (scale × fit)`），因此 `pdf-preview.tsx` 的
    `viewport.width/height`（`setWidthAndHeight`）在此对应 `dvFit()` 统计出的 scale-1 页宽。
  - **工具条**：上一页/下一页 + `1 / N` 页码、缩小/放大（`document-preview/hooks.ts` 的
    `ZOOM_STEPS = [25,50,75,100,125,150,175,200]`）、Fit（首屏按容器宽度自适应并像
    `useDocxPreviewZoom` 一样**上限 100%**）、下载、状态行；滚动时页码自动跟随。
  - **深链**：`?chunk=<id>` 直接定位并高亮该切片（对应上游 `PdfSheet` 从切片行打开预览的行为）。
- **上游预览端点** `GET /api/v1/documents/{id}/preview`（`useGetDocumentUrl` 的 document 分支）：
  仅凭文档 id 取原始字节，按扩展名给 MIME 并 `inline` 返回（数据集路由上的下载仍是 `attachment`）。
- 静态资源：随仓库内置 **pdf.js 5.6.205**（Apache-2.0）`pdf.min.mjs` + 配对 `pdf.worker.min.mjs`
  （替换此前孤立的 2022 版 worker，主库与 worker 必须同版本）。

### Notes

- 新增回归：`web::kb_ui_tests::document_viewer_renders_pdf_with_chunk_highlight_overlay`（12 个 DOM 标记
  + 12 条脚本钩子 + 缩放步进表 + Fit 上限 + 5 条 CSS 选择器）、
  `server::tests::document_preview_serves_stored_bytes_inline`（MIME、`inline` 处置、字节内容、未知文档
  回 404 包体）。
- 新增真点探针 内部 UI 遍历脚本（`document-viewer-cdp.mjs`：现场生成两页 PDF（坐标已知）→ 建库/上传/
  解析 → 打开预览页 → 断言 pdf.js 渲染出两页非空画布、切片行的 `positions` 存在、点击后高亮框数量等于
  `positions` 条数且 `left/top` 等于 `positions × scale`、弹层有文本、选中行被标记、放大后画布与高亮框同步
  缩放、上下页翻页、`?chunk=` 深链直接高亮。
- 挂账：docx/xlsx/ppt/图片等非 PDF 预览（上游各有独立预览器）、切片编辑后的增量重绘、PDF 文本层选择复制。
## [0.4.43] — 2026-09-23（对标批次 v0.3.8u）

### Added

- **PDF 文本坐标与切片 positions**（上游 `deepdoc/parser/pdf_parser.py` 的 box/`_line_tag` 与
  `rag/flow/parser/pdf_chunk_metadata.py::extract_pdf_positions`）：PDF 解析不再只吐文本，而是像上游一样
  为每个文本块打上定位标签 `@@page\tx0\tx1\ttop\tbottom##`，切片因此带上真实坐标——这是网页预览器在
  渲染页面上叠加高亮的几何数据（上游 `buildChunkHighlights` + `pdf-preview.tsx`），此前挂账的
  「文档预览只有切片文本」由此拿到半边地基。
  - **新增 `src/parser/pdf_text.rs`**（12 条单测）：自写内容流解释器，逐页跟踪图形状态与文本状态
    （`q/Q`、`cm`、`BT/ET`、`Tf/Tc/Tw/Tz/TL/Ts/Tr`、`Td/TD/Tm/T*`、`Tj/TJ/'/"`），把每次 show-text 送到
    设备空间得到行盒；行内按基线聚类、行间按阅读顺序（自上而下、自左向右）排序，坐标换算成上游那套
    「点为单位、top 从页顶往下量」的页面局部坐标；`CropBox`/`MediaBox` 沿页树继承并作为投影基准。
  - **字体度量**：简单字体读 `/Widths` + `/FirstChar`，CID 字体读 `/W` + `/DW`，缺失宽度按 `/MissingWidth`
    回退，升降部取 `/Ascent` / `/Descent`；文本解码优先 `/ToUnicode`（自写 CMap 解析，支持 `bfchar`、
    `bfrange` 及其数组形式），否则按 CID 双字节 UTF-16 或 WinAnsi 解码——子集 Identity-H 字体不再是一串
    字形编号。
  - **段落合并沿用已有移植**：行盒交给 `src/parser/pdfbox.rs` 里既有的 DeepDOC 算法
    （`concat_downward` 排序 → `naive_vertical_merge` 合段 → `line_tag` 打标），
    `mean_height`/`mean_width`/`page_cum_height`/页高（zoom=3）按上游口径重算，英文判定改为确定性扫描
    （上游是随机采样）。文档因此带上位置标签元数据，切片器的位置感知分支（`_rayrag_ragflow_position_tags`）
    自动把标签投影成 `position_int` / `page_num_int` / `top_int`，可见文本里不留标签。

### Fixed

- **服务端解析 PDF 会 panic 的嵌套运行时**：`PdfParser::parse` 一直在同步接口里新建 tokio 运行时并
  `block_on`，在服务器自身的 worker 线程上直接 `Cannot start a runtime from within a runtime`，上传 PDF
  解析必挂（文档停在 `RUNNING 0.05 Parsing document`）。现在无 OCR 客户端时走零运行时的同步驱动，配置了 OCR
  时借用当前运行时的 `block_in_place`，只有脱离运行时（CLI/测试）才新建临时运行时。该缺陷是本轮新增的
  端到端探针抓到的。

### Notes

- 新增探针 内部 UI 遍历脚本（`pdf-positions-e2e.py`：现场生成坐标已知的两段式 PDF → 建库 → 上传 →
  排队解析 → 轮询 → 拉切片接口，断言每个切片都有 5 元组坐标、页码 1 基、盒子良构、正文无标签残留、
  `page_num_int`/`top_int` 与 `positions` 一致，并且首个盒子的 x0/top 正是生成时写入的 72.0 / 83.0。
  部署镜像上 **22 项检查全 ok**。
- 台账记录的有意近似：RayRAG 没有上游的版面模型（ONNX layout recognizer），因此盒子是**文本行盒**而非
  模型判定的版面框，`layout_type` 恒为空；行内/行间合并、坐标口径、标签格式、`positions` 归一化均按上游。

## [0.4.42] — 2026-09-23（对标批次 v0.3.8t）

### Added

- **悬浮组件嵌入（floating widget）**（上游 `Routes.ChatWidget = /chats/widget` +
  `pages/next-chats/widget/index.tsx` + `components/floating-chat-widget.tsx`）：嵌入对话框里此前标注
  「即将推出」的悬浮组件类型现已可用，与上游一致支持三种模式：
  - `mode=full`（默认）：圆角气泡 + 聊天面板同页；`mode=master`：只渲染气泡，按上游协议向父页面
    `postMessage`，独立打开时自建 `chat-win` iframe 并响应 `TOGGLE_CHAT` 显示/隐藏；
    `mode=window`：只渲染面板（供 master 的 iframe 使用）。
  - 面板：渐变头部（头像 / 标题 / 副标题 + 最小化 / 关闭）、消息区、输入行、可选页脚横幅（可带跳转链接）、
    未读气泡角标（>9 显示 `9+`）；有新回复时播放提示音，`muted=true` 静音。
  - 全部组件设置走 URL 参数并逐项对齐上游默认值：`widget_title` / `widget_subtitle` / `widget_footer` /
    `widget_footer_link` / `widget_accent_color`（#2563eb）/ `widget_background_color`（#ffffff）/
    `widget_text_color`（#111827）/ `widget_header_text_color`（#ffffff）/ `widget_footer_text_color`
    （#111827）/ `streaming` / `muted`；六位十六进制以外的取值按上游 `normalizeHexColor` 回退，头部渐变用
    加深 15% 的强调色（上游 `darkenHexColor`）。
  - 后端按 `from` 分流：`agent` 走 `agentbots`（inputs / completions），其余走 `chatbots`（info /
    completions）；`streaming=true` 时按 SSE 增量渲染，否则等完整回答；`?auth=<beta>` 的凭据引导与分享页一致。
- **嵌入对话框补齐悬浮组件设置**（上游 widget 标签页）：启用「悬浮组件（Intercom 风格）」选项，新增
  Widget title / Subtitle / Footer text / Footer redirect link、五个成对的取色器 + 十六进制输入
  （accent / background / text / header text / footer text）以及「启用流式响应」「静音组件提示音」开关；
  生成的代码块改为上游那套两段式片段——外层 100×100 透明 iframe（`mode=master`）+ 页内脚本响应
  `CREATE_CHAT_WINDOW` / `TOGGLE_CHAT` / `SCROLL_PASSTHROUGH` 并管理 `chat-win` iframe。

### Fixed

- **第三方宿主里的悬浮组件不再因存储被拒而中断握手**：组件是被别的站点嵌进去的，宿主文档若是不透明源
  （`about:blank` / `srcdoc` 外壳）或 iframe 带 `sandbox`（无 `allow-same-origin`），Chrome 会直接让
  `window.localStorage` 抛 `SecurityError`。此前页面底座的 fetch 包装器、主题引导脚本都在顶层无保护地读它，
  异常会中断整个内联脚本，导致组件自身启动脚本里那句 `fcwMasterBootstrap()` 根本没执行、宿主收不到
  `CREATE_CHAT_WINDOW`。现在统一改走 `rayragStore()`（读 / 写 / 删都吞掉拒绝并退化为空存储），
  `?auth=` 的 cookie 写入与 `history.replaceState` 也各自加保护，组件启动本身再包一层 try/catch——
  存储不可用时气泡照常渲染、握手照常发出。另外 401 兜底不再把嵌入页面跳转到 `/login`。
- `mode=master` 不再预取 `inputs`（上游 master 只渲染气泡，内容由 window iframe 负责），
  省掉一次必然被 CORS 拒绝的请求，握手更快。

### Notes

- 台账记录的有意偏差：上游片段里的来源校验写成 `location.origin.replace(/:\d+/, ':9222')`（开发端口泄漏），
  生产环境下会丢弃自己的 postMessage；RayRAG 改为校验真实 `window.location.origin`，使组件真正可用。
- 台账记录的有意加固：上游 `floating-chat-widget.tsx` 同样直接使用 `localStorage`，在不透明源 / sandbox 宿主里
  会整体崩掉；RayRAG 的 `rayragStore()` 让嵌入面在存储被拒时仍可用（行为差异仅限“存储不可用”这一种宿主）。
- 端到端探针补强：宿主页改为**真实跨源页面**（探针自建 HTTP 服务，独立端口 = 独立源，`localStorage` 可用），
  另加一条 `sandbox="allow-scripts"`（无存储）iframe 回归，断言这种最恶劣宿主里仍能完成握手。
- 新增回归：`web::kb_ui_tests::chat_widget_page_matches_upstream_floating_widget`（20 个标记 + 三模式
  钩子 + 9 个设置参数 + 两个后端 + 中文面 + CSS），`agent_embed_modal_matches_upstream_embed_dialog` 增补
  13 个悬浮组件标记、6 条文案与 7 条片段断言。
- 全量 `cargo test --locked --lib` **1573 passed / 0 failed / 27 ignored**。

## [0.4.41] — 2026-09-23（对标批次 v0.3.8s）

### Added

- **工具调用时间线**（上游 `pages/agent/log-sheet/tool-timeline-item.tsx` + `tool_use_callback`）：
  智能体运行轨迹里现在会有**工具调用记录**，与上游一样和进度采样共用同一个 `trace` 数组
  （`{path, tool_name, arguments, result, elapsed_time}`），UI 按「有 `tool_name` 即工具记录」区分。
  - 运行时侧：Agent 组件的工具循环为**每一次真实执行**记录一条 `ToolUseTrace`（工具名、解析后的参数、
    结果文本、实测耗时），并随节点输出落到轨迹里；无工具调用的普通 LLM 回合不受影响。
  - 界面侧：探索页「运行轨迹」抽屉与分享页「思考过程」把工具记录渲染成可展开的行——工具名按上游
    `Agent <Words>` 规则美化、显示耗时，展开后是 **Arguments / Result** 两个代码块；上游黑名单
    （`add_memory`、`gen_citations`）同样过滤。

### Notes

- 新增/加强回归：`agent` 工具循环测试现在断言节点输出里的 `tool_trace`（工具名、`path`、
  解析后的 `arguments`、`result`、耗时），`api::agent_trace` 的采集用例断言工具记录与进度采样合并进
  同一数组，两个页面测试断言工具行的标记、美化函数与黑名单。
- 新增端到端探针 内部 UI 遍历脚本（`agent-tool-trace-e2e.py`：自建 OpenAI 兼容 stub（首轮返回工具调用、
  次轮返回最终答案）+ 知识库 + 带 Retrieval 工具的智能体，真实跑一次智能体，断言轨迹里的工具记录字段，
  再用 Chrome CDP 打开试跑页 → 会话 → 运行轨迹抽屉 → 展开工具行核对 Arguments/Result。
- 全量 `cargo test --locked --lib` **1572 passed / 0 failed / 27 ignored**。

## [0.4.40] — 2026-09-23（对标批次 v0.3.8r）

### Added

- **智能体运行轨迹（trace）**（上游 `agent_api.py::get_agent_logs` +
  `bot_api.py::agent_bot_logs` + `pages/agent/log-sheet/*`）：每次智能体运行都会按消息记录一条
  **运行轨迹**，键与上游一致（`{agent_id}-{message_id}-logs`），内容是上游的 `ITraceData[]`
  （`{component_id, trace:[{progress,message,datetime,timestamp,elapsed_time}]}`）。
  - 轨迹由画布生命周期事件（`node_started`/`node_finished`）实时累积，**运行中即可轮询**（上游日志抽屉
    是 `refetchInterval: 3000`），结束时完整落库；解析失败会在该采样的 `message` 上留下错误文本。
  - 新增 `GET /api/v1/agents/{agent_id}/logs/{message_id}`（登录态）与
    `GET /api/v1/agentbots/{shared_id}/logs/{message_id}`（beta 令牌，供嵌入页使用）；没有轨迹时按上游
    返回 `data: {}`。
- **探索页「运行轨迹」抽屉**：助手消息下方新增 🧭 按钮，打开右侧抽屉列出本次运行经过的每个组件及其
  时间线（时间 / 说明 / 耗时）；运行中每 3 秒自动刷新，结束后自动停止轮询。
- **分享/嵌入页「思考过程（Thinking）」展开**：助手气泡上新增 Thinking 开关，展开后通过 beta 令牌读取
  同一份轨迹（对应上游 `next-message-item` 的 Thinking 展开与 `fetchSharedTrace`）。

### Notes

- 新增回归：`api::agent_trace` 两条（键名与上游 Redis 键一致、事件聚合与边跑边落库）、
  `server::tests::agent_run_traces_match_upstream`（真实跑一次画布 → 轨迹含 begin/message 组件与上游采样
  字段 → 未知消息回 `{}` → beta 兄弟端点可用）；探索页与分享页测试各增补抽屉/展开标记与端点断言。
- 全量 `cargo test --locked --lib` **1572 passed / 0 failed / 27 ignored**。

## [0.4.39] — 2026-09-23（对标批次 v0.3.8q）

### Added

- **设计流水线结果可编辑**（上游 `pages/dataflow-result/components/parse-editer/*`）：
  `/dataflow-result` 右栏的解析结果现在可以**就地编辑**——点击文本块或某条切片即可修改，失焦保存后该步骤
  标记为「已修改」并显示 `dataflowParser.rerunFromCurrentStepTip` 提示；此时点「从当前步骤重新运行」，
  提交给 `POST /api/v1/agents/rerun` 的 `dsl` 会携带**编辑后的 outputs**（与上游 `handleReRunFunc`
  的构造方式一致：`components[key] = {...obj, params:{...params, outputs:{...outputs, [format]:{type,value}}}}`）。
  - `text` / `html` 格式：整块可编辑（上游 `ObjectContainer` 的 `contentEditable` + blur 保存）。
  - `chunks` / 数组型 `json` 格式：每张切片卡片正文可编辑（上游 `ArrayContainer`，文本字段取
    `params.field_name`，缺省为 `text`），空内容的条目按上游跳过不渲染。
- **切片结果条按上游重做**（`components/chunk-result-bar/index.tsx` + `checkbox-sets.tsx`）：
  - `Full text` / `Ellipse` 分段控件（`chunk.full` / `chunk.ellipse`），Ellipse 模式下切片正文按上游
    `contentEllipsis` 收成 4 行；
  - `＋` 新建切片按钮：真实调用 `POST /api/v1/datasets/{kb}/documents/{doc}/chunks`（上游
    `createChunk('')` 先建空切片再编辑）；
  - 「选择所有」改为上游的复选框 + 计数 + 垃圾桶按钮（替代此前的三个独立按钮）。

### Notes

- 新增回归：`web::kb_ui_tests::dataflow_result_page_matches_upstream_ingestion_viewer` 增补 11 个标记与
  三处行为断言（可编辑块、切片编辑、文本模式、新建切片）。
- CDP 探针扩展为 12 步：真点编辑并断言 rerun 请求体携带编辑内容。
- 全量 `cargo test --locked --lib` **1569 passed / 0 failed / 27 ignored**。
- GitHub Release 说明自本轮起统一使用英文书写（历史三个 Release 的中文说明与标题已改写为英文）。

## [0.4.38] — 2026-09-23（对标批次 v0.3.8p）

### Added

- **数据流水线结果页 `/dataflow-result` 上线**（上游 `Routes.DataflowResult` =
  `pages/dataflow-result/index.tsx` + `parser.tsx` + `components/time-line` +
  `components/chunk-result-bar`）：此前该路由**没有注册**（访问 404）。现在它按上游结构落地：
  - `PageHeader` 返回按钮（`LucideArrowBigLeft` + `Back`）回到知识库文件列表（带 `?agent_id=` 时回到智能体）；
  - 顶部时间线：按 `dsl.path` × `dsl.components` × `dsl.graph.nodes` 重建，每个节点显示标题与
    `outputs._elapsed_time` 秒数，点击切换当前步骤；若当前步骤被编辑过再切换，先弹上游
    `dataflowParser.changeStepModalTitle`（「切换步骤警告」/「继续切换」）确认；
  - 左栏（2/5）文档栏：文档名 + 已解析切片内容预览（上游是 PDF 高亮查看器的位置，此项在台账记为近似）；
  - 右栏（3/5）步骤结果：`Output format` 选择器（取该步骤 `outputs` 里真实存在的 text/html/json/chunks）、
    `chunks` 渲染为切片卡片并带「全选 / 反选 / 删除」结果条（删除走
    `DELETE /api/v1/datasets/{kb}/documents/{doc}/chunks`）、其余格式渲染为文本/JSON，另附原始 JSON 折叠块；
  - `Rerun from current step`：先弹上游 `dataflowParser.confirmRerun`，确认后把编辑过的 canvas 提交到
    `POST /api/v1/agents/rerun`。
- **数据摄入（ingestion）日志接口**（上游 `dataset_api.py` + `dataset_api_service.py`）：
  `GET /api/v1/datasets/{id}/ingestions`（分页/排序/状态/关键字/日期/`log_type` 过滤，回 `{total, logs}`）、
  `GET /api/v1/datasets/{id}/ingestions/{log_id}`（含 `dsl` 全量）、
  `GET /api/v1/datasets/{id}/ingestions/summary`（`doc_num` / `chunk_num` / `token_num` 与按
  `TaskStatus` 分桶的 `status`），错误文案逐字对齐（`Lack of "Dataset ID"` / `No authorization.` /
  `Log not found` / `Invalid "log_type", expected "dataset" or "file"`）。
- **`POST /api/v1/agents/rerun`**（上游 `agent_api.py::rerun_agent`）：把请求里的 `dsl` 写回日志
  （`dsl.path = [component_id]`）并重新排队解析该文档；文档处理中回 `` `{name}` is processing... ``，
  找不到回 `Document not found.`。
- **解析运行分阶段报告**：`Pipeline::process_with_report` 记录每个阶段（Parser / TokenChunker /
  Tokenizer）的真实耗时与产出（字数、切片数、token 数、向量维度），成功与失败都会写入一条
  ingestion 日志——dataflow-result 页展示的是 RayRAG 真正跑过的阶段，而非复制上游画布（台账已注明这一替代关系）。

### Notes

- 新增回归：`server::tests::ingestion_logs_and_rerun_match_upstream`、
  `web::kb_ui_tests::dataflow_result_page_matches_upstream_ingestion_viewer` 与 `api::ingestion` 两条存储/DSL 用例。
- 全量 `cargo test --locked --lib` **1568 passed / 0 failed / 27 ignored**。

## [0.4.37] — 2026-09-23（对标批次 v0.3.8o）

### Added

- **共享 / 嵌入智能体（`/agent/share`）上线**（上游 `Routes.AgentShare` + `pages/agent/share/index.tsx`
  + `components/embed-container.tsx` + `hooks/use-send-shared-message.ts`）：此前 `/agent/share` 会被
  `/agent/:id` 当成智能体 ID 命中，渲染成编辑器页面；现在它是上游那个可嵌入的独立聊天页：
  - `EmbedContainer` 外壳：品牌块（`/logo.svg` + RAGFlow）、带边框的圆角容器、头像 + 标题的头部行、
    右侧 `Reset` 按钮（上游硬编码英文，未翻译）；
  - 会话区：开场白（Begin 组件的 prologue）作为第一条消息、消息气泡、引用折叠块、底部输入框
    （占位符取上游 `chat.messagePlaceholder`：`Type your message here...` / `请输入消息...`），
    生成中可以停止；
  - Begin 组件的输入项会先弹出上游的 **`Parameter` 对话框**（无关闭按钮、无页脚、由 `Submit` 提交），
    提交后的值随请求的 `inputs` 一起发送；
  - `Reset` 按上游语义清空会话并恢复开场白（`resetSession()` + `clearEventList()` + 重新弹参数框）；
  - URL 参数完全按上游 `embed-dialog` 的构造器：`shared_id` / `from` / `auth` /
    `release=true` / `visible_avatar=1`（隐藏头像）/ `locale` / `theme`，以及 `data_*` 透传；
  - `?auth=<beta>` 的凭据引导按 RayRAG 的方式落地：写入 `rayrag_token` cookie（并同步 localStorage）
    后从地址栏移除（对应上游 `authorizationUtil.setAuthorization`）。
- **API Token 接口**（上游 `system_api.py`：`/api/v1/system/tokens` GET/POST/DELETE）：
  每个 token 行带 `token`（`ragflow-…`）与 **`beta`**（32 位）两个凭据，列表对历史缺 `beta` 的行
  自动补齐并持久化（上游 `list_token` 的 backfill，前端没有 beta 就不让打开嵌入），删除按
  `(tenant_id, token)` 精确匹配。
- **`beta` 凭据即 bearer**：请求中间件在会话 token 之外，按上游 `AUTH_BETA` 的语义用
  `APIToken.query(beta=…)` 反查所属账号；匿名嵌入页因此无需登录即可对话。
- **`agentbots` 运行时接口**（上游 `bot_api.py`）：`GET /api/v1/agentbots/{id}/inputs` 返回
  `{title, avatar, inputs, prologue, mode}`，`POST /api/v1/agentbots/{id}/completions` 复用智能体
  运行链路（流式 SSE），画布不可达时回上游文案 `Can't find agent by ID: <id>`。
- **编辑器「🔗 嵌入网站」入口**（上游 Management 下拉里的 `common.embedIntoSite` →
  `components/embed-dialog/index.tsx`）：对话框内含 API KEY 表格（复制 / 删除 / 创建新密钥）、
  嵌入类型（全屏 iframe；悬浮组件按上游标为即将推出）、主题（浅色/深色）、隐藏头像、已发布
  （勾选后 URL 带 `release=true`，含上游 tooltip 文案）、界面语言（上游 17 种），并实时生成
  上游同款全屏 iframe 代码块，支持复制与预览。

### Notes

- 新增回归：`server::tests::system_tokens_and_agentbots_match_upstream`（造 token → 仅用 beta 调
  agentbots inputs/completions → 未知 beta 401 → 删除后失效）、
  `web::kb_ui_tests::agent_share_page_matches_upstream_embed_container`、
  `web::kb_ui_tests::agent_embed_modal_matches_upstream_embed_dialog`，以及 `api::tokens` 三条
  存储层用例（beta 形态、backfill 持久化、租户隔离与删除）。
- 全量 `cargo test --locked --lib` **1564 passed / 0 failed / 27 ignored**。

## [0.4.36] — 2026-09-23（对标批次 v0.3.8n）

### Added

- **智能体「探索（Launch）」页面上线**（上游 `Routes.AgentExplore = /agent/:id/explore`，
  `web/src/pages/agent/explore/index.tsx` + `components/session-list.tsx` + `session-card.tsx` +
  `session-chat.tsx`）：这是此前**根本没有注册**的路由（访问 404）。现在按上游逐层复刻：
  - `PageHeader` 面包屑（`header.flow`「智能体」→ 画布标题，标题为空时回退 `explore.title`「探索」）。
  - 左侧 296px 会话列（`SessionList`）：`explore.sessions`「会话列表」标题 + 计数、ghost `Plus`
    新建临时会话（`explore.newSession`「新建会话」）、`explore.searchSessions` 搜索框按名称实时过滤
    （无命中显示 `explore.noSessionsFound`「未找到会话」）、每张会话卡右上角**悬停才出现**的 `Ellipsis`
    菜单，菜单里的删除行打开上游 `ConfirmDeleteDialog`（`common.deleteModalTitle`「确定删除吗?」+
    取消/删除）。
  - 右侧会话聊天（`SessionChat`）：未选会话时显示 `explore.noSessionSelected`「请选择一个会话或创建新会话」；
    选中后按 `/api/v1/agents/{id}/sessions/{session_id}` 的历史渲染消息；底部消息输入框（空内容时发送键禁用、
    Enter 发送、Shift+Enter 换行）。
  - **首次发送自动建会话**：与上游一致，先 `POST /api/v1/agents/{id}/sessions`（会话名 = 问题原文），
    拿到 `session_id` 后再 `POST /api/v1/agents/chat/completions` 流式生成，并把 `sessionId` 写回地址栏。
- **新增端点 `POST /api/v1/agents/{id}/sessions`**（上游 `agent_api.py::create_agent_session`）：
  `{name}` 建会话，`user_id` 缺省取租户；会话行按上游写入 `source=agent`、画布 DSL 快照、
  最新版本标题，并把 `canvas.get_prologue()`（Begin 组件 prologue）**作为第一条 assistant 消息**播种；
  返回 `_normalize_agent_session` 形状（`{code:0,data:{id,name,message,source,agent_id,...}}`）；
  画布不存在回 `{code:102,message:"Agent not found."}`。
- **新增端点 `POST /api/v1/agents/chat/completions`**（上游 `api.ts::agentChatCompletion`）：
  上游所有智能体聊天入口（编辑器调试、探索页、分享页）都往这个**把画布 id 放在 body 里**的地址发流式请求；
  此前只有 `/api/v1/agents/{id}/completions`，该地址会 404。现在与路径版共用同一处理器；
  请求体的 `session_id` 与 `conversation_id` 两种写法都能选中会话，`query` 缺省（上游表单消息只发
  `inputs`）也不再报错。
- **画布工具栏「🚀 探索」按钮**改为上游行为：点击跳转 `/agent/{id}/explore`（上游
  `navigateToAgentExplore`），不再弹旧的自测对话框。

### Notes

- 新增回归：`web::kb_ui_tests::agent_explore_page_matches_upstream_launch_surface`（标记/双语标签/CSS/
  Launch 接线）与 `server::tests::agent_explore_session_creation_and_chat_route_match_upstream`
  （建会话播种 prologue、列表回读、body 寻址聊天路由、未知画布 102）。
- 全量 `cargo test --locked --lib` **1558 passed / 0 failed / 27 ignored**。

## [0.4.35] — 2026-09-22（对标批次 v0.3.8m）

### Added

- **检索页设置抽屉按上游 `next-search/search-setting.tsx` 重建**：此前 `/search` 的检索参数是一个
  固定宽 240px 的侧栏卡片（Top K / 阈值 / 向量权重三个 `<select>` + 快速问题链接），上游实际是一个
  `w-[440px]` 的**滑出抽屉**。现在逐字段对齐上游顺序与语义：
  - 触发器（工具栏右侧齿轮，`data-testid="search-settings-trigger"`）→ 抽屉标题行（`search.searchSettings`
    + 16px `X`）→ 可滚动主体 → 底部固定的 Cancel/Save（`search.cancelText` / `search.okText`，
    Save 带 `data-testid="search-settings-save"`，即上游的 testId）。
  - 主体字段顺序：头像 + 名称 + 描述（描述默认 `search.descriptionValue`）→ 数据集多选 → 元数据过滤
    （Disabled / Automatic / Semi-automatic / Manual，manual 与 semi_auto 各自可增删行）→
    「显示块元数据」开关 + 元数据字段多选（键来自 `/api/v1/datasets/metadata/flattened`）→
    相似度阈值与向量权重滑杆（滑杆与数字框双向同步）→ 重排开关 + 必填重排模型树（复用共享
    `push_default_model_selector`）+ Top K 滑杆（0–2048，仅重排开启时出现，与上游一致）→
    AI 总结开关 + LLM 设置（自由度预设 + temperature / top_p / presence / frequency 四行，带启用勾选）→
    关联搜索开关 → 查询思维导图开关。
  - 请求侧：`meta_data_filter`（三种模式）、`reference_metadata{include,fields}`、`summary`、
    `query_mindmap` 现在真的进入 `/api/v1/retrieval` 请求体；思维导图按钮的可见性也随抽屉开关联动。
  - 保存走 `PUT /api/v1/searchapps/{id}`（未保存的应用则 `POST` 新建），Cancel 用应用记录回滚后关闭。
- **检索应用详情端点补齐（上游 `search_api.detail`）**：`GET /api/v1/searches/{id}` 之前**根本没有注册**
  （`/api/v1/searchapps/{id}` 也只挂了 PUT/DELETE，GET 回 405 空响应），所以按上游契约读取检索应用的客户端
  拿不到任何数据——设置抽屉同样无从回填。现在 `GET /api/v1/searches/{id}` 与
  `GET /api/v1/searchapps/{id}` 都返回 `{code:0,data:{…}}`，未知 id 回上游文案
  `Can't find this Search App!`（102），非属主回 `Has no permission for this operation.`（103）。
  同时把检索应用 CRUD 按上游路径补齐别名：`GET/POST /api/v1/searches`、`GET/PUT/DELETE /api/v1/searches/{id}`
  （上游 `web/src/utils/api.ts` 的 `createSearch`/`getSearchList`/`getSearchDetail`/`updateSearchSetting`/`deleteSearch`）。

### Fixed

- 检索工具栏里残留的 `id='rerank'` 复选框与抽屉里的重排开关**同 id**，`getElementById` 取到的是先出现的
  隐藏那个，于是点抽屉开关时行不展开、Top K 不出现。旧复选框已移除，`ssRerankSync` 负责 Top K 的显示。

## [0.4.34] — 2026-09-22（对标批次 v0.3.8l）

### Added

- **元数据过滤（metadata filter）三种模式全部按上游实现，含 LLM 自动生成**（上游
  `common/metadata_utils.py::apply_meta_data_filter` + `rag/prompts/generator.py::gen_meta_filter` +
  `rag/prompts/meta_filter.md`）：此前 RayRAG 对 `auto` / `semi_auto` 直接报
  “Only manual metadata filters are supported without an LLM filter generator”，只有 `manual` 可用。
  - 新增 `src/metadata_filter.rs`（约 1400 行含 11 条单测），逐行移植上游求值器
    `meta_filter`：14 个算子（`contains` / `not contains` / `in` / `not in` / `start with` / `end with` /
    `empty` / `not empty` / `=` / `≠` / `>` / `<` / `≥` / `≤`）、`and` 交集（空集提前返回）与 `or` 并集、
    未知 key 不参与匹配。三条容易写错的语义也照搬：比较类算子先把两侧按
    **Python `ast.literal_eval`** 求值（所以 `"5" > "30"` 是数字比较、`"5"` 与 `5` 相等），
    日期形状（`YYYY-MM-DD`，10 位且位置校验）走**字符串比较且“日期查询不匹配非日期值”**，
    非比较类算子只做小写化；类型不匹配按上游 `except` 吞掉（视为不匹配）。
  - `gen_meta_filter`：渲染上游 Jinja 模板（`{% if constraints %}` 为假时整行消失、`json.dumps`
    的 `ensure_ascii=True` 转义与 `", "`/`": "` 分隔符、元数据结构 `{key: [values]}`）、
    `Generate filters:` 用户消息、剥离 `</think>`/代码块后走 `json_repair` 宽松解析，
    失败回落到上游的 `{"conditions": []}`。
  - `apply_meta_data_filter`：`auto` 用全部元数据键、`semi_auto` 只用用户勾选的键并带上
    `{key: op}` 约束、`manual` 直传条件；返回值语义也照搬——`manual` 匹配为空回 **`["-999"]`**
    哨兵（检索侧落到“无命中”），`auto`/`semi_auto` 为空回 **`None`**（不退化为“无过滤”之外的任何行为）。
  - 接入四个检索调用点（`/api/v1/retrieval`、加权检索、Dify 检索、Agent 检索工具）：
    `resolve_metadata_doc_ids` 改为 async 并接收 `MetadataFilterContext{tenant_id, question, chat_selector}`，
    LLM 选型按上游——有搜索应用 `chat_id` 用它，否则用租户默认对话模型。
  - 搜索应用记录新增 `search_config.meta_data_filter` 字段（创建/更新/读取全链路），
    与上游一致：给了 `search_id` 时优先用应用里存的过滤器。

- **文档元数据批量更新按上游契约实现**（上游 `document_api.update_metadata`）：
  新增 `PATCH /api/v1/datasets/{id}/documents/metadatas`（数据集元数据管理器真实调用的端点），
  body 为 `{selector:{document_ids, metadata_condition}, updates, deletes}`，响应对齐
  `{updated, matched_docs}`；`metadata_condition` 走上面的 `convert_conditions` + `meta_filter`
  并与 `document_ids` 求交，条件列表非空但无命中时短路回 `{updated: 0, matched_docs: 0}`。
  逐字段报错文案逐字取自上游（`selector must be an object.` / `updates and deletes must be lists.` /
  `metadata_condition must be an object.` / `document_ids must be a list.` /
  `Each update requires key and value.` / `Each delete requires key.` /
  `These documents do not belong to dataset {id}: …` / `You don't own the dataset {id}.`，均为
  HTTP 200 + `RetCode.DATA_ERROR=102`）。RayRAG 原有的扁平 body 端点
  `POST /api/v1/datasets/{id}/metadata/batch` 保留为兼容别名。

### Fixed

- **租户默认对话模型无法用上游的复合选择器设置**：上游 Web 端（`use-set-default-model.ts`）发的是
  `{model_name}@{instance_name}@{provider_name}`，RayRAG 的 `resolve()` 认这个形式、随后的
  `validate_default_chat_models` 却只认裸模型名与内部 `provider/instance/model`，于是必然回
  409 `Tenant default chat selector is not a unique enabled chat model`。现在三种写法都能通过校验
  （已加回归测试）。

## [0.4.33] — 2026-09-22（对标批次 v0.3.8k）

### Added

- **头像裁剪对话框真正可用，并与上游 `components/avatar-upload.tsx` 的交互对齐**：
  - **滚轮缩放**按上游 `handleWheel` 实现——`delta = deltaY > 0 ? 0.9 : 1.1`，新边长夹在
    `[20, min(图宽, 图高)]`，保持选区中心比例后重新夹回图像范围（`preventDefault` 阻止页面滚动）。
  - 选区样式对齐上游：`2px dashed #fff` 虚线框 + `box-shadow: 0 0 0 9999px rgba(0,0,0,.5)` 遮罩，
    舞台补 `touch-action:none`。
  - **修复对话框的 Promise 管线**：此前 `openAvatarCropModal()` 直接把图片加载 Promise 返回给调用方，
    `avatarCropConfirm()` 只写预览、不 resolve，导致数据集头像的 `await` 永远拿到 `null`
    （点“确定”后不生效）；现在 OK 以裁剪结果 resolve、取消以 `null` resolve。
  - **用户设置页头像改走同一个裁剪对话框**（此前是自动居中裁剪的近似实现），与上游
    `AvatarUpload` 在资料页与数据集页行为一致：选文件 → 弹裁剪框 → 确定后 `PATCH /api/v1/users/me`。

### Fixed

- **注册开关被两套环境变量拆成两半：`REGISTER_ENABLED=1` 时登录页给注册表单、提交却回 403
  `Registration is disabled`**：`GET /api/v1/system/config`（登录页据此显示/隐藏注册入口）读的是上游
  `settings.REGISTER_ENABLED`，而 `POST /api/v1/user/register` 只认 RayRAG 私有的
  `RAYRAG_ALLOW_REGISTRATION`——该变量连 `docker-compose.yml` 都没有透传，于是页面与接口永远不可能同时
  成立。现在两处读同一个解析值（新增 `settings::resolve_register_enabled()`，在 `Settings::from_env()`
  里一次性算好存进 `AppState::register_enabled`）：`REGISTER_ENABLED` 未设默认 1（与上游一致），显式 `0`
  才关闭注册；旧的 `RAYRAG_ALLOW_REGISTRATION=true` 保留为只能“打开”的逃生开关（隔离测试 binary 仍可用）。
  行为矩阵实测：`0`→`registerEnabled:0` + 拒注册；`0`+旧开关→`1` + 放行；`1`→`1` + 放行。

- **注册接口按上游 `user_api.py::user_add` 的路径与契约重建**（此前只有 RayRAG 私有的
  `POST /api/v1/user/register`，且成功/失败状态码都是自造的）：
  - **路径**：上游 `web/src/utils/api.ts::register` 提交到 `` `${restAPIv1}/users` ``，blueprint
    `api/apps/restful_apis/user_api.py` 挂在 `/api/v1`，因此注册端点是 **`POST /api/v1/users`**——
    与列用户的 `GET` 同路径。现在两者合并注册，`GET` 仍需 token、`POST` 免登录（鉴权中间件由
    “只看路径”改为 `public_api_request(method, path)`）；旧的 `/api/v1/user/register` 保留为兼容别名。
  - **传输层**：上游 `get_json_result` 对 `RetCode` 一律回 **HTTP 200** 且仅在 `code == 0` 时带 `data`。
    原先禁用注册回 403、重复邮箱回 400、成功回 201 全部改为上游形状。新增 `src/user_register.rs`
    逐字承载 `api/utils/nickname_validation.py` 与 `user_api.py` 的校验与文案：
    `@validate_request` → `required argument are missing: nickname,email,password; `（101）、
    `User registration is disabled!`（103）、`Invalid email address: {email}!`（103）、
    `Email: {email} has already registered!`（103）、昵称五条（`Nickname is required.` /
    `Nickname must be a string.` / `Nickname cannot be empty.` /
    `Nickname must be at most 100 characters.` / `Nickname contains invalid characters.`，101）、
    `User registration failure, error: {e}`（100）、成功 `{nickname}, welcome aboard!` 并把凭据写进
    `Authorization` 响应头（上游 `construct_response(auth=user.get_id())`）。邮箱正则
    `^[\w\._-]+@([\w_-]+\.)+[\w-]{2,}$` 与昵称正则 `^[\w ._'-]+$` 均按上游原文。
  - **登录页注册流程对齐** `use-login-request.ts::useRegister` + `login-next/index.tsx::onCheck`：
    提交到 `/api/v1/users`，成功后提示 `message.registered`（en `Registered!` / zh `注册成功`）并
    **翻回登录面**（原先会自动登录并跳首页，属自造行为），消息含 `registration is disabled` 时提示
    `message.registerDisabled`；提示沿用 `.auth-error` 行并新增 `.success` 绿色态（上游是 antd 的
    3 秒 toast，这里同样 3 秒后自清）。
  - **顺带修掉一个真实缺陷**：`UserStore::register` 把小写化后的邮箱作为存储键，而 `login`/`get_user`
    用调用方原样传入的字符串查表，于是 `Ada@Example.com` 注册后永远登不进去（键是
    `ada@example.com`）。两处查表改用大小写不敏感解析（精确命中优先，随后回退扫描，兼容旧
    `users.json` 里的混合大小写键）。

## [0.4.32] — 2026-09-21（对标批次 v0.3.8j）

### Added

- **SSH 沙箱提供方真正执行代码**（上游 `agent/sandbox/providers/ssh.py`）：此前
  `/api/v1/admin/sandbox/test` 对 `ssh` 只做 TCP 可达性探测，并明确回复“本构建不通过 SSH 执行代码”；
  现在新增 `src/ssh_exec.rs`，按上游 `SSHProvider` 的**同一套配置字段**（`host` / `port` / `username` /
  `password` / `private_key` / `passphrase` / `known_hosts` / `python_bin` / `node_bin` / `work_dir` /
  `timeout` / `max_output_bytes`）与**同一种认证二选一**（密码或私钥）真实连上远端主机、
  在 `work_dir/rayrag-ssh` 工作目录里用配置的解释器跑代码，并回传上游形状的
  `{success, message, details{exit_code, execution_time, stdout, stderr}}`。
  - 实现方式：调用系统 OpenSSH 客户端（`ssh`；密码认证经 `sshpass -f`，密码只落临时文件、不进进程表），
    而不是引入新的 Rust SSH 依赖栈；`known_hosts` 配了就走 `StrictHostKeyChecking=yes`，
    未配则 `accept-new`（对应上游“系统 host key + 未知名主机拒绝”的取舍）。
  - 安全细节：内联私钥写入 `0600` 临时文件、调用结束立即删除；输出按 `max_output_bytes` 截断并标注；
    整体受 `timeout` 限制（超时按 executor-manager 约定报 `-1`/255）。
  - 运行时镜像补装 `openssh-client` 与 `sshpass`（`docker/Dockerfile` 运行阶段），
    纯 Linux 运行时若缺少 `ssh`/`sshpass` 会给出明确报错而不是静默失败。
- **踩坑（探针/实测发现）**：`SshConfig` 最初对所有字符串字段统一 `trim()`，导致私钥尾部的换行被吃掉
  （411 → 410 字节），`ssh` 直接报 `Load key …: error in libcrypto`；改为凭据字段（`password` /
  `private_key` / `passphrase`）逐字节保留、仅用 `trim()` 判断是否为空，并补了回归测试。

## [0.4.31] — 2026-09-21（对标批次 v0.3.8i）

### Changed

- **检索页思维导图进度按上游 `usePendingMindMap` 的语义实现**（`next-search/hooks.ts` +
  `components/ui/progress.tsx` + `mindmap-sheet.tsx`）：上游的百分比是**合成斜坡**——1 秒一次的
  `setInterval` 把计数器 +1，`count > 40` 时自清定时器，条上显示
  `Number(((count / 43) * 100).toFixed(0))`，因此每秒走约 2%、最终停在 **98%**（永远到不了 100）；
  `Progress` 组件本身是 `h-1 min-w-10 rounded-full bg-bg-accent` 的轨道，
  指示条用 `transform: translateX(-(100 - value)%)` 平移。
  RayRAG 之前是一条固定 40% 宽、靠 CSS `@keyframes` 无限循环的假动画（与真实进度无关，
  也不随请求结束而停在某处），本轮改为与上游逐字一致的 `mindProgressStart/Stop/Render`
  与 `mindProgressPercent()`，并在打开工作表时启动、请求成功/失败/关闭工作表时停止。
  轨道与指示条样式对齐上游（4px 高、`min-width:40px`、`border-radius:9999px`、
  `background:rgba(76,164,231,.05)`、`transition:transform .2s`），
  根元素带 `role='progressbar'` 与 `aria-valuemin/max/now`（Radix `Progress` 的输出形态）。
- **台账更正**：该行原残留写作「mindmap 百分比进度是 CSS 动画而非真实多步进度」。逐行读完上游后确认
  **上游本身也没有真实进度**（`usePendingMindMap` 就是定时器合成值），故残留的“真实多步进度”表述撤回，
  改为记录「已按上游合成斜坡逐字实现」这一事实。

## [0.4.30] — 2026-09-21（对标批次 v0.3.8h）

### Changed

- **「添加自定义模型」对话框改为逐字段报错**（上游 `add-custom-model-dialog.tsx` +
  `use-custom-model-fields.tsx` + `components/dynamic-form.tsx`）：上游字段由 `DynamicForm` 生成，
  每个字段下方渲染一条 `FormMessage`，字段 schema 为
  `name` required（`${label} is required` = `setting.modelNameRequired` "Model name is required"）、
  `name` 的 `customValidate` 重名检查（`setting.modelNameDuplicate` "Model name already exists"）、
  `max_tokens` 的 `z.coerce.number().min(0, setting.modelMaxTokensMinMessage)`
  （"Max tokens must be at least 0"）。RayRAG 之前把第一个错误写进对话框级的 `#pcmError`，
  看不出是哪个字段错了；现在改为 `#pcmNameError` / `#pcmMaxTokensError` 两个字段级槽位
  （`data-testid` = `pcm-name-error` / `pcm-max-tokens-error`），输入时实时更新、
  打开对话框时清空、校验不通过时不提交，`#pcmError` 保留给其它对话框级问题。
- **台账更正**：该行原残留里的 `Loader2` 提交 spinner 属上游 `viewMode` 的即时持久化路径——
  RayRAG 的自定义模型新增是纯本地目录更新、本版本没有该持久化调用，spinner 无可见状态，
  按既有做法**撤回该残留**并在台账写明依据。

## [0.4.29] — 2026-09-21（对标批次 v0.3.8g）

### Added

- **用户登录/注册页按上游 `pages/login-next/index.tsx` 的 zod 模式做逐字段校验**：
  - 登录面：邮箱为空 → `login.emailPlaceholder`（"Please input email"），非空非法 → "Invalid email"；
    密码为空 → `login.passwordPlaceholder`（"Please input password"）。
  - 注册面：昵称必填 → "Please input nickname"，不满足 `NICKNAME_PATTERN`
    （字母/数字/空格与 `. _ ' -`）→ `setting.usernameInvalidCharacters`
    （"Name can only contain letters, numbers, spaces, and . _ ' -"）；邮箱与密码同登录面规则。
    **台账注记**：上游把该必填文案写成 `message: 'nicknamePlaceholder'`（漏了 `t()`，界面会直接显示键名），
    RayRAG 按用户可读的占位文案显示，属有意偏离，已记录。
  - 两个面各字段一条 `FormMessage`（`auth-email-error` / `auth-password-error` /
    `auth-register-email-error` / `auth-nickname-error` / `auth-register-password-error`），
    校验不通过时不发 `POST /api/v1/auth/login` 或 `POST /api/v1/user/register`；
    接口失败仍在 `#error` 显示（上游无此提示，属既有增强）。
- **记住我**按上游 `disablePasswordLogin` 之外的规则着色：勾选时标签由 `--tm` 变 `--t`，
  复选框改为上游 `Checkbox` 尺寸（14px、2px 圆角、`appearance:none`、勾选画对勾）。
- **版式**：卡片 `max-w-[540px] rounded-2xl(10px) pt-14 pl-10 pr-10 pb-2 border shadow-xl`、
  标题 `text-xl font-semibold mb-8`、表单 `gap-8`、提交按钮改为上游
  `bg-metallic-gradient`（新增 `--metallic` 暗/亮主题变量）＋ `border-b-2 #00BEB4` ＋ `my-8`、
  切换行 `mt-10`、渠道容器 `mt-3 border`；移除与上游矛盾的旧 `.auth-card{max-width:400px}` 规则。

## [0.4.28] — 2026-09-21（对标批次 v0.3.8f）

### Added

- **控制台登录页按上游 `pages/admin/login.tsx` 对齐**：
  - 补上三层 **`Spotlight`** 辉光（`opcity` 0.4/0.3/0.3、颜色 `rgb(128,255,248)`、
    coverage 60/12/12、`backdrop-filter: blur(30px)`、`z-index:-1`），叠在既有的 `BgSvg` 光带之上。
  - 版式改为上游结构：头部绝对定位（品牌 `mt-12 ml-12`、标题 `mt-[6.5rem] text-4xl font-medium mb-12`）、
    540px 列 `mt-72 mb-48`、卡片拆成 `CardContent px-10 pt-14 pb-10` 与 `CardFooter px-10 pt-8 pb-14`。
  - **逐字段校验**（对应上游 `FormSchema`）：邮箱为空 → `login.emailPlaceholder`
    （"Please input email"），非空但不是合法邮箱 → "Invalid email"；密码为空 → `login.passwordPlaceholder`
    （"Please input password"）；每个字段一条 `FormMessage`（`admin-email-error` / `admin-password-error`），
    校验不通过时不发登录请求；接口失败仍在对话框级 `admin-login-error` 显示（上游只 `console.log`，
    RayRAG 保留可见提示）。
  - **记住我**：复选框按上游 `Checkbox` 尺寸（14px、2px 圆角、`appearance:none`、选中画对勾），
    标签文字随勾选状态在 `--t2` / `--t` 之间切换（上游 `field.value ? 'text-text-primary' : 'text-text-secondary'`）。
  - **提交按钮**改为上游 `variant="highlighted" size="lg" block`：`background:var(--t)`、`color:var(--bg)`、
    `border-bottom:4px solid var(--p)`、40px 高、8px 圆角、14px/500，并补上 `loading` 态的旋转指示。
  - 表单元素从纯 `<label>` 改为 `<form>` + `<label for>` + `type=submit`（回车提交、浏览器原生校验语义）。

## [0.4.27] — 2026-09-21（对标批次 v0.3.8e）

### Added

- **管理后台补上上游的 `CurrentUserInfo` 缓存与 `source` 判别位**
  （`pages/admin/layouts/root-layout.tsx` 的残留项）：
  - 控制台登录成功后按上游 `authorizationUtil.setItems({Authorization, Token, userInfo})`
    写入三个键：`Authorization` = `Bearer <token>`、`token` = `access_token`、
    `userInfo` = `JSON.stringify({...登录返回的 data, name: data.nickname})`；
    同时把上下文提升为 `{userInfo: data, source: 'serverRequest'}`（上游 `setCurrentUserInfo`）。
  - 新增 `adminCurrentUserInfo()` / `adminSetUserInfo()` / `adminConsoleIdentityEmail()`：
    有缓存时读 `localStorage.userInfo` 并标记 `source: 'localStorage'`（上游
    `getLocalStorageUserInfo()`），否则回落到 `{userInfo: null, source: null}`。
  - 退出登录按上游 `authorizationUtil.removeAll()` + `{userInfo: null, source: null}`
    清空三个键与上下文（`adminConsoleForgetIdentity()`）。
  - 用户表的「自己那一行」判断改为上游的 `userInfo?.email` 优先（`adminConsoleIdentityEmail()`），
    请求令牌解析出的邮箱作为兜底。

## [0.4.26] — 2026-09-21（对标批次 v0.3.8d）

### Changed

- **管理后台账号详情页的资产表回到上游版式**（`pages/admin/user-detail.tsx`）：上游两张表
  （数据集 / 智能体）实际上**只有一列**——头像 + 名称，`<TableHeader>` 整段被注释掉，
  其余列（status/chunk_num/doc_num/token_num/language/create_date/update_date/permission、
  agent 的 permission/canvas_category）都停在注释里。RayRAG 之前渲染了「Name/Language」与
  「Agent title/Canvas category」两列表头，本轮改为上游的单列无表头版式，
  行内容为 `RAGFlowAvatar` + 名称。
- **空态用上游 `TableEmpty`**：单行单格 `colSpan=1`、`h-24 text-center`、文案
  `common.noResults`（"No results found" / 未查到结果），替换掉 RayRAG 自造的 "No dataset"/"No agent"。
- **Tab 改为上游的边框胶囊样式**：`TabsList` 的 `p-0 mb-2 gap-4 justify-start`，触发器
  `rounded-sm px-3 py-1.5 text-sm font-medium border-0.5 border-border-button`，
  选中态填 `bg-bg-card` + `shadow-sm`；不再使用自创的下划线 tab。
- **返回按钮**用上游的 `Button variant="outline"`（`h-10 px-3`）加 `LucideArrowLeft` 图标 +
  `admin.back` 文案，替换原来的纯文本 "← Back" 链接。

## [0.4.25] — 2026-09-21（对标批次 v0.3.8c）

### Added

- **管理后台用户表补齐上游最后两列**（`pages/admin/users.tsx`）：
  - `admin.userType` 列不再是静态文本，而是上游的**普通/超级管理员切换**：登录管理员自己那一行显示
    `Badge variant="secondary"` 的 **Superuser** 徽章（`data-testid='admin-user-type-badge'`），
    其余行是 `Normal` / `Superuser` 选择器（`data-testid='admin-user-type-select'`），
    选中 Superuser 走 `PUT /api/v1/admin/users/{u}/admin`、选回 Normal 走同一路径的 `DELETE`
    （对应上游 `grantSuperuser` / `revokeSuperuser`），操作后自动重载列表。
  - 操作列改为上游**悬停才显现**的图标三件套（`ClipboardList` → 账号详情页、`UserLock` → 修改密码、
    危险色 `Trash2` → 删除），后两个在**自己那一行不出现**（上游 `{!isMe && …}`）；
    文本按钮 "Change password" / "Delete" 移除。
  - 删除确认框按上游结构重排：标题 `admin.deleteUser`、描述 `admin.deleteUserConfirmation`
    （"Are you sure you want to delete this user?"）、账号单独放在
    `rounded-lg mt-6 p-4 border-0.5` 的边框盒里，确认按钮保持危险色。
  - 新增 lucide `UserLock` / `Trash2` 图标常量与 `.admin-row-icon-danger`
    （`color:var(--d)` + `hover:bg-state-error/10`）。
  - 管理后台表格版式回到上游 `components/ui/table.tsx` 的粒度：表头 `h-14 px-4`（首列 `pl-6`、
    末列 `pr-6`）、字重 normal、**取消全局 `th` 的大写与字距**，单元格 `px-4 py-3`、
    行分隔 `border-b-0.5`、行悬停 `bg-card`（三个管理后台表格共用同一套上游原子组件）。

## [0.4.24] — 2026-09-20（对标批次 v0.3.8b）

### Added

- **管理后台用户表单按上游 `react-hook-form` 逐字段校验**（`pages/admin/forms/user-form.tsx`、
  `pages/admin/forms/change-password-form.tsx`）：上游每个字段后面都挂一个 `FormMessage`，
  由 zod resolver 决定文案；RayRAG 之前只有一行对话框级错误提示，现在改为**每字段一条**
  （`data-testid='admin-create-user-email-error'` / `-password-error` / `-confirm-error`、
  `admin-new-password-error` / `admin-new-password-confirm-error`），空消息时该槽位直接折叠
  （对应 `FormMessage` 无 message 时返回 null）。
  - 新建用户：`email` 必须是合法邮箱（`admin.invalidEmail` = “Please input a valid email address!”）、
    `password` 至少 6 位（`admin.passwordMinLength`，上游文案仍写作 “more than 8 characters.”）、
    新增 **Confirm password** 字段（`min(1)` → “Please confirm your password!”，
    且 `.refine` 相等 → “The password that you entered do not match!”）。
  - 修改密码：新增只读**邮箱行**（上游 `Input readOnly`，RayRAG 为 `adminPasswordEmail`）、
    `newPassword` 与 `confirmPassword` 均要求至少 8 位并用同一条不匹配文案，
    提交前校验不通过则不发请求。
  - 两个对话框在打开时清空字段值**和**消息槽，避免新表单一开始就是红的。

## [0.4.23] — 2026-09-20（对标批次 v0.3.8a）

### Added

- **检索结果渲染文档元数据（上游 `search-view.tsx` 的 metadata 芯片）**：上游在每条检索结果下渲染
  `document_metadata` 的芯片行（`flex flex-wrap gap-2 mt-2`，每个芯片 `text-xs border rounded px-2 py-1`，
  键为次要色、值为主色），值格式化规则是「数组 join(', ') / null 空 / 对象 JSON / 其余 String」。
  RayRAG 的 `/search` 结果行与数据集「Retrieval testing」结果行现在都按同一形状渲染
  （`data-testid='chunk-metadata'`、`chunk-metadata-chip`），并同时请求 `include_metadata: true`，
  让检索接口返回该字段；聊天参考面板用的
  `markdown-content/index.tsx` 版式（`space-y-1 border rounded p-2`）也实现为 `metadataSection()` 备用。
- 元数据缺失或为空对象时**不渲染任何空壳**（与上游 `Object.keys(...).length > 0` 判断一致），键与值都做 HTML 转义。

## [0.4.22] — 2026-09-20（对标批次 v0.3.7z）

### Added

- **检索测试表单补齐上游 `TestingForm` 的其余字段**（`pages/dataset/testing/testing-form.tsx`）：
  - 问题输入改为上游的 **textarea**，底部 Run 按钮按上游 `ButtonLoading` 的语义**未填问题即禁用**（`data-testid='testing-run'`）。
  - `TopSelectFormItem`：`knowledgeConfiguration.top` 标签 + `common.top` 选项（Top 10/20/50/100）的
    `SelectWithSearch`（复用共享组件，`data-testid='test-size-select'`），作为请求的 `size`。
  - `RerankFormFields` 的**条件 top-K 滑块**：选中 rerank 模型后才出现（`knowledgeDetails.topK` + `topKTip`，
    1–2048 的滑块与数字输入同步），选中时请求体带 `top_k`。
  - `MetadataFilter`：`chat.metadata` + `chat.metadataTip` 的方法选择器（Disabled / Automatic / Manual / Semi-automatic）；
    选 Manual 显示 `chat.conditions` 条件行（键输入 + `SwitchOperatorOptions` 的 14 个操作符 + 值输入 + AND/OR 逻辑 + 添加/删除），
    选 Semi-automatic 显示 `chat.metadataKeys`（可选过滤项，键选项来自 `GET /api/v1/datasets/metadata/flattened`）+ 操作符行；
    运行时按选择拼装并发送 `meta_data_filter`。
  - **已知后端限制**：上游 semi-auto 需要 LLM 生成过滤键，RayRAG 的检索接口目前只接受 `manual`（返回明确错误），台账已记录。

## [0.4.21] — 2026-09-20（对标批次 v0.3.7y）

### Changed

- **zvec 成为默认配置（三个入口统一）**：`zvec-backend` 与 `postgres-backend` 一起进入 Cargo 默认 feature，
  因此纯 Linux 的 `cargo build --release --locked` 就是发行配置（原生 zvec + PostgreSQL 元数据），
  Dockerfile 的 `RAYRAG_FEATURES` 默认值也从 `postgres-backend` 改为 `postgres-backend,zvec-backend`
  （compose、`.env.example`、`install.sh` 早就默认 zvec）。README（中英）与 `docs/advanced.md` 的纯 Linux
  步骤改为先导出 `ZVEC_LIB_DIR` 再 `cargo build --release --locked`，并说明无原生库环境可用
  `--no-default-features --features postgres-backend` 退回 JSON 索引。
- **运行期默认后端改为「编译进来就用 zvec」**：此前 `RAYRAG_VECTOR_BACKEND` 不设时一律走 JSON，即使
  二进制带了原生后端。现在默认值由编译期决定（`default_vector_backend()`：有 `zvec-backend` 用 `zvec`，
  否则 `json`），显式设置仍然优先。新增单测覆盖两条分支。
- **构建时把 zvec 库目录写进 runpath**：`build.rs` 在 `ZVEC_LIB_DIR` 指向含 `libzvec_c_api.so` 的目录时
  注入 `-Wl,-rpath`（依赖 crate 的 `rustc-link-arg` 不会传到本包二进制，此前运行必须手动
  `LD_LIBRARY_PATH`），纯 Linux 与测试因此可直接运行。全量 `cargo test --locked --lib` 现在默认即带
  zvec，1512 passed（比无 feature 时多 7 项原生后端用例）。

## [0.4.20] — 2026-09-20（对标批次 v0.3.7x）

### Added

- **OAuth 回调落地处理对齐上游 `useOAuthCallback`**：上游回调成功后跳 `/?auth={user_id}`，失败跳
  `/?error=…`，由 `hooks/auth-hooks.ts` 在落地页消费（`auth` 写入本地会话并清掉查询参数，`error` 先弹出提示、
  1 秒后回到 `/login` 并清掉参数）。RayRAG 之前回调端已实现（state 校验、换 token、拉用户信息、注册/登录、
  下发 cookie），但落地页不读这两个参数。现在共享外壳（无导航的 `layout_plain` 与带导航的 `layout`）
  都注入同一段 `OAUTH_LANDING_JS`：`auth` 写入 `localStorage.rayrag_token` 并补一次 cookie、清参数后停留；
  `error` 显示一个 `role='alert'` 的浮层提示（`data-testid='oauth-error-toast'`）并跳回 `/login`。

## [0.4.19] — 2026-09-20（对标批次 v0.3.7w）

### Added

- **默认模型选择树支持 Radix 键盘操作**：上游 `components/tree-select.tsx` 基于 Radix `Tree`，
  除已有的 ↑/↓（循环）、Home/End、首字母跳转、Enter 选中、Escape 关闭外，本轮补齐
  **→**（折叠的分支→展开；已展开→进入第一个子项）、**←**（已展开的分支→折叠；否则→跳到父项），
  以及**带 1 秒缓冲区的循环 typeahead**（连续输入累积匹配，同一字母可继续跳到下一个匹配项）。
- **行内 roving tabindex 与 `aria-level`**：树里始终只有一个可 Tab 到达的行（其余 `tabindex=-1`），
  焦点移动时同步更新，因此整棵树是一个 Tab 站点（与 Radix 的树一致），每行还带上 `aria-level`。

## [0.4.18] — 2026-09-20（对标批次 v0.3.7v）

### Added

- **后台「服务状态」页的详情弹层按上游组件渲染**：上游有两套渲染——`admin/service-detail.tsx`
  按载荷形状分支（对象数组 → 以首个对象的键为表头的表格；普通对象 → `键/JSON.stringify(值)` 的定义列表；
  字符串 → 卡片内 `<pre><code>`；其他 → 原样），`admin/task-executor-detail.tsx` 对 `task_executor`
  按执行器分组渲染卡片（标题 + `Lag:`/`Pending:` + done/failed 柱状图 + 图例 + 点击柱子的 JSON 提示）。
  RayRAG 之前把两种载荷都塞进一个 JSON 树里，现在按上游落地：标题也随之变成
  `Task executor detail` 或 `Service {name} detail`。
- **拆成两个弹层并补上行内图标动作**：上游的行操作是悬停显示的 `Settings2`（Extra information，渲染
  `item.extra` 的 JsonView）与 `ClipboardList`（服务详情，`GET /api/v1/admin/services/{id}`）两个图标按钮，
  RayRAG 之前合并成一个 "Show details" 按钮。

## [0.4.17] — 2026-09-20（对标批次 v0.3.7u）

### Added

- **搜索空结果用上游插画**：上游 `components/empty/empty.tsx` 在 `EmptyType.SearchData` 时渲染
  `assets/svg/empty/no-search-data-{dark,bri}.svg`（84×104，`iconWidth={80}`，配 `common.noResults` 文案）。
  RayRAG 之前只有一行 "No results found." 文字；现在把两个主题变体都内联进页面，按现有 `body.light`
  主题类切换（暗色 `no-search-data-dark`／亮色 `no-search-data-bri`），文案沿用 `common.noResults`。

## [0.4.16] — 2026-09-20（对标批次 v0.3.7t）

### Added

- **搜索页的文件筛选改成上游的 `RetrievalDocuments` 浮层**：上游 `next-search/retrieval-documents/index.tsx` 里，
  侧栏的「文件」控件是一个触发器（lucide `Files` 图标 + `已选/总数` + `knowledgeDetails.subbarFiles` 文案，
  右侧 X 清空、竖分隔线、ChevronDown），点开是带搜索框的浮层，逐文档复选框（选中打勾），底部
  `common.clear` / `common.close`；有文档时才渲染。RayRAG 之前只有一个「Document Type」下拉，现在按上游实现，
  并把选中的 `doc_ids` 直接送进 `POST /api/v1/retrieval`（勾选即生效，与上游 `onTesting` 一致）。
  上游没有文档类型过滤，因此该下拉一并移除（结果不再按扩展名二次过滤）。
- 新增 `t()` 命名空间键 `knowledgeDetails.subbarFiles`：它的中文是「文件列表」，与导航 `header.fileManager`
  的「文件」不同，按仓内既有约定用命名空间键区分。

## [0.4.15] — 2026-09-20（对标批次 v0.3.7s）

### Added

- **补齐上游的「用户资产」管理接口**：上游 `admin/server/routes.py` 的
  `GET /api/v1/admin/users/{username}/datasets` 与 `/agents` 此前缺失（详情页的 Agent 页签永远是空的）。
  现在两个接口按 `UserServiceMgr.get_user_datasets` / `get_user_agents` 的语义返回：数据集给出
  id/name/avatar/doc_num/chunk_num/token_num/language/permission/create_date/update_date，智能体给出
  `title` / `permission` / `canvas_category`（只取 `_` 前的第一段）/ `avatar`；未知账号 404 `User not found`，非管理员 403。
- **用户详情接口改为上游的列表形态**：上游 `UserMgr.get_user_details` 返回的是「匹配账号数组」，控制台读
  `data.data[0]`，而 RayRAG 之前返回单个对象。现在同样返回数组，字段名与上游一致
  （avatar/email/language/last_login_time/is_active/is_anonymous/login_channel/status/is_superuser/create_date/update_date），
  另外保留 RayRAG 用户表需要的 id/nickname/role/api_key_count。
- **`/admin/users/{id}` 页面按上游并行取数并分页**：与上游一样 `Promise.all([detail, datasets, agents])` 同时请求三个接口；
  Dataset / Agent 两个页签的表格带上了头像+名称（Agent 为标题）列与页脚分页（`Total N` + 10/20/50/100 + Previous/Next）。

## [0.4.14] — 2026-09-20（对标批次 v0.3.7r）

### Added

- **补齐上游的沙箱提供方管理接口与页面**：上游 `admin/server/routes.py` 的
  `/api/v1/admin/sandbox/{providers,providers/{id}/schema,config,test}` 五个动作此前未实现（RayRAG 只有本机沙箱与
  executor-manager 远程执行）。本轮按 `admin/server/services.py::SandboxMgr` 与 `agent/sandbox/providers/*.py`
  逐字段落地：5 个提供方注册表（local / self_managed / ssh / aliyun_codeinterpreter / e2b，含名称、描述、
  标签）与各自的配置 schema（字段名、标签、说明、默认值、最小/最大值、`secret`、`multiline`、只读部署项，
  共 local 8 / ssh 14 / self_managed 9 / aliyun 6 / e2b 3 项）。配置持久化到系统设置
  `sandbox.provider_type` 与 `sandbox.<provider>`（执行器读的就是这里），密钥沿用 `<redacted>` 哨兵：读接口不回显，
  原样回传即保留原值。
- **连接测试是真的跑代码**：`POST /sandbox/test` 使用上游同一段探针代码（打印 `2 + 2`、`json.dumps`、
  `math.sqrt` 后输出 `TEST_PASSED`），local 走配置的 Python 解释器、self_managed 走 executor-manager 的
  `GET /healthz` + `POST /run`，返回与上游相同的 `{success,message,details:{exit_code,execution_time,stdout,stderr}}`
  与 `Test PASSED | Exit code: 0 | Execution time: …` 文案；ssh 只做 TCP 可达性检查并如实说明本构建不执行远程
  SSH 代码；e2b / aliyun 明确报「本构建未实现该提供方执行」。**不做假成功**。
- **运行镜像内置 python3 与 nodejs**：Code 组件在本机沙箱执行用户代码需要解释器，此前运行镜像（Debian bookworm slim）只有 `curl/libssl3/zlib1g`，
  任何 Python/JavaScript 执行都会以 `No such file or directory` 失败（本机沙箱与 local 提供方的连接测试都会如实报这个错）。现在运行阶段安装
  `python3` 与 `nodejs`（镜像 183MB → 310MB），本机沙箱与 local 提供方开箱可用；也可以在提供方配置里把 `python_bin`/`node_bin` 指向别的解释器。
- **`/admin/sandbox-settings` 页面结构对齐上游**：标题与描述、`Provider selection` 单选卡片网格（图标/名称/
  描述/标签）、`{name} configuration` 卡片（Test connection + Save 配置按钮）、按 schema 生成的字段
  （必填星号、说明、数值上下界、布尔开关、`multiline` 文本域、`secret` 密码框、只读项进
  `Deployment Defaults` 折叠区；SSH 额外有 `Runtime Settings`／`Authentication`（Password / Private Key 切换）／
  `Execution` 分组）以及 `Connection test result` 弹层（成功/失败摘要、退出码、耗时、stdout/stderr）。
  RayRAG 原有的持久变量表保留为独立卡片。

## [0.4.13] — 2026-09-20（对标批次 v0.3.7q）

### Fixed

- **权限字段改用与语言字段同一个上游 `SelectWithSearch`**：上游 `dataset-setting/permission-form-field.tsx`
  与 `general-form.tsx` 用的是同一个组件，而 RayRAG 之前是另一套 `input-select` 实现。现在两处共用一个
  组件（新增 `SWS_JS` 脚本常量 + `push_select_with_search` / `push_permission_field` 渲染函数），
  触发器的 `data-testid='ds-settings-basic-permissions-select'`、`role=combobox` 与选中态 Check 图标一致；
  因为只有 2 个选项（`PermissionRole` 的 `me`/`team`），按上游 `showSearch` 规则**不渲染搜索行**。
- **权限提示文案取自正确的命名空间**：改为上游 `knowledgeConfiguration.permissionsTip`
  （"If it is set to 'Team', all your team members will be able to manage the dataset." /
  "如果把知识库权限设为“团队”，则所有团队成员都可以操作该知识库。"），并补上 `knowledgeConfiguration.me`
  的中文「只有我」；此前用的是另一命名空间（agent 设置）里的旧句。
- **修掉旧版 `/kbs/{id}` 页面的权限控件**：该页仍渲染旧的 `input-select` 标记，但其脚本里
  `openPermPicker` 早已不存在（点击即 `ReferenceError`）。现在该页也注入共享组件并注册实例。

## [0.4.12] — 2026-09-20（对标批次 v0.3.7p）

### Fixed

- **数据集语言字段改为上游 `SelectWithSearch` 组件**：上一轮误把该字段对齐到 `constants/common.ts::LanguageList`，
  但上游 `dataset-setting/general-form.tsx` 实际取的是 `Object.keys(LanguageTranslationMap)`——共 27 项，
  **包含 Korean**，并且 `Indonesian/Indonesia`、`Portuguese BR/pt-br/pt-BR` 这类近重复键都在其中，
  选项文案就是键本身（该字段是“文档语言”而不是界面语言）。本轮把原生 `<select>` 换成上游的
  `role=combobox` 触发器 + cmdk 浮层：选项超过 5 个因此渲染搜索行（`搜索...` / `Search...`），
  选中项右侧显示 `Check` 图标，无匹配时显示 `CommandEmpty`（`common.noDataFound`：没有找到数据。/ No data found.），
  点击空白或 Escape 关闭且不改值。
- **语言值现在真的会保存与回填**：`PUT /api/v1/datasets/{id}` 的请求体补上 `language`，
  `loadConfig` 用 `kb.language` 回填，此前该字段只是装饰性控件。
- **补齐上游的集合删除接口**：上游 `web/src/utils/api.ts::rmKb` 是 `DELETE /api/v1/datasets` +
  `{ids:[...]}`（`ids` 为 `null` 时需配合 `delete_all`，空数组不删任何东西，返回 `success_count`），
  RayRAG 此前只有 `DELETE /api/v1/datasets/{id}`。本轮新增该集合路由（含跨租户 id 的
  `lacks permission for datasets` 报错与 `delete_all`），并把首页卡片、知识库列表卡片、详情页删除弹层
  三处调用改到该路由；单条路由保留兼容。
- **后端补上 `Knowledgebase.language` 顶层列**：上游 `db_models.py` 里 `language` 是
  `knowledgebase` 表的列（`CharField(max_length=32)`，默认按进程 locale 取 `Chinese`/`English`），
  不在 `parser_config` 里。RayRAG 的 `KnowledgeBase` 结构体新增该字段（`#[serde(default)]` 保证旧记录照常加载），
  `POST/PUT /api/v1/datasets/{id}` 接受顶层 `language` 并写入该列，`parser_config` 不再夹带该键；
  空值与超过 32 字符的值按上游约束拒绝。

## [0.4.11] — 2026-09-20（对标批次 v0.3.7o，未单独发版，随 0.4.12 一起发布）

### Fixed

- **数据集配置的语言选项与上游 `LanguageList` 逐项一致**：删除了 RayRAG 自行添加的 `Korean / 한국어`
  （上游 `constants/common.ts::LanguageList` 只有 16 项，且没有韩语），顺序与显示名（`LanguageMap`：
  简体中文 / Русский / Bahasa Indonesia /Tiếng việt / 日本語 / Português BR / Deutsch / Français /
  Italiano / Български /العربية / Türkçe / Nederlands）保持上游原样，新增回归测试锁定值与顺序。

## [0.4.10] — 2026-09-20（对标批次 v0.3.7n）

### Fixed

- **头像裁剪弹层按钮本地化**：共享外壳里的 `Cancel` / `OK` 改由 `t()` 注入（新增 zh 词条 `确定`），
  中文界面不再出现英文按钮，与上游 `common.cancel` / `common.ok` 一致。

## [0.4.9] — 2026-09-19（对标批次 v0.3.7m）

### Fixed

- **`/user-setting/profile` 的头像裁剪改为拖拽弹层**：上一轮发现该页并未引入共享脚本块，
  本轮把裁剪交互抽成模块常量 `AVATAR_CROP_JS`（`<script>` 整块）并在 `user_setting_layout` 外壳中统一注入，
  profile 上传路径切换为 `openAvatarCropModal(...)`，与数据集页共用同一套「80% 初始选区 + 拖拽夹紧 + 64×64 输出」行为。

## [0.4.8] — 2026-09-19（对标批次 v0.3.7l）

### Added

- **头像裁剪弹层支持拖拽选择（对齐上游 `components/avatar-upload.tsx`）**：打开裁剪弹层时按图片较短边的
  80% 生成初始选区并居中，鼠标按住选区即可拖动（越界自动夹紧到图片范围），OK 时把该区域绘制成 64×64 PNG。
  此前是「居中固定方形裁剪」的近似实现。

### Note

- 发布说明自此以**英文为默认**（GitHub Release 记录同样）。

## [0.4.7] — 2026-09-19（对标批次 v0.3.7k）

### Changed

- **后台用户表格的状态单元格对齐上游**：非本人行改为「Active / Inactive」下拉框（改动即保存），
  当前登录管理员自己的那一行改为 Active/Inactive 徽章（上游对本人行不提供状态控件）；
  原先动作列里的 Enable/Disable 按钮随之移除（上游动作列只有改密/删除等操作）。

### Note

- **审计结论（撤回一条非特性）**：`admin/users.tsx` 里没有 Excel 导入弹层（全文件无 xlsx/import 相关实现），
  台账中该残留项据此撤回；「行内角色编辑」的角色下拉由 `IS_ENTERPRISE` 控制（开源版不加载 roleList、
  更新接口也是企业版），按企业专属处理、不计入开源对标缺口。

## [0.4.6] — 2026-09-19（对标批次 v0.3.7j）

### Added

- **后台「用户管理」表格补齐上游能力**：新增全局搜索框（按邮箱/用户名/昵称/角色/状态过滤）、
  可排序列头（Email / Nickname / Role / Status，点击切换升/降序并显示 ↑↓）与页脚分页
  （`Total N` + 页码 + 每页 10/20/50/100）；`Reset` 同时清空角色/状态筛选、搜索词与排序状态。
  与 `service-status.tsx` 不同，`users.tsx` 上游并未禁用排序，故本页保留排序。

## [0.4.5] — 2026-09-19（对标批次 v0.3.7i）

### Changed

- **后台「额外信息」弹层改为上游 `JsonView` 风格的树形渲染**：不再把 JSON 直接 `JSON.stringify` 成一段文本，
  而是递归渲染键/值树——对象与数组带 `{n}`/`[n]` 折叠按钮（`aria-expanded` 可切），字符串/数字/布尔/null
  分别着色，等宽字体、逐层缩进，与 RAGFlow 后台详情弹层的 `JsonView` 展示一致。

## [0.4.4] — 2026-09-19（对标批次 v0.3.7h）

### Added

- **管理后台「服务状态」表格补齐上游工具栏与页脚**：新增全局搜索框（上游 `header.search`，按 ID/名称/类型/主机/端口过滤）
  与卡片页脚的 `RAGFlowPagination` 等价控件（`Total N` + 页码 + 每页 10/20/50/100），`Reset` 同时清空类型筛选与搜索词。

### Note

- **审计结论（撤回两条非特性）**：上游 `service-status.tsx` 里 `enableSorting: false` 全局禁用列排序、
  `useQuery` 未设置 `refetchInterval`（即没有自动轮询）；台账中“tanstack 列排序 / 自动刷新轮询”两项残留据此撤回，
  真正的缺口只有分页与全局搜索（本版已补）。

## [0.4.3] — 2026-09-19（对标批次 v0.3.7g）

### Fixed

- **「可用模型」面板元素语义与标题字号对齐上游**：`un-add-model.tsx` 的根节点是
  `<aside className="text-text-primary h-full flex flex-col">`、标题是 16px 的 `<h3>`；
  RayRAG 之前用 `<section>` + 默认字号 `<h2>`，现改为 `aside` + `.available-models-title`（16px/400），
  同时保留 `id='availableModels'`、`data-testid='available-models-section'` 与新增 `aria-label`。

## [0.4.2] — 2026-09-19（对标批次 v0.3.7f）

### Fixed

- **`/user-setting/model` 页面布局对齐上游**：改为上游 `setting-model/index.tsx` 的两栏卡片——
  外框 `.5px` 边框 + 圆角，左栏 3/5（设置默认模型 + 已添加模型，间距 16px、左右 20px 内边距、右侧 `.5px` 分隔线），
  右栏 2/5（可用模型列表），两栏各自滚动；卡片内加入上游的单个 `<Spotlight />` 背景光晕
  （opacity .8、coverage 60%、中心 50%/190%、深色白/浅色 rgb(194,221,243)）。

## [0.4.1] — 2026-09-19（对标批次 v0.3.7e）

### Fixed

- **模型列表选择器对齐上游 `ToggleList`**：触发器图标改为 lucide `ChevronsDown`（16px），展开时
  以 200ms 过渡旋转 180°（`transition: transform .2s`），不再是无动效的 `⌄` 文本；
  搜索行的放大镜改为 lucide `Search`、清除按钮改为 lucide `X`，并在模型列表加载期间显示
  `Loader2` 同款旋转指示器（加载中隐藏清除按钮，加载完成且有输入时才显示），与上游
  `searchLoading && <Loader2 className="size-4 shrink-0 animate-spin" />` 的互斥关系一致。

### Note

- **审计结论**：上游 `used-model.tsx` 的 viewMode 编辑实例入口（设置齿轮 + `handleSettingsClick`）在
  v0.26.4 中是注释掉的，`onEditInstance` 只被转递、从未被调用，因此“viewMode 立即持久化路径”在
  当前 UI 中不可达；RayRAG 的“新增实例”弹层即为行为等价实现。

## [0.4.0] — 2026-09-19（对标批次 v0.3.7d）

### Changed

- **版本号进入 0.4.0**：`Cargo.toml version = 0.4.0`，对标切片常量同步为 `v0.3.7d`；
  自此每次公开发布都会带上新的版本号与 GitHub Release 记录。

### Fixed

- **模型供应商图标回归上游 `LlmIcon` 画法**：去掉 RayRAG 自加的字母色块（`background:var(--nab)` + 圆角），
  统一 32px 品牌字形、颜色继承上下文（Available 卡片 `text-text-primary`，Added 行 `text-text-secondary`），
  与上游 `un-add-model.tsx`/`used-model.tsx` 的 `<LlmIcon width={32}>` 一致。
- **已添加模型列表几何**：列表容器底色改回上游 `bg-bg-card`，行分隔线 `.5px`（上游 `border-b-[0.5px]`）。
- **实例删除确认弹层对齐 `ConfirmDeleteDialog`**：标题 `common.deleteModalTitle`、右上角 lucide `X` 关闭、
  底部 `common.cancel` / `common.delete`，容器与按钮带上游 testid
  （`confirm-delete-dialog` / `-cancel-btn` / `-confirm-btn`）。

## [0.3.4] — 2026-09-19（对标批次 v0.3.7c）

### Added

- **模型类型多选（`MultiSelect`）补齐 cmdk 行为**：`(Select all)` 全选行（`common.selectAll`）、
  搜索时按 cmdk 1.1.1 的 `command-score` 打分并重新排序、空结果提示改为上游 `common.noDataFound`
  （"No data found." / 没有找到数据。，中文另有 `common.searching` 文案备用）。

### Fixed

- **自定义模型对话框校验对齐 `DynamicForm`**：必填提示改为上游拼接的 `"{label} is required"`
  （英文 "Model name is required"，中文 "模型名称 is required"）；`model_types` 恢复为可选
  （上游 zod schema 中多选字段 `optional()`，`modelTypeRequired` 只用于 switch-group）；
  未勾选特性时不再发送 `extra`（对齐 `extra: item.features ? {is_tools} : undefined`）。

## [0.3.4] — 2026-09-19（对标批次 v0.3.7b）

### Fixed

- **模型供应商卡片的标签顺序**（上游 `un-add-model.tsx` 用 `sortModelTypes`/`orderMap` 排序，而非后端返回的字母序）：
  14 张卡片的标签顺序与上游逐字对齐（如 OpenAI `LLM Embedding TTS ASR`、StepFun `LLM TTS ASR VLM`、
  Tongyi-Qianwen `LLM Embedding Rerank TTS ASR VLM OCR`）。
- **文档链接范围**：卡片右上角的 `↗` 只对上游 `constants/llm.ts::APIMapUrl` 的 47 个厂商渲染
  （Ollama/VLLM/LM-Studio/LocalAI/Xinference/New API 上游没有该链接，RayRAG 之前多渲染了 6 个）。
- **默认模型行标签**（`system-setting.tsx`）：`Embedding`→`Embedding Model`（zh 嵌入模型）、
  `Rerank`→`Rerank model`（zh Rerank模型），并按语言本地化。

## [0.3.4] — 2026-09-19（对标批次 v0.3.7a）

### Added

- **智能体日志页 `/agent-log-page/:id`**（上游 `pages/agents/agent-log-page.tsx` 417 行、
  `agent-log-detail-modal.tsx` 151 行、`hooks/use-export-agent-log.ts` 86 行）：
  面包屑（Agent › 画布标题 › Log）、工具栏（Export、`ID/Title` 关键字框、`Latest date` 区间、
  Search/Reset）、八列可排序表格（ID / User ID / Title / State 圆点 / Number / Latest date /
  Create date / Version）、RAGFlow 分页（10/20/50/100）、click-through 详情弹层（CJK 15 字截断标题、
  逐条消息与引用块）与带 BOM 的 CSV 导出（`agent-logs-{canvasId}-{date}.csv`）。
- **会话列表 API**：`GET /api/v1/agents/{id}/sessions`、`/sessions/{session_id}`，以及浏览器侧
  `api.ts::fetchAgentLogs` 使用的 `GET /v1/canvas/{id}/sessions` 别名（同一处理函数）。查询语义对齐
  `API4ConversationService.get_list`（keywords/日期区间/orderby+desc/分页上限 100/`dsl=false`/
  `exp_user_id`），行结构对齐 `_normalize_agent_session`。
- **会话运行元数据**：`Conversation` 新增 `dsl`/`errors`/`version_title`；智能体运行后写入画布快照、
  版本标题与错误状态，日志页的 Version 列与红/绿状态点取自真实运行结果。
- `/agent/{id}` 编辑器页新增 Log 入口（对应上游编辑器下拉菜单的 `flow.log`）。

### Fixed

- **容器时区与浏览器不一致导致日期筛选为空**：新增 `RAYRAG_TIMEZONE`（`docker-compose.yml` 注入容器 `TZ`，默认 `Asia/Shanghai`，与 RAGFlow 的 `TIMEZONE` 同一语义），日志行时间与日期区间比较改用服务端本地时钟（对齐上游 `DataBaseModel` 的 `datetime.now` 本地时间与 `CustomJSONEncoder`）。

### Note

- 上游 v0.26.4 的日志页自身请求了已随 `sdk` blueprint 迁移而消失的 `/v1/canvas/{id}/sessions`，
  即官方页面在该版本 404；RayRAG 同时提供新旧两种拼写，页面可用且 REST 面保持一致。

## [0.3.4] — 2026-09-18（对标批次 v0.3.4e–v0.3.6d）

### Changed

- **检索请求改为上游形状（`search_id` 驱动）**：`/api/v1/retrieval` 支持 `search_id` —— 由应用记录
  提供 `kb_ids`/`top_k`/`similarity_threshold`/`vector_similarity_weight`/`rerank_id`/`doc_ids`，
  调用方显式给出的字段优先；同时接受上游单数 `kb_id` 与 `size` 别名。`/search` 页面改为发送
  `{question, highlight, page, size, search_id, kb_id, doc_ids}`，侧栏控件仅在用户实际修改后才覆盖
  对应参数（`window.__SEARCH_TOUCHED`）。

### Fixed

- **`BM25+Vector` 关闭时被强制 `vector_similarity_weight=0`**：该行为是 RayRAG 自造，导致未命中关键词的
  提问检索不到任何结果（上一轮 CDP 需手动打开开关才能复现结果）。现在权重由应用配置或用户显式修改决定，
  与上游 `SimilaritySliderFormField` 一致；探针在不触碰侧栏的情况下即可拿到真实结果。

### Added

- **`/search` 的 AI 总结流与思维导图**（上游 `next-search/search-view.tsx` + `mindmap-sheet.tsx` 残留收口）：
  新增 `POST /api/v1/searches/{search_id}/completions`（按应用 `kb_ids`/`chat_id`/`llm_setting` 生成，
  以 `chat.completion.chunk` 增量 + `chat.completion.references` + `[DONE]` 的 SSE 输出，与聊天页同一套解析）
  与 `POST /api/v1/chat/mindmap`（LLM 生成 `{name,children}` 树）。
  前端：问候语输入行（`search.searchGreeting` 占位、lucide `X` 清空、分隔线、圆形提交按钮，
  流式期间切换为停止控件并可中断）、`AI summary` 区块（24px 标题、流式骨架屏、边框内滚动
  208px 的 markdown 答案 + 引用 chip + 分隔线）、`Total: N` 结果条与思维导图按钮/抽屉
  （`chunk.mind` 标题、进度条、缩进树、关闭按钮），标签全部按语言注入并修正了注入时机。

### Fixed

- **`/search` 检索在知识库 chip 尚未加载时丢参数**：`runSearch` 只从 DOM chip 取 kb_ids，
  页面初始化竞态下会发出无 `kb_ids` 的请求并被服务端以 400 拒绝；现回退到应用的
  `search_config.kb_ids`（与上游直接读应用配置一致）。
- **搜索页脚本语法错误**：新增 JS 在 Rust 普通字符串中的转义（`\'`、`\n`）与一处多余花括号
  导致整段脚本不可解析；已修复并以 `node --check` 逐 `<script>` 块复验 5/5 通过。

### Added

- **`/chunk` 切片工作台子树**：新增上游 `Routes.Chunk` 的四个路由——`/chunk`（仅头部外壳）、
  `/chunk/parsed/chunks`、`/chunk/chunk/{doc}`、`/chunk/result/{doc}`。头部按上游
  `pages/chunk/index.tsx` 复刻：面包屑（`Agent` 链接到知识库 + 当前文档名）、`Segmented`
  三选项（`Parsed results` / `Chunk result` / `Result view`，活动态与 `data-section` 对齐）、
  ghost 省略号菜单（Refresh/Copy）与带 lucide `Save` 图标的 Save；面板按上游
  `chunk-toolbar.tsx`（30px 粗体标题、ghost `Copy` 图标按钮、outline `Export`）与
  `chunk-card.tsx`（`ParsedPageCard` 页面标签+正文；`ChunkCard` 含 lucide `Annoyed`、
  `Switch` + `Active` 文案、四行截断正文）渲染。
- **数据为真实切片**：数据集/文档由 `?knowledgeId=`（上游 `QueryStringMap.KnowledgeId`）解析，
  切片来自 `/api/v1/datasets/{id}/documents/{did}/chunks`；`Active` 开关经 `Save` 以
  `PATCH …/chunks/{chunk_id}`（`{available}`）持久化，Copy 复制到剪贴板、Export 下载 JSON，
  文档仍在解析时状态行给出提示。
- **台账更正**：上游该子树实为**静态原型**（面包屑 `Agent`/`xxx`、Save 无处理函数、两个面板
  都是 `new Array(10).fill(<同一段示例文本>)`），故 8 个文件由 `reference` 纠正为 `replaced`
  并逐行记录“路由对齐 + 真实数据替代”的取舍（replaced 101→109）。

### Fixed

- **`/chunk` 面板在文档未解析完成时无反馈**：现在状态行显示“文档仍在解析中，请稍后刷新”，
  避免空面板被误认为功能缺失。

### Changed

- **Added models 实例行与实例模型编辑对齐上游 `used-model.tsx`**：实例折叠按钮在
  `View models`/`Hide models` 文案切换时同步替换 lucide `ChevronsDown`/`ChevronsUp`（16px）图标；
  删除按钮由 emoji 改为 lucide `Trash2`（16px）图标按钮；模型行铅笔改为 lucide `Pencil`（12px）
  并使用按语言注入的 `Edit model` 可访问名。
- **`MultiSelect` 组件化复用**：把 v0.3.5z 的 Model type 组合控件抽成可注册前缀的共享实现
  （`piMsRegister/piMsToggleOpen/piMsRender/...`），Add custom model 弹层与**实例模型编辑弹层**
  共用同一控件与同一份上游选项顺序；实例编辑弹层的 Name / Max tokens 置为禁用（仅类型可改），
  标题 `Edit model`、底部 `Cancel`/`Confirm` 与上游 `AddCustomModelDialog` 一致。

### Fixed

- **模型供应商页脚本语法错误**：重构时 `toggleAddedInstance` 丢失 `async` 关键字，导致该页
  整段脚本解析失败、供应商弹层无法打开。已修复并用 node --check 逐 `<script>` 块复验（7/7 通过）。

### Changed

- **模型供应商弹层的 List models / Add custom model 逐行对齐**：模型行改为上游渲染——类型 chip 直接显示
  原始 `model_types` 字符串（不再经 `mapModelKey` 映射）、`All models` 哨兵行同款左右分组、空搜索行使用
  上游 `setting.noMatchingResults`（两地语言均为字面量 "None"）；行内编辑改为上游 lucide `Pencil`（12px，
  可访问名 `Edit model`）；底部 "Add custom model" 改为 `role=button tabindex=0` + lucide `Plus` +
  键盘 Enter/Space 与上游内边距。
- **Add custom model 弹层的 Model type 改为上游 `MultiSelect` 组合控件**：徽章触发器（含逐个 ×移除、
  `Select value` 占位、悬停清除）、搜索框（`common.search` + "..."）、带 `h-4 w-4 rounded-sm border
  border-primary` 勾选方块的选项行、底部 `common.clear` / `common.close`；`maxCount=100` 保持全部可见；
  `Model features` 改为上游 `rounded-md border p-3` 容器。顺带修正两处 i18n 取值：`common.clear` 由
  「清空选择」改为上游「清空」，`Select value` 由错误值改为「请选择」。

### Fixed

- **List models 目录请求会覆盖用户刚添加的自定义模型**：发现请求返回时整表替换，正在打开弹层期间新增/
  编辑的模型会被静默丢弃。改为**合并**（保留本地自定义项）并对追加按名称去重，等价上游
  `setModels(prev => prev.some(m => m.name === item.name) ? prev : [...prev, item])`。

### Added

- **RAGFlow 管理控制台（`/admin` 子树）**：`/admin` 由旧的 RayRAG 仪表盘改为上游语义的
  **管理端登录页**（`pages/admin/login.tsx`：共享登录背景 + 三束 spotlight、logo + `RAGFlow`、
  `Admin console` 标题、邮箱/密码/记住我 + 全宽登录按钮、卡片下方主题开关），登录走
  `POST /api/v1/admin/login`（仅超级管理员，非管理员返回上游文案
  `Only superuser can login admin system`，令牌写入 `Authorization` 响应头）。
  新增导航布局（288px 侧栏：logo + `admin.title`、三个社区入口 `Service status` /
  `User management` / `Sandbox settings` 配 lucide `ServerCrash`/`UserCog`/`Zap` 图标、
  版本号 + 主题开关 + 登出）：`/admin/services`（组件表 + 类型筛选 + 详情弹层）、
  `/admin/users`（用户表 + 角色/状态筛选 + 新建/改密/删除弹层）、
  `/admin/users/{id}`（账号详情 + 状态/密码/删除操作）、`/admin/sandbox-settings`
  （沙箱变量表）。新增 API：`POST /api/v1/admin/login`、`GET /api/v1/admin/logout`、
  `GET /api/v1/admin/services`、`GET /api/v1/admin/services/{id}`。
  未登录/非管理员访问控制台页面由服务端 307 回 `/admin`（对齐上游 `AdminAuthorizedLayout`）。

### Added

- **`install.sh` 一键部署脚本**：面向第一次使用的用户，检查 Docker/Compose、幂等生成 `.env`
  （随机 24 位 PostgreSQL 与管理员密码，重复执行不覆盖既有配置）、支持
  `--port/--no-build/--cn-mirror/--global-mirror/--dry-run/--help`、统一
  `RAYRAG_CMD_TIMEOUT`（默认且上限 2 小时）、校验 compose 配置、构建并启动、等待健康检查、
  打印局域网访问地址与登录账号；输出跟随系统语言（zh*/en）。
- **双语 README**：英文默认 `README.md` + 中文 `README.zh-CN.md`，面向零基础用户
  （五分钟部署、首次使用六步、65 家 provider 与本地模型对照表、12 条 FAQ、升级/备份/卸载、
  纯 Linux 安装）；原技术长文档迁至 `docs/advanced.md`。

### Changed

- **`/user-setting/model` 的 Set default models 段落逐行对齐上游 `system-setting.tsx`**：
  `article > header`（24px/500 标题 + 14px 说明）、`px-7 py-6 gap-6 max-h-[70vh] overflow-y-auto`
  边框滚动容器、六行 `flex gap-3`（label `w-1/4` / control `w-3/4`，仅 LLM 行带 `*`）；
  placeholder 改为上游 `setting.selectModelPlaceholder`；六个 tooltip 走 `t()` 并补 zh.ts 逐字译文；
  触发图标改为 lucide `CircleQuestionMark`（12px 描边圆 + 问号，保留悬停弹窗）。
- **同名异译 i18n 键分离**：上游 `setting.selectModelPlaceholder`（en `Select model` /
  zh `请选择模型`）与 `memories.selectModel`（en `Select model` / zh `选择模型`）英文同文、
  中文异译，扁平 `t()` 表改用上游 i18n 路径作为名字空间键区分。

### Added

- **上游路由表对齐（`routes.tsx` 第一批）**：新增 `/login-next`（上游 `/login` 与 `/login-next` 同页双入口）、
  上游规范的单数详情路径 `/agent/:id` 与 `/search/:id`（保留 RayRAG 既有 `/agents/{id}`、`/searches/{id}` 兼容别名）、
  上游独立文档查看器 `/document/:id`（按文档记录反查知识库渲染，未知 id 返回真实 404）、
  以及公开嵌入页 `/search/share`（`?tenantId=` 解析检索应用、`visible_avatar` 控制应用头像行，
  与 `/search` 共用 `search_page_markup`）。

### Fixed

- **Docker 构建可能无限挂起**：zvec 预编译库下载只有 `--retry` 而没有连接/总超时，国内镜像站卡死时
  构建会静默等待（本轮实测挂起 20+ 分钟）。改为 `--connect-timeout 20` +
  `--max-time ${ZVEC_DOWNLOAD_TIMEOUT}`（默认 900 秒，`docker-compose.yml`/`.env.example` 可覆盖）
  `--retry 3 --retry-all-errors`，主镜像超时后清理半包并自动切换备用镜像。
- **一条从未执行的测试**：`available_models_pills_and_local_provider_fields_match_upstream`
  缺少 `#[tokio::test]` 属性，长期被当作死代码；补属性后首次真实执行并通过。
- **`t()` 死分支**：删除被 `"混合搜索"` 永久遮蔽的 `"Hybrid" => "混合检索"`。

## [0.3.4] — 2026-09-18（对标批次 v0.3.4e–v0.3.5v）

### Added

- **Apache-2.0 `LICENSE` 与 `NOTICE`**：注明对标来源 RAGFlow v0.26.4 与第三方资产许可。
- **净化发布流程**：当前提交导出为净化快照（剔除开发脚本、抓取产物、模型权重与运行时状态，并对机器标识脱敏）后推送 GitHub；脚本保留在内部工程仓。

### Changed

- **相关搜索端到端对齐上游 next-search**：检索应用配置补齐 `search_config` 全字段（doc_ids/chat_id/llm_setting/
  cross_languages/use_kg/highlight/keyword/web_search/related_search/query_mindmap/summary，`top_k` 默认 1024）；
  `/search` 设置栏与 `/searches/{id}` 编辑页新增 `Enable related search` 开关；检索结果下方按上游渲染
  `Related search` 标题与最多 5 个 chip，点击即以该问题重跑检索。
- `POST /api/v1/chat/recommendation`：命中 `search_id` 时使用该应用的 `chat_id` 实例与 `llm_setting`（剔除
  `parameter`，默认 temperature 0.9），非法设置返回 400。
- **CDP 探针** 内部 UI 遍历脚本（`search-related-cdp.mjs`：开关置位、chip 数量与文本、请求体、点击重跑、
  关闭后不再请求、编辑页保存后 GET 复核、探针应用清理。

- **Joined teams 表逐列对齐上游 `tenant-table.tsx`**：四列 Name（头像+昵称）/ Update date（三态排序按钮，↕/↑/↓）/
  Email / Action；`invite` 行渲染链接式 Agree/Refuse，`normal` 且非本人租户渲染纯图标 ghost Quit，owner 行留空；
  补 loading 转圈行与 `No data` 空行；日期统一走上游 `utils/date.ts::formatDate` 的 `DD/MM/YYYY HH:mm:ss`，
  Quit 用 lucide `LogOut` 图标。后端 `/api/v1/tenants` 关联 owner 的 nickname/email/avatar 与 `update_date`
  （自持租户合成行回退到账号创建时间），成员接口同步补 avatar。
- **CDP 探针** 内部 UI 遍历脚本（`joined-teams-cdp.mjs`：live payload 键、loading 行、四态排序顺序与图标、
  搜索过滤、空数据行、恢复默认。

- **技能检索配置弹层对齐上游 `search-config-modal.tsx`**：补 `configDesc` 说明、权重/阈值改滑杆并带实时读数
  （`Hybrid (N% Keyword + M% Vector)` / `.toFixed(1)`）、三档说明与 Top K(1..100)、新增 Index Fields 四字段编辑器
  （Name/Tags/Description/Content 各带开关与权重，保存写入 `field_config` 并支持回填），全量文案中英双语。
- **CDP 探针** 内部 UI 遍历脚本（`skills-config-cdp.mjs`：滑块实时文案、字段编辑、保存后 GET 复核与重开回填。


### Added

- **数据集标签接口**：`GET /api/v1/datasets/{id}/tags`（按文档元数据聚合 `[[tag, count], …]`，count 降序）、
  `DELETE`（`{tags:[…]}`）与 `PUT`（`{from_tag,to_tag}` 重命名并去重），校验文案与未知数据集 404 对齐上游
  `dataset_api`；实现位于 `src/api/dataset_tags.rs`。
- **配置页标签云 / 标签表**：Tag sets 行下新增上游 `tag-tabs.tsx` 形态的分段切换——CSS 词云（字号随 count 变化、
  hover 显示计数、点击重命名）与标签表（Tag/Count/重命名/删除、空态与状态行）。
- **CDP 探针** 内部 UI 遍历脚本（`dataset-tags-cdp.mjs`：种子标签 → 云/表视图 → 重命名 → 删除，逐步复核 GET 结果。

### Fixed

- **标签重命名会残留旧标签**：`batch_update` 对数组是合并语义，改用整条记录 `replace()` 重写。
- **标签脚本跨页引用** `PI_PICKER_LABELS` 导致 `ReferenceError`，改为数据集页自带标签表；
  并清理 `t()` 中重复的 `"Rerank Model"`/`"Cancel"` 分支（unreachable pattern）。


### Changed

- **检索页重排字段对齐上游 `search-setting.tsx`**：开关文案改为 `search.rerankModel`（Rerank Model/重排模型），
  打开后显示共享重排模型树（必填），无模型时提交被上游文案
  「Rerank model is required when rerank is enabled」拦截且不发请求；关闭开关把 Top K 复位为上游默认 1024；
  检索统一走 `/api/v1/retrieval`，`rerank_id` 仅在开关打开时随包发送。

### Added

- **CDP 探针** 内部 UI 遍历脚本（`search-rerank-cdp.mjs`：home→results 视图、无模型校验、选中模型后的
  请求体断言、关闭后 top_k=1024，逐步截图。


### Changed

- **数据集权限字段对齐上游 `PermissionFormField`**：改为可搜索组合控件（`SelectWithSearch` 形态）+
  `PermissionRole` 两项（`me`=Only me / `team`=Team）+ 上游 label/tooltip + `ds-settings-basic-permissions-select`。
- **权限值统一为 `me`/`team`**：`normalize_permission()` 在存储加载、创建、更新与数据集 API 四处归一，
  历史 `private` 自动纠正；`GET /api/v1/datasets` 现在返回 `me`/`team`。
- **数据集头像改用上游 `AvatarUpload` 组合**：空态「＋ Upload」、64×64 预览 + 铅笔重选、圆形 `Remove image`、
  扩展名白名单与 64×64 canvas 裁剪，带 `ds-settings-basic-avatar-upload`。
- **CDP 探针** 内部 UI 遍历脚本（`dataset-permission-avatar-cdp.mjs`：覆盖权限弹层过滤/选择/保存回读与头像三态。

### Fixed

- **配置页横向溢出（28px）**：设置表格的固有宽度把 `.ds-content` 轨道撑出视口；现允许轨道收缩
  （`.ds-content{min-width:0}`）并让卡片内部横向滚动（`.ds-content>.card{overflow-x:auto}`），实测 `docScrollW == clientW`。
- **数据集头像脚本跨页引用**：初版误用 Profile 页的 `PROFILE_AVATAR_EXT`/`profileAvatarCrop`（页面脚本各自独立
  作用域）导致 `ReferenceError`，现改为数据集页自带的 `DATASET_AVATAR_EXT`/`cfgAvatarCrop`。


### Fixed

- **Logs 面板偶发空白（竞态）**：`loadLogs()` 在页面 `badge()` 定义之前就可能拿到响应，触发
  `ReferenceError: badge is not defined` 使日志表不渲染；现把 `badge()` 同时定义在 Logs 脚本内，并新增
  `dataset_logs_script_defines_badge_before_use` 断言定义早于首次调用。

### Added

- **OAuth 登录回调全链路**：`GET /api/v1/auth/oauth/{channel}/callback` —— 单次消费 `state`（10 分钟 TTL）、
  `token_url` 换码、`userinfo_url` 取用户（github 另取 primary email）、未知邮箱自动注册（昵称/头像落库）、
  签发会话并通过 `rayrag_token` cookie 跳 `/?auth=<id>`；失败按上游语义 302 到
  `/?error=invalid_state|missing_code|token_failed|userinfo_failed|email_missing|register_failed`。
- **`StateStore`/`UserStore::issue_token_for`**：前者为无状态服务提供 OAuth state 生命周期，后者为已认证用户签发 24h 会话。
- **回归护栏** `oauth_callback_flow_matches_upstream`：本地假 provider 驱动真实换码/userinfo 请求，并覆盖
  成功注册 → 发会话 → cookie 校验的完整链路。
- **配置透传**：`docker-compose.yml` 增加 `REGISTER_ENABLED` / `DISABLE_PASSWORD_LOGIN` / `RAYRAG_OAUTH_CONFIG`，`.env.example` 同步说明与示例。


### Added

- **登录渠道端点**：`GET /api/v1/auth/login/channels`（`[{channel, display_name, icon}]`，无配置返回空数组）与
  `GET /api/v1/auth/login/{channel}`（302 跳转授权地址，未知渠道 400 `Invalid channel name: …`），由新增的
  `src/oauth_config.rs` 注册表驱动（`RAYRAG_OAUTH_CONFIG`/`OAUTH` JSON，按上游 `settings.OAUTH_CONFIG` 形状，
  支持 OIDC `issuer` 推导端点），两条均免登录。
- **SSO-only 模式**：登录页遵循 `disablePasswordLogin`——隐藏两个密码表单与两个切换链接，渠道区块切换为上游
  `py-8`/`w-full` 单列布局；渠道按钮改为上游的 `GET /api/v1/auth/login/{channel}` 跳转。

### Fixed

- **`public_api_path` 动态段失效**：占位符（`{channel}`、`{account_id}`）此前按字面量匹配真实请求路径，导致
  OAuth 入口与既有的渠道路由回调一律 401；改为逐段匹配并加单测（含必须仍需鉴权的反向用例）。
- **登录页脚本 `SyntaxError`**：渠道按钮 `innerHTML` 模板中的嵌套双引号让整段登录脚本失效，改为
  `data-channel` + `addEventListener` 绑定。
- **`/api/v1/auth/login/channels` 此前不存在**（页面静默失败、渠道区永不渲染），现补实现。
- **`/api/v1/version` 短暂 401**：v0.3.5o 重写 `public_api_path` 时漏掉该自建端点，已补回并纳入白名单单测。


### Added

- **`/login` 上游 3D 背景**：`login-next/bg.tsx` 的三条霓虹弧带（viewBox 240/466/704、`#00BEB4` 底描边、
  `#80FFF8` 辉光渐变 + dash 遮罩、`#FFD700` 高光、`feGaussianBlur` 5.2/5.5、16s dash 流动）与
  `components/spotlight.tsx` 的三个径向辉光（0.4/0.3/0.3、coverage 60/12/12、`backdrop-filter: blur(30px)`）全部落地。
- **`registerEnabled` 闸门**：登录页读取 `GET /api/v1/system/config`，为 0 时隐藏注册入口并禁止翻面。
- **两个上游系统端点**（免登录）：`GET /api/v1/system/version`（返回版本字符串）与
  `GET /api/v1/system/config`（`registerEnabled` / `disablePasswordLogin`，取自 `REGISTER_ENABLED` /
  `DISABLE_PASSWORD_LOGIN`），并加入 `public_api_path` 白名单。
- **CDP 探针** 内部 UI 遍历脚本（`login-bg-cdp.mjs`：校验弧带高度/viewBox、spotlight 数量、动画名与时长、
  闸门开关与两个端点响应。


### Added

- **user-setting Profile 页对齐上游**：头部 `setting.profile` + `setting.profileDescription`，190px 标签列 + 边框值框 +
  `PenLine` 编辑按钮（用户名/头像/时区/邮箱/密码），`setting.emailDescription` 与 `setting.avatarTip` 说明文案。
- **AvatarUpload 交互**：无头像时 64×64 虚线上传按钮；有头像时 64×64 预览（hover 铅笔可重选）+ 右上角圆形
  `Remove image` 按钮清空；文件名白名单 `jpg|jpeg|png|webp|bmp`；选中后 canvas 居中裁剪为 64×64 PNG 再上传。
- **Profile 双语**：页面标签/说明/按钮与 JS 校验文案全部中英双语（概要/用户名/头像/时区/邮箱/密码/编辑、
  请选择图片文件、请确认新密码…），弹窗标题按上游保持英文硬编码。
- **CDP 探针** 内部 UI 遍历脚本（`profile-page-cdp.mjs`：验证空态→上传→预览→删除→非法扩展名拒绝→三个编辑弹窗→zh 文案。

### Fixed

- **昵称前端字符集校验**：补 `PROFILE_NAME_PATTERN=/^[\p{L}\p{N} ._'-]+$/u`（对齐上游 `NICKNAME_PATTERN`），
  与 100 字符上限及服务端 `validate_nickname` 一致。


### Added

- **本地提供商字段集对齐上游 `local-llm-configs.ts`**：ModelScope/RAGcon/TogetherAI/Replicate/HuggingFace/GPUStack
  改用显式字段（`model_type` 多选且不预选、`model_name`、`max_tokens` 默认 8192、`Enable tool call` 仅 chat/image2text、
  `Does it support Vision?` 仅 chat、`Base url` 必填、API-Key 可选）；OpenRouter 增加 `Provider order`；
  picker 厂（Ollama 等 10 个）保持模型列表并隐藏通用 Vision 开关。
- **payload 对齐 `buildModelInfoFromValues`**：`model_info[0].extra.is_tools`、chat+vision → 追加 `image2text`、
  `provider_order` 随提交带上，开关字段不再泄漏进 `payload.extra`。
- **CDP 探针** 内部 UI 遍历脚本（`local-provider-modal-cdp.mjs`：逐个打开六个本地厂与 OpenRouter 模态，
  记录类型集/默认值/开关可见性与 payload。

### Fixed

- **Available models 计数胶囊格式**：标签与计数改为相邻节点（`All63`/`VLM17`，间距交给 CSS），卡片 `+ Add`
  文案取自 `setting.addTheModel`（Add/添加）。


### Added

- **提供商「List models」选择器对齐上游 ToggleList**：内联面板 + `role='list'`、选项 >10 才显示带清空按钮的搜索行、
  `All models` 全选哨兵、每行能力胶囊（`setting.modelTypes` 文案，Chat/VLM/ASR）+ 悬停编辑铅笔、搜索无匹配
  `No matching results`、空目录 `No models available`、底部固定「＋ Add custom model」，滚动区上限 400px。
- **自定义模型弹窗编辑模式**：悬停铅笔进入 `Edit model`（名称锁定、回填类型/最大 Token/Tool call），保存写回目录与选中项；
  对话框仅保留上游的 Cancel/Confirm 两个按钮，关闭时重置表单与错误文案；文案与校验按 `use-custom-model-fields.tsx`
  （`modelNameRequired`/`modelNameDuplicate`/`modelTypeRequired`/`modelMaxTokensMinMessage`）本地化。
- **CDP 探针** 内部 UI 遍历脚本（`provider-model-picker-cdp.mjs`：用 `page.route` 桩出 12 个模型，逐步验证展开、
  勾选、全选、搜索无匹配、清空、编辑、新增自定义模型与校验，全程截图。

### Fixed

- **点击模型行误开编辑弹窗**：整行用 `<label>` 包裹时，label 激活会转发给第一个可标注后代，而 `<button>` 也是
  labelable —— 铅笔排在复选框之前导致点行体等于点铅笔。改为上游的 `div` 行 + 行内点击切换 + 铅笔/复选框
  `stopPropagation`。
- **隐藏的编辑铅笔仍可点击**：仅 `opacity:0` 不阻断命中测试，补 `pointer-events:none`（hover/focus 恢复），
  与上游 `hidden group-hover:flex` 对齐。


### Added

- **提供商弹层 Base-Url 对齐上游 `InputSelect`**：由「文本框 + datalist」改为组合控件（已选值 + × 清除 + 下拉箭头触发器、
  可过滤弹层、每行 URL + region 徽标、`Add "<value>"` 自定义行、`No results found` 空态、Enter/Escape/Backspace
  与 150ms blur 提交），默认选中 regionKey 为 `default` 的地址，占位符/提示按 provider 取上游
  `setting.*BaseUrl*` 文案（minimax / tongyi-qianwen / siliconflow / anthropic / openai），zh/en 双语言。
- **Added models 本地化与类型映射**：实例展开按钮改用 `setting.showMoreModels`/`hideModels`（View models/展示更多模型、
  Hide models/隐藏模型），实例类型徽标改用共享 `crate::providers` 的 `mapModelKey` 映射（image2text→VLM、speech2text→ASR、chat→LLM 等）。
- **弹层/弹窗不透明表面**：新增 `--bgm`（深色 `#15141c` / 浅色 `#fff`），`.modal` 与 6 个浮层选择器不再使用 5% 白的 `--bgc`，
  修复弹窗背后卡片透字问题。
- **CDP 探针** 内部 UI 遍历脚本（`provider-base-url-cdp.mjs`：真实点击卡片 → 弹窗 → 打开 Base-Url 弹层 → 切 region → 清除 →
  输入自定义 URL，逐步记录值与派生 region 并截图。

### Fixed

- **`region` 提交字段恒为空**：`data-api-base-options` 被 Rust 侧以 region 优先顺序输出（`[region, url]`），
  与前端/上游 `buildBaseUrlOptions` 的 `{value: url, regionKey}` 相反，导致 region 徽标显示 URL 且
  `providerRegionForBaseUrl` 永远返回 null；现改为 URL 优先并加断言防回归。


### Fixed

- **数据集 Configuration / Knowledge graph 面板错位（用户报告）**：Reranking 提示气泡
  `data-tip='…` 缺收尾单引号，浏览器把其后的 `<div class='provider-default-field'>` 等
  整段当作属性值吞掉，导致 `#tab3/#tab4` 落到 `.ds-layout`（侧栏列）而非 `.ds-content`。
  `kb_detail_page` 与 `dataset_page` 两处同源缺陷一并修复；新增引号感知深度扫描单测
  （`div_depth_at`，修复前 tab3 深度 7 vs tab0 4 必红）。
- **「Reranking model」标签被吞**：`<label …display:block'><` 末尾多余的 `<` 生成伪元素
  `<reranking model="" …>`，吞掉标签文字与 Use knowledge graph 复选框；同步修正
  `.ds-content` 收尾与检索面板左右两列结构。
- **配置页横向溢出 139677px**：dataset / kb_detail 两页残留 legacy JS 把 4000+ 个
  `<option>` 塞进已是弹层触发按钮的 `#embdSel`；删除注入后仍由共享默认模型树
  （`GET /api/v1/models`）提供模型列表，配置页宽度回到 1585px。
- **头像行结构**：配置表补回 `<tr><td>Avatar</td>` 行头、删除遗留空 Avatar 行、文件选择框限宽。
- **主样式表截断**：`src/web_css.txt` 中 `.cc-runtime-qr{width:224px;max-width:100.` 截断
  使浏览器在解析中途放弃，其后全部规则（`.provider-tz-list`、`.set-switch` 等）静默失效
  （CSSOM 517 → 537 条）；修复并新增 `stylesheet_has_no_truncated_rules` 结构化校验单测。

### Added

- **开关控件对齐上游 Switch**：`.set-switch input[type=checkbox]` 与
  `input[type=checkbox][role=switch]` 渲染为 36×20 圆角滑块（`appearance:none` + 白色滑块），
  替换原先在深色主题下显示为大白方块的浏览器原生复选框。
- **侧栏/导航全量实点脚本** 内部 UI 遍历脚本（`sidebar-routes-cdp.mjs`：顶部导航 7 项、
  用户设置侧栏 7 项、数据集详情侧栏 5 项与 `/user-setting` 共 21 条路由逐页点击，记录状态码、
  主列高度/文本长度、横向溢出、page error 与截图；本次结果 21/21 通过。
- **诊断脚本**：`overflow-cdp.mjs`（定位横向溢出源头）、`testing-switch-cdp.mjs`（开关与伪标签）、
  `cfg-overflow-cdp.mjs`、`embdsel-desc-cdp.mjs`。


### Added

- **`/agent-templates` 模板画廊**：新增模板元数据 fixture（25 条，取自上游
  `agent/templates/*.json` 的本地化 title/description + canvas_type/canvas_category）、
  `GET /api/v1/agents/templates` 与 `/templates/{id}` 端点，以及上游组合的页面
  （分类侧栏 + 搜索 + 模板卡片网格 + hover「使用」遮罩 + Use 弹层按模板 DSL 建 agent
  后跳 `/agent/{id}`）。CDP 实点：25 模板 / 分类切换 3→10 卡 / Use 弹层预填。

### Added

- **`/login` 3D 翻转卡片**：`FlipCard3D` 结构（三 JS/CSS 对齐上游：perspective 舞台、正反两面、
  `rotate-y-180` 翻转、`backface-visibility:hidden`、活动面 `data-testid='auth-card-active'`、
  200 ms 切面），新增 `Remember me`（邮箱本地记忆 + 请求带 `remember`）与
  `Sign in with {channel}` 渠道按钮区（`/api/v1/auth/login/channels`，无渠道自动隐藏）。

### Added

- **数据集详情改上游 route 导航**：侧栏五项（Files / Retrieval testing / Logs /
  Configuration / Knowledge graph）改为真实路由链接 `/dataset/{tab}/{id}`，活动项按路径判定；
  解析日志面板并入 Logs 面板；`/dataset/{id}` 307 跳 `/dataset/files/{id}`。
  **修复**：面板脚本引用未定义的 `kbId` 导致 `showTab` 从不执行（所有路由都停在 Files 面板）、
  以及 `showTab` 正则的 Rust 字符串转义错误。

### Added

- **数据集配置页 Basic 字段对齐上游**：新增 `Language`（17 种语言，`ds-settings-basic-language-select`）
  与 `Description` 字段，字段顺序调整为 Name → Language → Avatar → Description → Permission →
  Embedding model → Page rank → Tag sets（上游 `general-form.tsx`），保存按钮加
  `ds-settings-basic-save-btn`，`PUT /api/v1/datasets/{id}` payload 增加 `description`。

### Fixed

- **模型提供商弹层字段顺序对齐上游**：`Instance name → API-Key → Base-Url`（原为
  Instance name → Base url → API-Key），标签统一为 `Base-Url`，去掉标题行重复的文档
  链接；同时给该链接的 JS 加空值保护，避免打开弹层时 TypeError（同类问题此前在
  `/files` 出现过）。

### Changed

- **`/user-setting/model` 收敛为上游三段结构**：移除自造页标题与 `Reranker` /
  `RAG Chat Test` / provider administration 区块（整体迁至 `/admin`），
  `Available models` 标题去掉计数后缀，提供商卡按钮统一为上游的 `Add`。


- **`/memories` 列表页对齐上游**：共享渲染器新增 `form_extra` / `page_script` /
  `lpExtraPayload()` 三个页面级挂钩，`/memories` 渲染上游组合（ListFilterBar +
  记忆卡片网格 + 分页 + 两分支空态 + ⋮ Rename/Delete），创建弹层按
  `createMemoryFields` 提供 Name、Memory type（Raw 固定勾选）、Embedding model 与 LLM
  （共享默认模型树，按 `/api/v1/models/default` 播种）；CDP 实点并截图。


- **`/agents` 列表页对齐上游**：共享 `AppListSpec` 扩展出「创建下拉菜单 / FlowType 卡片 /
  额外 ⋮ 项 / JSON 导入」四个能力，`/agents` 渲染上游组合——Create agent 下拉
  （Create from blank / Create from template → `/agent-templates` / Import JSON file）、
  创建弹层（Name + Agent flow / Ingestion pipeline 卡片）、卡片 ⋮（Rename / Edit tags /
  Delete → `Delete agent` 二次确认）与分页；CDP 实点四步并截图。


- **`/chats` 与 `/searches` 列表页对齐上游**：抽出共享渲染器 `AppListSpec` +
  `render_app_list_page`，两页复用上游组合（ListFilterBar = 标题/搜索/`Create chat`、
  `Create search` 主按钮 → 卡片网格（字母头像/名称/描述或更新时间 + ⋮）→ 分页页脚 →
  两分支空态；⋮ = Rename（共享弹层）/ Delete（`Delete chat`、`Delete search` 二次确认））。
  CDP 实点两页的创建弹层、⋮ 菜单、删除弹层并截图；`web::` 58 passed、lib 1446 passed。


- **`/files` 文件管理页对齐上游 `pages/files/index.tsx`**：ListFilterBar（Files 图标 +
  标题 + 搜索 + `Add file` 下拉 = Upload file / New folder）、选中后出现的批量栏
  （Move / Link to dataset / Delete）、上游六列表格（全选、Name、Upload date、Size、
  Dataset、Action）与分页页脚（总共 N + 50/20/100 + ‹ ›），新建文件夹/移动/链接知识库
  三个弹层。**修复**：旧脚本引用已删除的 `addFileBtn` 抛错导致 `loadF()` 不执行、
  表格永挂 "Loading..."；顺带修 `.bulk-bar[hidden]` 被 `display:flex` 覆盖的问题，
  并把头部外链环境变量测试改为注入式解析器（消除并行测试竞态）。


- **`/datasets` 列表页对齐上游 `pages/datasets/index.tsx`**：ListFilterBar
  （标题 + 搜索 + `Create dataset` 主按钮）、卡片网格（字母头像/名称/`N files`/⋮ 菜单）、
  `RAGFlowPagination` 式页脚（共 N 条 + 每页 12/24/48 + 上一页/下一页）与两种空态
  （无数据无关键词 → 居中虚线空卡可点击创建；搜索无结果 → 保留工具栏、空卡不可点）。
  创建弹层 = Name + Embedding model（共享默认模型树，默认取租户默认 embedding，
  仅随 POST body 提交）+ Parse type（Built-in/Pipeline）+ Chunking method + Save/Close；
  ⋮ 菜单 = Rename（弹层）/ Delete（`Delete dataset` 二次确认弹层）。⋮ 字形改内联 SVG。


- **首页 `/` 对齐上游 `pages/home/index.tsx`**（原为 308 跳 `/dashboard`）：欢迎横幅
  （`header.welcome` + 渐变品牌字）、Dataset 区（最多 6 张卡 + `common.seeAll` 卡，
  空态为 `empty.datasetTitle` 虚线卡）、Applications 区（Chat/Search/Agent/Memory
  分段控件 `role=tablist`/`aria-selected`，标题与卡行随选项切换，数据来自
  `/api/v1/chats|searchapps|agents|memories`），每类空态用上游 en/zh `empty.*` 文案；
  登录/注册成功后落到 `/`（与上游一致）。CDP 逐 Tab 实点验证（`home-dump-cdp.mjs`）。


- **全局头部对齐 RAGFlow `layouts/components/header.tsx` + `global-navbar.tsx`**：
  导航胶囊只保留上游七个入口（Home 图标 → `/`、Dataset `/datasets`、
  Chat `/chats`、Search `/searches`、Agent `/agents`、Memory `/memories`、
  File `/files`，含 `nav-chat`/`nav-search`/`nav-agent` test id 与
  `aria-current='page'` 活动态）；右侧簇改为 Discord、GitHub、语言下拉菜单
  （当前语言 + ⌄，点击展开、点击外部/Esc 关闭、`role=menuitemradio`）、
  用户手册外链、主题按钮、头像 → `/user-setting`。原先挂在头部的
  Dashboard/数据源/Skills/模型提供商入口全部移入头像之后的用户设置侧栏（与
  上游一致）。外链默认与上游逐字相同，可用 `RAYRAG_DISCORD_URL` /
  `RAYRAG_GITHUB_URL` / `RAYRAG_HELP_URL` 指向大陆可达镜像。
- **用户设置侧栏对齐上游 `pages/user-setting/sidebar/index.tsx`**：七项顺序与
  `setting.*` 标签（含 Profile=概要）、上游 Lucide 图标（server /
  messages-square / box / plug / users / user / unplug）内联 SVG、底部
  版本号 + 日/月 `ThemeSwitch` 药丸（`role=switch` + `aria-checked`）+
  `Log out`/登出 按钮。
- **`/login` 独立页面壳**（上游 `login-next/index.tsx` 无全局头部）：新增
  `layout_plain`，登录/注册页不再渲染导航、不再触发用户信息请求。
- **Docker 构建双网络适配**：新增 `docker/mirror-setup.sh` 与
  `RAYRAG_MIRROR_PROFILE=auto|cn|global`——`auto` 先探测清华镜像再决定；
  `cn` 走清华 apt + rsproxy crates.io + ghfast.top GitHub 代理，`global` 直连
  deb.debian.org / crates.io / github.com；zvec 预编译库按 profile 决定
  xget 与 GitHub 的先后顺序（另一侧始终作为回退）。
- **`vendor/zvec-rust/`（v0.7.1 两 crate 逐字副本 + `VENDOR.md` 刷新步骤）**：
  Docker 构建用 `[patch]` 指向本地副本，镜像构建不再 clone 上游仓库及其
  C++ 子模块，构建可离线/可复现；crates.io 版本与 `--locked` 语义仅在
  本地（非 Docker）构建保留。

### Fixed

- **登录页无限刷新**：全局头部脚本会在每个页面拉取 `/api/v1/user/info`，
  配合 401 跳转在 `/login` 上形成「刷新 → 401 → 跳 /login → 刷新」死循环。
  修复为：登录页使用独立壳（无头部脚本）；头像请求要求已登录且不在
  `/login`；401 跳转在 `/login` 上短路。
- **启动日志中立化**：删除含他家产品名与具体部署地址的日志
  （原 `Not using elasticsearch as doc engine…` 改为描述自身行为的 debug；
  embedding 端点从 info 降到 debug，info 只报模型名）。

### Changed

- 新增 3 个头部单测（导航顺序/test id/右侧簇/外链环境变量覆盖）与
  `header-parity-cdp.mjs`（对 RAGFlow 与 RayRAG 跑同一采集器）、
  `login-loop-cdp.mjs`（无限刷新回归）；`web::` 54 通过、lib 1442 通过。
- 台账：`header.tsx`、`global-navbar.tsx`、`theme-button.tsx` 由 reference
  改判 aligned，`user-setting/sidebar/index.tsx` 由 partial 改判 aligned，
  `login-next/index.tsx` 记为 partial；汇总 aligned 208 → 212。

- **zvec 0.5.1 → 0.7.1**：`Cargo.toml` 以 git tag `v0.7.1` 锁定
  `zvec-ai/zvec-rust`（该 patch 版本尚未发布到 crates.io，0.7.0 为最新已发布版），
  `Dockerfile` `ZVEC_RUST_VERSION=0.7.1` 拉取同名预编译 `libzvec_c_api.so`
  （含 `data/jieba_dict/`）。0.7.1 的 Rust API 与既有用法完全兼容
  （`--locked --all-targets` 0 错；1439 测试全绿），无需源码改动。
- **构建标识**：新增 `build.rs` 注入 git rev / 工作树脏标记 / 构建时间，
  `src/lib.rs::build_info` 暴露 `VERSION` + `PARITY_SLICE`；启动横幅与
  `/api/v1/version`、`/api/v1/system`、`/api/v1/admin/version`、用户设置页脚
  统一显示 `v0.3.4 (parity v0.3.4u, rev …, built …)`（此前恒为 0.1.2，
  新旧二进制无法区分）。
- **`/search` 检索页 rerank 模型树选择器**（上游 `next-search/hooks.ts` +
  `search-setting.tsx` 的 `use_rerank` + `rerank_id` 段）：rerank 开关展开共享
  默认模型树选择器（`SEARCH_PAGE_RERANK_SELECTOR_FIELD`），选中值随
  `POST /api/v1/retrieval` body 的 `rerank_id` 下发；`/search?sapp={id}` 打开时
  由 `__SAPP__.rerank_id` 反向播种。该选择器保持表单本地态（上游
  react-hook-form 语义），不写租户默认模型。
- **RAGFlow UI 遍历工具链**（内部 UI 遍历脚本（``）：`ragflow-auth-cdp.mjs`
  （翻转卡片登录/注册自举，会话持久化于 Chrome profile）、
  `ragflow-ui-dump.mjs`（逐路由导出按钮/链接/字段/表头/标签页 + 截图，产物
  内部 UI 遍历记录（ragflow-pages/`），作为 RayRAG UI 一比一对标的结构化基准。

### Changed

- `RayRAG/<version>` User-Agent 统一取自 `CARGO_PKG_VERSION`（原先硬编码
  0.1.2）；crate 版本 0.1.2 → 0.3.4；`FORM_LOCAL_SAVE_MODEL_DEFAULT` 抽为
  共享常量（`/search` 与 memory 设置页同源）。

## [0.3.4d] — 2026-08-19（未发版累积批次 v0.3.3ba–v0.3.4c）

### Added

- **聊天应用公共分享页**：`/chats/share?shared_id=&from=`（免登录，纯 Rust SSR），
  `GET /api/v1/chatbots/{id}/info` + `POST .../completions` 免登录 SSE 端点，
  对齐 RAGFlow share/widget 页；embed-dialog 式 `auth/release` 参数。
- **统一命令超时输入**：`RAYRAG_CMD_TIMEOUT`（用户 env 环境文件，默认 7200s）钳制全部外呼客户端与沙箱超时。
- **Data source 详情页**逐源动态字段集、Add 弹层 schema 驱动创建、列表页 35 卡分组 + Sync Logs 表
  （1:1 RagFlow 列/徽章/分页），MCP 编辑对话框与卡片操作，Provider 模态框 API 面补齐（provider_api_service.py 对齐）。
- Google Drive/Gmail web-OAuth token 字段 + start/callback/result 端点；Box web-OAuth 同套流程。
- ChatChannelKey 23 渠道目录全量移植；reasoning/thinking toggle 端到端；chat settings drawer（rerank/reasoning/refmeta）。
- meta_data_filter 表单（DatasetMetadata 4-method）+ AirTable/Box/RSS 等连接器去 Local stand-in。

### Fixed

- RAGFlow 前端产物边界清理（web/static/vs 等非必要静态文件移除，873 files → 精简）；
  CDP 交互遍历基准 35 路由固化。

## [0.3.3] — 2026-08-06

### Added

- **Provider 模型目录 20 → 60**：从运行中的 RAGFlow v0.26.2 容器
  `/ragflow/conf/models/` 整目录提取 60 个提供商 fixture（verbatim mirror），
  新增 40 个缺失提供商（anthropic/azure-openai/bedrock/hunyuan/xunfei/
  baichuan/stepfun/togetherai/perplexity/modelscope/xinference/mineru/
  paddleocr/jina/voyage/groq/mistral/replicate/novita/upstage/xiaomi/qiniu/
  deepinfra/gpustack 等），共 556 个模型条目；`model_meta.rs` 注册表
  `PROVIDER_MODEL_FIXTURES` 扩至 60 条 + `resolve_model_factory` 别名
  从 11 扩至 50（覆盖 Claude/混元/讯飞/百川/Azure/Bedrock/华为云等）。
- **RAGFlow Web UI 清单** `内部界面盘点`：真实浏览器
  登录逐页实测，记录 15+ 页面层级的按钮/路由（登录/注册/首页/Dataset 列表/
  知识库 4 Tab/Chat/Search/Agent/Memory/File/用户设置 8 菜单 + 35 数据源 +
  Model providers 类型筛选 + API 文档页）——RayRAG 前端对齐的 checklist。

### Fixed

- 适配 60 fixture 的过时断言（catalog.len 20→60、端点覆盖 ≥50）。
- 自托管引擎（ollama/vllm/lmstudio/localai 等）无默认端点属正常，
  测试断言放宽而非强制补默认值。

### Added (follow-up)

- **PROVIDER_PRESETS 64 → 69**：新增 Xiaomi（MiMo，国内直连）、Qiniu
  （七牛云）、HuaweiCloud（ModelArts MaaS）、TokenHub（聚合中转）、
  OrcaRouter（模型路由）五个预设；新增 `FIXTURE_TO_PRESET` 映射表
  （24 条 fixture key → factory id），`provider_preset` 支持 fixture
  键回退解析，`provider_is_domestic("aliyun"/"hunyuan"/...)` 等语义
  修正为正确标记国内直连。
- **内嵌 llm_factories.json 63 → 65 工厂 / 1064 模型**：从运行中
  RAGFlow v0.26.2 容器同步最新工厂目录（补 Xiaomi、New API），
  转换回紧凑 `{n,t,mx,tools}` 格式并全校验。
- **Providers 页对齐 RAGFlow Model providers**：能力类型筛选
  （All/LLM/Embedding/Rerank/TTS/ASR/VLM/OCR + 计数）、每行
  `data-k` 类型属性、`+ Add` 按钮 + 添加提供商弹窗（POST
  /api/v1/providers/{id}，对齐 RAGFlow Add 流程）。

## [0.3.2] — 2026-08-05

### Fixed

- **OCR 解析链路打通**：FigureParser::new() 惰性接入 OcrClient（.env
  RAYRAG_OCR_PROVIDER=proxy + RAYRAG_OCR_BASE_URL）；修复 async 上下文
  内再建 runtime 的 panic（figure.rs:514 "Cannot start a runtime from
  within a runtime" → block_in_place + Handle::current().block_on）。
- 验证：PNG 上传→OCR 提取文字→索引→检索命中（score 0.902，content
  含 OCR 文字）。四个 GPU 服务（LLM/embedding/reranker/OCR）全部打通。

## [0.3.1] — 2026-08-05

### UI 按钮逐项核对补齐（RAGFlow JSX vs RayRAG 渲染 diff）

- 补 12 个按钮事件：batchDelSel/batchLinkSel（文档批量删除/关联）、
  clearConv/tempConv（清空/临时对话）、emptyAllDocs（清空文档）、
  exportAgentJson/exportChunks（导出 agent JSON/chunk）、inviteUser
  （邀请用户）、openVersions（版本）、sendTemp（临时发送）、
  showTemplates（模板）、uploadF（上传文件）
- web.rs +169/-12；新增 8 个 UI 测试（含按钮 diff 断言）

## [0.3.0] — 2026-08-05

### UI：页面按钮全面对齐 RAGFlow（web/src/pages 19 页面层级）

- **知识库组**：列表页创建按钮/搜索/排序/卡片操作菜单（重命名/导出/删除）
  + 弹窗；详情页文档表格（状态/进度/chunk 数）+ 批量操作栏（解析/禁用/
  删除）+ 上传按钮；dataset 5 tab（文档/配置/检索测试/chunk/概览）+ 配置
  表单（chunk 方法下拉/embedding/检索参数）+ 保存按钮 + 检索测试（输入/
  滑块/结果卡片）。
- **对话检索组**：对话页新建/重命名/删除/分享 + 消息区（发送/停止）+ 引用
  折叠；检索页知识库选择器/过滤/结果卡片；记忆页添加/编辑/删除；404 页。
- **Agent 管理组**：agents 列表新建/卡片操作；agent 详情运行/保存/发布 +
  组件面板 + 日志区；admin 用户表格 + 添加/删除/密码重置；files 上传/下载/
  删除；skills 安装/卸载；settings API Key 生成/删除。
- 所有按钮接现有 API（HTML + JS 事件）；web.rs +796 / server.rs +198 /
  kb.rs +28；新增 6 个 UI 渲染测试。

## [0.2.5] — 2026-08-05

### Changed

- **覆盖矩阵最终收尾**（纯文档，`内部对标台账`）：矩阵终态
  aligned 128 / full 541 / partial 18 / replaced 92 / N/A 61 / reference 3601 /
  unmapped 0（4441 文件快照）。摘要表按清单逐行重算，与库存行完全一致。
  agent/sandbox 22 个非代码文件、deepdoc/README×2、deepdoc/__init__.py、
  admin/build_cli_release.sh 由 partial 转为 N/A-by-design（Python 沙箱镜像/
  文档/构建脚本不属于 Rust 移植范围；等价语义在 src/sandbox.rs + Dockerfile
  多阶段构建中表达）。

## [0.2.5] — 2026-08-05

### Added

- **conf/ 配置处理**：all_models 逐字节一致核对（+27 个新模型回归锁定：
  gpt-5.2-pro/deepseek-v4-flash/glm-5/kimi-k2.6/minimax-m2.7/grok-4 等）、
  llm_factories 工厂映射（FactoryLlmInfo/factory_endpoint）、system_settings
  种子（14 行常量 + coerce_setting_value）、ServiceConf（Mysql/Minio/Es/Os/
  Infinity/Redis/UserDefaultLlm 子结构 + apply_to_settings）。
- **MCP SSE 服务器**（`src/mcp_server.rs` 新建）：ToolRegistry（ragflow_retrieval/
  search_knowledge/list_datasets）、McpServerCore（JSON-RPC 2.0 dispatch：
  initialize/notifications/ping/tools/list/tools/call，-32600/-32601/-32602 错误码）、
  RetrievalBackend trait + InMemory 后端；接入主 router `/mcp/sse` + `/mcp/messages`。
- **消息记忆服务**（`src/memory.rs` + `src/api/joint_services.rs`）：消息读写/查询、
  中文双引号正则对齐（Unicode 弯引号）。
- **矩阵终态**：partial 44 → **15**；sandbox 非代码 22 → N/A-by-design（等价语义
  在 src/sandbox.rs seccomp/超时 + Dockerfile）；whatsapp Node 网关/Go CLI → N/A。

## [0.2.4] — 2026-08-05

### Added

- **common 工具层**（`src/common.rs` ~1800 行）：常量表（RetCode/TaskStatus/
  StatusEnum/ParserType 15 种 chunk method/FileSource 31 种/LLMType/Storage/
  MemoryType 位标志）、string/text/time/float/misc/parser_config/query_base/
  connection_utils、crypto（AES-128/256-CBC + PKCS7 + RAGF magic 头 +
  PBKDF2-HMAC-SHA256，与 Python wire format 逐字节一致；SM4 标记 TODO）、
  ssrf_guard、参数校验；api/constants + api/validation 语义。
- **settings 层**（`src/settings.rs` 新建）：Settings::from_env/from_reader、
  StorageImpl/DocEngine 枚举、parse_model_entry（name@factory）、init_secret_key
  （≥32 字符）、FLOAT_ZERO/PARAM_MAXDEPTH。
- **benchmark**（`src/benchmark.rs` 新建）：BenchmarkDataset/Config、ndcg@10/
  map@5/mrr@10（rank-ordered）、LatencyStats（avg/p50/p95/max）、
  RetrievalRunner + run_retrieval_benchmark、to_markdown。
- **canvas 语义**（`src/agent.rs` 追加）：reset_sys_globals/reset_env_globals、
  get_history_window、is_canvas_reference、add_retrieval_reference、
  ToolUseTrace + merge_tool_use_trace、clean_tts_text。

## [0.2.3] — 2026-08-05

### Added

- **flow 组件契约**（`src/pipeline/flow.rs` ~700 行）：FromUpstream/check_payloads
  （alias 反序列化）、OutputFormat、ComponentOutput（ProcessBase 语义）、
  file_outputs、FlowLog/TraceEntry/ComponentTrace（trace 累积 + 加权进度）、
  ChunkDoc（REST 契约 + From<&Chunk> 转换）。
- **resume 实体资源修复**（`src/resume.rs`）：2 个 regex 崩溃 bug
  （`[—-]+` 非法区间、反斜杠转义层级）+ CORP_TKS 贪心最长匹配（对齐 jieba）。
- **服务层语义**（`src/api/joint_services.rs` +700）：seed_system_settings、
  SuperuserSeedPlan、LLM 工厂种子/清理（novita.ai 删除/QAnything→Youdao）、
  tenant_model 修复计划、canvas 模板规范化、langfuse/mcp_server/conversation/
  task 服务（SyncLog 单调计数 + 100 条裁剪 + resume 状态机）。
- **MCP 客户端**（`src/mcp_client.rs`）：SSE + Streamable HTTP 传输、
  initialize/list_tools/call_tool。
- **Helm chart**（Kubernetes 部署脚手架 新建）：最小可用 chart（deployment/service/configmap，
  镜像 rayrag:local，9390→8080，外部 PG18 依赖说明）；helm lint/template 通过。

## [0.2.2] — 2026-08-05

### Added

- **api/common**（`src/api/common.rs`）：ApiError/ApiErrorKind（400/403/404/409/500
  信封输出 `{"code","message"}`）、ok_json、base64、TeamPermission 检查。
- **运行时配置**（`src/api/runtime_config.rs`）：RwLock 热更新（init/get/get_env/
  set_env/load_config_manager/set_service_db，对齐 reload_config_base）。
- **联合服务**：memory_message（消息记忆读写）/tenant_model（租户模型绑定）/
  user_account 语义补入 api 层。
- **dialog.rs**：is_aggregate_sql/is_row_count_question/add_kb_filter（UUID 注入
  防护 + RAGFlow 两处怪癖忠实复刻）。
- **工具契约核对**（`src/runtime.rs` +278 / `src/agent.rs` +34）：17/19 工具已对齐；
  JIN10_INPUTS 补 7 参数、QWEATHER 补 web_apikey、retrieval 补 15 输入 4 输出、
  **wencai 别名**（canvas component_name=WenCai 此前匹配失败）。
- **资源加载器**（`src/resources.rs` 新建）：ner.json（235KB）+ synonym.json
  （268KB）磁盘加载（RAYRAG_RES_PATH 环境解析、缺失降级空表、值→键镜像）；
  CvModel 页面转写 prompt + figure 描述调用流补全。

## [0.2.1] — 2026-08-05

### Added

- **远程沙箱契约**（`src/sandbox.rs` +456）：RemoteSandboxClient/Config（task 提交/
  结果拉取、超时=执行超时、重试、/healthz）、CodeExecutionResult 系列、
  unsafe/unhandled_exception 失败分支。
- **插件机制**（`src/plugin.rs` 新建）：PluginManager（发现/加载/执行、env 注入、
  错误隔离）、LLM 工具插件（OpenAI tool 转换）、内置插件 1-2 个。
- **NLP 工具**（`src/nlp.rs` +658）：rag_tokenize（CJK 单字+二元组）、全角→半角、
  ~450 对繁→简映射（替代 OpenCC）、sub_special_char、QueryBase（rm_www/
  normalize_query 管线）、TermWeightComputer（NER×postag）。
- **LLM 工厂**（`src/llm.rs` +655）：ModelKind 7 类 + Chat/Ocr/Cv/Seq2txt/Tts
  trait 抽象 + OpenAi 系列实现（vision/audio/tts 多模态 payload）。
- **RAPTOR 检索**（`src/advanced_rag.rs`）：问题分解→子检索→汇总（复用已有
  Retrieval/Chunk 组件）。
- **doc_store 抽象**（`src/doc_store.rs` 新建）：DocStore trait（insert/delete/
  search/update/sql 全契约）+ Memory/Postgres/Zvec 三后端 + 连接池
  （es_conn_pool 语义：2 次重试/5s 间隔/按 URL 选后端）。
- **memory utils**（`src/memory.rs` 新建）：highlight_text（按句切分/<em> 标记）、
  aggregate_by_field。
- **admin 端点**：users CRUD/keys/activate/admin 授权/version/ping（对齐
  admin/server/routes.py）。

### Fixed

- `/api/v1/admin/users/{username}` 与既有 `{user_id}` 路由冲突 → 删除旧
  重复路由与 handler（新路由功能更全）。

## [0.2.0] — 2026-08-05

### Added

- **剩余 prompts 全部移植**（`src/prompts.rs` +1390 行）：26 个常量——
  related_question、resume 系列双语（system/basic_info/education/project_exp/
  work_exp）、structured_output/sufficiency_check/summary4memory/tool_call_summary、
  TOC 管线（detection/extraction/continue/index/relevance×2）、vision describe 系列
  （vision_llm_describe/figure_describe×2，Jinja 语法转 {var} 约定）。
- **问答模式分派**（`src/app_parsers.rs`）：naive/advanced/graph 模式选择逻辑
  （显式标记优先、全 KG 才走 graph）。
- **api/utils 服务层**（`src/api/utils.rs` ~600 行）：FileType/filename_type
  （扩展名逐字对齐）、sanitize_path、常量（255 限制/50MiB/100MiB）、
  hash128（xxh3）/md5 hex、get_parser_config（chunk_method 默认 + 递归合并）、
  init_dataset_defaults（parser_id=naive、llm_id 注入）。
- **解析器行为补齐**：`excel.rs`（多 sheet `表头:值;` 行格式、sheet_html 分块、
  >10000 行二分、row_number 计数）、`docx.rs`（blip rId→rels→media 图片提取）、
  `paddleocr_layout.rs`（table/figure/image 区域分组 + 位置 tag）。

### Fixed

- channels 测试断言 JSON map 遍历顺序不稳定 → 集合匹配（顺序无关）。

## [0.1.9] — 2026-08-05

### Added

- **连接器框架**（`src/connectors.rs` ~1480 行，对标 `common/data_source`）：
  Connector trait（load_credentials/list_files/fetch_file/normalize/fetch_all 分批
  runner）、RemoteFile/SyncBatch 契约、LocalConnector/WebDavConnector/BlobAdapter
  基础连接器 + FeishuConnector（tenant_access_token 授权、云空间递归列举、
  docx raw_content 导出）/ ConfluenceConnector（spaceKey 分页、HTML→markdown）；
  ConnectorRegistry 按类型分发；datasources 页面连接/断开/拉取按钮。
- **渠道框架**（`src/channels.rs` ~1200 行，对标 `api/channels`）：
  Channel trait（send_text/send_markdown/start/stop/dispatch 错误隔离）、
  ChannelRegistry（register/build_channels/start_all/stop_all/dispatch_inbound，
  bootstrap 语义）、webhook 渠道（OutgoingMessage 契约 POST）+ feishu 渠道
  （tenant_access_token 缓存、text/interactive-card 发送、v2.0 事件归一化 +
  URL verification challenge）；server.rs 启动/停止钩子。
- **rag/svr 语义补齐**：`src/api/file_mgr.rs` +236（DocFileCache：Redis 式
  `{kb_id}/{location}` 键、TTL 过期清理）；`src/task_executor.rs` +960
  （SyncTask 同步任务类型：fingerprint 跳过、poll_range_start 窗口推进、
  快照删除对账、超时永久 FAIL；ChunkEnrichment：auto_questions 两阶段 LLM
  缓存 + 离线 fallback、important_kwd/question_kwd 元数据、进度消息对齐）。

## [0.1.8] — 2026-08-05

### Added

- **TaskExecutor**（`src/task_executor.rs`，~680 行，对标 `task_executor.py`）：
  异步任务队列（tokio::mpsc + Semaphore 限流）、状态机 Pending→Processing→
  Done/Failed（对齐 RAGFlow doc 状态 UNSTARTED/RUNNING/DONE/FAILED）、
  进度回调（set_progress：`[ERROR]` 前缀/`Page(a~b):` 页前缀/HH:MM:SS 时间戳）、
  失败重试（retryable 标记 + 指数退避 + max_retries）、status/stats 上报、
  submit/shutdown 生命周期；接入 document.rs 上传队列。
- **ChatPipeline**（`src/chat.rs`，835 行，对标 `generator.py`）：
  问题改写（full_question：绝对 ISO 日期 + USER:/ASSISTANT: 会话，**ERROR** 回退）、
  多路查询（multi_queries_gen：<2 命中时生成 2-3 路补充查询，JSON 解析容错）、
  引用格式化（citation_plus 二次 LLM 生成 + repair_bad_citation_formats 规范化
  [ID:n] + references 按被引过滤）、format_knowledge（kb_prompt 树形渲染）。

## [0.1.7] — 2026-08-05

### Added

- **RAG 问答阶段 prompts**（`src/prompts.rs` +1194 行）：18 个 RAGFLOW_* 常量
  （citation/citation_plus/full_question/multi_queries_gen/next_step/reflect/
  rank_memory/content_tagging/cross_languages×2/analyze_task×2/ask_summary/
  assign_toc_levels/keyword/question_prompt/meta_data/meta_filter），注册
  PromptLibrary get()/list()。
- **graph_extractor NER 语义**（`src/structure_compile.rs` +1033 行）：实体/关系
  JSON 解析容错（LLM 非严格输出提取/修复）、upsert 语义（if not exists）、
  general/light extractor 剩余常量。
- **组件参数契约**（`src/runtime.rs`）：新增 iteration/iterationitem/exitloop/
  loopitem descriptor；扩展 categorize/listoperations/docsgenerator/excelprocessor
  参数（对齐 RAGFlow Param schema 的 camelCase 语义）。

## [0.1.6] — 2026-08-05

### Added

- **translate 工具**（`src/translate.rs` + `src/runtime.rs`）：注册 translate descriptor
  （对齐 RAGFlow deepl 参数契约），百度翻译 VIP 签名 API + 免 key sug 降级双路径
  （大陆可用）；3 个 mock-HTTP 测试（SUG 解析/VIP MD5 签名表单校验/错误码透传）。
- **KeyTerm Extractor**（`src/extractor.rs`，402 行）：RAGFlow KeyTermExtractor 语义的
  统计实现——词频/位置/POS 代理权重、CJK n-gram（跳过虚词开头、单字过滤）、
  停用词表；`extract_key_terms` / `extract_key_terms_with_top_n` / chunk 批量接口；
  pipeline 可选集成。
- **title_chunker.py 主文件语义**：标题连续性检查/章节序号处理补入 `src/chunk/title.rs`。

### Fixed

- extractor CJK 测试断言（默认 top-N 截断遮蔽 4 字短语 → 显式 wide top_n 验证）。

## [0.1.5] — 2026-08-05

### Added

- **Provider Model Catalog**（`src/model_meta.rs` + `src/api/fixtures/models/*.json`）：
  RAGFlow conf/models 全部 20 家厂商 / **157 个静态模型**种子化（include_str! 内嵌）；
  `list_models_for` / `resolve_model_factory`（zhipu→zhipu-ai、qwen→aliyun 别名）/
  `provider_models_payload`；providers 页面新增 Model Catalog 卡片 + 点击弹窗
  （模型名 + 能力 badge：chat/embedding/rerank/image2text/thinking）。
- **沙箱 executor_manager 契约补齐**（`src/sandbox.rs` +1031 行）：
  SupportLanguage/ResultStatus/ResourceLimitType/UnauthorizedAccessType/RuntimeErrorType
  枚举、失败协议（exit code + stderr 分类）、运行时环境约定（容器池/镜像/run 参数/
  bundle）、artifact 契约、安全校验契约。
- **GraphRAG 管线补齐**（`src/structure_compile.rs` +321 行、`src/prompts.rs` +116 行）：
  phase_markers 阶段标记（PHASE_RESOLUTION/PHASE_COMMUNITY、`graphrag:phase:{kb}:{phase}`
  键格式、7d TTL）、实体消解确定性门控（levenshtein/2gram 数字差/字符集重叠、
  entity_resolution_candidates 分组排序、parse_resolution_results）、light graph
  默认常量（LIGHTRAG_DEFAULT_LANGUAGE/TUPLE_DELIMITER/ENTITY_TYPES/FAIL_RESPONSE）。

### Fixed

- 子 agent 并发期间 prompts.rs 临时借用 E0716（I 已修，测试全绿）。

## [0.1.4] — 2026-08-05

### Added

- **RAGFlow 25 个 Agent 工作流模板种子化**（`src/agent_templates.rs` + `src/api/fixtures/agent_templates/*.json`）：
  include_str! 编译期嵌入（零转义风险），agents 页面模板库 3 → **28**（web_search_assistant、
  deep_research、text2sql_data_expert、trip_planner、ingestion_pipeline_* 系列等）。
- **GraphRAG prompts 移植**（`src/prompts.rs`，18 个常量 + PromptLibrary 方法）：
  GRAPH_EXTRACTION_PROMPT/CONTINUE/LOOP、GRAPH_SUMMARIZE_DESCRIPTIONS_PROMPT、
  GRAPH_COMMUNITY_REPORT_PROMPT、ENTITY_EXTRACTION_PROMPT 系列、KEYWORDS_EXTRACTION_PROMPT、
  LIGHTRAG_RESPONSE_PROMPT、NAIVE_RAG_RESPONSE_PROMPT、QUERY_KEYWORDS_EXTRACTION_PROMPT、
  MINIRAG_QUERY2KWD_PROMPT、ENTITY_RESOLUTION_PROMPT、MIND_MAP_EXTRACTION_PROMPT、
  QUESTION_PROPOSAL_PROMPT、KEYWORD_EXTRACTION_PROMPT。
- **paddleocr_layout 补齐**（`src/parser/paddleocr_layout.rs`，66 行 stub → 完整 layout 流程）：
  blocks→bbox→position tags、parse_methods、extract_positions（对齐 paddleocr_parser.py）。
- **figure.rs 补齐**：figure-describe prompts + 上下文感知 describe helper（对齐 figure_parser.py/picture.py）。

### Fixed

- `exact_dedup_by_key` canonical 名称保留组内首现（对齐 RAGFlow 语义，而非频次最高）。
- `parse_run_response` 缺省 result 时始终注入 result_present=false / result_value=null / result_type=json。

## [0.1.3] — 2026-08-05

### Added

- **Token Chunker**（`src/chunk/token.rs`，对标 RAGFlow `token_chunker.py`）：
  `one` / `token_size` / `delimiter` 三模式；反引号分隔符编译（最长优先）；
  分隔符并入段尾（re.split capture-group 语义）；重叠率 normalize（0<p<1→×100，
  clamp [0,90]）；children_delimiters 二次拆分（裸模式 escape，`mom` 挂父块原文）。
- **Title Chunker**（`src/chunk/title.rs`，对标 `title_chunker/{hierarchy,group,common}.py`）：
  hierarchy 目标层级合并、group 32/1024 token 常量合并、include_heading_content、
  root_chunk_as_heading；标题来源优先 PDF 大纲 `__outline__` 元数据，fallback 正文
  markdown/枚举标题行解析。
- **iwencai 问财工具**（`src/wencai.rs`，对齐 RAGFlow `agent/tools/wencai.py`）：
  get-robot-data 请求契约（perpage 字符串/page 整数）、xuangu_tableV1 landing
  分页数据接口、多组件 Markdown 渲染、`agent.rs` 工具注册与 provider 执行链。
- **沙箱 executor_manager 远程客户端**（`src/code_exec.rs`）：`parse_run_response`
  缺省 result 元数据兜底（result_present=false / result_value=null / result_type=json）；
  `process_result` 结构化结果优先、stderr-only 错误分支、canonical 渲染。
- **KB 详情页路由** `/kbs/{id}` → `kb_detail_page`（文档/配置/测试/知识图谱 tab，
  对齐 RAGFlow 数据集详情页；与旧 `/dataset/{id}` 并存）。

### Changed

- `Pipeline::new` 按 `config.chunk_method`（naive | token | title）选择 chunker。
- `ParserConfig` 新增 chunk_method / delimiter_mode / delimiters /
  children_delimiters / title_levels / hierarchy / include_heading_content /
  root_chunk_as_heading 字段（serde alias 兼容 RAGFlow camelCase）。
- 修复上一会话遗留 `src/structure_compile.rs` 编译错误（as_any ×2、
  semaphore 借用、Arc 包装、downcast and_then）。

### Fixed

- 子 agent 半成品编译错误（wencai `Map` 类型、code_exec `base64::Engine`、
  agent `SandboxResult` 缺字段）与测试缺陷（重复 bind listener、serde_json
  Map Index panic、process_result 断言语义）。

## [0.1.2] — 2026-08-04

### Added

- **UI 全面对标 RAGFlow**（16 页面 Rust 手写）：dashboard（统计卡片/最近会话）、
  knowledge base（批量上传/删除/清空/重命名）、search（多 KB 过滤/分页/图谱上下文
  卡片/导出 Markdown）、chat（模型选择器/流式/引用/会话管理）、agents（模板库
  3 种工作流/DSL 编辑器/画布拓扑/搜索过滤）、files（搜索）、admin（用户搜索）、
  status（Refresh/Copy Report）、api-docs、login（注册/cookie 会话）
- **Agent 画布完整闭环**：components 格式 DSL（RAGFlow 标准）、模板创建
  （RAG 问答/网络搜索/RAG 对话）、JSON 编辑保存、错误传播（11 个工具函数统一
  formalized_content 注入 + `{Provider} error:` 前缀）
- **OpenAI 兼容层**：`/api/v1/models`、`/api/v1/embeddings`（租户表空回落 env）、
  `/api/v1/chat/completions`（SSE 流式 + references 帧）、`/api/v1/rerank`
  （Jina 风格）
- **GraphRAG 生命周期**：Build（LLM 实体抽取）→ 持久化（graphrag.json 重启保留）
  → Query API → RAG 对话注入 → KB 删除清理
- **SearXNG 自托管**：Docker 部署 + cn.bing 引擎中国适配 + `SEARXNG_URL` 透传
- **性能优化**：zvec 增量 upsert（仅写变更）、`RAYRAG_NO_FSYNC` 开关
  （容器 overlay 上传 53s→37s）、docx 正则静态缓存 + HashMap tie-break
  （偶发测试失败根治）

### Fixed

- docx 并行偶发测试失败（OnceLock 缓存 + 确定性 tie-break）
- Agent 工具静默失败（错误传播到 formalized_content）
- 模板 JS 错位（showTemplates 误入 kbs_page）
- Retrieval 模板字段（formalized_content 非 result）
- 上传性能（容器 overlay fsync 瓶颈 + 增量 upsert）
- 知识图谱不落盘（build 只返回统计）

### Verified (container)

- 844 passed / 0 failed / 26 ignored
- GPU 四链路实连（embedding 8888 / LLM 8088 / rerank 8081 / OCR 8097）
- OpenAI 兼容 4 端点 + Agent 模板 3/3 + SearXNG 成功/失败路径
