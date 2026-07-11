#!/bin/bash
# RayRAG 镜像加速配置
# 用法: bash scripts/mirror.sh

set -e

MIRRORS=("ghproxy.com" "hub.fastgit.xyz" "gh.api.99988866.xyz")

echo "🔍 检测可用 GitHub 镜像..."
WORKING=""
for m in "${MIRRORS[@]}"; do
  if curl -s --max-time 3 "https://${m}/https://github.com" -o /dev/null -w "%{http_code}" | grep -q "200\|301\|302"; then
    WORKING="$m"
    echo "  ✅ ${m} 可用"
    break
  else
    echo "  ❌ ${m} 不可用"
  fi
done

if [ -z "$WORKING" ]; then
  echo "⚠️ 无可用镜像，使用 GitHub 原站（可能较慢）"
  git config --global --unset url."https://ghproxy.com/https://github.com/".insteadOf 2>/dev/null || true
else
  git config --global url."https://${WORKING}/https://github.com/".insteadOf "https://github.com/"
  echo "✅ Git mirror: ${WORKING}"
fi

# crates.io 镜像已通过 .cargo/config.toml 配置（ustc 源）
echo ""
echo "=== 当前配置 ==="
echo "crates.io → mirrors.ustc.edu.cn"
echo "git fetch  → $(git config --global url.'https://.*/https://github.com/'.insteadOf 2>/dev/null || echo 'direct')"
