#!/usr/bin/env bash
# JianArtifact 统一质量门脚本
# 仅在 Docker 容器内执行；CI 与本地共用本脚本，保证两端行为完全一致。
set -euo pipefail

cd "$(dirname "$0")/.."
readonly PROJECT_ROOT="$(pwd)"
readonly RUNTIME_DIR=.tmp/quality-gate

cleanup() {
    node -e "require('node:fs').rmSync(process.argv[1], { recursive: true, force: true })" "$RUNTIME_DIR"
}
trap cleanup EXIT
cleanup
mkdir -p "$RUNTIME_DIR"

if [ ! -f pnpm-lock.yaml ]; then
    echo "错误：缺少 pnpm-lock.yaml，前端门禁无法执行" >&2
    exit 1
fi

echo "==> [1/14] 安装前端依赖并构建 web"
pnpm install --frozen-lockfile
pnpm --filter @jianartifact/web run build

echo "==> [2/14] Rust 格式检查"
cargo fmt --all -- --check

echo "==> [3/14] Rust 静态分析（警告视为错误）"
cargo clippy --workspace --all-targets --all-features -- -D warnings

echo "==> [4/14] Rust 测试"
cargo test --workspace

echo "==> [5/14] OpenAPI 契约快照新鲜度"
cargo run -q --bin jianartifact -- openapi > "$RUNTIME_DIR/openapi-current.json"
if ! diff -q "$RUNTIME_DIR/openapi-current.json" packages/devmock/openapi.json > /dev/null 2>&1; then
    echo "错误：契约快照 packages/devmock/openapi.json 已过期，请运行 make openapi 重新生成并提交" >&2
    exit 1
fi

pnpm --filter @jianartifact/devmock exec openapi-typescript openapi.json -o "$PROJECT_ROOT/$RUNTIME_DIR/schema-current.d.ts" > /dev/null 2>&1
if ! diff -q "$RUNTIME_DIR/schema-current.d.ts" packages/devmock/src/schema.d.ts > /dev/null 2>&1; then
    echo "错误：契约类型 packages/devmock/src/schema.d.ts 已过期，请运行 make openapi 重新生成并提交" >&2
    exit 1
fi

echo "==> [6/14] 前端格式、类型、静态分析与测试"
pnpm run format:check
pnpm turbo typecheck lint test --concurrency=2

echo "==> [7/14] 依赖漏洞审计"
cargo audit

echo "==> [8/14] 依赖许可证检查"
cargo deny check licenses

echo "==> [9/14] 部署脚本逻辑测试"
bash scripts/deploy/test-remote-install.sh

echo "==> [10/14] 前端完整构建"
pnpm turbo build

echo "==> [11/14] 浏览器 E2E（web 对 devmock）"
pnpm --filter @jianartifact/web run test:e2e

echo "==> [12/14] 浏览器 E2E（web 对真实 Rust 后端）"
pnpm --filter @jianartifact/web run test:e2e:real

echo "==> [13/14] 官方客户端验收（curl、Maven、npm）"
bash scripts/acceptance/run.sh all

echo "==> [14/14] 5 GiB cgroup 性能证据（Raw 单飞与大对象流式传输）"
ACCEPTANCE_RELEASE_BINARY=true bash scripts/acceptance/run.sh performance

echo "质量门全部通过"
