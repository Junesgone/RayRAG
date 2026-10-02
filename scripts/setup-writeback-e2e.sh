#!/usr/bin/env bash
# First-run setup must really write to the env file.
#
# Runs a throwaway instance in an isolated directory (its own static dir, data dir and env
# file — `users.json` lives next to the static dir, so both must be isolated or the probe
# reuses the real user store) and asserts that `/api/v1/setup/complete` persists a value.
#
#   ./scripts/setup-writeback-e2e.sh
set -uo pipefail
cd "$(dirname "$0")/.."

PORT="${RAYRAG_E2E_PORT:-9399}"
DIR="$(mktemp -d)"
LOG="$DIR/rayrag.log"
cleanup() {
  [ -n "${PID:-}" ] && kill "$PID" 2>/dev/null
  sleep 1
  [ -n "${PID:-}" ] && kill -9 "$PID" 2>/dev/null
  rm -rf "$DIR"
}
trap cleanup EXIT

mkdir -p "$DIR/data" "$DIR/web"
cat > "$DIR/.env" <<ENV
RAYRAG_PORT=$PORT
RAYRAG_ENV_FILE=$DIR/.env
RAYRAG_DATA_DIR=$DIR/data
RAYRAG_STATIC_DIR=$DIR/web
RAYRAG_ADMIN_EMAIL=admin@rayrag.local
RAYRAG_ADMIN_PASSWORD=setup-e2e-password-123
ENV

RAYRAG_ENV_FILE="$DIR/.env" ./target/release/rayrag serve >"$LOG" 2>&1 &
PID=$!
for _ in $(seq 1 30); do
  curl -fsS --max-time 2 "http://127.0.0.1:$PORT/api/v1/version" >/dev/null 2>&1 && break
  sleep 2
done

TOKEN=$(curl -s --max-time 15 -X POST "http://127.0.0.1:$PORT/api/v1/auth/login" \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@rayrag.local","password":"setup-e2e-password-123"}' \
  | python3 -c "import json,sys;print(json.load(sys.stdin).get('data',{}).get('access_token',''))" 2>/dev/null)
if [ -z "$TOKEN" ]; then
  echo "FAIL: a fresh deployment could not log in with RAYRAG_ADMIN_PASSWORD" >&2
  tail -5 "$LOG" >&2
  exit 1
fi

KEY=RAYRAG_MAX_CONCURRENT_TASKS
RESPONSE=$(curl -s --max-time 20 -X POST "http://127.0.0.1:$PORT/api/v1/setup/complete" \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d "{\"updates\":[{\"key\":\"$KEY\",\"value\":\"7\"}],\"mark_completed\":false}")
echo "setup/complete: $(echo "$RESPONSE" | head -c 160)"
if ! grep -q "^$KEY=7$" "$DIR/.env"; then
  echo "FAIL: $KEY was not written to the env file" >&2
  exit 1
fi
echo "fresh login: OK"
echo "env write-back: OK ($KEY=7 written to the isolated env file)"
