#!/usr/bin/env bash
# 性能域契约：验证 32 并发 proxy 单飞，以及四类大对象路径的有界内存流式处理。
set -euo pipefail

: "${ACCEPTANCE_BASE_URL:?缺少服务地址}"
: "${ACCEPTANCE_UPSTREAM_URL:?缺少受控上游地址}"
: "${ACCEPTANCE_RUNTIME_DIR:?缺少运行目录}"
: "${ACCEPTANCE_FIXTURE_DIR:?缺少 fixture 目录}"
: "${ACCEPTANCE_TOKEN_FILE:?缺少令牌文件}"
command -v curl > /dev/null
command -v node > /dev/null
command -v sha256sum > /dev/null
command -v cmp > /dev/null

readonly CONCURRENCY=32
readonly SINGLEFLIGHT_REQUIRED_CONSECUTIVE_SAMPLES=2
readonly FORMAL_ONE_GIB_BYTES=1073741824
readonly FORMAL_FIVE_GIB_BYTES=5368709120
readonly ONE_GIB_BYTES="${ACCEPTANCE_PERFORMANCE_1G_BYTES:-$FORMAL_ONE_GIB_BYTES}"
readonly FIVE_GIB_BYTES="${ACCEPTANCE_PERFORMANCE_5G_BYTES:-$FORMAL_FIVE_GIB_BYTES}"
readonly RELEASE_BINARY="${ACCEPTANCE_RELEASE_BINARY:-false}"
readonly HOSTED_SEED=17
readonly PROXY_SEED=34
readonly MIGRATION_SEED=51
readonly CGROUP_LIMIT_BYTES=5368709120
readonly CGROUP_LIMIT_TOLERANCE_BYTES=67108864
readonly RSS_PEAK_LIMIT_BYTES=1073741824
readonly RSS_DELTA_LIMIT_BYTES=268435456
readonly STREAM_BLOCK_BYTES=8388608
readonly BLOCK_PATTERN_STRIDE=131
readonly EVIDENCE_DIR="${ACCEPTANCE_EVIDENCE_DIR:-.tmp/performance-evidence}"
readonly MEASUREMENT_DIR="$EVIDENCE_DIR/measurements"
readonly SAMPLE_DIR="$EVIDENCE_DIR/samples"
readonly SAMPLE_FILE="$EVIDENCE_DIR/memory-samples.tsv"
readonly MIGRATION_POLL_ATTEMPTS="${ACCEPTANCE_PERFORMANCE_MIGRATION_POLL_ATTEMPTS:-18000}"

sampler_pid=""
current_sample_marker=""
current_sample_file=""
server_pid=""
cgroup_limit_file=""
cgroup_current_file=""
cgroup_peak_file=""
cgroup_events_file=""
measurement_baseline_rss=0
measurement_rss_peak=0
measurement_rss_delta=0
measurement_cgroup_peak=0
measurement_kernel_peak_before=0
measurement_kernel_peak_after=0
measurement_sample_count=0
measurement_oom_before=0
measurement_oom_after=0
execution_mode=smoke
singleflight_hold_active=0
singleflight_ready_count=0
singleflight_barrier_evidence_file=""
singleflight_pre_release_upstream_file=""
singleflight_response_evidence_file=""

declare -a singleflight_pids=()
declare -A expected_sha256
declare -A proxy_sha256
declare -A migration_sha256

fail() {
    printf '错误：性能验收失败：%s\n' "$1" >&2
    exit 1
}

assert_equal() {
    local expected="$1"
    local actual="$2"
    local message="$3"
    [ "$actual" = "$expected" ] || fail "$message，期望 $expected，实际 $actual"
}

validate_positive_integer() {
    local value="$1"
    local name="$2"
    [[ "$value" =~ ^[1-9][0-9]*$ ]] || fail "$name 必须是正整数"
}

validate_configuration() {
    validate_positive_integer "$ONE_GIB_BYTES" "1 GiB 样本字节数"
    validate_positive_integer "$FIVE_GIB_BYTES" "5 GiB 样本字节数"
    validate_positive_integer "$MIGRATION_POLL_ATTEMPTS" "迁移轮询次数"
    [ "$ONE_GIB_BYTES" -lt "$FIVE_GIB_BYTES" ] || fail "1 GiB 样本必须小于 5 GiB 样本"
    [ "$FIVE_GIB_BYTES" -le "$FORMAL_FIVE_GIB_BYTES" ] || fail "样本不得超过正式 5 GiB 上限"
    case "$RELEASE_BINARY" in
        true)
            [ "$ONE_GIB_BYTES" = "$FORMAL_ONE_GIB_BYTES" ] || fail "正式验收禁止覆写 1 GiB 样本大小"
            [ "$FIVE_GIB_BYTES" = "$FORMAL_FIVE_GIB_BYTES" ] || fail "正式验收禁止覆写 5 GiB 样本大小"
            execution_mode=formal
            ;;
        false) execution_mode=smoke ;;
        *) fail "ACCEPTANCE_RELEASE_BINARY 只能为 true 或 false" ;;
    esac
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

read_kernel_peak() {
    local peak
    read -r peak < "$cgroup_peak_file"
    [[ "$peak" =~ ^[0-9]+$ ]] || fail "cgroup memory.peak 不是有效数值"
    printf '%s\n' "$peak"
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
    local marker="$2"
    local output="$3"
    local sample_index=0 rss_bytes cgroup_bytes
    while [ -f "$marker" ]; do
        rss_bytes="$(read_rss_bytes "$process_id")" || return 1
        read -r cgroup_bytes < "$cgroup_current_file"
        printf '%s\t%s\t%s\n' "$sample_index" "$rss_bytes" "$cgroup_bytes" >> "$output"
        sample_index=$((sample_index + 1))
        sleep 0.05
    done
}

stop_sampler() {
    if [ -n "$current_sample_marker" ]; then
        rm -f "$current_sample_marker"
    fi
    if [ -n "$sampler_pid" ]; then
        wait "$sampler_pid"
        sampler_pid=""
    fi
    current_sample_marker=""
}

cleanup_acceptance() {
    local request_pid
    stop_sampler || true
    if [ "$singleflight_hold_active" -eq 1 ]; then
        curl --silent --show-error --max-time 2 --request POST --output /dev/null \
            "$ACCEPTANCE_UPSTREAM_URL/__acceptance/artifact/release" || true
        singleflight_hold_active=0
    fi
    for request_pid in "${singleflight_pids[@]}"; do kill "$request_pid" 2>/dev/null || true; done
    for request_pid in "${singleflight_pids[@]}"; do wait "$request_pid" 2>/dev/null || true; done
    singleflight_pids=()
}
trap cleanup_acceptance EXIT

start_measurement() {
    local path_name="$1"
    local sample_name="$2"
    local attempts=100
    current_sample_file="$SAMPLE_DIR/${path_name}-${sample_name}.tsv"
    current_sample_marker="$ACCEPTANCE_RUNTIME_DIR/sample-${path_name}-${sample_name}"
    measurement_kernel_peak_before="$(read_kernel_peak)"
    measurement_baseline_rss="$(read_rss_bytes "$server_pid")" || fail "无法读取服务基线 RSS"
    measurement_oom_before="$(read_oom_count)"
    : > "$current_sample_file"
    : > "$current_sample_marker"
    sample_memory "$server_pid" "$current_sample_marker" "$current_sample_file" &
    sampler_pid=$!
    while [ ! -s "$current_sample_file" ]; do
        attempts=$((attempts - 1))
        [ "$attempts" -gt 0 ] || fail "内存采样器未能启动"
        sleep 0.01
    done
}

finish_measurement() {
    local path_name="$1"
    local sample_name="$2"
    local sample_index rss_bytes cgroup_bytes
    stop_sampler
    measurement_rss_peak=0
    measurement_cgroup_peak=0
    measurement_sample_count=0
    while IFS=$'\t' read -r sample_index rss_bytes cgroup_bytes; do
        measurement_sample_count=$((measurement_sample_count + 1))
        [ "$rss_bytes" -le "$measurement_rss_peak" ] || measurement_rss_peak="$rss_bytes"
        [ "$cgroup_bytes" -le "$measurement_cgroup_peak" ] || measurement_cgroup_peak="$cgroup_bytes"
        printf '%s\t%s\t%s\t%s\t%s\n' "$path_name" "$sample_name" "$sample_index" "$rss_bytes" "$cgroup_bytes" >> "$SAMPLE_FILE"
    done < "$current_sample_file"
    [ "$measurement_sample_count" -gt 0 ] || fail "$path_name/$sample_name 未采集到内存样本"
    measurement_rss_delta=$((measurement_rss_peak - measurement_baseline_rss))
    [ "$measurement_rss_delta" -ge 0 ] || measurement_rss_delta=0
    measurement_kernel_peak_after="$(read_kernel_peak)"
    measurement_oom_after="$(read_oom_count)"
    [ "$measurement_rss_peak" -le "$RSS_PEAK_LIMIT_BYTES" ] || fail "$path_name/$sample_name 服务 RSS 峰值超过 1 GiB"
    [ "$measurement_rss_delta" -le "$RSS_DELTA_LIMIT_BYTES" ] || fail "$path_name/$sample_name 服务 RSS 增量超过 256 MiB"
    [ "$measurement_cgroup_peak" -le "$cgroup_limit" ] || fail "$path_name/$sample_name 采样 memory.current 峰值超过容器内存上限"
    assert_equal "$measurement_oom_before" "$measurement_oom_after" "$path_name/$sample_name 出现新的 OOM 事件"
}

write_measurement() {
    local path_name="$1" sample_name="$2" bytes="$3" duration="$4"
    local status_expected="$5" status_actual="$6" summary_algorithm="$7"
    local summary_expected="$8" summary_actual="$9" details_file="${10}"
    PATH_NAME="$path_name" SAMPLE_NAME="$sample_name" EXECUTION_MODE="$execution_mode" RESULT_BYTES="$bytes" DURATION_SECONDS="$duration" \
    STATUS_EXPECTED="$status_expected" STATUS_ACTUAL="$status_actual" SUMMARY_ALGORITHM="$summary_algorithm" \
    SUMMARY_EXPECTED="$summary_expected" SUMMARY_ACTUAL="$summary_actual" DETAILS_FILE="$details_file" \
    RAW_SAMPLE_FILE="$current_sample_file" RAW_SAMPLE_RELATIVE_PATH="samples/${path_name}-${sample_name}.tsv" SERVER_PID="$server_pid" \
    BASELINE_RSS="$measurement_baseline_rss" RSS_PEAK="$measurement_rss_peak" RSS_DELTA="$measurement_rss_delta" \
    SAMPLE_COUNT="$measurement_sample_count" CGROUP_OBSERVED_PEAK="$measurement_cgroup_peak" CGROUP_LIMIT="$cgroup_limit" \
    KERNEL_PEAK_BEFORE="$measurement_kernel_peak_before" KERNEL_PEAK_AFTER="$measurement_kernel_peak_after" \
    OOM_BEFORE="$measurement_oom_before" OOM_AFTER="$measurement_oom_after" \
    RSS_PEAK_LIMIT="$RSS_PEAK_LIMIT_BYTES" RSS_DELTA_LIMIT="$RSS_DELTA_LIMIT_BYTES" \
    node -e '
const crypto = require("node:crypto");
const fs = require("node:fs");
const number = (name) => Number(process.env[name]);
const details = JSON.parse(fs.readFileSync(process.env.DETAILS_FILE, "utf8"));
const raw = fs.readFileSync(process.env.RAW_SAMPLE_FILE);
const rows = raw.toString("utf8").trim().split("\n").filter(Boolean).map((line) => {
  const values = line.split("\t").map(Number);
  if (values.length !== 3 || values.some((value) => !Number.isInteger(value) || value < 0)) {
    throw new Error("原始内存采样行无效");
  }
  return { index: values[0], server_rss_bytes: values[1], cgroup_memory_current_bytes: values[2] };
});
const serverRssPeak = Math.max(...rows.map((row) => row.server_rss_bytes));
const recalculated = {
  sample_count: rows.length,
  server_rss_peak_bytes: serverRssPeak,
  server_rss_delta_bytes: Math.max(0, serverRssPeak - number("BASELINE_RSS")),
  cgroup_memory_current_peak_bytes: Math.max(...rows.map((row) => row.cgroup_memory_current_bytes)),
};
const rawSamplesValid = rows.length > 0
  && rows.every((row, index) => row.index === index)
  && recalculated.sample_count === number("SAMPLE_COUNT")
  && recalculated.server_rss_peak_bytes === number("RSS_PEAK")
  && recalculated.server_rss_delta_bytes === number("RSS_DELTA")
  && recalculated.cgroup_memory_current_peak_bytes === number("CGROUP_OBSERVED_PEAK");
const result = {
  path: process.env.PATH_NAME,
  sample: process.env.SAMPLE_NAME,
  execution_mode: process.env.EXECUTION_MODE,
  bytes: number("RESULT_BYTES"),
  duration_seconds: number("DURATION_SECONDS"),
  status: {
    expected: process.env.STATUS_EXPECTED,
    actual: process.env.STATUS_ACTUAL,
    consistent: process.env.STATUS_EXPECTED === process.env.STATUS_ACTUAL,
  },
  summary: {
    algorithm: process.env.SUMMARY_ALGORITHM,
    expected: process.env.SUMMARY_EXPECTED,
    actual: process.env.SUMMARY_ACTUAL,
    consistent: process.env.SUMMARY_EXPECTED === process.env.SUMMARY_ACTUAL,
  },
  raw_samples: {
    path: process.env.RAW_SAMPLE_RELATIVE_PATH,
    sha256: crypto.createHash("sha256").update(raw).digest("hex"),
    bytes: raw.length,
    columns: ["index", "server_rss_bytes", "cgroup_memory_current_bytes"],
    interval_milliseconds: 50,
    server_pid: number("SERVER_PID"),
    recalculated,
    valid: rawSamplesValid,
  },
  server_rss: {
    baseline_bytes: number("BASELINE_RSS"),
    peak_bytes: number("RSS_PEAK"),
    delta_bytes: number("RSS_DELTA"),
    sample_count: number("SAMPLE_COUNT"),
    peak_limit_bytes: number("RSS_PEAK_LIMIT"),
    delta_limit_bytes: number("RSS_DELTA_LIMIT"),
  },
  cgroup_memory_current: {
    sampled_peak_bytes: number("CGROUP_OBSERVED_PEAK"),
    limit_bytes: number("CGROUP_LIMIT"),
  },
  kernel_cgroup_peak: {
    scope: "container_lifetime_context_only",
    authoritative: false,
    before_bytes: number("KERNEL_PEAK_BEFORE"),
    after_bytes: number("KERNEL_PEAK_AFTER"),
    delta_bytes: number("KERNEL_PEAK_AFTER") - number("KERNEL_PEAK_BEFORE"),
    limit_bytes: number("CGROUP_LIMIT"),
  },
  oom: {
    before: number("OOM_BEFORE"),
    after: number("OOM_AFTER"),
    delta: number("OOM_AFTER") - number("OOM_BEFORE"),
  },
  details,
};
const hardAssertionsValid = result.raw_samples.valid
  && result.server_rss.sample_count > 0
  && result.server_rss.peak_bytes <= result.server_rss.peak_limit_bytes
  && result.server_rss.delta_bytes <= result.server_rss.delta_limit_bytes
  && result.cgroup_memory_current.sampled_peak_bytes <= result.cgroup_memory_current.limit_bytes
  && result.oom.delta === 0;
result.authoritative = result.execution_mode === "formal" && hardAssertionsValid;
result.valid = result.status.consistent
  && result.summary.consistent
  && hardAssertionsValid
  && (result.execution_mode !== "formal" || result.authoritative)
  && (result.path !== "migration" || result.details.actual_body_transfer_verified === true);
fs.writeFileSync(process.argv[1], `${JSON.stringify(result, null, 2)}\n`);
' "$MEASUREMENT_DIR/${path_name}-${sample_name}.json"
}

stream_repeated_with_hash() {
    local bytes="$1"
    local seed="$2"
    local sha256_file="$3"
    STREAM_BYTES="$bytes" STREAM_SEED="$seed" GENERATOR_BLOCK_BYTES="$STREAM_BLOCK_BYTES" \
    PATTERN_STRIDE="$BLOCK_PATTERN_STRIDE" SHA256_FILE="$sha256_file" node -e '
const crypto = require("node:crypto");
const fs = require("node:fs");
const total = Number(process.env.STREAM_BYTES);
const blockBytes = Number(process.env.GENERATOR_BLOCK_BYTES);
const seed = Number(process.env.STREAM_SEED);
const stride = Number(process.env.PATTERN_STRIDE);
const sha256 = crypto.createHash("sha256");
let remaining = total;
let blockIndex = 0;
const fillByte = () => ((seed + blockIndex * stride) % 255) + 1;
process.stdout.on("error", (error) => {
  if (error.code === "EPIPE") process.exit(1);
  throw error;
});
function write() {
  while (remaining > 0) {
    const chunkBytes = Math.min(blockBytes, remaining);
    const chunk = Buffer.alloc(chunkBytes, fillByte());
    remaining -= chunkBytes;
    blockIndex += 1;
    sha256.update(chunk);
    if (!process.stdout.write(chunk)) {
      process.stdout.once("drain", write);
      return;
    }
  }
  fs.writeFileSync(process.env.SHA256_FILE, sha256.digest("hex"));
}
write();
'
}

hash_repeated_bytes() {
    local bytes="$1"
    local seed="$2"
    STREAM_BYTES="$bytes" STREAM_SEED="$seed" GENERATOR_BLOCK_BYTES="$STREAM_BLOCK_BYTES" \
    PATTERN_STRIDE="$BLOCK_PATTERN_STRIDE" node -e '
const crypto = require("node:crypto");
const total = Number(process.env.STREAM_BYTES);
const blockBytes = Number(process.env.GENERATOR_BLOCK_BYTES);
const seed = Number(process.env.STREAM_SEED);
const stride = Number(process.env.PATTERN_STRIDE);
const block = Buffer.alloc(Math.min(blockBytes, total));
const sha256 = crypto.createHash("sha256");
let remaining = total;
let blockIndex = 0;
while (remaining > 0) {
  const chunkBytes = Math.min(blockBytes, remaining);
  block.fill(((seed + blockIndex * stride) % 255) + 1);
  sha256.update(block.subarray(0, chunkBytes));
  remaining -= chunkBytes;
  blockIndex += 1;
}
process.stdout.write(sha256.digest("hex"));
'
}

header_status() {
    HEADER_FILE="$1" node -e '
const fs = require("node:fs");
const matches = [...fs.readFileSync(process.env.HEADER_FILE, "utf8").matchAll(/^HTTP\/\S+\s+(\d+)/gm)];
if (matches.length === 0) process.exit(1);
process.stdout.write(matches.at(-1)[1]);
'
}

header_value() {
    HEADER_FILE="$1" HEADER_NAME="$2" node -e '
const fs = require("node:fs");
const prefix = `${process.env.HEADER_NAME.toLowerCase()}:`;
const line = fs.readFileSync(process.env.HEADER_FILE, "utf8")
  .split(/\r?\n/)
  .reverse()
  .find((value) => value.toLowerCase().startsWith(prefix));
if (!line) process.exit(1);
process.stdout.write(line.slice(line.indexOf(":") + 1).trim());
'
}

json_value() {
    JSON_FILE="$1" JSON_PATH="$2" node -e '
const fs = require("node:fs");
let value = JSON.parse(fs.readFileSync(process.env.JSON_FILE, "utf8"));
for (const part of process.env.JSON_PATH.split(".")) value = value?.[part];
if (value === undefined) process.exit(1);
process.stdout.write(typeof value === "string" ? value : JSON.stringify(value));
'
}

json_length() {
    JSON_FILE="$1" JSON_PATH="$2" node -e '
const fs = require("node:fs");
let value = JSON.parse(fs.readFileSync(process.env.JSON_FILE, "utf8"));
for (const part of process.env.JSON_PATH.split(".")) value = value?.[part];
if (!Array.isArray(value)) process.exit(1);
process.stdout.write(String(value.length));
'
}

capture_server_established_snapshot() {
    local output="$1"
    SERVER_PID="$server_pid" BASE_URL="$ACCEPTANCE_BASE_URL" node -e '
const fs = require("node:fs");
const serverPid = Number(process.env.SERVER_PID);
let url;
try {
  url = new URL(process.env.BASE_URL);
} catch {
  console.error("错误：验收服务地址不是有效 URL");
  process.exit(1);
}
const defaultPort = url.protocol === "https:" ? 443 : url.protocol === "http:" ? 80 : NaN;
const localPort = Number(url.port || defaultPort);
const tcpState = "01";
if (!Number.isInteger(serverPid) || serverPid <= 0 || !Number.isInteger(localPort) || localPort <= 0 || localPort > 65535) {
  console.error("错误：验收服务进程或本地端口无效");
  process.exit(1);
}
const sortInodes = (values) => [...values].sort((left, right) => {
  const leftValue = BigInt(left);
  const rightValue = BigInt(right);
  return leftValue < rightValue ? -1 : leftValue > rightValue ? 1 : 0;
});
let fdNames;
try {
  fdNames = fs.readdirSync(`/proc/${serverPid}/fd`);
} catch {
  console.error("错误：无法读取验收服务进程的 /proc fd");
  process.exit(1);
}
const ownedSocketInodes = new Set();
for (const fdName of fdNames) {
  try {
    const target = fs.readlinkSync(`/proc/${serverPid}/fd/${fdName}`);
    const match = /^socket:\[(\d+)\]$/.exec(target);
    if (match) ownedSocketInodes.add(match[1]);
  } catch (error) {
    if (error.code === "ENOENT") continue;
    console.error("错误：无法读取验收服务进程的 socket fd");
    process.exit(1);
  }
}
const procSources = [];
const establishedSocketInodes = new Set();
for (const sourcePath of ["/proc/net/tcp", "/proc/net/tcp6"]) {
  let content;
  try {
    content = fs.readFileSync(sourcePath, "utf8");
  } catch (error) {
    procSources.push({ path: sourcePath, readable: false, error: error.code ?? "UNKNOWN", matched_socket_inodes: [] });
    continue;
  }
  const matchedSocketInodes = [];
  for (const line of content.trim().split("\n").slice(1)) {
    const fields = line.trim().split(/\s+/);
    const localPortHex = fields[1]?.split(":").at(-1);
    const inode = fields[9];
    if (fields[3] !== tcpState || Number.parseInt(localPortHex, 16) !== localPort || !ownedSocketInodes.has(inode)) continue;
    matchedSocketInodes.push(inode);
    establishedSocketInodes.add(inode);
  }
  procSources.push({ path: sourcePath, readable: true, error: null, matched_socket_inodes: sortInodes(matchedSocketInodes) });
}
if (!procSources.some((source) => source.readable)) {
  console.error("错误：/proc/net/tcp 与 /proc/net/tcp6 均不可读");
  process.exit(1);
}
const socketInodes = sortInodes(establishedSocketInodes);
fs.writeFileSync(process.argv[1], `${JSON.stringify({
  observer: "linux_proc_server_owned_established",
  server_pid: serverPid,
  local_port: localPort,
  tcp_state: tcpState,
  proc_sources: procSources,
  owned_socket_inodes: sortInodes(ownedSocketInodes),
  socket_inodes: socketInodes,
  socket_count: socketInodes.length,
}, null, 2)}\n`);
' "$output" || fail "无法采集验收服务进程拥有的 ESTABLISHED 入站连接"
}

api_post_json() {
    local url="$1" body="$2" output="$3" expected="$4"
    local status
    status="$(curl --silent --show-error --output "$output" --write-out '%{http_code}' \
        -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
        --request POST --data "$body" "$url")"
    assert_equal "$expected" "$status" "迁移 API 请求失败：$url"
}

api_get_json() {
    local url="$1" output="$2"
    curl --fail --silent --show-error -H "Authorization: Bearer $token" --output "$output" "$url"
}

upstream_reset() {
    curl --fail --silent --show-error --request POST --output /dev/null \
        "$ACCEPTANCE_UPSTREAM_URL/__acceptance/reset"
}

upstream_artifact_hold() {
    local state_file="$ACCEPTANCE_RUNTIME_DIR/upstream-artifact-hold.json"
    singleflight_hold_active=1
    curl --fail --silent --show-error --request POST --output "$state_file" \
        "$ACCEPTANCE_UPSTREAM_URL/__acceptance/artifact/hold"
    assert_equal true "$(json_value "$state_file" artifact_hold)" "受控上游未进入单飞 hold 状态"
}

upstream_artifact_release() {
    local state_file="$ACCEPTANCE_RUNTIME_DIR/upstream-artifact-release.json"
    curl --fail --silent --show-error --request POST --output "$state_file" \
        "$ACCEPTANCE_UPSTREAM_URL/__acceptance/artifact/release"
    assert_equal false "$(json_value "$state_file" artifact_hold)" "受控上游未解除单飞 hold 状态"
    singleflight_hold_active=0
}

upstream_method_count() {
    local method="$1"
    local path="$2"
    local state_file="$ACCEPTANCE_RUNTIME_DIR/upstream-state.json"
    curl --fail --silent --show-error --output "$state_file" "$ACCEPTANCE_UPSTREAM_URL/__acceptance/state"
    STATE_FILE="$state_file" COUNT_KEY="$method $path" node -e '
const fs = require("node:fs");
const state = JSON.parse(fs.readFileSync(process.env.STATE_FILE, "utf8"));
process.stdout.write(String(state.method_counts[process.env.COUNT_KEY] ?? 0));
'
}

create_repository() {
    local payload="$1"
    local output="$2"
    local status
    status="$(curl --silent --show-error --output "$output" --write-out '%{http_code}' \
        -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
        --data "$payload" "$ACCEPTANCE_BASE_URL/api/v1/repos")"
    assert_equal 201 "$status" "创建性能验收仓库失败"
}

run_singleflight() {
    local response_dir="$ACCEPTANCE_RUNTIME_DIR/singleflight-responses"
    local ready_dir="$ACCEPTANCE_RUNTIME_DIR/singleflight-ready"
    local held_samples_dir="$ACCEPTANCE_RUNTIME_DIR/singleflight-proc-held-samples"
    local start_marker="$ACCEPTANCE_RUNTIME_DIR/start-singleflight"
    local baseline_snapshot="$ACCEPTANCE_RUNTIME_DIR/singleflight-proc-baseline.json"
    local held_snapshot="$ACCEPTANCE_RUNTIME_DIR/singleflight-proc-held-candidate.json"
    local pre_release_state="$ACCEPTANCE_RUNTIME_DIR/singleflight-upstream-pre-release.json"
    local upstream_before upstream_held upstream_after upstream_delta artifact_hold artifact_waiting
    local request_failed=0 started attempts new_count consecutive_successful_samples=0
    local request_index request_pid
    singleflight_barrier_evidence_file="$ACCEPTANCE_RUNTIME_DIR/singleflight-connection-barrier.json"
    singleflight_pre_release_upstream_file="$ACCEPTANCE_RUNTIME_DIR/singleflight-pre-release-upstream.json"
    singleflight_response_evidence_file="$ACCEPTANCE_RUNTIME_DIR/singleflight-response-evidence.json"
    rm -rf "$response_dir" "$ready_dir" "$held_samples_dir" "$start_marker" "$baseline_snapshot" \
        "$held_snapshot" "$pre_release_state" "$singleflight_barrier_evidence_file" \
        "$singleflight_pre_release_upstream_file" "$singleflight_response_evidence_file"
    mkdir -p "$response_dir" "$ready_dir" "$held_samples_dir"
    singleflight_pids=()
    upstream_reset
    upstream_artifact_hold
    upstream_before="$(upstream_method_count GET '/raw/artifact.txt')"
    started=$SECONDS
    for ((request_index = 1; request_index <= CONCURRENCY; request_index++)); do
        (
            : > "$ready_dir/$request_index"
            while [ ! -f "$start_marker" ]; do sleep 0.01; done
            curl --fail --silent --show-error --output "$response_dir/$request_index.bin" \
                "$ACCEPTANCE_BASE_URL/repository/performance-proxy/artifact.txt"
        ) &
        singleflight_pids+=("$!")
    done
    for ((request_index = 1; request_index <= CONCURRENCY; request_index++)); do
        attempts=500
        while [ ! -f "$ready_dir/$request_index" ]; do
            attempts=$((attempts - 1))
            [ "$attempts" -gt 0 ] || fail "第 $request_index 个单飞请求未进入 ready 状态"
            sleep 0.01
        done
    done
    singleflight_ready_count="$CONCURRENCY"
    capture_server_established_snapshot "$baseline_snapshot"
    : > "$start_marker"
    attempts=500
    while true; do
        upstream_held="$(upstream_method_count GET '/raw/artifact.txt')"
        [ "$upstream_held" -gt "$upstream_before" ] && break
        attempts=$((attempts - 1))
        [ "$attempts" -gt 0 ] || fail "单飞请求未在 hold 状态下到达受控上游"
        sleep 0.01
    done
    attempts=500
    while true; do
        capture_server_established_snapshot "$held_snapshot"
        new_count="$(BASELINE_SNAPSHOT="$baseline_snapshot" HELD_SNAPSHOT="$held_snapshot" node -e '
const fs = require("node:fs");
const baseline = new Set(JSON.parse(fs.readFileSync(process.env.BASELINE_SNAPSHOT, "utf8")).socket_inodes);
const held = JSON.parse(fs.readFileSync(process.env.HELD_SNAPSHOT, "utf8")).socket_inodes;
process.stdout.write(String(held.filter((inode) => !baseline.has(inode)).length));
')" || fail "无法计算服务进程新 ESTABLISHED 入站连接数"
        [[ "$new_count" =~ ^[0-9]+$ ]] || fail "服务进程新 ESTABLISHED 入站连接数不是有效数值"
        if [ "$new_count" -ge "$CONCURRENCY" ]; then
            consecutive_successful_samples=$((consecutive_successful_samples + 1))
            mv "$held_snapshot" "$held_samples_dir/$consecutive_successful_samples.json"
            [ "$consecutive_successful_samples" -ge "$SINGLEFLIGHT_REQUIRED_CONSECUTIVE_SAMPLES" ] && break
        else
            consecutive_successful_samples=0
            rm -f "$held_samples_dir"/*.json
        fi
        attempts=$((attempts - 1))
        [ "$attempts" -gt 0 ] || fail "服务进程未形成 32 个连续稳定的新 ESTABLISHED 入站连接"
        sleep 0.01
    done
    curl --fail --silent --show-error --output "$pre_release_state" "$ACCEPTANCE_UPSTREAM_URL/__acceptance/state"
    upstream_held="$(STATE_FILE="$pre_release_state" COUNT_KEY='GET /raw/artifact.txt' node -e '
const fs = require("node:fs");
const state = JSON.parse(fs.readFileSync(process.env.STATE_FILE, "utf8"));
process.stdout.write(String(state.method_counts[process.env.COUNT_KEY] ?? 0));
')"
    upstream_delta=$((upstream_held - upstream_before))
    artifact_hold="$(json_value "$pre_release_state" artifact_hold)"
    artifact_waiting="$(json_value "$pre_release_state" artifact_waiting)"
    assert_equal true "$artifact_hold" "释放前受控上游已不在 artifact hold 状态"
    assert_equal 1 "$artifact_waiting" "释放前受控上游 artifact 等待请求数不正确"
    assert_equal 1 "$upstream_delta" "释放前单飞 GET 次数不正确"
    BASELINE_SNAPSHOT="$baseline_snapshot" HELD_SAMPLES_DIR="$held_samples_dir" \
    REQUIRED_NEW_COUNT="$CONCURRENCY" REQUIRED_CONSECUTIVE_SAMPLES="$SINGLEFLIGHT_REQUIRED_CONSECUTIVE_SAMPLES" \
    CONSECUTIVE_SUCCESSFUL_SAMPLES="$consecutive_successful_samples" node -e '
const fs = require("node:fs");
const path = require("node:path");
const baselineSnapshot = JSON.parse(fs.readFileSync(process.env.BASELINE_SNAPSHOT, "utf8"));
const baselineSet = new Set(baselineSnapshot.socket_inodes);
const consecutiveSuccessfulSamples = Number(process.env.CONSECUTIVE_SUCCESSFUL_SAMPLES);
const samples = Array.from({ length: consecutiveSuccessfulSamples }, (_, index) => {
  const snapshot = JSON.parse(fs.readFileSync(path.join(process.env.HELD_SAMPLES_DIR, `${index + 1}.json`), "utf8"));
  const newSocketInodes = snapshot.socket_inodes.filter((inode) => !baselineSet.has(inode));
  return {
    proc_sources: snapshot.proc_sources,
    owned_socket_inodes: snapshot.owned_socket_inodes,
    socket_inodes: snapshot.socket_inodes,
    new: { socket_inodes: newSocketInodes, count: newSocketInodes.length },
    count: snapshot.socket_count,
  };
});
fs.writeFileSync(process.argv[1], `${JSON.stringify({
  observer: baselineSnapshot.observer,
  server_pid: baselineSnapshot.server_pid,
  local_port: baselineSnapshot.local_port,
  tcp_state: baselineSnapshot.tcp_state,
  baseline: {
    proc_sources: baselineSnapshot.proc_sources,
    owned_socket_inodes: baselineSnapshot.owned_socket_inodes,
    socket_inodes: baselineSnapshot.socket_inodes,
    count: baselineSnapshot.socket_count,
  },
  samples,
  required_new_count: Number(process.env.REQUIRED_NEW_COUNT),
  required_consecutive_samples: Number(process.env.REQUIRED_CONSECUTIVE_SAMPLES),
  consecutive_successful_samples: consecutiveSuccessfulSamples,
}, null, 2)}\n`);
' "$singleflight_barrier_evidence_file"
    assert_equal "$SINGLEFLIGHT_REQUIRED_CONSECUTIVE_SAMPLES" \
        "$(json_length "$singleflight_barrier_evidence_file" samples)" \
        "服务进程 ESTABLISHED 入站连接屏障样本数量不正确"
    UPSTREAM_BEFORE="$upstream_before" UPSTREAM_HELD="$upstream_held" UPSTREAM_DELTA="$upstream_delta" \
    ARTIFACT_HOLD="$artifact_hold" ARTIFACT_WAITING="$artifact_waiting" node -e '
const fs = require("node:fs");
const number = (name) => Number(process.env[name]);
fs.writeFileSync(process.argv[1], `${JSON.stringify({
  before: number("UPSTREAM_BEFORE"),
  held: number("UPSTREAM_HELD"),
  delta: number("UPSTREAM_DELTA"),
  artifact_hold: process.env.ARTIFACT_HOLD === "true",
  artifact_waiting: number("ARTIFACT_WAITING"),
}, null, 2)}\n`);
' "$singleflight_pre_release_upstream_file"
    upstream_artifact_release
    for request_pid in "${singleflight_pids[@]}"; do
        if ! wait "$request_pid"; then request_failed=1; fi
    done
    singleflight_pids=()
    assert_equal 0 "$request_failed" "32 并发 proxy 请求存在失败"
    upstream_after="$(upstream_method_count GET '/raw/artifact.txt')"
    singleflight_seconds=$((SECONDS - started))
    singleflight_upstream_before="$upstream_before"
    singleflight_upstream_after="$upstream_after"
    singleflight_upstream_delta=$((upstream_after - upstream_before))
    assert_equal 1 "$singleflight_upstream_delta" "32 并发单飞的有效上游 GET 次数不正确"
    for ((request_index = 1; request_index <= CONCURRENCY; request_index++)); do
        cmp -s "$ACCEPTANCE_FIXTURE_DIR/raw/artifact.txt" "$response_dir/$request_index.bin" \
            || fail "第 $request_index 个并发响应字节不一致"
    done
    FIXTURE_FILE="$ACCEPTANCE_FIXTURE_DIR/raw/artifact.txt" RESPONSE_DIR="$response_dir" \
    RESPONSE_COUNT="$CONCURRENCY" node -e '
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const describe = (file) => {
  const content = fs.readFileSync(file);
  return { sha256: crypto.createHash("sha256").update(content).digest("hex"), bytes: content.length };
};
const responses = Array.from({ length: Number(process.env.RESPONSE_COUNT) }, (_, index) => ({
  index: index + 1,
  ...describe(path.join(process.env.RESPONSE_DIR, `${index + 1}.bin`)),
}));
fs.writeFileSync(process.argv[1], `${JSON.stringify({
  fixture: describe(process.env.FIXTURE_FILE),
  responses,
}, null, 2)}\n`);
' "$singleflight_response_evidence_file"
}

run_upload() {
    local sample_name="$1" bytes="$2"
    local sha256_file="$ACCEPTANCE_RUNTIME_DIR/upload-${sample_name}.sha256"
    local headers="$ACCEPTANCE_RUNTIME_DIR/upload-${sample_name}.headers"
    local details="$ACCEPTANCE_RUNTIME_DIR/upload-${sample_name}-details.json"
    local started duration upload_status head_status actual_bytes actual_sha256
    start_measurement upload "$sample_name"
    started=$SECONDS
    upload_status="$(stream_repeated_with_hash "$bytes" "$HOSTED_SEED" "$sha256_file" \
        | curl --silent --show-error --output /dev/null --write-out '%{http_code}' \
            -H "Authorization: Bearer $token" --request PUT --upload-file - \
            "$ACCEPTANCE_BASE_URL/repository/performance-hosted/large-${sample_name}.bin")"
    curl --fail --silent --show-error --head --dump-header "$headers" --output /dev/null \
        "$ACCEPTANCE_BASE_URL/repository/performance-hosted/large-${sample_name}.bin"
    duration=$((SECONDS - started))
    finish_measurement upload "$sample_name"
    expected_sha256[$sample_name]="$(<"$sha256_file")"
    head_status="$(header_status "$headers")"
    actual_bytes="$(header_value "$headers" content-length)"
    actual_sha256="$(header_value "$headers" x-checksum-sha256)"
    assert_equal 201 "$upload_status" "$sample_name hosted 上传状态不正确"
    assert_equal 200 "$head_status" "$sample_name hosted HEAD 状态不正确"
    assert_equal "$bytes" "$actual_bytes" "$sample_name hosted 字节数不正确"
    assert_equal "${expected_sha256[$sample_name]}" "$actual_sha256" "$sample_name hosted 摘要不正确"
    UPLOAD_STATUS="$upload_status" HEAD_STATUS="$head_status" CONTENT_LENGTH="$actual_bytes" CONTENT_SEED="$HOSTED_SEED" \
    node -e 'const fs=require("node:fs"); fs.writeFileSync(process.argv[1], JSON.stringify({upload_http_status:Number(process.env.UPLOAD_STATUS),head_http_status:Number(process.env.HEAD_STATUS),reported_content_length:Number(process.env.CONTENT_LENGTH),content_seed:Number(process.env.CONTENT_SEED)},null,2));' "$details"
    write_measurement upload "$sample_name" "$bytes" "$duration" 201 "$upload_status" sha256 \
        "${expected_sha256[$sample_name]}" "$actual_sha256" "$details"
}

run_download() {
    local sample_name="$1" bytes="$2"
    local headers="$ACCEPTANCE_RUNTIME_DIR/download-${sample_name}.headers"
    local details="$ACCEPTANCE_RUNTIME_DIR/download-${sample_name}-details.json"
    local started duration status actual_sha256
    start_measurement download "$sample_name"
    started=$SECONDS
    actual_sha256="$(curl --fail --silent --show-error --dump-header "$headers" \
        "$ACCEPTANCE_BASE_URL/repository/performance-hosted/large-${sample_name}.bin" \
        | sha256sum | cut -d ' ' -f 1)"
    duration=$((SECONDS - started))
    finish_measurement download "$sample_name"
    status="$(header_status "$headers")"
    assert_equal 200 "$status" "$sample_name hosted 下载状态不正确"
    assert_equal "${expected_sha256[$sample_name]}" "$actual_sha256" "$sample_name hosted 下载摘要不一致"
    DOWNLOAD_STATUS="$status" node -e 'const fs=require("node:fs"); fs.writeFileSync(process.argv[1], JSON.stringify({http_status:Number(process.env.DOWNLOAD_STATUS)},null,2));' "$details"
    write_measurement download "$sample_name" "$bytes" "$duration" 200 "$status" sha256 \
        "${expected_sha256[$sample_name]}" "$actual_sha256" "$details"
}

run_proxy() {
    local sample_name="$1" bytes="$2"
    local path="/raw/large-${sample_name}.bin"
    local headers="$ACCEPTANCE_RUNTIME_DIR/proxy-${sample_name}.headers"
    local details="$ACCEPTANCE_RUNTIME_DIR/proxy-${sample_name}-details.json"
    local before after delta started duration status actual_sha256 reported_sha256
    upstream_reset
    before="$(upstream_method_count GET "$path")"
    start_measurement proxy "$sample_name"
    started=$SECONDS
    actual_sha256="$(curl --fail --silent --show-error --dump-header "$headers" \
        "$ACCEPTANCE_BASE_URL/repository/performance-proxy/large-${sample_name}.bin" \
        | sha256sum | cut -d ' ' -f 1)"
    duration=$((SECONDS - started))
    finish_measurement proxy "$sample_name"
    after="$(upstream_method_count GET "$path")"
    delta=$((after - before))
    status="$(header_status "$headers")"
    reported_sha256="$(header_value "$headers" x-checksum-sha256)"
    assert_equal 200 "$status" "$sample_name proxy 回源状态不正确"
    assert_equal 1 "$delta" "$sample_name proxy GET 上游命中次数不正确"
    assert_equal "${proxy_sha256[$sample_name]}" "$reported_sha256" "$sample_name proxy 服务端摘要不符合索引化内容"
    assert_equal "${proxy_sha256[$sample_name]}" "$actual_sha256" "$sample_name proxy 回源摘要不一致"
    [ "$reported_sha256" != "${expected_sha256[$sample_name]}" ] || fail "$sample_name proxy 与 hosted 内容摘要不应相同"
    [ "$reported_sha256" != "${migration_sha256[$sample_name]}" ] || fail "$sample_name proxy 与 migration 内容摘要不应相同"
    PROXY_STATUS="$status" UPSTREAM_BEFORE="$before" UPSTREAM_AFTER="$after" UPSTREAM_DELTA="$delta" CONTENT_SEED="$PROXY_SEED" \
    node -e 'const fs=require("node:fs"); fs.writeFileSync(process.argv[1], JSON.stringify({http_status:Number(process.env.PROXY_STATUS),content_seed:Number(process.env.CONTENT_SEED),upstream_method:"GET",upstream_before:Number(process.env.UPSTREAM_BEFORE),upstream_after:Number(process.env.UPSTREAM_AFTER),upstream_delta:Number(process.env.UPSTREAM_DELTA)},null,2));' "$details"
    write_measurement proxy "$sample_name" "$bytes" "$duration" 200 "$status" sha256 \
        "${proxy_sha256[$sample_name]}" "$actual_sha256" "$details"
}

configure_migration_upstream() {
    local configure_url status
    printf '==> 计算 proxy 与迁移索引化非零块摘要\n'
    proxy_sha256[1g]="$(hash_repeated_bytes "$ONE_GIB_BYTES" "$PROXY_SEED")"
    proxy_sha256[5g]="$(hash_repeated_bytes "$FIVE_GIB_BYTES" "$PROXY_SEED")"
    migration_sha256[1g]="$(hash_repeated_bytes "$ONE_GIB_BYTES" "$MIGRATION_SEED")"
    migration_sha256[5g]="$(hash_repeated_bytes "$FIVE_GIB_BYTES" "$MIGRATION_SEED")"
    [ "${proxy_sha256[1g]}" != "${expected_sha256[1g]}" ] || fail "1g proxy 与 hosted 内容摘要不应相同"
    [ "${proxy_sha256[5g]}" != "${expected_sha256[5g]}" ] || fail "5g proxy 与 hosted 内容摘要不应相同"
    [ "${migration_sha256[1g]}" != "${expected_sha256[1g]}" ] || fail "1g migration 与 hosted 内容摘要不应相同"
    [ "${migration_sha256[5g]}" != "${expected_sha256[5g]}" ] || fail "5g migration 与 hosted 内容摘要不应相同"
    [ "${migration_sha256[1g]}" != "${proxy_sha256[1g]}" ] || fail "1g migration 与 proxy 内容摘要不应相同"
    [ "${migration_sha256[5g]}" != "${proxy_sha256[5g]}" ] || fail "5g migration 与 proxy 内容摘要不应相同"
    configure_url="$(UPSTREAM_URL="$ACCEPTANCE_UPSTREAM_URL" ONE_BYTES="$ONE_GIB_BYTES" FIVE_BYTES="$FIVE_GIB_BYTES" \
        ONE_SHA256="${migration_sha256[1g]}" FIVE_SHA256="${migration_sha256[5g]}" node -e '
const url = new URL("/__acceptance/configure-large", process.env.UPSTREAM_URL);
for (const [name, value] of Object.entries({one_bytes:process.env.ONE_BYTES,five_bytes:process.env.FIVE_BYTES,one_sha256:process.env.ONE_SHA256,five_sha256:process.env.FIVE_SHA256})) url.searchParams.set(name.replace("one", "1g").replace("five", "5g"), value);
process.stdout.write(url.toString());
')"
    status="$(curl --silent --show-error --request POST --output "$ACCEPTANCE_RUNTIME_DIR/upstream-large-config.json" --write-out '%{http_code}' "$configure_url")"
    assert_equal 200 "$status" "配置受控迁移上游失败"
}

wait_for_task() {
    local task_id="$1" expected_status="$2" expected_phase="$3" output="$4"
    local attempt status phase
    for ((attempt = 1; attempt <= MIGRATION_POLL_ATTEMPTS; attempt++)); do
        api_get_json "$ACCEPTANCE_BASE_URL/api/v1/migration/tasks/$task_id" "$output"
        status="$(json_value "$output" status)"
        phase="$(json_value "$output" phase)"
        if [ "$status" = "$expected_status" ] && [ "$phase" = "$expected_phase" ]; then
            return
        fi
        case "$status" in
            failed|partial|cancelled|interrupted) fail "迁移任务 $task_id 异常结束为 $status/$phase" ;;
        esac
        sleep 0.2
    done
    fail "迁移任务 $task_id 未在限定轮询次数内到达 $expected_status/$expected_phase"
}

run_migration() {
    local sample_name="$1" bytes="$2"
    local source_repo="performance-migration-source-${sample_name}"
    local target_repo="performance-migration-${sample_name}"
    local source_url="$ACCEPTANCE_UPSTREAM_URL/migration-${sample_name}"
    local large_path="/migration-${sample_name}/repository/${source_repo}/large-${sample_name}.bin"
    local discover_file="$ACCEPTANCE_RUNTIME_DIR/migration-${sample_name}-discover.json"
    local plan_file="$ACCEPTANCE_RUNTIME_DIR/migration-${sample_name}-plan.json"
    local task_file="$ACCEPTANCE_RUNTIME_DIR/migration-${sample_name}-task.json"
    local report_file="$ACCEPTANCE_RUNTIME_DIR/migration-${sample_name}-report.json"
    local details="$ACCEPTANCE_RUNTIME_DIR/migration-${sample_name}-details.json"
    local headers="$ACCEPTANCE_RUNTIME_DIR/migration-${sample_name}-download.headers"
    local discover_body plan_body discovery_id plan_id task_id started duration
    local initial_status initial_phase initial_done_bytes final_status final_phase
    local before after_initial after_final upstream_delta finalize_status actual_sha256 download_status
    local actual_body_transfer_verified=false
    local report_done report_total report_failed report_migrated report_skipped validation_passed validation_failed failure_count
    upstream_reset
    before="$(upstream_method_count GET "$large_path")"
    discover_body="$(SOURCE_URL="$source_url" node -e 'process.stdout.write(JSON.stringify({kind:"online",url:process.env.SOURCE_URL}))')"
    api_post_json "$ACCEPTANCE_BASE_URL/api/v1/migration/discover" "$discover_body" "$discover_file" 200
    discovery_id="$(json_value "$discover_file" discovery_id)"
    assert_equal "$source_repo" "$(json_value "$discover_file" repositories.0.name)" "$sample_name 迁移发现仓库不正确"
    assert_equal 1 "$(json_value "$discover_file" repositories.0.object_count)" "$sample_name 迁移发现对象数不正确"
    assert_equal "$bytes" "$(json_value "$discover_file" repositories.0.total_bytes)" "$sample_name 迁移发现字节数不正确"
    plan_body="$(DISCOVERY_ID="$discovery_id" SOURCE_REPO="$source_repo" TARGET_REPO="$target_repo" node -e 'process.stdout.write(JSON.stringify({discovery_id:process.env.DISCOVERY_ID,mappings:[{source:process.env.SOURCE_REPO,target:process.env.TARGET_REPO,excluded:false,upstream_credential_ref:null,manual_upstream_url:null,manual_members:null,conflict_decisions:[]}]}))')"
    api_post_json "$ACCEPTANCE_BASE_URL/api/v1/migration/plan" "$plan_body" "$plan_file" 200
    plan_id="$(json_value "$plan_file" plan_id)"
    assert_equal false "$(json_value "$plan_file" blocked)" "$sample_name 迁移计划不应阻断"
    assert_equal 1 "$(json_value "$plan_file" object_count)" "$sample_name 迁移计划对象数不正确"
    assert_equal "$bytes" "$(json_value "$plan_file" total_bytes)" "$sample_name 迁移计划字节数不正确"
    start_measurement migration "$sample_name"
    started=$SECONDS
    task_status="$(curl --silent --show-error --output "$task_file" --write-out '%{http_code}' \
        -H "Authorization: Bearer $token" --request POST \
        "$ACCEPTANCE_BASE_URL/api/v1/migration/tasks?plan_id=$plan_id")"
    assert_equal 202 "$task_status" "$sample_name 创建迁移任务失败"
    task_id="$(json_value "$task_file" task_id)"
    wait_for_task "$task_id" paused awaiting-cutover "$task_file"
    initial_status="$(json_value "$task_file" status)"
    initial_phase="$(json_value "$task_file" phase)"
    initial_done_bytes="$(json_value "$task_file" done_bytes)"
    assert_equal "$bytes" "$initial_done_bytes" "$sample_name 迁移首轮完成字节数不正确"
    after_initial="$(upstream_method_count GET "$large_path")"
    upstream_delta=$((after_initial - before))
    assert_equal 1 "$upstream_delta" "$sample_name 迁移搬运的大对象 GET 次数不正确"
    finalize_status="$(curl --silent --show-error --output "$task_file" --write-out '%{http_code}' \
        -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
        --request POST --data '{}' "$ACCEPTANCE_BASE_URL/api/v1/migration/tasks/$task_id/finalize")"
    assert_equal 202 "$finalize_status" "$sample_name 迁移收口请求失败"
    wait_for_task "$task_id" succeeded completed "$task_file"
    final_status="$(json_value "$task_file" status)"
    final_phase="$(json_value "$task_file" phase)"
    after_final="$(upstream_method_count GET "$large_path")"
    assert_equal "$after_initial" "$after_final" "$sample_name 迁移收口不应重复 GET 一致对象"
    api_get_json "$ACCEPTANCE_BASE_URL/api/v1/migration/tasks/$task_id/report" "$report_file"
    report_done="$(json_value "$report_file" repositories.0.done_bytes)"
    report_total="$(json_value "$report_file" repositories.0.total_bytes)"
    report_failed="$(json_value "$report_file" repositories.0.failed)"
    report_migrated="$(json_value "$report_file" repositories.0.migrated)"
    report_skipped="$(json_value "$report_file" repositories.0.skipped)"
    validation_passed="$(json_value "$report_file" validation.blob.passed)"
    validation_failed="$(json_value "$report_file" validation.blob.failed)"
    failure_count="$(json_length "$report_file" failures)"
    assert_equal "$bytes" "$report_done" "$sample_name 迁移报告完成字节数不正确"
    assert_equal "$bytes" "$report_total" "$sample_name 迁移报告总字节数不正确"
    assert_equal 0 "$report_failed" "$sample_name 迁移报告存在失败对象"
    assert_equal 1 "$((report_migrated + report_skipped))" "$sample_name 迁移报告对象结果计数不正确"
    assert_equal 1 "$validation_passed" "$sample_name 迁移 blob 校验通过数不正确"
    assert_equal 0 "$validation_failed" "$sample_name 迁移 blob 校验失败数不正确"
    assert_equal 0 "$failure_count" "$sample_name 迁移报告存在失败明细"
    actual_sha256="$(curl --fail --silent --show-error --dump-header "$headers" \
        "$ACCEPTANCE_BASE_URL/repository/${target_repo}/large-${sample_name}.bin" \
        | sha256sum | cut -d ' ' -f 1)"
    download_status="$(header_status "$headers")"
    assert_equal 200 "$download_status" "$sample_name 迁移目标下载状态不正确"
    assert_equal "${migration_sha256[$sample_name]}" "$actual_sha256" "$sample_name 迁移目标摘要不一致"
    [ "$actual_sha256" != "${expected_sha256[$sample_name]}" ] || fail "$sample_name migration 与 hosted 内容摘要不应相同"
    [ "$actual_sha256" != "${proxy_sha256[$sample_name]}" ] || fail "$sample_name migration 与 proxy 内容摘要不应相同"
    if [ "$upstream_delta" -eq 1 ] && [ "$initial_done_bytes" = "$bytes" ] \
        && [ "$actual_sha256" = "${migration_sha256[$sample_name]}" ]; then
        actual_body_transfer_verified=true
    fi
    assert_equal true "$actual_body_transfer_verified" "$sample_name 迁移真实 body 搬运证据不完整"
    duration=$((SECONDS - started))
    finish_measurement migration "$sample_name"
    TASK_CREATE_STATUS="$task_status" INITIAL_STATUS="$initial_status" INITIAL_PHASE="$initial_phase" \
    INITIAL_DONE_BYTES="$initial_done_bytes" FINALIZE_STATUS="$finalize_status" FINAL_STATUS="$final_status" \
    FINAL_PHASE="$final_phase" REPORT_TOTAL="$report_total" REPORT_DONE="$report_done" REPORT_FAILED="$report_failed" \
    REPORT_MIGRATED="$report_migrated" REPORT_SKIPPED="$report_skipped" VALIDATION_PASSED="$validation_passed" \
    VALIDATION_FAILED="$validation_failed" UPSTREAM_BEFORE="$before" UPSTREAM_AFTER="$after_final" \
    UPSTREAM_DELTA="$upstream_delta" TARGET_DOWNLOAD_STATUS="$download_status" CONTENT_SEED="$MIGRATION_SEED" \
    ACTUAL_BODY_TRANSFER_VERIFIED="$actual_body_transfer_verified" node -e '
const fs=require("node:fs");
const number=(name)=>Number(process.env[name]);
fs.writeFileSync(process.argv[1], JSON.stringify({
  source_kind:"online",
  content_seed:number("CONTENT_SEED"),
  task_create_http_status:number("TASK_CREATE_STATUS"),
  initial_transfer:{status:process.env.INITIAL_STATUS,phase:process.env.INITIAL_PHASE,done_bytes:number("INITIAL_DONE_BYTES")},
  finalize_http_status:number("FINALIZE_STATUS"),
  final:{status:process.env.FINAL_STATUS,phase:process.env.FINAL_PHASE},
  report:{total_bytes:number("REPORT_TOTAL"),done_bytes:number("REPORT_DONE"),failed:number("REPORT_FAILED"),migrated:number("REPORT_MIGRATED"),skipped:number("REPORT_SKIPPED"),validation_passed:number("VALIDATION_PASSED"),validation_failed:number("VALIDATION_FAILED")},
  upstream:{method:"GET",before:number("UPSTREAM_BEFORE"),after:number("UPSTREAM_AFTER"),delta:number("UPSTREAM_DELTA")},
  target_download_http_status:number("TARGET_DOWNLOAD_STATUS"),
  actual_body_transfer_verified:process.env.ACTUAL_BODY_TRANSFER_VERIFIED === "true",
},null,2));
' "$details"
    write_measurement migration "$sample_name" "$bytes" "$duration" succeeded "$final_status" sha256 \
        "${migration_sha256[$sample_name]}" "$actual_sha256" "$details"
}

write_result() {
    local cgroup_peak_after="$1" oom_after="$2"
    RESULT_MEASUREMENT_DIR="$MEASUREMENT_DIR" RESULT_SAMPLE_FILE="$SAMPLE_FILE" RESULT_FILE="$EVIDENCE_DIR/result.json" \
    RESULT_ONE_GIB_BYTES="$ONE_GIB_BYTES" RESULT_FIVE_GIB_BYTES="$FIVE_GIB_BYTES" \
    RESULT_FORMAL_ONE_GIB_BYTES="$FORMAL_ONE_GIB_BYTES" RESULT_FORMAL_FIVE_GIB_BYTES="$FORMAL_FIVE_GIB_BYTES" \
    RESULT_EXECUTION_MODE="$execution_mode" RESULT_RELEASE_BINARY="$RELEASE_BINARY" \
    HOSTED_CONTENT_SEED="$HOSTED_SEED" PROXY_CONTENT_SEED="$PROXY_SEED" MIGRATION_CONTENT_SEED="$MIGRATION_SEED" \
    CONTENT_BLOCK_BYTES="$STREAM_BLOCK_BYTES" CONTENT_PATTERN_STRIDE="$BLOCK_PATTERN_STRIDE" \
    CGROUP_LIMIT="$cgroup_limit" CGROUP_CURRENT_BEFORE="$cgroup_current_before" CGROUP_CURRENT_AFTER="$cgroup_current_after" \
    CGROUP_PEAK_BEFORE="$cgroup_peak_before" CGROUP_PEAK_AFTER="$cgroup_peak_after" CGROUP_PEAK_WRITABLE="$cgroup_peak_writable" \
    OOM_BEFORE="$oom_before" OOM_AFTER="$oom_after" RSS_PEAK_LIMIT="$RSS_PEAK_LIMIT_BYTES" RSS_DELTA_LIMIT="$RSS_DELTA_LIMIT_BYTES" \
    SINGLEFLIGHT_SECONDS="$singleflight_seconds" SINGLEFLIGHT_READY_COUNT="$singleflight_ready_count" \
    UPSTREAM_BEFORE="$singleflight_upstream_before" UPSTREAM_AFTER="$singleflight_upstream_after" \
    UPSTREAM_DELTA="$singleflight_upstream_delta" SINGLEFLIGHT_BARRIER_FILE="$singleflight_barrier_evidence_file" \
    SINGLEFLIGHT_PRE_RELEASE_UPSTREAM_FILE="$singleflight_pre_release_upstream_file" \
    SINGLEFLIGHT_RESPONSE_FILE="$singleflight_response_evidence_file" node -e '
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const number = (name) => Number(process.env[name]);
const read = (name, sample) => JSON.parse(fs.readFileSync(path.join(process.env.RESULT_MEASUREMENT_DIR, `${name}-${sample}.json`), "utf8"));
const compare = (one, five) => {
  const objectSizeRatio = five.bytes / one.bytes;
  const peakGrowthRatio = five.server_rss.peak_bytes / one.server_rss.peak_bytes;
  const deltaGrowthRatio = one.server_rss.delta_bytes === 0 ? null : five.server_rss.delta_bytes / one.server_rss.delta_bytes;
  const cgroupCurrentGrowthRatio = one.cgroup_memory_current.sampled_peak_bytes === 0
    ? null
    : five.cgroup_memory_current.sampled_peak_bytes / one.cgroup_memory_current.sampled_peak_bytes;
  const verified = objectSizeRatio > 1
    && peakGrowthRatio < objectSizeRatio
    && one.valid
    && five.valid;
  return {
    object_size_ratio: objectSizeRatio,
    server_rss_peak_difference_bytes: five.server_rss.peak_bytes - one.server_rss.peak_bytes,
    server_rss_delta_difference_bytes: five.server_rss.delta_bytes - one.server_rss.delta_bytes,
    sampled_cgroup_current_peak_difference_bytes: five.cgroup_memory_current.sampled_peak_bytes - one.cgroup_memory_current.sampled_peak_bytes,
    server_rss_peak_growth_ratio: peakGrowthRatio,
    server_rss_delta_growth_ratio: deltaGrowthRatio,
    sampled_cgroup_current_peak_growth_ratio: cgroupCurrentGrowthRatio,
    non_linear_growth_verified: verified,
  };
};
const paths = {};
for (const name of ["upload", "download", "proxy", "migration"]) {
  const one = read(name, "1g");
  const five = read(name, "5g");
  paths[name] = { samples: { "1g": one, "5g": five }, comparison: compare(one, five) };
}
const singleflightBarrier = JSON.parse(fs.readFileSync(process.env.SINGLEFLIGHT_BARRIER_FILE, "utf8"));
const preReleaseUpstream = JSON.parse(fs.readFileSync(process.env.SINGLEFLIGHT_PRE_RELEASE_UPSTREAM_FILE, "utf8"));
const responseEvidence = JSON.parse(fs.readFileSync(process.env.SINGLEFLIGHT_RESPONSE_FILE, "utf8"));
const aggregateSamples = fs.readFileSync(process.env.RESULT_SAMPLE_FILE);
const evidence = {
  specifications: ["NFR-01", "NFR-02", "NFR-03", "NFR-06", "NFR-08"],
  sample_configuration: {
    execution_mode: process.env.RESULT_EXECUTION_MODE,
    release_binary: process.env.RESULT_RELEASE_BINARY === "true",
    one_gib_bytes: number("RESULT_ONE_GIB_BYTES"),
    five_gib_bytes: number("RESULT_FIVE_GIB_BYTES"),
    formal_one_gib_bytes: number("RESULT_FORMAL_ONE_GIB_BYTES"),
    formal_five_gib_bytes: number("RESULT_FORMAL_FIVE_GIB_BYTES"),
    formal_defaults: number("RESULT_ONE_GIB_BYTES") === number("RESULT_FORMAL_ONE_GIB_BYTES") && number("RESULT_FIVE_GIB_BYTES") === number("RESULT_FORMAL_FIVE_GIB_BYTES"),
  },
  content_pattern: {
    block_bytes: number("CONTENT_BLOCK_BYTES"),
    fill_formula: "((seed + block_index * stride) % 255) + 1",
    stride: number("CONTENT_PATTERN_STRIDE"),
  },
  content_profiles: {
    hosted: { seed: number("HOSTED_CONTENT_SEED"), sha256: { "1g": paths.upload.samples["1g"].summary.actual, "5g": paths.upload.samples["5g"].summary.actual } },
    proxy: { seed: number("PROXY_CONTENT_SEED"), sha256: { "1g": paths.proxy.samples["1g"].summary.actual, "5g": paths.proxy.samples["5g"].summary.actual } },
    migration: { seed: number("MIGRATION_CONTENT_SEED"), sha256: { "1g": paths.migration.samples["1g"].summary.actual, "5g": paths.migration.samples["5g"].summary.actual } },
  },
  server_rss_limits: {
    peak_bytes: number("RSS_PEAK_LIMIT"),
    delta_bytes: number("RSS_DELTA_LIMIT"),
  },
  raw_memory_samples: {
    path: "memory-samples.tsv",
    sha256: crypto.createHash("sha256").update(aggregateSamples).digest("hex"),
    bytes: aggregateSamples.length,
    columns: ["path", "sample", "index", "server_rss_bytes", "cgroup_memory_current_bytes"],
  },
  cgroup: {
    evidence_mode: "server_rss_sequence_with_cgroup_current_samples",
    limit_bytes: number("CGROUP_LIMIT"),
    current_before_bytes: number("CGROUP_CURRENT_BEFORE"),
    current_after_bytes: number("CGROUP_CURRENT_AFTER"),
    kernel_peak_scope: "container_lifetime_context_only",
    kernel_peak_authoritative: false,
    kernel_peak_writable: process.env.CGROUP_PEAK_WRITABLE === "true",
    kernel_peak_before_bytes: number("CGROUP_PEAK_BEFORE"),
    kernel_peak_after_bytes: number("CGROUP_PEAK_AFTER"),
    kernel_peak_delta_bytes: number("CGROUP_PEAK_AFTER") - number("CGROUP_PEAK_BEFORE"),
    kernel_peak_limit_bytes: number("CGROUP_LIMIT"),
    oom_before: number("OOM_BEFORE"),
    oom_after: number("OOM_AFTER"),
    oom_delta: number("OOM_AFTER") - number("OOM_BEFORE"),
  },
  singleflight: {
    concurrency: 32,
    ready_count: number("SINGLEFLIGHT_READY_COUNT"),
    upstream_method: "GET",
    upstream_before: number("UPSTREAM_BEFORE"),
    upstream_after: number("UPSTREAM_AFTER"),
    upstream_delta: number("UPSTREAM_DELTA"),
    duration_seconds: number("SINGLEFLIGHT_SECONDS"),
    connection_barrier: singleflightBarrier,
    pre_release_upstream: preReleaseUpstream,
    response_evidence: responseEvidence,
  },
  paths,
};
const records = Object.values(paths).flatMap((item) => Object.values(item.samples));
const barrier = evidence.singleflight.connection_barrier;
const preRelease = evidence.singleflight.pre_release_upstream;
const responses = evidence.singleflight.response_evidence;
const stringArray = (value) => Array.isArray(value) && value.every((item) => typeof item === "string");
const sameUniqueSet = (values, expected) => stringArray(values)
  && new Set(values).size === values.length
  && values.length === expected.size
  && values.every((value) => expected.has(value));
const snapshotValid = (snapshot) => {
  if (!snapshot || !Array.isArray(snapshot.proc_sources) || !stringArray(snapshot.owned_socket_inodes)
    || !stringArray(snapshot.socket_inodes) || !Number.isInteger(snapshot.count)) return false;
  if (!snapshot.proc_sources.some((source) => source.readable === true)
    || !snapshot.proc_sources.every((source) => typeof source.readable === "boolean"
      && stringArray(source.matched_socket_inodes))) return false;
  const sourceInodes = new Set(snapshot.proc_sources
    .filter((source) => source.readable)
    .flatMap((source) => source.matched_socket_inodes));
  const ownedInodes = new Set(snapshot.owned_socket_inodes);
  return sameUniqueSet(snapshot.socket_inodes, sourceInodes)
    && new Set(snapshot.owned_socket_inodes).size === snapshot.owned_socket_inodes.length
    && snapshot.socket_inodes.every((inode) => ownedInodes.has(inode))
    && snapshot.count === snapshot.socket_inodes.length;
};
const baselineInodes = new Set(barrier.baseline.socket_inodes);
const sampleValid = (sample) => {
  if (!snapshotValid(sample) || !sample.new || !stringArray(sample.new.socket_inodes)
    || !Number.isInteger(sample.new.count)) return false;
  const recalculatedNewInodes = new Set(sample.socket_inodes.filter((inode) => !baselineInodes.has(inode)));
  return sameUniqueSet(sample.new.socket_inodes, recalculatedNewInodes)
    && sample.new.count === sample.new.socket_inodes.length
    && sample.new.count >= barrier.required_new_count;
};
const barrierValid = barrier.observer === "linux_proc_server_owned_established"
  && Number.isInteger(barrier.server_pid)
  && barrier.server_pid > 0
  && Number.isInteger(barrier.local_port)
  && barrier.local_port > 0
  && barrier.tcp_state === "01"
  && snapshotValid(barrier.baseline)
  && Array.isArray(barrier.samples)
  && barrier.required_new_count === evidence.singleflight.concurrency
  && barrier.required_consecutive_samples === 2
  && barrier.samples.length === barrier.required_consecutive_samples
  && barrier.consecutive_successful_samples === barrier.samples.length
  && barrier.samples.every(sampleValid);
const preReleaseValid = preRelease.before === evidence.singleflight.upstream_before
  && preRelease.held - preRelease.before === preRelease.delta
  && preRelease.delta === 1
  && preRelease.artifact_hold === true
  && preRelease.artifact_waiting === 1;
const releaseUpstreamValid = evidence.singleflight.upstream_after - evidence.singleflight.upstream_before
  === evidence.singleflight.upstream_delta
  && evidence.singleflight.upstream_delta === 1;
const digestValid = (value) => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
const responseRecordValid = (record) => record && Number.isInteger(record.index)
  && digestValid(record.sha256) && Number.isInteger(record.bytes) && record.bytes >= 0;
const fixtureValid = responses.fixture && digestValid(responses.fixture.sha256)
  && Number.isInteger(responses.fixture.bytes) && responses.fixture.bytes >= 0;
const responseRecordsValid = fixtureValid && Array.isArray(responses.responses)
  && responses.responses.length === evidence.singleflight.concurrency
  && responses.responses.every(responseRecordValid);
const responseIndices = responseRecordsValid
  ? new Set(responses.responses.map((response) => response.index))
  : new Set();
const responseIndicesValid = responseIndices.size === evidence.singleflight.concurrency
  && Array.from({ length: evidence.singleflight.concurrency }, (_, index) => index + 1)
    .every((index) => responseIndices.has(index));
const allResponsesByteEqual = responseRecordsValid && responseIndicesValid
  && responses.responses.every((response) => response.sha256 === responses.fixture.sha256
    && response.bytes === responses.fixture.bytes);
barrier.valid = barrierValid;
evidence.singleflight.hold_verified_before_release = preReleaseValid;
evidence.singleflight.release_upstream_delta_verified = releaseUpstreamValid;
evidence.singleflight.all_responses_byte_equal = allResponsesByteEqual;
evidence.singleflight.valid = evidence.singleflight.ready_count === evidence.singleflight.concurrency
  && barrierValid
  && preReleaseValid
  && releaseUpstreamValid
  && allResponsesByteEqual;
evidence.singleflight.authoritative = evidence.sample_configuration.execution_mode === "formal"
  && evidence.singleflight.valid;
evidence.cgroup.sampled_current_peak_bytes = Math.max(
  ...records.map((item) => item.cgroup_memory_current.sampled_peak_bytes),
);
evidence.cgroup.valid = evidence.cgroup.current_before_bytes <= evidence.cgroup.limit_bytes
  && evidence.cgroup.current_after_bytes <= evidence.cgroup.limit_bytes
  && evidence.cgroup.sampled_current_peak_bytes <= evidence.cgroup.limit_bytes
  && evidence.cgroup.oom_delta === 0
  && records.every((item) => item.raw_samples.valid);
evidence.cgroup.authoritative = evidence.sample_configuration.execution_mode === "formal"
  && evidence.cgroup.valid;
evidence.authoritative = evidence.cgroup.authoritative
  && evidence.singleflight.authoritative
  && records.every((item) => item.authoritative);
const formalSampleGuardValid = evidence.sample_configuration.execution_mode !== "formal"
  || evidence.sample_configuration.formal_defaults;
const formalAuthorityValid = evidence.sample_configuration.execution_mode !== "formal"
  || evidence.authoritative;
const distinctContentSeeds = new Set(Object.values(evidence.content_profiles).map((item) => item.seed)).size === 3;
const distinctContentDigests = ["1g", "5g"].every((sample) => new Set(
  Object.values(evidence.content_profiles).map((item) => item.sha256[sample]),
).size === 3);
const valid = evidence.cgroup.valid
  && evidence.singleflight.valid
  && formalSampleGuardValid
  && formalAuthorityValid
  && distinctContentSeeds
  && distinctContentDigests
  && records.every((item) => item.valid)
  && Object.values(paths).every((item) => item.comparison.non_linear_growth_verified);
evidence.valid = valid;
if (!valid) {
  console.error("错误：性能验收原始证据未通过完整性断言");
  process.exit(1);
}
fs.writeFileSync(process.env.RESULT_FILE, `${JSON.stringify(evidence, null, 2)}\n`);
'
}

validate_configuration
node -e 'const fs=require("node:fs"); for (const value of process.argv.slice(1)) fs.mkdirSync(value,{recursive:true});' "$EVIDENCE_DIR" "$MEASUREMENT_DIR" "$SAMPLE_DIR"
rm -f "$MEASUREMENT_DIR"/*.json "$SAMPLE_DIR"/*.tsv "$SAMPLE_FILE" "$EVIDENCE_DIR/result.json"
printf 'path\tsample\tindex\tserver_rss_bytes\tcgroup_memory_current_bytes\n' > "$SAMPLE_FILE"
configure_cgroup_paths
read -r cgroup_limit < "$cgroup_limit_file"
[[ "$cgroup_limit" =~ ^[0-9]+$ ]] || fail "cgroup 内存上限不是有限数值"
if [ "$cgroup_limit" -lt "$CGROUP_LIMIT_BYTES" ] \
    || [ "$cgroup_limit" -gt "$((CGROUP_LIMIT_BYTES + CGROUP_LIMIT_TOLERANCE_BYTES))" ]; then
    fail "cgroup 内存上限不得低于 5 GiB 且不得偏高超过 64 MiB：$cgroup_limit 字节"
fi
oom_before="$(read_oom_count)"
read -r cgroup_current_before < "$cgroup_current_file"
[[ "$cgroup_current_before" =~ ^[0-9]+$ ]] || fail "cgroup memory.current 初值不是有效数值"
[ "$cgroup_current_before" -le "$cgroup_limit" ] || fail "性能验收开始前 cgroup memory.current 已超过容器内存上限"
cgroup_peak_before="$(read_kernel_peak)"
cgroup_peak_writable=false
[ -w "$cgroup_peak_file" ] && cgroup_peak_writable=true
printf '==> 已确认 Docker cgroup 内存上限约 5 GiB：%s 字节；memory.current 初值：%s 字节；容器生命周期 memory.peak：%s 字节（仅作诊断，不参与本阶段判定）\n' \
    "$cgroup_limit" "$cgroup_current_before" "$cgroup_peak_before"

token="$(<"$ACCEPTANCE_TOKEN_FILE")"
[ -n "$token" ] || fail "令牌文件为空"
hosted_payload='{"name":"performance-hosted","format":"raw","repo_type":"hosted","visibility":"public"}'
proxy_payload="$(UPSTREAM_URL="$ACCEPTANCE_UPSTREAM_URL/raw" node -e 'process.stdout.write(JSON.stringify({name:"performance-proxy",format:"raw",repo_type:"proxy",visibility:"public",upstream_url:process.env.UPSTREAM_URL}))')"
create_repository "$hosted_payload" "$ACCEPTANCE_RUNTIME_DIR/hosted-create.json"
create_repository "$proxy_payload" "$ACCEPTANCE_RUNTIME_DIR/proxy-create.json"
server_pid="$(server_process_id)" || fail "无法定位验收服务进程"

echo '==> 验证 NFR-08：32 并发同路径未命中只回源一次'
run_singleflight

echo '==> 验证上传路径：1 GiB/5 GiB 状态、摘要与 RSS 对比'
run_upload 1g "$ONE_GIB_BYTES"
run_upload 5g "$FIVE_GIB_BYTES"
configure_migration_upstream

echo '==> 验证下载路径：1 GiB/5 GiB 摘要与 RSS 对比'
run_download 1g "$ONE_GIB_BYTES"
run_download 5g "$FIVE_GIB_BYTES"

echo '==> 验证 proxy 回源路径：1 GiB/5 GiB 上游计数、摘要与 RSS 对比'
run_proxy 1g "$ONE_GIB_BYTES"
run_proxy 5g "$FIVE_GIB_BYTES"

echo '==> 验证迁移搬运路径：在线迁移 API 的 1 GiB/5 GiB 摘要与 RSS 对比'
run_migration 1g "$ONE_GIB_BYTES"
run_migration 5g "$FIVE_GIB_BYTES"

oom_after="$(read_oom_count)"
assert_equal "$oom_before" "$oom_after" "完整性能验收出现新的 OOM 事件"
read -r cgroup_current_after < "$cgroup_current_file"
[[ "$cgroup_current_after" =~ ^[0-9]+$ ]] || fail "cgroup memory.current 终值不是有效数值"
[ "$cgroup_current_after" -le "$cgroup_limit" ] || fail "性能验收结束后 cgroup memory.current 已超过容器内存上限"
cgroup_peak_after="$(read_kernel_peak)"
write_result "$cgroup_peak_after" "$oom_after"

if [ "$execution_mode" = formal ]; then
    echo "正式性能验收通过：32 并发单飞命中 1 次，四类路径完成正式 1 GiB/5 GiB RSS 原始序列与 memory.current 采样对比，无新增 OOM"
else
    echo "性能 smoke 通过：32 并发单飞命中 1 次，四类路径完成覆写样本 RSS 原始序列与 memory.current 采样对比，无新增 OOM"
fi
echo "证据已写入 $EVIDENCE_DIR/result.json 与 $SAMPLE_FILE"
