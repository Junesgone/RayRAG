#!/usr/bin/env bash
# One command for the round verification battery (internal; not part of the published snapshot).
#
#   ./scripts/verify-all.sh
#
# Requires the deployed instance to be reachable and the fnOS Chrome CDP endpoint open.
# Exits non-zero as soon as one check fails, so a round cannot be reported on a red suite.
set -uo pipefail
cd "$(dirname "$0")/.."
: "${RAYRAG_CDP_ORIGIN:=http://127.0.0.1:9390}"
: "${RAYRAG_CDP_ENDPOINT:=http://127.0.0.1:16002}"
: "${RAYRAG_ORIGIN:=$RAYRAG_CDP_ORIGIN}"
: "${RAYRAG_LOGIN_PASSWORD:?set RAYRAG_LOGIN_PASSWORD}"
export RAYRAG_CDP_ORIGIN RAYRAG_CDP_ENDPOINT RAYRAG_ORIGIN RAYRAG_LOGIN_PASSWORD

fail=0
run() { # name, command...
  local name="$1"; shift
  echo "── $name"
  if "$@"; then echo "   OK"; else echo "   FAILED"; fail=1; fi
}

run "API parity ledger" python3 scripts/api-parity-audit.py --check
run "HTML tag balance" python3 scripts/ui-traversal/tag-balance-check.py
run "Sidebar routes (CDP)" node scripts/ui-traversal/sidebar-routes-cdp.mjs
run "Layout audit (CDP)" node scripts/ui-traversal/layout-audit-cdp.mjs

if [ "$fail" -eq 0 ]; then echo "verify-all: ALL CHECKS PASSED"; else echo "verify-all: FAILURES PRESENT"; fi
exit "$fail"
