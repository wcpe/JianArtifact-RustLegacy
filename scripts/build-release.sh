#!/usr/bin/env bash
# JianArtifact 发布构建脚本（M1-FR-042）
# 仅在 Docker 容器内执行：版本校验 → 前端构建（未落地则跳过）→ cargo release 构建 → 产物校验 → 打包到 dist/
set -euo pipefail

# 切到仓库根目录，保证相对路径稳定
cd "$(dirname "$0")/.."

TARGET_TRIPLE="x86_64-unknown-linux-gnu"

echo "==> [1/5] 版本一致性校验"
version_file="$(tr -d '[:space:]' < VERSION)"
version_cargo="$(cargo pkgid -p jianartifact-server | sed 's/.*[@#]//')"
if [ "$version_file" != "$version_cargo" ]; then
    echo "错误：VERSION 文件（${version_file}）与 Cargo workspace 版本（${version_cargo}）不一致，请先对齐两处版本号" >&2
    exit 1
fi

echo "==> [2/5] 前端构建"
if [ -f pnpm-lock.yaml ]; then
    pnpm install --frozen-lockfile
    pnpm turbo build
else
    echo "前端子工程尚未落地，跳过前端构建"
fi

echo "==> [3/5] Rust release 构建"
cargo build --release --workspace

echo "==> [4/5] 产物校验"
bin_path="target/release/jianartifact"
if ! readelf -h "$bin_path" | grep -q "ELF64"; then
    echo "错误：产物不是 ELF64 二进制" >&2
    exit 1
fi
if ! readelf -h "$bin_path" | grep -q "X86-64"; then
    echo "错误：产物架构不是 x86-64" >&2
    exit 1
fi

echo "==> [5/5] 打包二进制、systemd unit、VERSION 与第三方许可证 notice"
pkg_name="jianartifact-${version_file}-${TARGET_TRIPLE}"
stage_dir="$(mktemp -d)"
mkdir -p "${stage_dir}/${pkg_name}/bin" "${stage_dir}/${pkg_name}/systemd"
cp "$bin_path" "${stage_dir}/${pkg_name}/bin/jianartifact"
cp scripts/deploy/jianartifact.service "${stage_dir}/${pkg_name}/systemd/jianartifact.service"
cp VERSION "${stage_dir}/${pkg_name}/VERSION"
cp THIRD_PARTY_NOTICES "${stage_dir}/${pkg_name}/THIRD_PARTY_NOTICES"
mkdir -p dist
tar -czf "dist/${pkg_name}.tar.gz" -C "$stage_dir" "$pkg_name"
(cd dist && sha256sum "${pkg_name}.tar.gz" > "${pkg_name}.tar.gz.sha256")
rm -rf "$stage_dir"

echo "发布构建完成：dist/${pkg_name}.tar.gz（含 THIRD_PARTY_NOTICES 与 sha256 伴随文件）"
