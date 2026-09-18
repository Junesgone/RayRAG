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
    "models"
    "scripts/publish-github.sh"
    "scripts/ui-traversal"
    "docs/ui-traversal"
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
if grep -rIln -E 'junesgone|Nicpet|fnOS|rayrag-smoke-admin|Junesgone@|root@[A-Za-z0-9._-]+:[0-9]+' "$WORK" \
    | grep -vE '/(LICENSE|NOTICE)$' | grep -v '^$'; then
    echo "ERROR: personal identifiers remain in the snapshot" >&2
    exit 1
fi
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
git add -A
if git diff --cached --quiet; then
    echo "==> nothing to publish"
else
    git -c user.name="RayRAG Publisher" -c user.email="rayrag@example.invalid" \
        commit -q -m "$(git -C "$ROOT" log -1 --pretty=%s)"
fi

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
    gh repo create "$REPO" --private \
        --description "RayRAG — Rust implementation tracking RAGFlow v0.26.4 parity (zvec + PostgreSQL)" \
        >/dev/null
fi
echo "==> pushing to $REPO"
git push -q origin main
echo "==> published $(git rev-parse --short HEAD) to https://github.com/$REPO"
