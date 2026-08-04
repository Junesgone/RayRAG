#!/usr/bin/env bash
# RayRAG 运维监控脚本
# 检查：容器健康 / GPU 四链路 / SearXNG / 页面可用性
# 用法: ./scripts/health-check.sh [--json]
set -uo pipefail

BASE="http://127.0.0.1:9390"
SEARXNG="http://127.0.0.1:18888"
JSON_MODE="${1:-}"

check() {
  local name="$1" url="$2" expect="$3"
  local code
  code=$(curl -s -o /dev/null -w "%{http_code}" -m 10 "$url" 2>/dev/null)
  if [ "$code" = "$expect" ]; then
    echo "OK   $name ($code)"
  else
    echo "FAIL $name (got $code, want $expect)"
  fi
}

check_container() {
  local name="$1"
  local status
  status=$(docker ps --filter "name=$name" --format "{{.Status}}" 2>/dev/null)
  if [ -n "$status" ]; then
    echo "OK   container:$name ($status)"
  else
    echo "FAIL container:$name (not running)"
  fi
}

echo "== RayRAG 运维检查 $(date '+%Y-%m-%d %H:%M') =="
check_container rayrag-rayrag-1
check_container rayrag-postgres-1
check_container rayrag-searxng

echo "-- 健康探针 --"
check "healthz" "$BASE/api/v1/system/healthz" 200

echo "-- 页面 --"
for page in dashboard kbs search chat agents memories files skills providers settings admin status api-docs login; do
  check "page:$page" "$BASE/$page" 200
done

echo "-- GPU 四链路 --"
check "embedding 8888" "http://127.0.0.1:8888/v1/models" 200
check "LLM 8088" "http://127.0.0.1:8088/v1/models" 200
code=$(curl -s -o /dev/null -w "%{http_code}" -m 10 -H "Authorization: Bearer mxbai-rerank-key" -X POST -d '{"query":"t","documents":["t"]}' "http://127.0.0.1:8081/rerank" 2>/dev/null)
if [ "$code" = "200" ] || [ "$code" = "400" ]; then echo "OK   rerank 8081 ($code)"; else echo "FAIL rerank 8081 (got $code)"; fi
check "SearXNG 18888" "$SEARXNG/" 200

echo "-- 完成 --"
