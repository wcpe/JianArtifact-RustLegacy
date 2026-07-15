#!/usr/bin/env bash
# 官方客户端统一验收入口；必须由 Docker 内的质量门或 Makefile 调用。
set -euo pipefail

cd "$(dirname "$0")/../.."

readonly DOMAIN="${1:-all}"
readonly RUNTIME_DIR="${ACCEPTANCE_RUNTIME_DIR:-.tmp/acceptance}"
readonly SERVER_PORT="${ACCEPTANCE_SERVER_PORT:-18081}"
readonly UPSTREAM_PORT="${ACCEPTANCE_UPSTREAM_PORT:-18082}"
readonly BASE_URL="http://127.0.0.1:${SERVER_PORT}"
readonly UPSTREAM_URL="http://127.0.0.1:${UPSTREAM_PORT}"
readonly FIXTURE_DIR="scripts/acceptance/fixtures"
server_pid=""
upstream_pid=""

cleanup() {
    if [ -n "$server_pid" ]; then kill "$server_pid" 2>/dev/null || true; fi
    if [ -n "$upstream_pid" ]; then kill "$upstream_pid" 2>/dev/null || true; fi
    if [ -n "$server_pid" ]; then wait "$server_pid" 2>/dev/null || true; fi
    if [ -n "$upstream_pid" ]; then wait "$upstream_pid" 2>/dev/null || true; fi
}
trap cleanup EXIT INT TERM

wait_for_url() {
    local url="$1"
    local attempts=60
    while ! curl --fail --silent --show-error "$url" > /dev/null 2>&1; do
        attempts=$((attempts - 1))
        if [ "$attempts" -eq 0 ]; then
            printf '错误：验收依赖未在限定时间内就绪：%s\n' "$url" >&2
            return 1
        fi
        sleep 1
    done
}

prepare_runtime() {
    RUNTIME_TO_CLEAR="$RUNTIME_DIR" node -e '
const fs = require("node:fs");
const path = process.env.RUNTIME_TO_CLEAR;
if (!path || path === "/" || path === ".") throw new Error("拒绝清理不安全的运行目录");
fs.rmSync(path, { recursive: true, force: true });
fs.mkdirSync(path, { recursive: true, mode: 0o700 });
for (const name of ["blobs", "home"]) fs.mkdirSync(`${path}/${name}`, { mode: 0o700 });
'
}

create_credentials() {
    umask 077
    admin_password="$(node -e "process.stdout.write(require('node:crypto').randomBytes(24).toString('base64url'))")"
    jwt_secret="$(node -e "process.stdout.write(require('node:crypto').randomBytes(32).toString('base64url'))")"
    ACCEPTANCE_ADMIN_PASSWORD="$admin_password" CREDENTIALS_PATH="$RUNTIME_DIR/credentials.json" node -e '
const fs = require("node:fs");
fs.writeFileSync(process.env.CREDENTIALS_PATH, JSON.stringify({
  username: "acceptance-admin",
  password: process.env.ACCEPTANCE_ADMIN_PASSWORD,
}), { mode: 0o600 });
'
}

login() {
    local response_file="$RUNTIME_DIR/login-response.json"
    curl --fail --silent --show-error \
        --header 'Content-Type: application/json' \
        --data-binary "@$RUNTIME_DIR/credentials.json" \
        --output "$response_file" \
        "$BASE_URL/api/v1/auth/login"
    LOGIN_RESPONSE="$response_file" TOKEN_FILE="$RUNTIME_DIR/token" node -e '
const fs = require("node:fs");
const value = JSON.parse(fs.readFileSync(process.env.LOGIN_RESPONSE, "utf8"));
if (typeof value.token !== "string" || value.token.length === 0) throw new Error("登录响应缺少令牌");
fs.writeFileSync(process.env.TOKEN_FILE, value.token, { mode: 0o600 });
fs.rmSync(process.env.LOGIN_RESPONSE, { force: true });
'
}

start_dependencies() {
    prepare_runtime
    create_credentials

    node scripts/acceptance/local-upstream.mjs \
        "--root=$FIXTURE_DIR" \
        "--port=$UPSTREAM_PORT" \
        "--ready=$RUNTIME_DIR/upstream-ready.json" > "$RUNTIME_DIR/upstream.log" 2>&1 &
    upstream_pid=$!

    local server_binary="target/debug/jianartifact"
    if [ "${ACCEPTANCE_RELEASE_BINARY:-false}" = "true" ]; then
        cargo build --quiet --release -p jianartifact-server --bin jianartifact
        server_binary="target/release/jianartifact"
    else
        cargo build --quiet -p jianartifact-server --bin jianartifact
    fi
    JIANARTIFACT_BIND="127.0.0.1:${SERVER_PORT}" \
    JIANARTIFACT_DATABASE_PATH="$RUNTIME_DIR/jianartifact.db" \
    JIANARTIFACT_DATA_DIR="$RUNTIME_DIR/blobs" \
    JIANARTIFACT_LOG_LEVEL=warn \
    JIANARTIFACT_JWT_SECRET="$jwt_secret" \
    JIANARTIFACT_ADMIN_USER=acceptance-admin \
    JIANARTIFACT_ADMIN_PASSWORD="$admin_password" \
        "$server_binary" > "$RUNTIME_DIR/server.log" 2>&1 &
    server_pid=$!

    wait_for_url "$UPSTREAM_URL/__acceptance/state"
    wait_for_url "$BASE_URL/readyz"
    login
    unset admin_password jwt_secret
}

run_domain() {
    local domain="$1"
    ACCEPTANCE_BASE_URL="$BASE_URL" \
    ACCEPTANCE_UPSTREAM_URL="$UPSTREAM_URL" \
    ACCEPTANCE_RUNTIME_DIR="$RUNTIME_DIR" \
    ACCEPTANCE_FIXTURE_DIR="$FIXTURE_DIR" \
    ACCEPTANCE_TOKEN_FILE="$RUNTIME_DIR/token" \
        bash "scripts/acceptance/${domain}.sh"
}

case "$DOMAIN" in
    curl|maven|npm|performance)
        start_dependencies
        run_domain "$DOMAIN"
        ;;
    all)
        start_dependencies
        for domain in curl maven npm; do run_domain "$domain"; done
        ;;
    *)
        printf '用法：bash scripts/acceptance/run.sh {all|curl|maven|npm|performance}\n' >&2
        exit 2
        ;;
esac
