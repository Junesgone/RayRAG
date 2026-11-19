# RayRAG

**English** · [简体中文](README.zh-CN.md)

### Document Parsing Configuration

- JPG/JPEG/PNG/WebP/GIF/BMP/TIFF require a working OCR client or a tenant default image-to-text model with image-input support, API base URL and credentials. BMP/TIFF are converted to PNG for vision requests with bounded decoding; valid small images are not skipped by file size. Missing clients, failed requests and empty output produce actionable errors instead of metadata-only chunks.
- SVG parsing extracts text elements only. For path-only or outlined text images, export PNG/JPEG and configure OCR or an image-to-text model; marker-only SVG output is rejected.
- The standalone `picture` parser also extracts image text and returns setup guidance when recognition is unavailable.
- OCR proxy: `RAYRAG_OCR_PROVIDER=proxy` and `RAYRAG_OCR_BASE_URL`. PaddleOCR: `RAYRAG_OCR_PROVIDER=paddleocr`, `PADDLEOCR_BASE_URL`, `PADDLEOCR_ACCESS_TOKEN`. DeepSeek OCR: `RAYRAG_OCR_PROVIDER=deepseek`, `RAYRAG_OCR_BASE_URL`, `RAYRAG_OCR_API_KEY`. Restart the server after environment changes and reparse.
- Explicit PaddleOCR layouts also use `PADDLEOCR_ALGORITHM`. OXR layouts require `OXR_BASE_URL` and `OXR_API_KEY` when authentication is required. Missing clients, request failures and empty responses include configuration and retry steps.
- SoMark requires `SOMARK_BASE_URL` and `SOMARK_API_KEY` when authentication is required. Mistral OCR uses `MISTRAL_OCR_API_KEY`, `MISTRAL_OCR_BASE_URL` and `MISTRAL_OCR_MODEL`. Verify the parsing service and model permissions, then reparse.
- MinerU uses `MINERU_APISERVER`, `MINERU_API_KEY` when authentication is required, and `MINERU_BACKEND`. Supported current backends are `pipeline`, `vlm-engine`, `hybrid-engine`, `vlm-http-client` and `hybrid-http-client`. Both HTTP model backends require `MINERU_SERVER_URL`. Legacy backend names remain accepted.
- External PDF parsing failures include the provider name and configuration checks. Ensure RayRAG can reach the provider endpoint and its models are loaded, then retry parsing after correcting the settings.
- Local/LAN vision endpoints have no connection or total request timeout; public endpoints use a 10-second connection timeout and `RAYRAG_MODEL_TIMEOUT`. Response-size limits still apply.
- The built-in PDF parser reads text layers only. For scanned PDFs select an OCR layout backend in the dataset parser settings and configure its endpoint/credentials. A generic image OCR client does not add PDF page rendering.
- In mixed PDFs, a page with drawn image or Form content and no extracted text reports its page number and OCR layout setup steps. Configure a PDF-capable OCR backend and reparse the whole document.
- Nested PDF Form text is extracted with its local fonts and coordinates. Broken or cyclic Form references report the page and resource name with repair or OCR setup steps. Full page rendering and text inside scanned images still require a PDF-capable OCR backend.
- PDF single-quote and double-quote text operations retain their text, line spacing and character/word spacing during extraction.
- If a PDF page or ToUnicode font mapping stream cannot be decoded, parsing fails with the page number and recovery guidance. Font mapping failures include the font name. Repair and export the PDF with embedded fonts, or select a configured OCR document parser and retry.
- Convert legacy DOC to DOCX; legacy XLS workbooks are read natively through calamine. CLI `parse` loads environment clients, not the web tenant's vision model.
- CSV, XLS and XLSX extraction includes rows beyond 1000 and all worksheets. Dates become readable datetime strings, booleans become True/False, and CSV read failures include UTF-8 repair and re-upload guidance.
- Damaged or encrypted Office/PDF files report the file name, underlying error and repair steps. Repair and export an unencrypted DOCX/XLSX/PPTX/PDF, then upload it again. PDF errors also direct users to check the selected OCR client's URL, credentials and model settings.
- Audio ingestion requires `RAYRAG_ASR_API_BASE` and `RAYRAG_ASR_MODEL`, plus `RAYRAG_ASR_API_KEY` when authentication is required. The endpoint must support `/audio/transcriptions`. Missing clients, request errors and empty transcriptions fail with configuration guidance instead of indexing audio metadata.
- YAML/YML, TOML and EML are accepted as text documents and indexed through the normal parsing pipeline. Office documents use DOCX/XLSX/XLS/PPTX; legacy DOC must be converted first.

**In one sentence: throw your documents (PDF / Word / Excel / PPT / images / web pages) at it, then ask questions in plain language and get answers grounded in those documents.**

- Written in **Rust**. It idles at **a few tens of MB of RAM** (RAGFlow needs ~1 GB to start).
- The UI and feature set follow the open-source project RAGFlow, but RayRAG needs **no** MySQL,
  Elasticsearch, Redis or MinIO: vectors live in [zvec](https://github.com/zvec-ai/zvec-rust)
  and metadata lives in **PostgreSQL 18.4**.
- **Works on networks in mainland China**: container images, package mirrors and web search all
  use endpoints reachable from there.
- Ships with **71 model providers** (DeepSeek, Qwen, Zhipu, Moonshot, SiliconFlow, VolcEngine,
  Baichuan, …) plus local Ollama / vLLM / LM Studio / llama.cpp.
- No Docker? Run a single binary instead (pure Linux mode) — or install the packaged
  **native fnOS app (FPK)** on a 飞牛 NAS with one click.

---

## Table of contents

- [1. Up and running in five minutes](#1-up-and-running-in-five-minutes)
- [2. Open the web UI, first steps](#2-open-the-web-ui-first-steps)
- [3. Connect a model (what makes answers smart)](#3-connect-a-model-what-makes-answers-smart)
- [4. FAQ (read this first when something breaks)](#4-faq-read-this-first-when-something-breaks)
- [5. Day-2 operations: upgrade / backup / uninstall](#5-day-2-operations-upgrade--backup--uninstall)
- [6. No Docker: pure Linux install](#6-no-docker-pure-linux-install)
- [7. fnOS NAS app (FPK)](#7-fnos-nas-app-fpk)
- [8. What is inside](#8-what-is-inside)
- [9. More documentation](#9-more-documentation)

---

## 1. Up and running in five minutes

### Step 1 — make sure Docker is installed

Open a terminal and run:

```bash
docker compose version
```

- **You see a version** (e.g. `Docker Compose version v2.24.0`) → go to step 2.
- **Command not found** → install Docker first:
  - Windows / macOS: install [Docker Desktop](https://www.docker.com/products/docker-desktop/), then restart.
  - Linux: `curl -fsSL https://get.docker.com | sh`

> No Docker, or you would rather not use it? Jump to
> [section 6: pure Linux install](#6-no-docker-pure-linux-install).

### Step 2 — clone and run the one-command installer

```bash
git clone https://github.com/Junesgone/RayRAG.git
cd RayRAG
./install.sh
```

`install.sh` does everything else for you:

1. writes `.env` — the **bundled database starts with the `admin` role**; the first account you register becomes the administrator. That administrator password also becomes the PostgreSQL admin role password (the first registration and every later password change update both sides automatically; the rotated credential is kept in the state volume and the env file),
2. pulls base images and dependencies through **mirrors that work in mainland China**
   (the first run takes 5–15 minutes depending on your connection);
3. starts the database and the application and waits until the service is really healthy;
4. prints the URL and asks you to register the first administrator account.

When it finishes you will see something like:

```
============================================================
  RayRAG is up and running 🎉
------------------------------------------------------------
  Web UI   : http://192.168.1.10:9380
  Login    : the first account you register becomes the administrator (no preset password)
------------------------------------------------------------
  Logs     : docker compose logs -f rayrag
  Stop     : docker compose down
  Restart  : docker compose restart
============================================================
```

Open the Web UI, register with your email and chosen password, then complete the first-run guide.

If startup reports a damaged `pg_credentials.json`, restore the file from a trusted backup or repair its JSON `url` field using the database's current credentials, then restart. The file lives in the configured static/state directory. Keep its contents private.

### Useful flags

```bash
./install.sh --port 8080        # listen on another web port (default 9380)
./install.sh --global-mirror    # build against upstream mirrors instead of CN ones
./install.sh --no-build         # start without rebuilding the image
./install.sh --dry-run          # check everything and write .env, then stop
./install.sh --help             # all options
```

### Prefer doing it by hand? Three commands

```bash
cp .env.example .env
# Review the database connection settings in .env, then start.
# Register your administrator account in the Web UI after startup.
docker compose up -d --build
```

> **Where your settings live:** the `.env` file in this project folder. RayRAG mounts it
> into the container, so the first-login setup page (section 2) writes exactly the file
> you can open, edit and back up — one file, no hidden copy.

---

## 2. Open the web UI, first steps

1. **Open a browser** at `http://<your-host-ip>:9380` (locally:
   `http://127.0.0.1:9380`).

> **Running RAGFlow on the same machine?** RayRAG's default port, 9380, is also RAGFlow's, so the
> two cannot both hold it. Change one line in `.env` — `RAYRAG_PORT=9390` — and run
> `docker compose up -d` again; the command above prints the URL that is actually in use, and
> `docker compose port rayrag 8080` tells you the published port at any time.
2. **Register** with your email and a password of at least 12 characters. The first registered
   account becomes the administrator. Sign in and complete the first-run setup.
3. **Create a knowledge base**: left menu `Knowledge Base` → `Create knowledge base` →
   type a name → save.
4. **Upload documents**: open the knowledge base → `Files` tab → `+ Add file`, drop your
   PDF / Word / Excel / images → confirm.
5. **Parse**: tick the files → click `Parse` (the green play button) → wait for the status to
   become `DONE`. Large files take a while; the `Logs` tab shows progress.
6. **Ask questions**:
   - quick check: left menu `Search` → type a question → Enter, and you get the matching passages;
   - ChatGPT-style chat: left menu `Chat` → `Create chat assistant` → pick your knowledge base →
     start chatting.

Almost every button is bilingual (`English` / `简体中文`); switch it under your avatar → `Language`.

**First login already configures the important parts for you.** The opening page (`/setup`)
detects this host's CPU and memory, suggests how many documents to parse at once, and lists
the settings that matter — model endpoints, storage, resource limits, the search engine, and
the sign-up switch. Saving writes them to the project's `.env` and tells you which took
effect immediately and which need a restart. Everything there can also be set by environment
variable, exactly as in RAGFlow.

Two things make it quick to trust:

- **Values are filled in already.** Anything RayRAG would default to is shown and tagged
  `default`, so you read it and press Save; fields that have no safe default (a model
  endpoint, an API key, your database) stay empty rather than guessing an address for you.
- **The page and `.env` stay in step.** Edit `.env` in another window and the page picks the
  change up within seconds. If you have unsaved edits at that moment it asks before
  reloading, so nothing you typed is thrown away silently.

---

## 3. Connect a model (what makes answers smart)

**RayRAG works without any model**: parsing, chunking and keyword retrieval all run. But
*chat answers* and *vector semantic search* need one chat model and one embedding model.
RayRAG ships no models and is not tied to any vendor.

### 3.1 Through the web UI (recommended)

1. Avatar (top right) → **`Model providers`**
2. Under **Available models**, find your provider (e.g. `DeepSeek`) and click `Add` on its card.
3. Fill in:
   - **Instance name** — any label, e.g. `my-deepseek`
   - **API-Key** — the key from the provider's console
   - **Base-Url** — usually prefilled; only change it for a gateway or self-hosted endpoint
4. Click **`Verify`** to test connectivity, then **`Ok`**.
5. Back at the top of the page, **Set default models**: pick your new model as `LLM` and
   `Embedding` (`VLM`, `ASR`, `Rerank`, `TTS` are optional).

### 3.2 Not sure what a model can do? Let RayRAG look it up

Custom models (gateways, self-hosted servers, a provider RayRAG has never heard of) ask you
for facts you may not have at hand: does it support tool calls, how large is its context
window, what does it cost per million tokens. RayRAG can look those up for you.

1. In the model dialog, open **`List models` → `Add custom model`**.
2. Type the model name (for example `deepseek-flash`) and press **`Look up model info`**.
3. RayRAG consults the public model catalogue at
   [models.agent-one.dev](https://models.agent-one.dev/list) and shows what it found:
   the provider, the context window, capability tags (tool calls / reasoning / vision /
   structured output), and the published price per million tokens in and out.
4. The dialog fills in the fields you had left empty — model types, max tokens, tool-call
   support. Anything you typed yourself is kept until you press **`Use these values`**.

The same lookup is available when you add a whole provider: fill in the **API Base** in
**`+ Add Provider`** and press **`Look up provider`** — RayRAG recognises the endpoint,
fills in the provider id and name, and offers that provider's model names for the model box.
The answer is cached for a day, so lookups are instant and keep working even when the
catalogue is unreachable (you then simply see the last copy it had).

### 3.3 Popular providers

| Provider | Pick in RayRAG | Base-Url (usually prefilled) | Notes |
|---|---|---|---|
| DeepSeek | `DeepSeek` | `https://api.deepseek.com/v1` | `deepseek-flash` (V4.1) with built-in web search, URL reading and safety; legacy `deepseek-chat`/`deepseek-reasoner` still work |
| Alibaba Qwen | `Tongyi-Qianwen` | DashScope compatible endpoint | chat + embedding |
| SiliconFlow | `SILICONFLOW` | `https://api.siliconflow.cn/v1` | chat + embedding + rerank in one |
| Zhipu AI | `ZHIPU-AI` | `https://open.bigmodel.cn/api/paas/v4` | free tier available |
| Moonshot (Kimi) | `Moonshot` | `https://api.moonshot.cn/v1` | long context |
| VolcEngine (Doubao) | `VolcEngine` | endpoint from the console | create an inference endpoint first |
| Baidu Qianfan | `BaiduYiyan` | endpoint from the console | — |
| iFlytek Spark | `XunFei Spark` | — | — |
| Tencent Hunyuan | `Tencent Hunyuan` | — | — |
| Any OpenAI-compatible service | `OpenAI-API-Compatible` | your endpoint | gateways, proxies, self-hosted |

### 3.4 Local models on your own GPU (free, data never leaves the machine)

| Runtime | Pick in RayRAG | Default endpoint |
|---|---|---|
| Ollama | `Ollama` | `http://127.0.0.1:11434` |
| vLLM | `VLLM` | `http://127.0.0.1:8000/v1` |
| LM Studio | `LM-Studio` | `http://127.0.0.1:1234/v1` |
| llama.cpp (llama-server) | `OpenAI-API-Compatible` | `http://127.0.0.1:8080/v1` |
| Xinference | `Xinference` | `http://127.0.0.1:9997` |
| GPUStack | `GPUStack` | see your GPUStack dashboard |

> From inside Docker, a model server on the host is **not** `127.0.0.1`. Use
> `http://host.docker.internal:11434` (on Linux use the host's LAN IP, e.g.
> `http://192.168.1.10:11434`).

Local OpenAI-compatible endpoints (loopback, private/LAN IPs, single-label hostnames,
`.local`/`.lan`/`.home`/`.internal`) are called **without connection or total request timeouts**, so a
slow local GPU model can finish; public providers use a 10-second connection timeout and keep
the `RAYRAG_MODEL_TIMEOUT` budget (default 300 s). Set it lower only if your remote gateway should give up sooner.

### 3.5 Through environment variables (good for fleets)

Edit `.env`, then `docker compose up -d`:

```ini
LLM_API_BASE=http://192.168.1.10:8088/v1     # chat model
LLM_API_KEY=sk-xxxx
LLM_MODEL=Qwen3.5-9B-Q4_K_M.gguf

EMBED_API_BASE=http://192.168.1.10:8888/v1   # embedding model
EMBED_API_KEY=
EMBED_MODEL=Qwen3-Embedding-4B-Q4_K_M.gguf

RERANK_API_BASE=http://192.168.1.10:8899/v1  # reranker (optional)
RERANK_MODEL=bge-reranker-v2-m3
```

> **Important:** if the model server is not on the same machine as RayRAG, `127.0.0.1` will
> never work — use the real IP. The `System` page shows whether each link is healthy.

---

## 4. FAQ (read this first when something breaks)

**Q1 — `install.sh` says `docker: command not found`, or `docker compose` is unknown.**
Install Docker (step 1). On Linux also make sure your user is in the `docker` group:
`sudo usermod -aG docker $USER`, then log out and back in.

**Q2 — port 9380 is already in use.**
Pick another port: `./install.sh --port 8080`. Already deployed? Change `RAYRAG_PORT` in `.env`
and run `docker compose up -d`.

**Q3 — are there password rules?**
Choose a password of at least **12 characters** when registering or changing your password.
There is no preset admin account: the first account registered in the browser becomes the
administrator.

**Q4 — I forgot the login password.**
Use **Forgot password** on the login page and follow the verification-code flow.
The response explains where the code was delivered. While signed in, use
avatar → `Profile` → `Password`. An administrator can also reset another account's password
from user management.

**Q4b — how do I change the database credentials?**
Before the first startup, configure `RAYRAG_POSTGRES_USER` and `RAYRAG_POSTGRES_PASSWORD`
in `.env`. After deployment, change the RayRAG administrator password through the profile page
to rotate the database role password and save the updated connection settings automatically.
The PostgreSQL role name stays at the configured value. A failed rotation returns repair guidance.

**Q5 — builds or image pulls are slow.**
CN mirrors are already the default (DaoCloud images, Tsinghua apt, a CN crates mirror).
If it is still slow, force the CN profile:
```bash
RAYRAG_MIRROR_PROFILE=cn docker compose build rayrag
```

**Q6 — chat says it found nothing.**
Check in order: ① are the documents `DONE` (see the `Files` tab); ② is an embedding model
selected under `Set default models`; ③ does the `System` page report the embedding link healthy?

**Q7 — where is my data?**
Entirely in Docker volumes: `rayrag-state` (files and business data), `rayrag-zvec-data`
(vector index), `rayrag-postgres-data` (database). Nothing is sent to a third party.

**Q8 — can it handle images and scanned PDFs?**
Yes, with an OCR / vision model configured (`OCR` tab, or a `VLM` default model). Mainland-China
options include PaddleOCR, MinerU, SoMark, OpenDataLoader, Docling and OXR, or a local vision model.

**Q9 — why is my reranker unavailable?**
If the GPU is fully occupied by the chat model, the reranker cannot load. Move it to another
machine/port, or simply leave `Rerank` unset under `Set default models` — retrieval still works,
just without reranking.

**Q10 — how do I know the service is really healthy?**
```bash
docker compose ps                      # both services should say healthy
curl http://127.0.0.1:9380/api/v1/system/healthz
# {"code":0,"data":{"status":"healthy","postgres":"healthy","zvec":"healthy"}}
```

**Q11 — no Docker at all?**
See the next section.

**Q12 — Windows / macOS?**
Docker mode runs on both (arm64 / Apple Silicon included). The bare-metal binary is Linux-only.

---

## 4b. Artifact / navigation generation produces nothing

If "Build navigation tree" or the artifact **Generate** button reports
`extracts: 0` / `failed: n` instead of pages, the usual cause is the **chat model endpoint**, not the
dataset. Check it in this order:

1. **Is the endpoint actually running?** From the host:
   `curl -s -o /dev/null -w '%{http_code}\n' "$LLM_API_BASE/v1/models"` — `000` means nothing is
   listening. RayRAG logs the reason (`wiki_gen_json: the chat call failed error=error sending request
   for url (…)`) and reports the run as failed rather than pretending the dataset was empty.
2. **Is it reachable from inside the container?** A service on the machine running RayRAG is **not**
   `127.0.0.1` from inside the container; use the docker gateway address
   (`docker compose exec rayrag ip route` shows it). The embedding endpoint answering `200` proves the
   path works — if it does and the chat endpoint does not, the chat service is simply down.
3. **Do not point `LLM_API_BASE` at the embedding port.** An embedding server will happily answer
   `/v1/chat/completions` with HTTP 200 and return nonsense (`systemsystemsystem…`), so the failure
   looks like "the model produced no entities" instead of "wrong model". `EMBED_API_BASE` is for
   embeddings, `LLM_API_BASE` for chat.

Then re-run the compile; `extracts > 0` proves the model is really being used.

---

## 5. Day-2 operations: upgrade / backup / uninstall

```bash
# upgrade
git pull
./install.sh                     # rebuild and start

# logs (first thing to check)
docker compose logs -f rayrag

# stop / start
docker compose stop
docker compose start

# backup the data volume into the current directory
docker run --rm -v rayrag-state:/data -v "$PWD":/backup alpine \
  tar czf /backup/rayrag-backup-$(date +%F).tar.gz -C /data .

# uninstall (keep data)
docker compose down

# uninstall and delete everything (irreversible!)
docker compose down -v
```

### Release versioning

RayRAG publishes small `0.x.y` releases. Every round ships a new version number and a matching
GitHub release, so upgrade notes always identify the exact code you are running. `1.0.0` is
reserved until the maintainer explicitly approves it; do not expect an automatic jump to `1.0.0`.
Upgrading within the `0.x` line keeps the existing data volume and configuration.

---

## 6. No Docker: pure Linux install

You need three things: **Rust 1.97+**, a **PostgreSQL 18.4** and the **zvec shared library**.

```bash
# 1) Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

# 2) build (PostgreSQL metadata + the native zvec vector backend — both default)
#    ZVEC_LIB_DIR points at the folder holding libzvec_c_api.so and TARGET
export ZVEC_LIB_DIR="$HOME/.local/lib/zvec/0.8.0"   # any version present under ~/.local/lib/zvec
cargo build --release --locked

# 3) configure
export RAYRAG_POSTGRES_URL='postgresql://your-db-role:your-db-password@127.0.0.1:5432/rayrag'
# Register the first administrator through the Web UI after startup.

# 4) run — flag first, then RAYRAG_PORT from .env, then 9380
./target/release/rayrag serve --port 9380
```

`serve` also reads `RAYRAG_PORT` and `RAYRAG_STATIC_DIR` from the environment, including the `.env`
file at the project root. On a machine where RAGFlow already occupies 9380, put `RAYRAG_PORT=9398`
in `.env` and run `./target/release/rayrag serve` with **no arguments**. Precedence is
**`--port` > `.env` `RAYRAG_PORT` > 9380**.

Running it also needs `libzvec_c_api.so` to be findable. Building with `ZVEC_LIB_DIR` bakes that
directory into the binary's runpath (`readelf -d target/release/rayrag | grep RUNPATH` shows it), so
no `LD_LIBRARY_PATH` is needed afterwards; if you build without it, or move the library, you will see
`error while loading shared libraries: libzvec_c_api.so` — re-export `ZVEC_LIB_DIR` (this machine has
0.7.2 under `$HOME/.local/lib/zvec/`) and rebuild, or run with
`LD_LIBRARY_PATH=/path/to/zvec/lib`.

`postgres-backend` and `zvec-backend` are the crate's default features, so the command above already
builds the shipped configuration and the binaries use **zvec** unless asked otherwise: point it at a
data directory with `RAYRAG_ZVEC_DIR=/var/lib/rayrag/zvec` (default `./zvec-data`). The resolved
`ZVEC_LIB_DIR` is baked into the binary's runpath, so no `LD_LIBRARY_PATH` is needed to run it. For an
environment without the native library, `cargo build --release --locked --no-default-features
--features postgres-backend` keeps the portable JSON index (`RAYRAG_VECTOR_BACKEND=json`).

> Full environment reference, systemd units and private-deployment notes:
> [`docs/advanced.md`](docs/advanced.md).

---

## 7. fnOS NAS app (FPK)

On a 飞牛 fnOS NAS you can install RayRAG as a normal App Center application instead of
running Docker yourself. The package bundles the Rust server, PostgreSQL 18.4, zvec and
every runtime library it needs, and registers a desktop icon.

1. Download \`rayrag_<version>_x86_64.fpk\` from the [Releases](https://github.com/Junesgone/RayRAG/releases) page.
2. App Center → **Manual install** → choose the \`.fpk\` and confirm.
3. Accept the wizard (the bundled PostgreSQL password defaults to \`admin\`).
register the first account (it becomes the administrator).

The database and user data live in the \`rayrag\` share on the volume you pick, so backups
are a file-manager copy. Build your own package with \`./fpk/build-fpk.sh\` (details in
[\`fpk/README.md\`](fpk/README.md)); the default portable-PostgreSQL download uses the
Aliyun Maven mirror, reachable in mainland China.

---

## 8. What is inside

| Page | What you can do |
|---|---|
| `Knowledge Base` | create KBs, upload/parse documents, inspect and edit chunks, knowledge graph, retrieval test, logs |
| `Chat` | create assistants, streaming answers, citations, conversation management |
| `Search` | multi-KB retrieval, reranking, related-question suggestions |
| `Agent` | visual canvas: retrieval, web search, code execution, branches, loops — each agent keeps a run log you can search, sort and export as CSV |
| `Files` | one place for every uploaded file |
| `Skills` | skill index configuration and search |
| `Memories` | long-term memory (write and recall) |
| `Model providers` | 71 vendors and local runtimes, default-model settings |
| `Data sources` | sync from 35 sources (S3, Notion, Yuque, Feishu, GitLab, …) |
| `System` / `Admin` | health, users and teams, permissions, monitoring |
| `API` | OpenAI-compatible `/api/v1/chat/completions`, `/embeddings`, `/rerank` — drop-in for LangChain, OpenWebUI, Dify |

**Stack**: Rust (Axum + Tokio) · zvec vector store · PostgreSQL 18.4 · single binary.

**Relationship to RAGFlow**: page hierarchy, buttons and interactions track RAGFlow v0.27.2;
the parsing pipeline is rewritten in Rust; MySQL / Elasticsearch / Redis / MinIO are replaced by
PostgreSQL + zvec, which makes deployment much lighter.

---

WebDAV data sources served over HTTPS from a local-network host (private IP, single-label
hostname or a .local/.lan/.home/.internal suffix) accept a NAS self-signed certificate;
public hostnames keep normal certificate verification. Set
RAYRAG_WEBDAV_ALLOW_INSECURE_TLS=1 to allow a self-signed certificate for any other host.

## 9. More documentation

| Document | Content |
|---|---|
| [`README.zh-CN.md`](README.zh-CN.md) | this README in Chinese |
| [`docs/advanced.md`](docs/advanced.md) | architecture, full environment reference, password/TLS, Agent canvas executor, parser integrations |
| [`docs/providers-cn.md`](docs/providers-cn.md) | connectors and search sources for mainland-China networks |
| [`CHANGELOG.md`](CHANGELOG.md) | what changed in every release |
| [`NOTICE`](NOTICE) | upstream attribution and third-party assets |

---

## License

Apache License 2.0 — see [`LICENSE`](LICENSE).
