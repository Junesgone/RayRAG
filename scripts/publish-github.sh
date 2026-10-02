#!/usr/bin/env bash
# Publish a sanitized snapshot of RayRAG to GitHub.
#
# The local working tree carries development-only material (CDP probe scripts,
# captured UI dumps, playwright's node_modules, an 87 MB model weight, runtime
# state JSON) plus machine-specific identifiers (home directory, host name,
# personal e-mail). None of that belongs in the published repository, so this
# script exports the tracked tree at HEAD into a separate clone, drops those
# paths, applies deterministic redactions, and pushes the result.
#
# Usage: scripts/publish-github.sh [--dry-run]
#   PUBLISH_REPO   GitHub repository slug (default Junesgone/RayRAG)
#   PUBLISH_DIR    local clone of the published repository
#   PUBLISH_PROXY  HTTP proxy used for GitHub (default http://127.0.0.1:7890,
#                  the local mihomo listener; set to "off" for a direct push)
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
REPO="${PUBLISH_REPO:-Junesgone/RayRAG}"
PUB_DIR="${PUBLISH_DIR:-$HOME/.openclaw/workspace/RayRAG-public}"
PROXY="${PUBLISH_PROXY:-http://127.0.0.1:7890}"
DRY_RUN="${1:-}"

# Files and directories that never ship.
EXCLUDES=(
    # Large model weights and development-only material.
    "models"
    "scripts"
    "docs/ui-traversal"
    # Internal operations ledger and parity ledger (engineering assets, not
    # product material).
    "运维记录.md"
    "MAINTENANCE.md"
    "docs/ragflow-v0.26.4-coverage.md"
    "docs/ragflow-v0.27.2-coverage.md"
    "docs/ragflow-v0.27.2-delta.md"
    "docs/ragflow-v0.27.2-worklist.md"
    "docs/ragflow-file-list.txt"
    "docs/ragflow-file-manifest.json"
    "docs/ragflow-ui-inventory.md"
    "docs/ragflow-ui-ref"
    # Internal parameter/interface registry (engineering lookup surface).
    "docs/parameter-interface-registry.md"
    # Internal parity ledgers and the plan derived from them: never published.
    "docs/api-parity-worklist.md"
    "docs/api-parity-plan.md"
    # The coverage gate validates the internal parity ledger against a pinned
    # RAGFlow checkout; neither the ledger nor the reference tree ships, so the
    # binary is internal engineering tooling.
    "src/bin/ragflow-coverage.rs"
    # Deployment scaffolding kept out of the published tree.
    "helm"
    "web/agents.json"
    "web/canvas_versions.json"
    "web/chat_channels.json"
    "web/compilation_templates.json"
    "web/conversations.json"
    "web/data_sources.json"
    "web/evaluations.json"
    "web/kbs.json"
    "web/memories.json"
    "web/memory_messages.json"
    "web/providers.json"
    "web/search_apps.json"
    "web/skill_index.json"
    "web/tasks.json"
    "web/users.json"
    "web/tenants.json"
)

# Personal / machine-specific strings, rewritten deterministically.
redact() {
    local file="$1"
    python3 - "$file" <<'PY'
import re, sys

path = sys.argv[1]
try:
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
except (UnicodeDecodeError, FileNotFoundError):
    sys.exit(0)

original = text
# The published repository slug is public by definition and appears in the
# clone instructions; shield it before the personal-name rewrite so the
# quickstart keeps working, then restore it below.
SLUG = "Junesgone/RayRAG"
SHIELD = "\x00repo-slug\x00"
text = text.replace(SLUG, SHIELD)
text = text.replace("/home/junesgone", "$HOME")
text = text.replace("junesgone", "user").replace("Junesgone", "User")
text = text.replace("Nicpet", "dev-host")
text = text.replace("fnOS Chrome 149", "Chrome").replace("fnOS Chrome", "Chrome")
text = text.replace("fnOS", "自托管主机")
# `<user>@<host>:<port>` SSH references and LAN endpoints.
text = re.sub(r"root@[A-Za-z0-9._-]+:[0-9]+", "root@<host>:<port>", text)
text = re.sub(r"http://192\.168\.[0-9]+\.[0-9]+:18888", "http://<searxng-host>:18888", text)
# The author's LAN host (used as the local model/OCR endpoint in the ops log).
text = text.replace("192.168.3.6", "<host-lan-ip>")
text = text.replace("rayrag-smoke-admin", "<redacted>")
text = re.sub(r"[A-Za-z0-9._%+-]*Junesgone[A-Za-z0-9._%+-]*@[A-Za-z0-9.-]+", "user@example.com", text)
# The environment baseline section of the ops log describes one private host.
text = re.sub(
    r"## 环境基线（[^）]*）\n.*?(?=\n## )",
    "## 环境基线\n\n"
    "- 本项目在纯 Linux（直接运行 `target/*/rayrag`）与纯 Docker（`docker compose up`）两种形态下均已验证；\n"
    "  具体验证机的硬件、容器清单与内存占用属于部署方私有信息，不随仓库发布。\n"
    "- 本机 Rust 工具链与容器内工具链存在补丁级差异，构建标识由 `build.rs` 注入，可用于区分二进制来源。\n\n",
    text,
    flags=re.S,
)
# The published tree carries no operations or parity ledger. The READMEs are
# written for end users and never point at internal assets, so they need no
# dedicated rewriting here -- the generic `.md` rules below are the backstop
# that rewrites any reference that slips through.
if path.endswith("Cargo.toml"):
    # The coverage gate is excluded from the snapshot, so its explicit [[bin]]
    # stanza must go too or cargo refuses to resolve the manifest.
    text = re.sub(
        r'\n\[\[bin\]\]\nname = "ragflow-coverage"\npath = "src/bin/ragflow-coverage\.rs"\n',
        "\n",
        text,
    )
if path.endswith("CHANGELOG.md"):
    text = re.sub(r"\*\*CDP 探针\*\* `scripts/ui-traversal/[^`]+`：", "**浏览器端验证**：", text)
    text = re.sub(
        r"- \*\*`scripts/publish-github\.sh`\*\*：.*?守卫扫描。",
        "- **净化发布流程**：当前提交导出为净化快照（剔除开发脚本、抓取产物、模型权重与运行时状态，"
        "并对机器标识脱敏）后推送 GitHub；脚本保留在内部工程仓。",
        text,
        flags=re.S,
    )

if path.endswith(".md"):
    # Documents must not point at engineering assets that stay internal.
    for pattern, replacement in (
        (r"`scripts/ui-traversal/[^`]*`", "浏览器端验证脚本"),
        (r"`scripts/ui-traversal/`", "浏览器端验证脚本"),
        (r"`scripts/publish-github\.sh`", "内部发布脚本"),
        (r"`scripts/[A-Za-z0-9_.-]+\.(sh|py)`", "内部辅助脚本"),
        (r"`docs/ui-traversal/[^`]+`", "内部 UI 抓取产物"),
        (r"docs/ragflow-v0\.[0-9.]+-coverage\.md", "内部对标台账"),
        (r"docs/ragflow-v0\.[0-9.]+-delta\.md", "内部对标台账"),
        (r"docs/ragflow-v0\.[0-9.]+-worklist\.md", "内部对标工作清单"),
        (r"docs/ragflow-file-(list\.txt|manifest\.json)", "内部文件清单"),
        (r"docs/ragflow-ui-inventory\.md", "内部界面盘点"),
        (r"docs/parameter-interface-registry\.md", "内部参数接口登记表"),
        (r"`docs/ragflow-ui-ref/[^`]+`", "内部界面盘点"),
        (r"`MAINTENANCE\.md`", "内部运维台账"),
        (r"`运维记录\.md`", "内部运维台账"),
        (r"`helm/`", "Kubernetes 部署脚手架"),
    ):
        text = re.sub(pattern, replacement, text)

text = text.replace(SHIELD, SLUG)

if text != original:
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)
    print(f"  redacted {path}")
PY
}

echo "==> exporting $ROOT at $(git rev-parse --short HEAD)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
git archive --format=tar HEAD | tar -x -C "$WORK"
# Cargo.lock is intentionally tracked in the published snapshot so the
# documented `--locked` builds are reproducible.
cp -f Cargo.lock "$WORK/Cargo.lock"
for path in "${EXCLUDES[@]}"; do
    rm -rf "$WORK/$path"
done
# The exclusion list above is written by name, and that mechanism has no memory of
# files added later: `docs/api-parity-plan.md` would have been published the moment it
# was created. Re-check the export against the list and refuse to publish on any hit, so
# the next internal document cannot leak just because nobody remembered the list.
leaked=0
for path in "${EXCLUDES[@]}"; do
    if [ -e "$WORK/$path" ]; then
        echo "FATAL: internal path survived exclusion: $path" >&2
        leaked=1
    fi
done
# Second guard, independent of the list: no internal ledger/registry may appear anywhere
# in the export under any name.
while IFS= read -r found; do
    echo "FATAL: internal document in the export: ${found#"$WORK"/}" >&2
    leaked=1
done < <(find "$WORK" -type f \( \
    -name '运维记录.md' -o -name '*api-parity*' -o -name '*parameter-interface-registry*' \
    -o -name 'MAINTENANCE.md' -o -name 'ragflow-*-coverage.md' -o -name 'ragflow-*-worklist.md' \
    -o -name 'ragflow-file-list.txt' -o -name 'ragflow-ui-inventory.md' \) 2>/dev/null)
if [ "$leaked" -ne 0 ]; then
    echo "publish aborted: fix EXCLUDES or remove the file from the tracked tree" >&2
    exit 1
fi
# `scripts/docker-build.sh` stamps the git revision into the image, so the
# documented Docker workflow needs it even though the rest of scripts/ is
# development tooling.
mkdir -p "$WORK/scripts"
cp -f scripts/docker-build.sh "$WORK/scripts/docker-build.sh"
# Development scratch scripts (bisect/patch helpers, the local SSH proxy).
find "$WORK/scripts" -maxdepth 1 -name '_*.py' -delete
rm -f "$WORK/scripts/ssh-websocket-proxy.py"
# LICENSE / NOTICE for the published snapshot.
cp -f LICENSE "$WORK/LICENSE" 2>/dev/null || true
cp -f NOTICE "$WORK/NOTICE" 2>/dev/null || true

echo "==> redacting machine-specific identifiers"
while IFS= read -r -d '' file; do
    redact "$file"
done < <(find "$WORK" -type f \( -name '*.md' -o -name '*.rs' -o -name '*.json' -o -name '*.toml' \
    -o -name '*.yml' -o -name '*.yaml' -o -name '*.sh' -o -name '*.py' -o -name '*.mjs' \
    -o -name '*.html' -o -name '*.txt' -o -name 'Dockerfile' -o -name '*.example' \) -print0)

echo "==> scanning the snapshot for leftovers"
# Two classes of leftovers fail the push: any personal identifier other than
# the canonical `Junesgone/RayRAG` slug, and any pointer to an internal asset
# (the ops log, the parity ledgers, the UI-traversal scripts) that the snapshot
# does not ship -- the latter almost always means machine-specific or
# engineering-only context leaked into product material.
guard_hits=""
while IFS= read -r file; do
    if grep -nIE 'junesgone|Nicpet|fnOS|rayrag-smoke-admin|Junesgone@|root@[A-Za-z0-9._-]+:[0-9]+' "$file" \
        | grep -vE 'github\.com/Junesgone/RayRAG|Junesgone/RayRAG' | grep -q .; then
        guard_hits="${guard_hits}${file}"$'\n'
    fi
done < <(grep -rIln -E 'junesgone|Nicpet|fnOS|rayrag-smoke-admin|Junesgone@|root@[A-Za-z0-9._-]+:[0-9]+' "$WORK" \
    | grep -vE '/(LICENSE|NOTICE)$' | grep -v '^$')
if [ -n "$guard_hits" ]; then
    echo "ERROR: personal identifiers remain in the snapshot:" >&2
    printf '%s' "$guard_hits" >&2
    exit 1
fi

while IFS= read -r file; do
    if grep -nIE '运维记录|MAINTENANCE\.md|ragflow-v0\.[0-9.]+-coverage|ragflow-v0\.[0-9.]+-delta|ragflow-v0\.[0-9.]+-worklist|ragflow-file-list|ragflow-ui-inventory|parameter-interface-registry|ui-traversal' "$file" \
        | grep -q .; then
        echo "ERROR: internal-asset reference remains in the snapshot: $file" >&2
        grep -nIE '运维记录|MAINTENANCE\.md|ragflow-v0\.[0-9.]+-coverage|ragflow-v0\.[0-9.]+-delta|ragflow-v0\.[0-9.]+-worklist|ragflow-file-list|ragflow-ui-inventory|parameter-interface-registry|ui-traversal' "$file" >&2
        exit 1
    fi
done < <(grep -rIln -E '运维记录|MAINTENANCE\.md|ragflow-v0\.[0-9.]+-coverage|ragflow-v0\.[0-9.]+-delta|ragflow-v0\.[0-9.]+-worklist|ragflow-file-list|ragflow-ui-inventory|parameter-interface-registry|ui-traversal' "$WORK" \
    | grep -vE '/(LICENSE|NOTICE)$' | grep -v '^$')
if find "$WORK" -type f -size +20M | grep -q .; then
    echo "ERROR: large binary artifacts remain in the snapshot:" >&2
    find "$WORK" -type f -size +20M >&2
    exit 1
fi
echo "    snapshot files: $(find "$WORK" -type f | wc -l), size: $(du -sh "$WORK" | cut -f1)"

if [ "$DRY_RUN" = "--dry-run" ]; then
    echo "==> dry run: leaving the sanitized snapshot at $WORK"
    trap - EXIT
    exit 0
fi

if [ ! -d "$PUB_DIR/.git" ]; then
    echo "==> initialising published clone at $PUB_DIR"
    mkdir -p "$PUB_DIR"
    git -C "$PUB_DIR" init -q -b main
fi

echo "==> syncing into $PUB_DIR"
rsync -a --delete \
    --exclude '.git' \
    --exclude 'target' \
    --exclude 'data' \
    --exclude 'logs' \
    --exclude '.env' \
    "$WORK"/ "$PUB_DIR"/

cd "$PUB_DIR"
# The published repository carries a single snapshot commit: earlier snapshots
# may have contained material that must not stay reachable through history.
rm -rf "$PUB_DIR/.git"
git init -q -b main
git add -A
# `.gitignore` ignores Cargo.lock for the development tree; the published
# snapshot documents `--locked` builds, so it must travel with the sources.
git add -f Cargo.lock
git -c user.name="RayRAG Publisher" -c user.email="rayrag@example.invalid" \
    commit -q -m "$(git -C "$ROOT" log -1 --pretty=%s)"
echo "    published history: $(git rev-list --count HEAD) commit(s)"

if [ "$PROXY" != "off" ]; then
    export http_proxy="$PROXY" https_proxy="$PROXY" all_proxy="$PROXY"
fi
export GH_PROMPT_DISABLED=1
if ! git remote get-url origin >/dev/null 2>&1; then
    git remote add origin "https://github.com/$REPO.git"
fi
gh auth setup-git >/dev/null 2>&1 || true
if ! gh repo view "$REPO" >/dev/null 2>&1; then
    echo "==> creating $REPO"
    gh repo create "$REPO" --public \
        --description "RayRAG — Rust implementation tracking RAGFlow v0.27.2 parity (zvec + PostgreSQL)" \
        >/dev/null
fi
echo "==> pushing to $REPO"
git push -q --force origin main
# The published repository is public; flip an existing private repo over.
if [ "$(gh repo view "$REPO" --json visibility -q .visibility 2>/dev/null)" != "PUBLIC" ]; then
    gh repo edit "$REPO" --visibility public --accept-visibility-change-consequences >/dev/null
fi
echo "==> published $(git rev-parse --short HEAD) to https://github.com/$REPO"
