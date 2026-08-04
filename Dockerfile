ARG RUST_IMAGE=rust:1.97.1-bookworm
ARG RUNTIME_IMAGE=debian:bookworm-slim
FROM ${RUST_IMAGE} AS builder

ARG DEBIAN_MIRROR=https://mirrors.tuna.tsinghua.edu.cn/debian
ARG DEBIAN_SECURITY_MIRROR=https://mirrors.tuna.tsinghua.edu.cn/debian-security
RUN set -eux; \
    case "${DEBIAN_MIRROR}" in \
      https://*) bootstrap_mirror="http://${DEBIAN_MIRROR#https://}" ;; \
      *) bootstrap_mirror="${DEBIAN_MIRROR}" ;; \
    esac; \
    case "${DEBIAN_SECURITY_MIRROR}" in \
      https://*) bootstrap_security_mirror="http://${DEBIAN_SECURITY_MIRROR#https://}" ;; \
      *) bootstrap_security_mirror="${DEBIAN_SECURITY_MIRROR}" ;; \
    esac; \
    sed -i \
      -e "s|http://deb.debian.org/debian-security|${bootstrap_security_mirror}|g" \
      -e "s|http://deb.debian.org/debian|${bootstrap_mirror}|g" \
      /etc/apt/sources.list.d/debian.sources; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates; \
    sed -i \
      -e "s|${bootstrap_security_mirror}|${DEBIAN_SECURITY_MIRROR}|g" \
      -e "s|${bootstrap_mirror}|${DEBIAN_MIRROR}|g" \
      /etc/apt/sources.list.d/debian.sources; \
    apt-get update; \
    apt-get install -y --no-install-recommends curl; \
    rm -rf /var/lib/apt/lists/*

WORKDIR /src
# 容器内 cargo 国内镜像源（宿主 ~/.cargo/config.toml 不会进入构建上下文）
# git 依赖（如 huggingface/candle）直连 GitHub 会卡死 → 走 xget 加速镜像
RUN mkdir -p /usr/local/cargo && printf '%s\n' \
    '[source.crates-io]' \
    'replace-with = "rsproxy-sparse"' \
    '[source.rsproxy-sparse]' \
    'registry = "sparse+https://rsproxy.cn/index/"' \
    '[registries.rsproxy]' \
    'index = "https://rsproxy.cn/crates.io-index"' \
    '[net]' \
    'git-fetch-with-cli = true' > /usr/local/cargo/config.toml \
 && git config --global url."https://ghfast.top/https://github.com/".insteadOf "https://github.com/"
COPY Cargo.toml Cargo.lock ./
COPY src ./src

ARG RAYRAG_FEATURES=postgres-backend
ARG ZVEC_RUST_VERSION=0.5.1
ARG ZVEC_RELEASE_BASE=https://xget.xi-xu.me/gh/zvec-ai/zvec-rust/releases/download
ARG ZVEC_RELEASE_FALLBACK_BASE=https://github.com/zvec-ai/zvec-rust/releases/download
ARG TARGETARCH
RUN set -eux; \
    mkdir -p /opt/zvec/lib; \
    if echo ",${RAYRAG_FEATURES}," | grep -q ',zvec-backend,'; then \
      case "${TARGETARCH:-amd64}" in \
        amd64) rust_target=x86_64-unknown-linux-gnu ;; \
        arm64) rust_target=aarch64-unknown-linux-gnu ;; \
        *) echo "Unsupported zvec Docker architecture: ${TARGETARCH}" >&2; exit 1 ;; \
      esac; \
      asset="v${ZVEC_RUST_VERSION}/zvec-prebuilt-${rust_target}.tar.gz"; \
      primary_url="${ZVEC_RELEASE_BASE}/${asset}"; \
      fallback_url="${ZVEC_RELEASE_FALLBACK_BASE}/${asset}"; \
      if ! curl -fsSL --retry 5 --retry-delay 2 "$primary_url" -o /tmp/zvec.tar.gz; then \
        echo "Primary zvec mirror failed; retrying the configured fallback" >&2; \
        curl -fsSL --retry 5 --retry-delay 2 "$fallback_url" -o /tmp/zvec.tar.gz; \
      fi; \
      tar xzf /tmp/zvec.tar.gz -C /opt/zvec/lib; \
      rm /tmp/zvec.tar.gz; \
      test -f /opt/zvec/lib/libzvec_c_api.so; \
    fi; \
    ZVEC_LIB_DIR=/opt/zvec/lib LIBRARY_PATH=/opt/zvec/lib \
      cargo build --release --locked --features "${RAYRAG_FEATURES}"

FROM ${RUNTIME_IMAGE}

ARG DEBIAN_MIRROR=https://mirrors.tuna.tsinghua.edu.cn/debian
ARG DEBIAN_SECURITY_MIRROR=https://mirrors.tuna.tsinghua.edu.cn/debian-security
RUN set -eux; \
    case "${DEBIAN_MIRROR}" in \
      https://*) bootstrap_mirror="http://${DEBIAN_MIRROR#https://}" ;; \
      *) bootstrap_mirror="${DEBIAN_MIRROR}" ;; \
    esac; \
    case "${DEBIAN_SECURITY_MIRROR}" in \
      https://*) bootstrap_security_mirror="http://${DEBIAN_SECURITY_MIRROR#https://}" ;; \
      *) bootstrap_security_mirror="${DEBIAN_SECURITY_MIRROR}" ;; \
    esac; \
    sed -i \
      -e "s|http://deb.debian.org/debian-security|${bootstrap_security_mirror}|g" \
      -e "s|http://deb.debian.org/debian|${bootstrap_mirror}|g" \
      /etc/apt/sources.list.d/debian.sources; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates; \
    sed -i \
      -e "s|${bootstrap_security_mirror}|${DEBIAN_SECURITY_MIRROR}|g" \
      -e "s|${bootstrap_mirror}|${DEBIAN_MIRROR}|g" \
      /etc/apt/sources.list.d/debian.sources; \
    apt-get update; \
    apt-get install -y --no-install-recommends curl libssl3 zlib1g; \
    rm -rf /var/lib/apt/lists/*

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
