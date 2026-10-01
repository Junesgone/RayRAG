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
# Prefer the deployed container state (users/tenants/datasets) when available:
# the dev tree carries an empty store whose admin password differs from the
# deployment, so probes would fail to log in.
if docker inspect rayrag-rayrag-1 >/dev/null 2>&1; then
    for name in $(docker exec rayrag-rayrag-1 sh -c 'cd /app/web && ls *.json 2>/dev/null'); do
        docker cp "rayrag-rayrag-1:/app/web/$name" "$INSTANCE/web/$name" 2>/dev/null || true
    done
fi
export ZVEC_LIB_DIR="${ZVEC_LIB_DIR:-$HOME/.local/lib/zvec/0.7.1}"
export RAYRAG_ZVEC_DIR="$INSTANCE/zvec"
exec "$ROOT/target/debug/rayrag" serve --port "$PORT" --static-dir "$INSTANCE/web/static"
