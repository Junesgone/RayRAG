#!/usr/bin/env bash
# Launch an isolated RayRAG debug instance for UI/probe verification.
# Usage: scripts/ui-traversal/run-isolated-instance.sh [INSTANCE_DIR] [PORT]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
INSTANCE="${1:-/tmp/rr-verify-instance}"
PORT="${2:-9391}"
mkdir -p "$INSTANCE/web" "$INSTANCE/zvec"
rsync -a --delete "$ROOT/web/static" "$INSTANCE/web/"
for f in "$ROOT"/web/*.json; do
    [ -f "$f" ] && cp -f "$f" "$INSTANCE/web/"
done
export ZVEC_LIB_DIR="${ZVEC_LIB_DIR:-$HOME/.local/lib/zvec/0.7.1}"
export RAYRAG_ZVEC_DIR="$INSTANCE/zvec"
exec "$ROOT/target/debug/rayrag" serve --port "$PORT" --static-dir "$INSTANCE/web/static"
