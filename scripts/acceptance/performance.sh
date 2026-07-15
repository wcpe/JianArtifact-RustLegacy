#!/usr/bin/env bash
# 性能域契约：以 Raw 验证 32 并发 proxy 单飞和 5 GiB hosted 流式传输。
set -euo pipefail

: "${ACCEPTANCE_BASE_URL:?缺少服务地址}"
: "${ACCEPTANCE_UPSTREAM_URL:?缺少受控上游地址}"
: "${ACCEPTANCE_RUNTIME_DIR:?缺少运行目录}"
: "${ACCEPTANCE_FIXTURE_DIR:?缺少 fixture 目录}"
: "${ACCEPTANCE_TOKEN_FILE:?缺少令牌文件}"
command -v curl > /dev/null
command -v node > /dev/null

readonly CONCURRENCY=32
readonly LARGE_BYTES=5368709120
readonly CGROUP_LIMIT_BYTES=5368709120
readonly CGROUP_LIMIT_TOLERANCE_BYTES=67108864
readonly RSS_PEAK_LIMIT_BYTES=1073741824
readonly RSS_DELTA_LIMIT_BYTES=268435456
readonly EVIDENCE_DIR="${ACCEPTANCE_EVIDENCE_DIR:-.tmp/performance-evidence}"
readonly SAMPLE_FILE="$EVIDENCE_DIR/memory-samples.tsv"
readonly SAMPLE_MARKER="$ACCEPTANCE_RUNTIME_DIR/sample-memory"
sampler_pid=""

fail() {
    echo "错误：$1" >&2
    exit 1
}

assert_equal() {
    local expected="$1"
    local actual="$2"
    local message="$3"
    if [ "$actual" != "$expected" ]; then
        fail "$message，期望 $expected，实际 $actual"
    fi
}

server_process_id() {
    local process_dir executable
    for process_dir in /proc/[0-9]*; do
        executable="$(readlink "$process_dir/exe" 2>/dev/null || true)"
        case "$executable" in
            */target/debug/jianartifact|*/target/release/jianartifact)
                printf '%s\n' "${process_dir##*/}"
                return 0
                ;;
        esac
    done
    return 1
}

read_rss_bytes() {
    local process_id="$1"
    local key value unit
    while read -r key value unit; do
        if [ "$key" = "VmRSS:" ]; then
            printf '%s\n' "$((value * 1024))"
            return 0
        fi
    done < "/proc/$process_id/status"
    return 1
}

configure_cgroup_paths() {
    if [ -f /sys/fs/cgroup/memory.max ]; then
        cgroup_limit_file=/sys/fs/cgroup/memory.max
        cgroup_current_file=/sys/fs/cgroup/memory.current
        cgroup_peak_file=/sys/fs/cgroup/memory.peak
        cgroup_events_file=/sys/fs/cgroup/memory.events
        return
    fi
    if [ -f /sys/fs/cgroup/memory/memory.limit_in_bytes ]; then
        cgroup_limit_file=/sys/fs/cgroup/memory/memory.limit_in_bytes
        cgroup_current_file=/sys/fs/cgroup/memory/memory.usage_in_bytes
        cgroup_peak_file=/sys/fs/cgroup/memory/memory.max_usage_in_bytes
        cgroup_events_file=/sys/fs/cgroup/memory/memory.failcnt
        return
    fi
    fail "无法读取容器 cgroup 内存限制"
}

read_oom_count() {
    local total=0 key value
    if [[ "$cgroup_events_file" == */memory.events ]]; then
        while read -r key value; do
            case "$key" in
                oom|oom_kill) total=$((total + value)) ;;
            esac
        done < "$cgroup_events_file"
        printf '%s\n' "$total"
        return
    fi
    read -r total < "$cgroup_events_file"
    printf '%s\n' "$total"
}

sample_memory() {
    local process_id="$1"
    local sample_index=0 rss_bytes cgroup_bytes
    while [ -f "$SAMPLE_MARKER" ]; do
        rss_bytes="$(read_rss_bytes "$process_id")" || return 1
        read -r cgroup_bytes < "$cgroup_current_file"
        printf '%s\t%s\t%s\n' "$sample_index" "$rss_bytes" "$cgroup_bytes" >> "$SAMPLE_FILE"
        sample_index=$((sample_index + 1))
        sleep 0.05
    done
}

stop_sampler() {
    if [ -f "$SAMPLE_MARKER" ]; then
        node -e "require('node:fs').unlinkSync(process.argv[1])" "$SAMPLE_MARKER"
    fi
    if [ -n "$sampler_pid" ]; then
        wait "$sampler_pid"
        sampler_pid=""
    fi
}

cleanup_sampler() {
    stop_sampler || true
}
trap cleanup_sampler EXIT

node -e "require('node:fs').rmSync(process.argv[1], { recursive: true, force: true }); require('node:fs').mkdirSync(process.argv[1], { recursive: true })" "$EVIDENCE_DIR"
configure_cgroup_paths
read -r cgroup_limit < "$cgroup_limit_file"
[[ "$cgroup_limit" =~ ^[0-9]+$ ]] || fail "cgroup 内存上限不是有限数值"
if [ "$cgroup_limit" -lt "$((CGROUP_LIMIT_BYTES - CGROUP_LIMIT_TOLERANCE_BYTES))" ] || [ "$cgroup_limit" -gt "$((CGROUP_LIMIT_BYTES + CGROUP_LIMIT_TOLERANCE_BYTES))" ]; then
    fail "cgroup 内存上限不是约 5 GiB：$cgroup_limit 字节"
fi
oom_before="$(read_oom_count)"
cgroup_peak_before="$(<"$cgroup_peak_file")"
echo "==> 已确认 Docker cgroup 内存上限约 5 GiB：$cgroup_limit 字节"

token="$(<"$ACCEPTANCE_TOKEN_FILE")"
[ -n "$token" ] || fail "令牌文件为空"

hosted_status="$(curl --silent --show-error --output "$ACCEPTANCE_RUNTIME_DIR/hosted-create.json" --write-out '%{http_code}' \
    -H "Authorization: Bearer $token" \
    -H "Content-Type: application/json" \
    --data '{"name":"performance-hosted","format":"raw","repo_type":"hosted","visibility":"public"}' \
    "$ACCEPTANCE_BASE_URL/api/v1/repos")"
assert_equal 201 "$hosted_status" "创建 Raw hosted 仓库失败"
proxy_payload="$(UPSTREAM_URL="$ACCEPTANCE_UPSTREAM_URL/raw" node -e "process.stdout.write(JSON.stringify({name:'performance-proxy',format:'raw',repo_type:'proxy',visibility:'public',upstream_url:process.env.UPSTREAM_URL}))")"
proxy_status="$(curl --silent --show-error --output "$ACCEPTANCE_RUNTIME_DIR/proxy-create.json" --write-out '%{http_code}' \
    -H "Authorization: Bearer $token" \
    -H "Content-Type: application/json" \
    --data "$proxy_payload" \
    "$ACCEPTANCE_BASE_URL/api/v1/repos")"
assert_equal 201 "$proxy_status" "创建 Raw proxy 仓库失败"

echo "==> 验证 NFR-08：32 并发同路径未命中只回源一次"
readonly RESPONSE_DIR="$ACCEPTANCE_RUNTIME_DIR/singleflight-responses"
readonly START_MARKER="$ACCEPTANCE_RUNTIME_DIR/start-singleflight"
node -e "const fs=require('node:fs'); fs.rmSync(process.argv[1], { recursive: true, force: true }); fs.mkdirSync(process.argv[1], { recursive: true })" "$RESPONSE_DIR"
curl --fail --silent --show-error --request POST \
    --output "$ACCEPTANCE_RUNTIME_DIR/upstream-before.json" \
    "$ACCEPTANCE_UPSTREAM_URL/__acceptance/reset"
upstream_before="$(STATE_FILE="$ACCEPTANCE_RUNTIME_DIR/upstream-before.json" node -e "const state=require(process.env.STATE_FILE.startsWith('/') ? process.env.STATE_FILE : process.cwd()+'/'+process.env.STATE_FILE); process.stdout.write(String(state.counts['/raw/artifact.txt'] ?? 0))")"
[[ "$upstream_before" =~ ^[0-9]+$ ]] || fail "上游请求计数不是数值"
pids=()
singleflight_started=$SECONDS
for ((request_index = 1; request_index <= CONCURRENCY; request_index++)); do
    (
        while [ ! -f "$START_MARKER" ]; do sleep 0.01; done
        curl --fail --silent --show-error \
            --output "$RESPONSE_DIR/$request_index.bin" \
            "$ACCEPTANCE_BASE_URL/repository/performance-proxy/artifact.txt"
    ) &
    pids+=("$!")
done
node -e "require('node:fs').writeFileSync(process.argv[1], '')" "$START_MARKER"
request_failed=0
for request_pid in "${pids[@]}"; do
    if ! wait "$request_pid"; then request_failed=1; fi
done
assert_equal 0 "$request_failed" "并发 proxy 请求存在失败"
singleflight_seconds=$((SECONDS - singleflight_started))
curl --fail --silent --show-error \
    --output "$ACCEPTANCE_RUNTIME_DIR/upstream-after.json" \
    "$ACCEPTANCE_UPSTREAM_URL/__acceptance/state"
upstream_after="$(STATE_FILE="$ACCEPTANCE_RUNTIME_DIR/upstream-after.json" node -e "const state=require(process.env.STATE_FILE.startsWith('/') ? process.env.STATE_FILE : process.cwd()+'/'+process.env.STATE_FILE); process.stdout.write(String(state.counts['/raw/artifact.txt'] ?? 0))")"
[[ "$upstream_after" =~ ^[0-9]+$ ]] || fail "上游请求计数不是数值"
upstream_delta=$((upstream_after - upstream_before))
assert_equal 1 "$upstream_delta" "32 并发单飞的有效上游命中次数不正确"
for ((request_index = 1; request_index <= CONCURRENCY; request_index++)); do
    cmp -s "$ACCEPTANCE_FIXTURE_DIR/raw/artifact.txt" "$RESPONSE_DIR/$request_index.bin" || fail "第 $request_index 个并发响应字节不一致"
done

server_pid="$(server_process_id)" || fail "无法定位验收服务进程"
baseline_rss="$(read_rss_bytes "$server_pid")" || fail "无法读取服务基线 RSS"
node -e "require('node:fs').writeFileSync(process.argv[1], '')" "$SAMPLE_FILE"
node -e "require('node:fs').writeFileSync(process.argv[1], '')" "$SAMPLE_MARKER"
sample_memory "$server_pid" &
sampler_pid=$!

echo "==> 验证 NFR-01/02/03/06：5 GiB Raw hosted 上传与下载"
readonly ZERO_BLOCK_BYTES=8388608
readonly ZERO_BLOCK_COUNT=640
expected_sha256="$(HASH_BLOCK_BYTES="$ZERO_BLOCK_BYTES" HASH_BLOCK_COUNT="$ZERO_BLOCK_COUNT" node -e '
const crypto=require("node:crypto");
const hash=crypto.createHash("sha256");
const block=Buffer.alloc(Number(process.env.HASH_BLOCK_BYTES));
for (let index=0; index<Number(process.env.HASH_BLOCK_COUNT); index += 1) hash.update(block);
process.stdout.write(hash.digest("hex"));
')"
large_size="$LARGE_BYTES"
large_started=$SECONDS
upload_status="$(dd if=/dev/zero bs="$ZERO_BLOCK_BYTES" count="$ZERO_BLOCK_COUNT" status=none \
    | curl --silent --show-error --output /dev/null --write-out '%{http_code}' \
        -H "Authorization: Bearer $token" --request PUT --upload-file - \
        "$ACCEPTANCE_BASE_URL/repository/performance-hosted/large-5g.bin")"
assert_equal 201 "$upload_status" "5 GiB Raw hosted 上传失败"
download_sha256="$(curl --fail --silent --show-error \
    "$ACCEPTANCE_BASE_URL/repository/performance-hosted/large-5g.bin" \
    | sha256sum | cut -d ' ' -f 1)"
assert_equal "$expected_sha256" "$download_sha256" "5 GiB 下载内容摘要与上传零流不一致"
large_seconds=$((SECONDS - large_started))
stop_sampler

rss_peak=0
cgroup_observed_peak=0
sample_count=0
while IFS=$'\t' read -r sample_index rss_bytes cgroup_bytes; do
    sample_count=$((sample_count + 1))
    if [ "$rss_bytes" -gt "$rss_peak" ]; then rss_peak="$rss_bytes"; fi
    if [ "$cgroup_bytes" -gt "$cgroup_observed_peak" ]; then cgroup_observed_peak="$cgroup_bytes"; fi
done < "$SAMPLE_FILE"
[ "$sample_count" -gt 0 ] || fail "未采集到 5 GiB 传输内存样本"
rss_delta=$((rss_peak - baseline_rss))
if [ "$rss_delta" -lt 0 ]; then rss_delta=0; fi
if [ "$rss_peak" -gt "$RSS_PEAK_LIMIT_BYTES" ]; then
    fail "5 GiB 传输期间服务 RSS 峰值超过 NFR-06 的 1 GiB：$rss_peak 字节"
fi
if [ "$rss_delta" -gt "$RSS_DELTA_LIMIT_BYTES" ]; then
    fail "5 GiB 传输导致的服务 RSS 增量超过 NFR-06 的 256 MiB：$rss_delta 字节"
fi
oom_after="$(read_oom_count)"
assert_equal "$oom_before" "$oom_after" "5 GiB cgroup 出现新的 OOM 事件"
cgroup_peak_after="$(<"$cgroup_peak_file")"

CGROUP_LIMIT="$cgroup_limit" CGROUP_PEAK_BEFORE="$cgroup_peak_before" CGROUP_PEAK_AFTER="$cgroup_peak_after" \
CGROUP_OBSERVED_PEAK="$cgroup_observed_peak" OOM_BEFORE="$oom_before" OOM_AFTER="$oom_after" \
BASELINE_RSS="$baseline_rss" RSS_PEAK="$rss_peak" RSS_DELTA="$rss_delta" SAMPLE_COUNT="$sample_count" \
UPSTREAM_BEFORE="$upstream_before" UPSTREAM_AFTER="$upstream_after" UPSTREAM_DELTA="$upstream_delta" \
SINGLEFLIGHT_SECONDS="$singleflight_seconds" RESULT_LARGE_BYTES="$large_size" LARGE_SECONDS="$large_seconds" \
node -e "const fs=require('node:fs'); const number=(name)=>Number(process.env[name]); const evidence={specifications:['NFR-01','NFR-02','NFR-03','NFR-06','NFR-08'],cgroup:{limit_bytes:number('CGROUP_LIMIT'),peak_before_bytes:number('CGROUP_PEAK_BEFORE'),peak_after_bytes:number('CGROUP_PEAK_AFTER'),observed_peak_bytes:number('CGROUP_OBSERVED_PEAK'),oom_before:number('OOM_BEFORE'),oom_after:number('OOM_AFTER')},server_rss:{baseline_bytes:number('BASELINE_RSS'),peak_bytes:number('RSS_PEAK'),delta_bytes:number('RSS_DELTA'),sample_count:number('SAMPLE_COUNT'),peak_limit_bytes:1073741824,delta_limit_bytes:268435456},singleflight:{concurrency:32,upstream_before:number('UPSTREAM_BEFORE'),upstream_after:number('UPSTREAM_AFTER'),upstream_delta:number('UPSTREAM_DELTA'),all_responses_byte_equal:true,duration_seconds:number('SINGLEFLIGHT_SECONDS')},large_transfer:{bytes:number('RESULT_LARGE_BYTES'),upload_status:201,download_byte_equal:true,duration_seconds:number('LARGE_SECONDS')}}; fs.writeFileSync(process.argv[1], JSON.stringify(evidence,null,2)+'\\n')" "$EVIDENCE_DIR/result.json"

echo "性能验收通过：单飞上游命中 1 次，5 GiB 传输 RSS 峰值 $rss_peak 字节、增量 $rss_delta 字节，无新增 OOM"
echo "证据已写入 $EVIDENCE_DIR/result.json 与 $SAMPLE_FILE"
