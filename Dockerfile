# RayRAG image — works from mainland-China and global networks alike.
#
# Network selection:
#   RAYRAG_MIRROR_PROFILE=auto   probe a domestic mirror, fall back to upstream
#   RAYRAG_MIRROR_PROFILE=cn     Tsinghua apt mirror + rsproxy crates.io +
#                                ghfast.top GitHub proxy
#   RAYRAG_MIRROR_PROFILE=global deb.debian.org + crates.io + github.com
# Every individual URL stays overridable (DEBIAN_MIRROR, ZVEC_RELEASE_BASE, ...).
#
# The zvec Rust crates are vendored under vendor/zvec-rust (see VENDOR.md), so
# the image build links the prebuilt native library from a release mirror and
# never clones the upstream repository/submodules.

ARG RUST_IMAGE=rust:1.97.1-bookworm
ARG RUNTIME_IMAGE=debian:bookworm-slim
FROM ${RUST_IMAGE} AS builder

ARG RAYRAG_MIRROR_PROFILE=auto
ARG DEBIAN_MIRROR=
ARG DEBIAN_SECURITY_MIRROR=
COPY docker/mirror-setup.sh /usr/local/bin/rayrag-mirror-setup
RUN chmod 0755 /usr/local/bin/rayrag-mirror-setup \
 && rayrag-mirror-setup "${RAYRAG_MIRROR_PROFILE}" "${DEBIAN_MIRROR}" "${DEBIAN_SECURITY_MIRROR}" builder curl git

WORKDIR /src
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY vendor ./vendor

ARG RAYRAG_FEATURES=postgres-backend
ARG ZVEC_RUST_VERSION=0.7.1
ARG ZVEC_RELEASE_BASE=https://xget.xi-xu.me/gh/zvec-ai/zvec-rust/releases/download
ARG ZVEC_RELEASE_FALLBACK_BASE=https://github.com/zvec-ai/zvec-rust/releases/download
# Bounded zvec download: a stalled mirror used to hang the build forever
# because the fetch had no connect/overall timeout. Both attempts are capped and
# fall back to the other mirror; raise it for very slow links.
ARG ZVEC_DOWNLOAD_TIMEOUT=900
ARG TARGETARCH
# Supplied by docker compose from `git rev-parse --short HEAD`; the build
# context has no .git, so build.rs cannot discover it on its own.
ARG RAYRAG_BUILD_GIT_REV=
RUN set -eux; \
    # Point the git dependency at the vendored crates so the build performs no
    # GitHub fetch; the lock file keeps every other dependency pinned.
    printf '\n[patch."https://github.com/zvec-ai/zvec-rust"]\nzvec-rust = { path = "/src/vendor/zvec-rust/zvec" }\nzvec-rust-sys = { path = "/src/vendor/zvec-rust/zvec-sys" }\n' >> Cargo.toml; \
    mkdir -p /opt/zvec/lib; \
    if echo ",${RAYRAG_FEATURES}," | grep -q ',zvec-backend,'; then \
      profile="$(cat /etc/rayrag-mirror-profile)"; \
      case "${TARGETARCH:-amd64}" in \
        amd64) rust_target=x86_64-unknown-linux-gnu ;; \
        arm64) rust_target=aarch64-unknown-linux-gnu ;; \
        *) echo "Unsupported zvec Docker architecture: ${TARGETARCH}" >&2; exit 1 ;; \
      esac; \
      asset="v${ZVEC_RUST_VERSION}/zvec-prebuilt-${rust_target}.tar.gz"; \
      if [ "${profile}" = "cn" ]; then \
        primary_url="${ZVEC_RELEASE_BASE}/${asset}"; \
        fallback_url="${ZVEC_RELEASE_FALLBACK_BASE}/${asset}"; \
      else \
        primary_url="${ZVEC_RELEASE_FALLBACK_BASE}/${asset}"; \
        fallback_url="${ZVEC_RELEASE_BASE}/${asset}"; \
      fi; \
      if ! curl -fsSL --connect-timeout 20 --max-time "${ZVEC_DOWNLOAD_TIMEOUT}" \
           --retry 3 --retry-delay 2 --retry-all-errors \
           "$primary_url" -o /tmp/zvec.tar.gz; then \
        echo "Primary zvec mirror failed within ${ZVEC_DOWNLOAD_TIMEOUT}s; trying the configured fallback" >&2; \
        rm -f /tmp/zvec.tar.gz; \
        curl -fsSL --connect-timeout 20 --max-time "${ZVEC_DOWNLOAD_TIMEOUT}" \
          --retry 3 --retry-delay 2 --retry-all-errors \
          "$fallback_url" -o /tmp/zvec.tar.gz; \
      fi; \
      tar xzf /tmp/zvec.tar.gz -C /opt/zvec/lib; \
      rm /tmp/zvec.tar.gz; \
      test -f /opt/zvec/lib/libzvec_c_api.so; \
    fi; \
    ZVEC_LIB_DIR=/opt/zvec/lib LIBRARY_PATH=/opt/zvec/lib \
      cargo build --release --features "${RAYRAG_FEATURES}"

FROM ${RUNTIME_IMAGE}

ARG RAYRAG_MIRROR_PROFILE=auto
ARG DEBIAN_MIRROR=
ARG DEBIAN_SECURITY_MIRROR=
COPY docker/mirror-setup.sh /usr/local/bin/rayrag-mirror-setup
RUN chmod 0755 /usr/local/bin/rayrag-mirror-setup \
 && rayrag-mirror-setup "${RAYRAG_MIRROR_PROFILE}" "${DEBIAN_MIRROR}" "${DEBIAN_SECURITY_MIRROR}" base curl libssl3 zlib1g python3 nodejs

WORKDIR /app
COPY --from=builder /src/target/release/rayrag /usr/local/bin/rayrag
COPY --from=builder /opt/zvec/lib /opt/zvec/lib
COPY web/static /opt/rayrag-static
COPY docker-entrypoint.sh /usr/local/bin/rayrag-entrypoint
RUN chmod 0755 /usr/local/bin/rayrag-entrypoint \
    && mkdir -p /app/web

ENV RUST_LOG=info \
    LD_LIBRARY_PATH=/opt/zvec/lib
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=5s --start-period=20s --retries=5 \
  CMD curl -fsS http://127.0.0.1:8080/api/v1/system/healthz >/dev/null || exit 1
ENTRYPOINT ["rayrag-entrypoint"]
CMD ["serve", "--port", "8080"]
