#!/usr/bin/env bash
# JianArtifact 统一质量门脚本
# 仅在 Docker 容器内执行；CI 与本地共用本脚本，保证两端行为完全一致。
set -euo pipefail

# 切到仓库根目录（脚本所在目录的上一级），保证相对路径稳定
cd "$(dirname "$0")/.."

echo "==> [1/8] Rust 格式检查"
cargo fmt --all -- --check

echo "==> [2/8] Rust 静态分析（警告视为错误）"
cargo clippy --workspace --all-targets --all-features -- -D warnings

echo "==> [3/8] Rust 测试"
cargo test --workspace

echo "==> [4/8] 契约快照新鲜度（devmock 消费的 OpenAPI 与代码一致）"
cargo run -q --bin jianartifact -- openapi > /tmp/openapi-current.json
if ! diff -q /tmp/openapi-current.json packages/devmock/openapi.json > /dev/null 2>&1; then
    echo "错误：契约快照 packages/devmock/openapi.json 已过期，请运行 make openapi 重新生成并提交" >&2
    exit 1
fi

echo "==> [5/8] 依赖漏洞审计"
cargo audit

echo "==> [6/8] 依赖许可证检查"
cargo deny check licenses

echo "==> [7/8] 部署脚本逻辑测试"
bash scripts/deploy/test-remote-install.sh

echo "==> [8/8] 前端质量门"
# 前端子工程清单（apps/server 是 Rust crate，不参与前端门禁判定）
frontend_src=""
for dir in apps/web apps/wiki packages/ui packages/devmock packages/eslint-config packages/typescript-config; do
    if [ -e "$dir/tsconfig.json" ] || [ -d "$dir/src" ]; then
        frontend_src=1
    fi
done
if [ -f pnpm-lock.yaml ]; then
    pnpm install --frozen-lockfile
    # 契约类型生成物新鲜度（依赖 openapi-typescript，须在 install 之后校验）
    if [ -f packages/devmock/src/schema.d.ts ]; then
        pnpm --filter @jianartifact/devmock exec openapi-typescript openapi.json -o /tmp/schema-current.d.ts > /dev/null 2>&1
        if ! diff -q /tmp/schema-current.d.ts packages/devmock/src/schema.d.ts > /dev/null 2>&1; then
            echo "错误：契约类型 packages/devmock/src/schema.d.ts 已过期，请运行 make openapi 重新生成并提交" >&2
            exit 1
        fi
    fi
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
