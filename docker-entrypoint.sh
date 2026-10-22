#!/bin/sh
set -eu

# The image's static assets (`/opt/rayrag-static`) are the source of truth: the
# state volume keeps them under `/app/web/static` so the app can serve them next
# to its state files, but a volume outlives an upgrade. Seed it on first boot and
# refresh it on every boot, so a new release's assets (pdf.js, icons, the shell)
# actually reach the running container instead of the ones the first boot copied.
mkdir -p /app/web/static
cp -a /opt/rayrag-static/. /app/web/static/

exec rayrag "$@"