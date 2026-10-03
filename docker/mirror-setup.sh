#!/bin/sh
# Configure apt (and, in the builder stage, cargo/git) for either a
# mainland-China or a global network. Both paths are first-class: the profile
# can be forced with RAYRAG_MIRROR_PROFILE=cn|global, or left on `auto` so the
# script probes a domestic mirror once and falls back to the upstream one.
#
# Usage:
#   rayrag-mirror-setup <profile> <debian-mirror> <debian-security-mirror> <role> [packages...]
#
#   profile                  auto | cn | global
#   debian-mirror            optional explicit override (empty = profile default)
#   debian-security-mirror   optional explicit override (empty = profile default)
#   role                     base | builder
#   packages                 extra apt packages to install
#
# The resolved profile is written to /etc/rayrag-mirror-profile so later build
# steps (e.g. the zvec prebuilt download) can pick the same side of the network.
set -eu

PROFILE="${1:-auto}"
DEBIAN_MIRROR="${2:-}"
DEBIAN_SECURITY_MIRROR="${3:-}"
ROLE="${4:-base}"
shift 4 2>/dev/null || shift $#
EXTRA_PACKAGES="$*"

probe() {
    curl -fsS --max-time 6 -o /dev/null "$1" 2>/dev/null
}

if [ "$PROFILE" = "auto" ]; then
    # The base image has no ca-certificates yet, so probe the http endpoint.
    if probe "http://mirrors.tuna.tsinghua.edu.cn/debian/dists/bookworm/Release"; then
        PROFILE=cn
    else
        PROFILE=global
    fi
fi

case "$PROFILE" in
    cn)
        [ -n "$DEBIAN_MIRROR" ] || DEBIAN_MIRROR="https://mirrors.tuna.tsinghua.edu.cn/debian"
        [ -n "$DEBIAN_SECURITY_MIRROR" ] || DEBIAN_SECURITY_MIRROR="https://mirrors.tuna.tsinghua.edu.cn/debian-security"
        ;;
    global)
        [ -n "$DEBIAN_MIRROR" ] || DEBIAN_MIRROR="http://deb.debian.org/debian"
        [ -n "$DEBIAN_SECURITY_MIRROR" ] || DEBIAN_SECURITY_MIRROR="http://deb.debian.org/debian-security"
        ;;
    *)
        echo "Unknown RAYRAG_MIRROR_PROFILE: $PROFILE (expected auto|cn|global)" >&2
        exit 1
        ;;
esac

http_form() {
    case "$1" in
        https://*) printf 'http://%s' "${1#https://}" ;;
        *) printf '%s' "$1" ;;
    esac
}

set_sources() {
    sed -i \
        -e "s|http://deb.debian.org/debian-security|$2|g" \
        -e "s|http://deb.debian.org/debian|$1|g" \
        /etc/apt/sources.list.d/debian.sources
}

# Phase 1 — bootstrap over the plain-http form of the mirror (no
# ca-certificates in the base image yet), install the TLS trust store.
set_sources "$(http_form "$DEBIAN_MIRROR")" "$(http_form "$DEBIAN_SECURITY_MIRROR")"
apt-get update
apt-get install -y --no-install-recommends ca-certificates

# Phase 2 — switch to the final (possibly https) mirror and install the rest.
set_sources "$DEBIAN_MIRROR" "$DEBIAN_SECURITY_MIRROR"
apt-get update
if [ -n "$EXTRA_PACKAGES" ]; then
    # shellcheck disable=SC2086
    apt-get install -y --no-install-recommends $EXTRA_PACKAGES
fi
rm -rf /var/lib/apt/lists/*

if [ "$ROLE" = "builder" ]; then
    mkdir -p /usr/local/cargo
    if [ "$PROFILE" = "cn" ]; then
        # Container-side crates.io mirror; the host ~/.cargo/config.toml is not
        # part of the build context. GitHub git dependencies are rewritten to a
        # domestic proxy because the vendored zvec tree only covers zvec itself.
        cat > /usr/local/cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "rsproxy-sparse"

[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"

[registries.rsproxy]
index = "https://rsproxy.cn/crates.io-index"

[net]
git-fetch-with-cli = true
EOF
        git config --global url."https://ghfast.top/https://github.com/".insteadOf "https://github.com/"
    else
        # Global networks talk to crates.io and GitHub directly.
        cat > /usr/local/cargo/config.toml <<'EOF'
[net]
git-fetch-with-cli = true
EOF
        git config --global --remove-section 'url.https://ghfast.top/https://github.com/' 2>/dev/null || true
    fi
fi

printf '%s\n' "$PROFILE" > /etc/rayrag-mirror-profile
echo "rayrag mirror profile: $PROFILE (debian: $DEBIAN_MIRROR)"
