#!/usr/bin/env bash
# curl 域：Raw hosted、proxy 与 group 的官方客户端验收。
set -euo pipefail

: "${ACCEPTANCE_BASE_URL:?缺少服务地址}"
: "${ACCEPTANCE_UPSTREAM_URL:?缺少受控上游地址}"
: "${ACCEPTANCE_RUNTIME_DIR:?缺少运行目录}"
: "${ACCEPTANCE_FIXTURE_DIR:?缺少 fixture 目录}"
: "${ACCEPTANCE_TOKEN_FILE:?缺少令牌文件}"
command -v curl > /dev/null

readonly WORK_DIR="$ACCEPTANCE_RUNTIME_DIR/curl"
readonly AUTH_CONFIG="$WORK_DIR/auth.conf"
readonly RAW_FIXTURE="$ACCEPTANCE_FIXTURE_DIR/raw/artifact.txt"
mkdir -p "$WORK_DIR"
umask 077
TOKEN_FILE="$ACCEPTANCE_TOKEN_FILE" CONFIG_FILE="$AUTH_CONFIG" node -e '
const fs = require("node:fs");
const token = fs.readFileSync(process.env.TOKEN_FILE, "utf8").trim();
if (!token) throw new Error("令牌为空");
fs.writeFileSync(process.env.CONFIG_FILE, `header = "Authorization: Bearer ${token}"\n`, { mode: 0o600 });
'

fail() {
    printf '错误：curl 验收失败：%s\n' "$1" >&2
    exit 1
}

status() {
    local expected="$1"
    shift
    local actual
    actual="$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' "$@")"
    [ "$actual" = "$expected" ] || fail "期望 HTTP $expected，实际为 $actual"
}

create_repo() {
    local body="$1"
    status 201 --config "$AUTH_CONFIG" --header 'Content-Type: application/json' \
        --data-binary "$body" "$ACCEPTANCE_BASE_URL/api/v1/repos"
}

upstream_control() {
    curl --fail --silent --show-error --request POST --output /dev/null \
        "$ACCEPTANCE_UPSTREAM_URL/__acceptance/$1"
}

assert_upstream_count() {
    local path="$1"
    local expected="$2"
    local state="$WORK_DIR/upstream-state.json"
    curl --fail --silent --show-error --output "$state" "$ACCEPTANCE_UPSTREAM_URL/__acceptance/state"
    STATE_FILE="$state" COUNT_PATH="$path" EXPECTED_COUNT="$expected" node -e '
const fs = require("node:fs");
const state = JSON.parse(fs.readFileSync(process.env.STATE_FILE, "utf8"));
const actual = state.counts[process.env.COUNT_PATH] ?? 0;
if (actual !== Number(process.env.EXPECTED_COUNT)) {
  console.error(`错误：上游路径 ${process.env.COUNT_PATH} 期望请求 ${process.env.EXPECTED_COUNT} 次，实际 ${actual} 次`);
  process.exit(1);
}
'
}

create_repo '{"name":"accept-raw-hosted","format":"raw","repo_type":"hosted","visibility":"public","overwrite_policy":"allow"}'
create_repo "{\"name\":\"accept-raw-proxy\",\"format\":\"raw\",\"repo_type\":\"proxy\",\"visibility\":\"public\",\"upstream_url\":\"$ACCEPTANCE_UPSTREAM_URL/raw\"}"
create_repo '{"name":"accept-raw-first","format":"raw","repo_type":"hosted","visibility":"public","overwrite_policy":"allow"}'
create_repo '{"name":"accept-raw-second","format":"raw","repo_type":"hosted","visibility":"public","overwrite_policy":"allow"}'
create_repo '{"name":"accept-raw-group","format":"raw","repo_type":"group","visibility":"public","members":["accept-raw-first","accept-raw-second"]}'

hosted_url="$ACCEPTANCE_BASE_URL/repository/accept-raw-hosted/official/artifact.txt"
printf 'hosted-v1\n' > "$WORK_DIR/hosted-v1.txt"
printf 'hosted-v2\n' > "$WORK_DIR/hosted-v2.txt"
status 201 --config "$AUTH_CONFIG" --request PUT --upload-file "$WORK_DIR/hosted-v1.txt" "$hosted_url"
curl --fail --silent --show-error --output "$WORK_DIR/hosted-get.txt" "$hosted_url"
cmp --silent "$WORK_DIR/hosted-v1.txt" "$WORK_DIR/hosted-get.txt" || fail 'hosted GET 字节不一致'
curl --fail --silent --show-error --head --output "$WORK_DIR/hosted-head.txt" "$hosted_url"
[ ! -s "$WORK_DIR/hosted-head-body.txt" ] 2>/dev/null || fail 'hosted HEAD 不应返回响应体'
status 200 --config "$AUTH_CONFIG" --request PUT --upload-file "$WORK_DIR/hosted-v2.txt" "$hosted_url"
curl --fail --silent --show-error --output "$WORK_DIR/hosted-overwrite.txt" "$hosted_url"
cmp --silent "$WORK_DIR/hosted-v2.txt" "$WORK_DIR/hosted-overwrite.txt" || fail 'hosted 覆盖内容不一致'
status 204 --config "$AUTH_CONFIG" --request DELETE "$hosted_url"
status 404 "$hosted_url"

upstream_control reset
proxy_url="$ACCEPTANCE_BASE_URL/repository/accept-raw-proxy/artifact.txt"
curl --fail --silent --show-error --output "$WORK_DIR/proxy-first.txt" "$proxy_url"
cmp --silent "$RAW_FIXTURE" "$WORK_DIR/proxy-first.txt" || fail 'proxy 首次回源字节不一致'
assert_upstream_count '/raw/artifact.txt' 1
curl --fail --silent --show-error --output "$WORK_DIR/proxy-cached.txt" "$proxy_url"
assert_upstream_count '/raw/artifact.txt' 1
upstream_control disable
curl --fail --silent --show-error --output "$WORK_DIR/proxy-offline.txt" "$proxy_url"
cmp --silent "$RAW_FIXTURE" "$WORK_DIR/proxy-offline.txt" || fail 'proxy 断源缓存字节不一致'
upstream_control enable

printf 'group-first\n' > "$WORK_DIR/group-first.txt"
printf 'group-conflict-second\n' > "$WORK_DIR/group-conflict-second.txt"
printf 'group-second-only\n' > "$WORK_DIR/group-second-only.txt"
status 201 --config "$AUTH_CONFIG" --request PUT --upload-file "$WORK_DIR/group-first.txt" \
    "$ACCEPTANCE_BASE_URL/repository/accept-raw-first/conflict.txt"
status 201 --config "$AUTH_CONFIG" --request PUT --upload-file "$WORK_DIR/group-conflict-second.txt" \
    "$ACCEPTANCE_BASE_URL/repository/accept-raw-second/conflict.txt"
status 201 --config "$AUTH_CONFIG" --request PUT --upload-file "$WORK_DIR/group-second-only.txt" \
    "$ACCEPTANCE_BASE_URL/repository/accept-raw-second/second-only.txt"
curl --fail --silent --show-error --output "$WORK_DIR/group-conflict.txt" \
    "$ACCEPTANCE_BASE_URL/repository/accept-raw-group/conflict.txt"
cmp --silent "$WORK_DIR/group-first.txt" "$WORK_DIR/group-conflict.txt" || fail 'group 冲突未按前序成员命中'
curl --fail --silent --show-error --output "$WORK_DIR/group-second.txt" \
    "$ACCEPTANCE_BASE_URL/repository/accept-raw-group/second-only.txt"
cmp --silent "$WORK_DIR/group-second-only.txt" "$WORK_DIR/group-second.txt" || fail 'group 未命中后序独有路径'

printf 'curl 官方客户端验收通过\n'
