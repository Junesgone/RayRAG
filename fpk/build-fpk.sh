#!/usr/bin/env bash
# Build the RayRAG 自托管主机 package (.fpk).
#
# Usage: fpk/build-fpk.sh [--skip-build]
#
# Environment:
#   RAYRAG_FPK_OUT          output directory (default: dist)
#   RAYRAG_FPK_STAGE        staging directory (default: build/fpk-stage)
#   RAYRAG_FPK_CACHE        cache directory (default: build/fpk-cache)
#   RAYRAG_FPK_BINARY       prebuilt rayrag binary (default: target/release/rayrag)
#   RAYRAG_ZVEC_LIB         libzvec_c_api.so path
#                           (default: $HOME/.local/lib/zvec/0.8.0/libzvec_c_api.so)
#   RAYRAG_FPK_PG_ARCHIVE   pre-downloaded portable PostgreSQL archive (.txz)
#   RAYRAG_FPK_PG_URL       portable PostgreSQL archive URL
#   RAYRAG_FPK_PG_VERSION   portable PostgreSQL version (default: 18.4.0)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
OUT_DIR="${RAYRAG_FPK_OUT:-$ROOT/dist}"
STAGE_DIR="${RAYRAG_FPK_STAGE:-$ROOT/build/fpk-stage}"
CACHE_DIR="${RAYRAG_FPK_CACHE:-$ROOT/build/fpk-cache}"
BINARY="${RAYRAG_FPK_BINARY:-$ROOT/target/release/rayrag}"
ZVEC_LIB="${RAYRAG_ZVEC_LIB:-$HOME/.local/lib/zvec/0.8.0/libzvec_c_api.so}"
PG_VERSION="${RAYRAG_FPK_PG_VERSION:-18.4.0}"
PG_URL="${RAYRAG_FPK_PG_URL:-https://maven.aliyun.com/repository/public/io/zonky/test/postgres/embedded-postgres-binaries-linux-amd64/$PG_VERSION/embedded-postgres-binaries-linux-amd64-$PG_VERSION.jar}"
PG_ARCHIVE="${RAYRAG_FPK_PG_ARCHIVE:-$CACHE_DIR/postgres-linux-x86_64.txz}"
PG_DIR="$CACHE_DIR/pgsql"

SKIP_BUILD="${1:-}"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
if [ -z "$VERSION" ]; then
  echo "build-fpk: could not read the version from Cargo.toml" >&2
  exit 1
fi

if ! command -v fnpack >/dev/null 2>&1; then
  echo "build-fpk: fnpack is required (https://developer.fnnas.com/docs/cli/fnpack/)" >&2
  exit 1
fi

echo "== RayRAG 自托管主机 package $VERSION"

if [ "$SKIP_BUILD" != "--skip-build" ]; then
  echo "== cargo build --release"
  cargo build --release --locked --features postgres-backend,zvec-backend
fi
if [ ! -x "$BINARY" ]; then
  echo "build-fpk: rayrag binary not found at $BINARY" >&2
  exit 1
fi
if [ ! -f "$ZVEC_LIB" ]; then
  echo "build-fpk: libzvec_c_api.so not found at $ZVEC_LIB" >&2
  exit 1
fi

mkdir -p "$CACHE_DIR"
if [ ! -d "$PG_DIR/bin" ]; then
  if [ ! -f "$PG_ARCHIVE" ]; then
    echo "== downloading portable PostgreSQL $PG_VERSION (China mirror)"
    tmpjar="$CACHE_DIR/embedded-postgres-binaries.jar"
    curl -fL --retry 3 -m 600 -o "$tmpjar" "$PG_URL"
    unzip -o -q "$tmpjar" postgres-linux-x86_64.txz -d "$CACHE_DIR"
    rm -f "$tmpjar"
    if [ ! -f "$CACHE_DIR/postgres-linux-x86_64.txz" ]; then
      echo "build-fpk: archive did not contain postgres-linux-x86_64.txz" >&2
      exit 1
    fi
  fi
  mkdir -p "$PG_DIR"
  tar -xJf "$PG_ARCHIVE" -C "$PG_DIR"
fi
if [ ! -x "$PG_DIR/bin/initdb" ]; then
  echo "build-fpk: portable PostgreSQL is incomplete under $PG_DIR" >&2
  exit 1
fi

echo "== staging package"
rm -rf "$STAGE_DIR/rayrag"
mkdir -p "$STAGE_DIR/rayrag/app/runtime/lib"
STAGE="$STAGE_DIR/rayrag"
cp -a fpk/manifest fpk/LICENSE NOTICE fpk/ICON.PNG fpk/ICON_256.PNG "$STAGE/"
cp -a fpk/config fpk/cmd fpk/wizard "$STAGE/"
mkdir -p "$STAGE/app/ui"
cp -a fpk/app/ui/config fpk/app/ui/images "$STAGE/app/ui/"
sed -i "s/^version               = .*/version               = $VERSION/" "$STAGE/manifest"
cp -a "$BINARY" "$STAGE/app/runtime/rayrag"
strip --strip-unneeded "$STAGE/app/runtime/rayrag" 2>/dev/null || true
chmod 755 "$STAGE/app/runtime/rayrag"
cp -a web/static "$STAGE/app/runtime/static"
cp -a "$ZVEC_LIB" "$STAGE/app/runtime/lib/libzvec_c_api.so"
cp -a "$PG_DIR" "$STAGE/app/runtime/pgsql"
find "$STAGE/cmd" -type f -exec chmod 755 {} +
chmod 644 "$STAGE/manifest" "$STAGE/NOTICE" "$STAGE/config/privilege" "$STAGE/config/resource" "$STAGE/wizard/install" "$STAGE/wizard/config" "$STAGE/app/ui/config" "$STAGE/LICENSE"
chmod 644 "$STAGE"/ICON*.PNG "$STAGE"/app/ui/images/*.png

echo "== sanity check: bundled PostgreSQL and RayRAG"
PGTEST="$(mktemp -d)"
trap 'rm -rf "$PGTEST"' EXIT
(umask 077; printf 'admin\n' >"$PGTEST/pw")
LD_LIBRARY_PATH="$STAGE/app/runtime/pgsql/lib" "$STAGE/app/runtime/pgsql/bin/initdb" \
  -D "$PGTEST/data" -U admin --pwfile="$PGTEST/pw" --encoding=UTF8 --locale=C >"$PGTEST/initdb.log" 2>&1
LD_LIBRARY_PATH="$STAGE/app/runtime/lib:$STAGE/app/runtime/pgsql/lib" "$STAGE/app/runtime/rayrag" --version
rm -rf "$PGTEST"

echo "== fnpack build"
(cd "$STAGE" && fnpack build)
mkdir -p "$OUT_DIR"
OUT="$OUT_DIR/rayrag_${VERSION}_x86_64.fpk"
mv "$STAGE/rayrag.fpk" "$OUT"
echo "== built $OUT"
ls -lh "$OUT"
sha256sum "$OUT"
