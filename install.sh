#!/usr/bin/env bash
#
# RayRAG one-command deployment (written for first-time users).
#
#   ./install.sh                  # generate passwords -> build -> start -> print URL
#   ./install.sh --port 8080      # web port (default 9380)
#   ./install.sh --no-build       # start without rebuilding the image
#   ./install.sh --global-mirror  # build against upstream mirrors (default: CN mirrors)
#   ./install.sh --cn-mirror      # force the mainland-China mirror profile
#   ./install.sh --dry-run        # checks and .env only, no build, no start
#   ./install.sh --help
#
# Idempotent: re-running never overwrites an existing .env, so your passwords
# and settings survive. Messages follow the system locale (zh* -> Chinese).
set -euo pipefail

cd "$(dirname "$0")"
ROOT="$PWD"

# Every long-running command shares one timeout (seconds): 2 hours by default,
# capped at 2 hours, settable through RAYRAG_CMD_TIMEOUT (see .env.example).
TIMEOUT="${RAYRAG_CMD_TIMEOUT:-7200}"
case "$TIMEOUT" in
    ''|*[!0-9]*) TIMEOUT=7200 ;;
esac
[ "$TIMEOUT" -gt 7200 ] && TIMEOUT=7200
[ "$TIMEOUT" -lt 60 ] && TIMEOUT=60

PORT=""
BUILD=1
MIRROR=""
DRY_RUN=0
ASSUME_YES=0

case "${LC_ALL:-${LC_CTYPE:-${LANG:-}}}" in
    zh*|*_CN*|*_cn*) LANG_ZH=1 ;;
    *) LANG_ZH=0 ;;
esac

# m "<中文>" "<English>"
m() {
    if [ "$LANG_ZH" -eq 1 ]; then printf '%s' "$1"; else printf '%s' "$2"; fi
}
blue()  { printf '\033[1;34m%s\033[0m\n' "$*"; }
green() { printf '\033[1;32m%s\033[0m\n' "$*"; }
warn()  { printf '\033[1;33m%s\033[0m\n' "$*" >&2; }
die()   { printf '\033[1;31m%s\033[0m\n' "$(m "错误：$1" "Error: $1")" >&2; exit 1; }

usage() {
    awk 'NR>1 && /^#/ {sub(/^# ?/, ""); print; next} NR>1 {exit}' "$0"
    exit 0
}

while [ $# -gt 0 ]; do
    case "$1" in
        --port) PORT="${2:-}"; shift 2 ;;
        --port=*) PORT="${1#*=}"; shift ;;
        --no-build) BUILD=0; shift ;;
        --build) BUILD=1; shift ;;
        --cn-mirror) MIRROR="cn"; shift ;;
        --global-mirror|--mirror-global) MIRROR="global"; shift ;;
        --dry-run) DRY_RUN=1; shift ;;
        -y|--yes) ASSUME_YES=1; shift ;;
        -h|--help) usage ;;
        *) die "$(m "无法识别的参数 $1（用 --help 查看用法）" "unknown argument $1 (see --help)")" ;;
    esac
done

if [ -n "$PORT" ]; then
    case "$PORT" in
        ''|*[!0-9]*) die "$(m "--port 需要一个数字，例如 --port 8080" "--port needs a number, e.g. --port 8080")" ;;
    esac
    if [ "$PORT" -lt 1 ] || [ "$PORT" -gt 65535 ]; then
        die "$(m "--port 必须在 1-65535 之间" "--port must be between 1 and 65535")"
    fi
fi

# ---------------------------------------------------------------- environment
blue "$(m "==> 检查 Docker 环境" "==> Checking the Docker environment")"

command -v docker >/dev/null 2>&1 || die "$(m "没有找到 docker，请先安装：
    Linux  : curl -fsSL https://get.docker.com | sh
    Win/Mac: 安装 Docker Desktop 后重启终端" "docker not found. Install it first:
    Linux  : curl -fsSL https://get.docker.com | sh
    Win/Mac: install Docker Desktop, then restart your terminal")"

if ! docker info >/dev/null 2>&1; then
    die "$(m "Docker 已安装但当前用户连不上守护进程。
    如果是 Linux：sudo usermod -aG docker \$USER 后重新登录。" "Docker is installed but this user cannot reach the daemon.
    On Linux: sudo usermod -aG docker \$USER, then log out and back in.")"
fi

if docker compose version >/dev/null 2>&1; then
    COMPOSE=(docker compose)
elif command -v docker-compose >/dev/null 2>&1; then
    COMPOSE=(docker-compose)
else
    die "$(m "没有找到 docker compose，请安装 Docker Compose v2 插件。" "docker compose not found. Install the Docker Compose v2 plugin.")"
fi
green "    $(m "Docker 正常" "Docker OK"): $(${COMPOSE[*]} version --short 2>/dev/null || echo ok)"

# ---------------------------------------------------------------- .env
random_secret() {
    # 24 chars of [A-Za-z0-9]. Deliberately avoids `tr </dev/urandom | head`:
    # head exiting early sends SIGPIPE to tr, which `set -o pipefail` reports as
    # a failure of the whole pipeline.
    local value=""
    while [ "${#value}" -lt 24 ]; do
        value="${value}$(LC_ALL=C dd if=/dev/urandom bs=64 count=1 2>/dev/null |
            LC_ALL=C tr -dc 'A-Za-z0-9' || true)"
    done
    printf '%s' "${value:0:24}"
}

set_env() {
    # set_env KEY VALUE -- replace in place, or append when the key is absent
    local key="$1" value="$2"
    if grep -qE "^${key}=" .env; then
        python3 - "$key" "$value" <<'PY'
import sys
key, value = sys.argv[1], sys.argv[2]
path = ".env"
with open(path, encoding="utf-8") as handle:
    lines = handle.read().splitlines()
out = []
for line in lines:
    if line.startswith(key + "="):
        out.append(f"{key}={value}")
    else:
        out.append(line)
with open(path, "w", encoding="utf-8") as handle:
    handle.write("\n".join(out) + "\n")
PY
    else
        printf '%s=%s\n' "$key" "$value" >>.env
    fi
}

blue "$(m "==> 准备配置文件 .env" "==> Preparing .env")"
NEW_PASSWORDS=0
# The compose file bind-mounts `.env` into the container so the first-login setup page
# writes the file the operator edits; the file must therefore exist before `up`.
if [ -f .env ]; then
    green "    $(m "已有 .env，保留现有密码与配置" "existing .env kept (passwords and settings preserved)")"
else
    [ -f .env.example ] || die "$(m "找不到 .env.example，请在 RayRAG 项目根目录执行本脚本" "no .env.example here; run this script from the RayRAG project root")"
    cp .env.example .env
    PG_PASSWORD="$(random_secret)"
    ADMIN_PASSWORD="$(random_secret)"
    set_env RAYRAG_POSTGRES_PASSWORD "$PG_PASSWORD"
    set_env RAYRAG_ADMIN_PASSWORD "$ADMIN_PASSWORD"
    set_env RAYRAG_POSTGRES_URL "postgresql://rayrag:${PG_PASSWORD}@127.0.0.1:5432/rayrag"
    NEW_PASSWORDS=1
    green "    $(m "已生成 .env，并随机生成两个密码（各 24 位）" "created .env with two randomly generated 24-character passwords")"
fi

if [ -n "$PORT" ]; then
    set_env RAYRAG_PORT "$PORT"
    green "    $(m "网页端口 -> $PORT" "web port -> $PORT")"
fi

ADMIN_EMAIL="$(grep -E '^RAYRAG_ADMIN_EMAIL=' .env | head -1 | cut -d= -f2- || true)"
ADMIN_EMAIL="${ADMIN_EMAIL:-admin@rayrag.local}"
ADMIN_PASSWORD_VALUE="$(grep -E '^RAYRAG_ADMIN_PASSWORD=' .env | head -1 | cut -d= -f2- || true)"
LISTEN_PORT="$(grep -E '^RAYRAG_PORT=' .env | head -1 | cut -d= -f2- || true)"
LISTEN_PORT="${LISTEN_PORT:-9380}"

if [ "${#ADMIN_PASSWORD_VALUE}" -lt 12 ]; then
    die "$(m ".env 里的 RAYRAG_ADMIN_PASSWORD 少于 12 位，RayRAG 会拒绝启动。请改成至少 12 位后重试。" "RAYRAG_ADMIN_PASSWORD in .env is shorter than 12 characters and RayRAG will refuse to start. Use at least 12 and retry.")"
fi

if [ -n "$MIRROR" ]; then
    set_env RAYRAG_MIRROR_PROFILE "$MIRROR"
    green "    $(m "构建镜像档位 -> $MIRROR" "build mirror profile -> $MIRROR")"
fi

if command -v ss >/dev/null 2>&1 && ss -ltn 2>/dev/null | grep -q ":${LISTEN_PORT} "; then
    if ! docker ps --format '{{.Ports}}' 2>/dev/null | grep -q ":${LISTEN_PORT}->"; then
        warn "    $(m "提示：本机 $LISTEN_PORT 端口已被其它程序占用，可用 ./install.sh --port 8080 换端口" "note: local port $LISTEN_PORT is already taken; use ./install.sh --port 8080 to change it")"
    fi
fi

# ---------------------------------------------------------------- build & start
blue "$(m "==> 校验 docker compose 配置" "==> Validating the docker compose configuration")"
timeout "$TIMEOUT" "${COMPOSE[@]}" config >/dev/null || die "$(m "docker compose 配置校验失败（上方有具体原因）" "docker compose configuration is invalid (reason above)")"
green "    $(m "配置有效" "configuration is valid")"

if [ "$DRY_RUN" -eq 1 ]; then
    echo
    green "$(m "==> --dry-run：检查全部通过，未构建也未启动。" "==> --dry-run: all checks passed, nothing was built or started.")"
    echo  "    $(m "去掉 --dry-run 即可正式部署。当前配置：" "Drop --dry-run to deploy. Current settings:")"
    echo  "      $(m "网页端口" "web port") : ${LISTEN_PORT}"
    echo  "      $(m "登录邮箱" "login")    : ${ADMIN_EMAIL}"
    echo  "      $(m "构建镜像" "build")    : $(if [ "$BUILD" -eq 1 ]; then m "是" "yes"; else m "否" "no"; fi)"
    exit 0
fi

if [ "$BUILD" -eq 1 ]; then
    blue "$(m "==> 构建镜像并启动（首次 5~15 分钟，取决于网速；超过 ${TIMEOUT} 秒会中断）" "==> Building and starting (5-15 minutes on the first run; aborts after ${TIMEOUT}s)")"
    timeout "$TIMEOUT" "${COMPOSE[@]}" up -d --build || die "$(m "构建或启动失败。
    国内网络慢可以试：RAYRAG_MIRROR_PROFILE=cn docker compose build rayrag
    需要完整日志：docker compose build --progress plain rayrag" "build or start failed.
    Slow network? Try: RAYRAG_MIRROR_PROFILE=cn docker compose build rayrag
    Full log: docker compose build --progress plain rayrag")"
else
    blue "$(m "==> 启动服务（跳过构建）" "==> Starting services (build skipped)")"
    timeout "$TIMEOUT" "${COMPOSE[@]}" up -d || die "$(m "启动失败，看日志：docker compose logs --tail=100" "start failed; see: docker compose logs --tail=100")"
fi

# ---------------------------------------------------------------- health
blue "$(m "==> 等待服务就绪（最多 3 分钟）" "==> Waiting for the service to become healthy (up to 3 minutes)")"
HEALTH_URL="http://127.0.0.1:${LISTEN_PORT}/api/v1/system/healthz"
READY=0
for _ in $(seq 1 90); do
    if curl -fsS --max-time 5 "$HEALTH_URL" 2>/dev/null | grep -q '"status":"healthy"'; then
        READY=1
        break
    fi
    sleep 2
done

LAN_IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
[ -n "$LAN_IP" ] || LAN_IP="127.0.0.1"

echo
if [ "$READY" -eq 1 ]; then
    green "============================================================"
    green "  $(m "RayRAG 已经跑起来了 🎉" "RayRAG is up and running 🎉")"
    green "------------------------------------------------------------"
    echo  "  $(m "网页地址" "Web UI")   : http://${LAN_IP}:${LISTEN_PORT}"
    echo  "  $(m "本机访问" "Local")    : http://127.0.0.1:${LISTEN_PORT}"
    echo  "  $(m "登录邮箱" "Login")    : ${ADMIN_EMAIL}"
    if [ "$NEW_PASSWORDS" -eq 1 ]; then
        echo  "  $(m "登录密码" "Password") : ${ADMIN_PASSWORD_VALUE}   $(m "（已写入 .env，请妥善保存）" "(saved in .env, keep it safe)")"
    else
        echo  "  $(m "登录密码" "Password") : $(m "见 .env 文件里的 RAYRAG_ADMIN_PASSWORD" "see RAYRAG_ADMIN_PASSWORD in .env")"
    fi
    green "------------------------------------------------------------"
    echo  "  $(m "查看日志" "Logs")    : ${COMPOSE[*]} logs -f rayrag"
    echo  "  $(m "停止服务" "Stop")    : ${COMPOSE[*]} down"
    echo  "  $(m "重启服务" "Restart") : ${COMPOSE[*]} restart"
    echo  "  $(m "下一步" "Next")      : $(m "浏览器登录后，右上角头像 -> Model providers 接入大模型" "log in, then avatar (top right) -> Model providers to add a model")"
    green "============================================================"
else
    warn "$(m "服务还没通过健康检查（首次启动可能在建库）。" "The service has not passed its health check yet (first start may still be initialising the database).")"
    warn "$(m "请稍等 1~2 分钟后访问： http://${LAN_IP}:${LISTEN_PORT}" "Wait a minute or two, then open: http://${LAN_IP}:${LISTEN_PORT}")"
    warn "$(m "如果一直不通： ${COMPOSE[*]} logs --tail=200 rayrag" "Still failing? ${COMPOSE[*]} logs --tail=200 rayrag")"
    exit 1
fi
