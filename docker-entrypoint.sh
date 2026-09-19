#!/bin/sh
set -eu

if [ ! -f /app/web/static/index.html ]; then
  mkdir -p /app/web/static
  cp -a /opt/rayrag-static/. /app/web/static/
fi

exec rayrag "$@"
