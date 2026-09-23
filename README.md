# RayRAG

**English** · [简体中文](README.zh-CN.md)

**In one sentence: throw your documents (PDF / Word / Excel / PPT / images / web pages) at it, then ask questions in plain language and get answers grounded in those documents.**

- Written in **Rust**. It idles at **a few tens of MB of RAM** (RAGFlow needs ~1 GB to start).
- The UI and feature set follow the open-source project RAGFlow, but RayRAG needs **no** MySQL,
  Elasticsearch, Redis or MinIO: vectors live in [zvec](https://github.com/zvec-ai/zvec-rust)
  and metadata lives in **PostgreSQL 18.4**.
- **Works on networks in mainland China**: container images, package mirrors and web search all
  use endpoints reachable from there.
- Ships with **65 model providers** (DeepSeek, Qwen, Zhipu, Moonshot, SiliconFlow, VolcEngine,
  Baichuan, …) plus local Ollama / vLLM / LM Studio / llama.cpp.
- No Docker? Run a single binary instead (pure Linux mode).

---

## Table of contents

- [1. Up and running in five minutes](#1-up-and-running-in-five-minutes)
- [2. Open the web UI, first steps](#2-open-the-web-ui-first-steps)
- [3. Connect a model (what makes answers smart)](#3-connect-a-model-what-makes-answers-smart)
- [4. FAQ (read this first when something breaks)](#4-faq-read-this-first-when-something-breaks)
- [5. Day-2 operations: upgrade / backup / uninstall](#5-day-2-operations-upgrade--backup--uninstall)
- [6. No Docker: pure Linux install](#6-no-docker-pure-linux-install)
- [7. What is inside](#7-what-is-inside)
- [8. More documentation](#8-more-documentation)

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

1. creates `.env` and generates **two random passwords** (PostgreSQL + admin login);
2. pulls base images and dependencies through **mirrors that work in mainland China**
   (the first run takes 5–15 minutes depending on your connection);
3. starts the database and the application and waits until the service is really healthy;
4. prints the URL, the login e-mail and the password on screen.

When it finishes you will see something like:

```
============================================================
  RayRAG is up and running 🎉
------------------------------------------------------------
  Web UI   : http://192.168.1.10:9380
  Login    : admin@rayrag.local
  Password : see RAYRAG_ADMIN_PASSWORD in the .env file
------------------------------------------------------------
  Logs     : docker compose logs -f rayrag
  Stop     : docker compose down
  Restart  : docker compose restart
============================================================
```

> **Write the password down: it only exists in the `.env` file.**

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
# Open .env in an editor and replace RAYRAG_ADMIN_PASSWORD and
# RAYRAG_POSTGRES_PASSWORD with your own (at least 12 characters each), then save.
docker compose up -d --build
```

> **Where your settings live:** the `.env` file in this project folder. RayRAG mounts it
> into the container, so the first-login setup page (section 2) writes exactly the file
> you can open, edit and back up — one file, no hidden copy.

---

## 2. Open the web UI, first steps

1. **Open a browser** at `http://<your-host-ip>:9380` (locally:
   `http://127.0.0.1:9380`).
2. **Log in** with `admin@rayrag.local` and the `RAYRAG_ADMIN_PASSWORD` from `.env`.
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
2. Type the model name (for example `deepseek-chat`) and press **`Look up model info`**.
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
| DeepSeek | `DeepSeek` | `https://api.deepseek.com/v1` | cheapest solid chat model |
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
Yes: at least **12 characters**. A shorter one makes startup fail with
`RAYRAG_ADMIN_PASSWORD must be at least 12 characters`.

**Q4 — I forgot the login password.**
Change `RAYRAG_ADMIN_PASSWORD` in `.env`, then:
```bash
docker compose down && docker volume rm rayrag-state && ./install.sh
```
> This **wipes existing data**. To only change the password, log in and use
> avatar → `Profile` → `Password`.

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
options include PaddleOCR, MinerU and SoMark, or a local vision model.

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

---

## 6. No Docker: pure Linux install

You need three things: **Rust 1.97+**, a **PostgreSQL 18.4** and the **zvec shared library**.

```bash
# 1) Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

# 2) build (PostgreSQL metadata + the native zvec vector backend — both default)
#    ZVEC_LIB_DIR points at the folder holding libzvec_c_api.so and TARGET
export ZVEC_LIB_DIR="$HOME/.local/lib/zvec/0.7.1"
cargo build --release --locked

# 3) configure
export RAYRAG_POSTGRES_URL=postgresql://rayrag:YOUR-PASSWORD@127.0.0.1:5432/rayrag
export RAYRAG_ADMIN_EMAIL=admin@rayrag.local
export RAYRAG_ADMIN_PASSWORD='at-least-12-characters'

# 4) run
./target/release/rayrag serve --port 9380
```

`postgres-backend` and `zvec-backend` are the crate's default features, so the command above already
builds the shipped configuration and the binaries use **zvec** unless asked otherwise: point it at a
data directory with `RAYRAG_ZVEC_DIR=/var/lib/rayrag/zvec` (default `./zvec-data`). The resolved
`ZVEC_LIB_DIR` is baked into the binary's runpath, so no `LD_LIBRARY_PATH` is needed to run it. For an
environment without the native library, `cargo build --release --locked --no-default-features
--features postgres-backend` keeps the portable JSON index (`RAYRAG_VECTOR_BACKEND=json`).

> Full environment reference, systemd units and private-deployment notes:
> [`docs/advanced.md`](docs/advanced.md).

---

## 7. What is inside

| Page | What you can do |
|---|---|
| `Knowledge Base` | create KBs, upload/parse documents, inspect and edit chunks, knowledge graph, retrieval test, logs |
| `Chat` | create assistants, streaming answers, citations, conversation management |
| `Search` | multi-KB retrieval, reranking, related-question suggestions |
| `Agent` | visual canvas: retrieval, web search, code execution, branches, loops — each agent keeps a run log you can search, sort and export as CSV |
| `Files` | one place for every uploaded file |
| `Skills` | skill index configuration and search |
| `Memories` | long-term memory (write and recall) |
| `Model providers` | 65 vendors and local runtimes, default-model settings |
| `Data sources` | sync from 35 sources (S3, Notion, Yuque, Feishu, GitLab, …) |
| `System` / `Admin` | health, users and teams, permissions, monitoring |
| `API` | OpenAI-compatible `/api/v1/chat/completions`, `/embeddings`, `/rerank` — drop-in for LangChain, OpenWebUI, Dify |

**Stack**: Rust (Axum + Tokio) · zvec vector store · PostgreSQL 18.4 · single binary.

**Relationship to RAGFlow**: page hierarchy, buttons and interactions track RAGFlow v0.26.4;
the parsing pipeline is rewritten in Rust; MySQL / Elasticsearch / Redis / MinIO are replaced by
PostgreSQL + zvec, which makes deployment much lighter.

---

## 8. More documentation

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
