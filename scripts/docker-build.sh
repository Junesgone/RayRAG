#!/usr/bin/env bash
# Thin wrapper around `docker compose` that stamps the image with the current
# git revision. The build context has no `.git` directory, so `build.rs` cannot
# discover the revision itself; without this the banner would read
# `rev unknown`.
#
# Usage: scripts/docker-build.sh build rayrag
#        scripts/docker-build.sh up -d
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -z "${RAYRAG_BUILD_GIT_REV:-}" ]; then
    RAYRAG_BUILD_GIT_REV="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
    export RAYRAG_BUILD_GIT_REV
fi

exec docker compose "$@"
