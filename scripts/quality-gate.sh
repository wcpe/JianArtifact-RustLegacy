#!/usr/bin/env bash
# JianArtifact 统一质量门脚本
# 仅在 Docker 容器内执行；CI 与本地共用本脚本，保证两端行为完全一致。
set -euo pipefail

# 切到仓库根目录（脚本所在目录的上一级），保证相对路径稳定
cd "$(dirname "$0")/.."

echo "==> [1/5] Rust 格式检查"
cargo fmt --all -- --check

echo "==> [2/5] Rust 静态分析（警告视为错误）"
cargo clippy --workspace --all-targets --all-features -- -D warnings

echo "==> [3/5] Rust 测试"
cargo test --workspace

echo "==> [4/5] 部署脚本逻辑测试"
bash scripts/deploy/test-remote-install.sh

echo "==> [5/5] 前端质量门"
# 前端子工程清单（apps/server 是 Rust crate，不参与前端门禁判定）
frontend_src=""
for dir in apps/web apps/wiki packages/ui packages/devmock packages/eslint-config packages/typescript-config; do
    if [ -e "$dir/tsconfig.json" ] || [ -d "$dir/src" ]; then
        frontend_src=1
    fi
done
if [ -f pnpm-lock.yaml ]; then
    pnpm install --frozen-lockfile
    pnpm run format:check
    pnpm turbo typecheck lint test build
elif [ -n "$frontend_src" ]; then
    # 防静默漏检：已有前端源码却缺锁文件时必须失败，不得绿灯放行
    echo "错误：检测到前端源码但缺少 pnpm-lock.yaml，前端门禁无法执行" >&2
    exit 1
else
    echo "前端子工程尚未落地，跳过前端门禁"
fi

echo "质量门全部通过"
