#!/usr/bin/env bash
# JianArtifact 发布构建脚本（M1-FR-042）
# 仅在 Docker 容器内执行：版本校验 → 前端构建 → 内嵌前端的 release 构建 → 启动 smoke → 打包到 dist/
set -euo pipefail

# 切到仓库根目录，保证相对路径稳定
cd "$(dirname "$0")/.."

TARGET_TRIPLE="x86_64-unknown-linux-gnu"
EMBED_FEATURE="embedded-web"

echo "==> [1/6] 版本一致性校验"
version_file="$(tr -d '[:space:]' < VERSION)"
version_cargo="$(cargo pkgid -p jianartifact-server | sed 's/.*[@#]//')"
if [ "$version_file" != "$version_cargo" ]; then
    echo "错误：VERSION 文件（${version_file}）与 Cargo workspace 版本（${version_cargo}）不一致，请先对齐两处版本号" >&2
    exit 1
fi

echo "==> [2/6] 前端构建"
pnpm install --frozen-lockfile
pnpm --filter @jianartifact/web run build

echo "==> [3/6] Rust release 构建"
cargo build --release --workspace --features "$EMBED_FEATURE"

echo "==> [4/6] 产物校验"
bin_path="target/release/jianartifact"
if ! readelf -h "$bin_path" | grep -q "ELF64"; then
    echo "错误：产物不是 ELF64 二进制" >&2
    exit 1
fi
if ! readelf -h "$bin_path" | grep -q "X86-64"; then
    echo "错误：产物架构不是 x86-64" >&2
    exit 1
fi

echo "==> [5/6] 启动产物 smoke 验证"
runtime_dir="$(mktemp -d)"
smoke_port="$((20000 + RANDOM % 20000))"
smoke_pid=""
cleanup() {
    if [ -n "$smoke_pid" ]; then
        kill "$smoke_pid" 2>/dev/null || true
        wait "$smoke_pid" 2>/dev/null || true
    fi
    rm -rf "$runtime_dir" "${stage_dir:-}"
}
trap cleanup EXIT
jwt_secret="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
JIANARTIFACT_BIND="127.0.0.1:${smoke_port}" \
JIANARTIFACT_DATABASE_PATH="${runtime_dir}/smoke.db" \
JIANARTIFACT_DATA_DIR="${runtime_dir}/blobs" \
JIANARTIFACT_JWT_SECRET="$jwt_secret" \
"$bin_path" > "${runtime_dir}/server.log" 2>&1 &
smoke_pid="$!"
ready="false"
for _ in $(seq 1 30); do
    if curl -fsS "http://127.0.0.1:${smoke_port}/readyz" > /dev/null; then
        ready="true"
        break
    fi
    if ! kill -0 "$smoke_pid" 2>/dev/null; then
        echo "错误：发布产物 smoke 启动失败" >&2
        exit 1
    fi
    sleep 1
done
if [ "$ready" != "true" ]; then
    echo "错误：发布产物 readyz smoke 超时" >&2
    exit 1
fi
homepage="$(curl -fsS "http://127.0.0.1:${smoke_port}/")"
openapi="$(curl -fsS "http://127.0.0.1:${smoke_port}/api/v1/openapi.json")"
case "$homepage" in
    *'<div id="root"></div>'*) ;;
    *) echo "错误：发布产物未返回内嵌前端首页" >&2; exit 1 ;;
esac
case "$openapi" in
    *'"openapi"'*) ;;
    *) echo "错误：发布产物管理 API smoke 失败" >&2; exit 1 ;;
esac
kill "$smoke_pid"
wait "$smoke_pid" || true
smoke_pid=""
unset jwt_secret JIANARTIFACT_JWT_SECRET

echo "==> [6/6] 打包二进制、systemd unit、VERSION 与第三方许可证 notice"
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
