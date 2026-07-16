#!/usr/bin/env bash
# 部署脚本逻辑测试（M1-FR-043、M1-FR-046）：用桩 systemctl/loginctl/curl/ssh 驱动部署脚本，
# 覆盖部署、回滚、保留清理、rootless unit、初始化链接输出与敏感令牌不泄露。
set -euo pipefail

# Git Bash 默认会把目录符号链接退化为复制；测试需保留远端 Linux 的真实链接语义
if [[ "${OSTYPE:-}" == msys* ]]; then
    export MSYS=winsymlinks:sys
fi

cd "$(dirname "$0")/../.."

PASS_COUNT=0
ok() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  通过：$1"; }
die() { echo "  失败：$1" >&2; exit 1; }
assert_secret_absent() {
    local path="$1" description="$2" secret="$3"
    if grep -Fq -- "$secret" "$path"; then
        die "$description 出现敏感内容"
    fi
}

# ---------- 测试环境 ----------
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
export HOME="$WORK/home"
mkdir -p "$HOME"

# 桩 systemctl 与 curl：分别模拟服务状态和 readyz 请求结果
STUB="$WORK/stub-bin"
mkdir -p "$STUB"
REAL_STAT="$(command -v stat)"
export REAL_STAT
cat > "$STUB/systemctl" <<'EOF'
#!/usr/bin/env bash
echo "systemctl $*" >> "$SYSTEMCTL_LOG"
if [ "$1" = "--user" ] && [ "$2" = "is-active" ]; then
    [ "${SYSTEMCTL_FAIL_ACTIVE:-0}" = "1" ] && exit 1
fi
exit 0
EOF
cat > "$STUB/loginctl" <<'EOF'
#!/usr/bin/env bash
echo "Linger=yes"
EOF
cat > "$STUB/curl" <<'EOF'
#!/usr/bin/env bash
echo "curl $*" >> "$CURL_LOG"
url="${!#}"
if [[ "$url" == */api/v1/setup/status ]]; then
    [ "${CURL_FAIL_SETUP:-0}" = "1" ] && exit 22
    printf '%s' "${SETUP_STATUS_RESPONSE:-}"
    exit 0
fi
[ "${CURL_FAIL:-0}" = "1" ] && exit 22
exit 0
EOF
cat > "$STUB/ssh" <<'EOF'
#!/usr/bin/env bash
printf '<%s>\n' "$@" >> "$SSH_LOG"
exit 0
EOF
cat > "$STUB/stat" <<'EOF'
#!/usr/bin/env bash
last="${!#}"
if [ "${USE_STUB_TOKEN_MODE:-0}" = "1" ] && [ "$1" = "--format=%a" ] && [ "$last" = "$TOKEN_FILE" ]; then
    printf '%s\n' "$STUB_TOKEN_MODE"
    exit 0
fi
exec "$REAL_STAT" "$@"
EOF
chmod +x "$STUB/systemctl" "$STUB/loginctl" "$STUB/curl" "$STUB/ssh" "$STUB/stat"
export PATH="$STUB:$PATH"
export SYSTEMCTL_LOG="$WORK/systemctl.log"
export CURL_LOG="$WORK/curl.log"
export SSH_LOG="$WORK/ssh.log"
: > "$SYSTEMCTL_LOG"
: > "$CURL_LOG"
: > "$SSH_LOG"

export DEPLOY_DIR="$HOME/custom-jianartifact"
export DEPLOY_SERVICE="jianartifact"
export DEPLOY_KEEP_RELEASES="3"
export DEPLOY_HEALTH_URL="http://127.0.0.1:18080/readyz"
export DEPLOY_PUBLIC_URL="https://artifact.example.test/base/"
export DEPLOY_DATABASE_PATH="data/jianartifact.db"
export SETUP_STATUS_RESPONSE='{"setup_required":true}'
export HEALTH_RETRIES="1"
export HEALTH_INTERVAL="0"
TOKEN="0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
INVALID_TOKEN="ABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCD"
SETUP_URL="https://artifact.example.test/base/setup#token=$TOKEN"
TOKEN_FILE=""
USE_STUB_TOKEN_MODE=0
[[ "${OSTYPE:-}" == msys* ]] && USE_STUB_TOKEN_MODE=1
STUB_TOKEN_MODE=600

set_database_path() {
    DEPLOY_DATABASE_PATH="$1"
    export DEPLOY_DATABASE_PATH
    if [[ "$DEPLOY_DATABASE_PATH" = /* ]]; then
        TOKEN_FILE="$(dirname -- "$DEPLOY_DATABASE_PATH")/initial-admin.token"
    else
        TOKEN_FILE="$(dirname -- "$DEPLOY_DIR/$DEPLOY_DATABASE_PATH")/initial-admin.token"
    fi
    export TOKEN_FILE
}

set_database_path "$DEPLOY_DATABASE_PATH"
export USE_STUB_TOKEN_MODE STUB_TOKEN_MODE

write_token() {
    local content="$1" mode="$2"
    mkdir -p "$(dirname "$TOKEN_FILE")"
    printf '%s' "$content" > "$TOKEN_FILE"
    chmod "$mode" "$TOKEN_FILE"
    STUB_TOKEN_MODE="$mode"
    export STUB_TOKEN_MODE
}

# 构造一个假发布包（结构与 build-release.sh 产物一致）
make_pkg() {
    local version="$1" marker="$2" pkg_dir pkg_name
    pkg_name="jianartifact-${version}-x86_64-unknown-linux-gnu"
    pkg_dir="$WORK/stage/$pkg_name"
    rm -rf "$WORK/stage"
    mkdir -p "$pkg_dir/bin" "$pkg_dir/systemd"
    printf '#!/usr/bin/env bash\necho %s\n' "$marker" > "$pkg_dir/bin/jianartifact"
    chmod +x "$pkg_dir/bin/jianartifact"
    cp scripts/deploy/jianartifact.service "$pkg_dir/systemd/jianartifact.service"
    printf '%s\n' "$version" > "$pkg_dir/VERSION"
    tar -czf "$WORK/$pkg_name.tar.gz" -C "$WORK/stage" "$pkg_name"
    (cd "$WORK" && sha256sum "$pkg_name.tar.gz" > "$pkg_name.tar.gz.sha256")
    echo "$WORK/$pkg_name.tar.gz"
}

deploy() { bash scripts/deploy/remote-install.sh deploy "$1"; }
rollback() { bash scripts/deploy/remote-install.sh rollback; }
current() { readlink -f "$DEPLOY_DIR/current"; }
release_count() { ls -1 "$DEPLOY_DIR/releases" | wc -l; }

echo "==> 部署脚本逻辑测试"

# ---------- 1. 无 sudo 断言 ----------
[ -f scripts/deploy.sh ] || die "scripts/deploy.sh 不存在"
[ -f scripts/deploy/remote-install.sh ] || die "scripts/deploy/remote-install.sh 不存在"
for f in scripts/deploy.sh scripts/deploy/remote-install.sh; do
    # 剔除注释行后检查，注释里提及 sudo 不算调用
    if grep -vE '^[[:space:]]*#' "$f" | grep -qw sudo; then
        die "$f 出现 sudo 调用，违反 rootless 要求"
    fi
done
ok "脚本无 sudo 调用"

# ---------- 2. 首次部署 ----------
PKG1="$(make_pkg 0.1.0 v1)"
write_token "$TOKEN" 600
deploy "$PKG1" > "$WORK/first-deploy.log"
[ -x "$(current)/bin/jianartifact" ] || die "首次部署后 current 未指向可执行二进制"
UNIT_FILE="$HOME/.config/systemd/user/jianartifact.service"
[ -f "$UNIT_FILE" ] || die "unit 未安装到用户 systemd 目录"
grep -Fq "ExecStart=$DEPLOY_DIR/current/bin/jianartifact" "$UNIT_FILE" \
    || die "unit 的 ExecStart 未改写为部署目录路径"
grep -Fq "WorkingDirectory=$DEPLOY_DIR" "$UNIT_FILE" \
    || die "unit 的 WorkingDirectory 未改写为部署目录路径"
grep -Fq "EnvironmentFile=-$DEPLOY_DIR/jianartifact.env" "$UNIT_FILE" \
    || die "unit 的 EnvironmentFile 未改写为部署目录路径"
grep -Fqx "JIANARTIFACT_DATABASE_PATH='data/jianartifact.db'" "$DEPLOY_DIR/jianartifact.env" \
    || die "运行环境文件未写入默认数据库路径"
grep -Fqx "$SETUP_URL" "$WORK/first-deploy.log" || die "首次部署未输出初始化链接"
grep -q -- "/api/v1/setup/status" "$CURL_LOG" || die "输出链接前未查询初始化状态"
grep -q -- "--user daemon-reload" "$SYSTEMCTL_LOG" || die "未执行 daemon-reload"
grep -q -- "--user restart" "$SYSTEMCTL_LOG" || die "未重启服务"
grep -q -- "/readyz" "$CURL_LOG" || die "部署成功前未请求 readyz"
ok "首次部署：rootless unit 接线、readyz 与初始化链接均正确"

# ---------- 3. 升级切换与幂等 ----------
CUR1="$(current)"
PKG2="$(make_pkg 0.1.0 v2)"
deploy "$PKG2" > "$WORK/upgrade-deploy.log"
CUR2="$(current)"
[ "$CUR1" != "$CUR2" ] || die "升级后 current 未切换到新发布"
[ "$("$CUR2/bin/jianartifact")" = "v2" ] || die "current 指向的不是新版本二进制"
[ -d "$CUR1" ] || die "上一发布被误删"
grep -Fqx "$SETUP_URL" "$WORK/upgrade-deploy.log" || die "升级时未再次输出待用初始化链接"
SETUP_STATUS_RESPONSE='{"setup_required":false}'
export SETUP_STATUS_RESPONSE
deploy "$(make_pkg 0.1.0 v3)" > "$WORK/initialized-deploy.log"
if grep -Fq 'setup#token=' "$WORK/initialized-deploy.log"; then
    die "服务已关闭初始化入口时仍输出旧令牌链接"
fi
assert_secret_absent "$WORK/initialized-deploy.log" "初始化关闭后的部署输出" "$TOKEN"
rm -f "$TOKEN_FILE"
SETUP_STATUS_RESPONSE='{"setup_required":true}'
export SETUP_STATUS_RESPONSE
ok "升级切换：待用令牌可重复展示，服务关闭入口后不再输出旧链接"

# ---------- 4. sha256 校验失败 ----------
PKG_BAD="$(make_pkg 0.1.0 v4)"
printf '损坏' >> "$PKG_BAD"
CUR_BEFORE="$(current)"
if deploy "$PKG_BAD" > /dev/null 2>&1; then
    die "sha256 不匹配的包被接受了"
fi
[ "$(current)" = "$CUR_BEFORE" ] || die "校验失败后 current 被改动"
ok "sha256 校验失败：拒绝部署且 current 不受影响"

# ---------- 5. readyz 失败自动回滚 ----------
CUR_GOOD="$(current)"
PKG_FAIL="$(make_pkg 0.1.0 vbad)"
write_token "$TOKEN" 600
: > "$CURL_LOG"
if CURL_FAIL=1 deploy "$PKG_FAIL" > "$WORK/autoroll.log" 2>&1; then
    die "systemd active 但 readyz 失败的部署未以非零退出"
fi
[ "$(current)" = "$CUR_GOOD" ] || die "自动回滚未把 current 切回上一发布"
grep -q -- "/readyz" "$CURL_LOG" || die "失败部署未请求 readyz"
grep -q "自动回滚" "$WORK/autoroll.log" || die "输出未报告自动回滚"
assert_secret_absent "$WORK/autoroll.log" "健康失败输出" "$TOKEN"
rm -f "$TOKEN_FILE"
ok "自动回滚：健康失败不输出初始化链接且切回上一发布"

# ---------- 6. 手动回滚 ----------
CUR_NOW="$(current)"
write_token "$TOKEN" 600
rollback > "$WORK/manual-rollback.log"
[ "$(current)" != "$CUR_NOW" ] || die "手动回滚未切换 current"
assert_secret_absent "$WORK/manual-rollback.log" "手动回滚输出" "$TOKEN"
rm -f "$TOKEN_FILE"
while rollback > /dev/null 2>&1; do :; done
if rollback > "$WORK/rollback.log" 2>&1; then
    die "无更早发布时 rollback 未报错"
fi
grep -q "没有更早的发布" "$WORK/rollback.log" || die "rollback 报错文案缺失"
ok "手动回滚：不输出初始化链接且无更早发布时明确报错"

# ---------- 7. 保留清理 ----------
rm -rf "$DEPLOY_DIR"
for marker in a b c d e; do
    deploy "$(make_pkg 0.1.0 "$marker")" > /dev/null
done
[ "$(release_count)" -le 3 ] || die "保留清理未生效：现存 $(release_count) 份 > 3"
[ -d "$(current)" ] || die "清理误删了活动发布"
ok "保留清理：超出份数的最旧发布被清理，活动发布保留"

# ---------- 8. 宿主侧配置校验 ----------
if DEPLOY_ENV_FILE=/dev/null bash scripts/deploy.sh deploy > "$WORK/deploy.log" 2>&1; then
    die "缺少必填配置时 deploy.sh 未失败"
fi
grep -q "DEPLOY_HOST" "$WORK/deploy.log" || die "deploy.sh 报错未指出缺失变量"
: > "$SSH_LOG"
for invalid_health_url in /readyz https:///readyz 'http://?x'; do
    if DEPLOY_ENV_FILE=/dev/null DEPLOY_HOST=deploy.example.test DEPLOY_USER=deployer \
        DEPLOY_HEALTH_URL="$invalid_health_url" bash scripts/deploy.sh rollback \
        > "$WORK/invalid-health-url.log" 2>&1; then
        die "非法探活地址未被拒绝：$invalid_health_url"
    fi
    grep -q "完整的 http:// 或 https:// URL" "$WORK/invalid-health-url.log" \
        || die "非法探活地址未输出明确中文错误"
done
[ ! -s "$SSH_LOG" ] || die "非法探活地址仍发起了 SSH 连接"
ok "宿主侧：缺必填配置或非法探活地址立即失败且不发起连接"

# ---------- 9. Windows Git Bash 远端路径透传 ----------
if ! grep -q "MSYS2_ARG_CONV_EXCL" scripts/deploy.sh; then
    die "deploy.sh 未禁用 MSYS 参数路径转换，远端绝对路径会被改写为 Windows 路径"
fi
ok "宿主侧：Windows Git Bash 原样透传远端绝对路径"

# ---------- 10. 自定义 SSH 端口 ----------
if ! grep -q "DEPLOY_SSH_PORT" scripts/deploy.sh; then
    die "deploy.sh 未读取 DEPLOY_SSH_PORT"
fi
if ! grep -q 'SCP_OPTS.*-P' scripts/deploy.sh; then
    die "deploy.sh 未使用 SCP 的大写 -P 透传自定义端口"
fi
ok "宿主侧：SSH 与 SCP 使用自定义端口"

# ---------- 11. 公网地址安全透传 ----------
: > "$SSH_LOG"
DEPLOY_ENV_FILE=/dev/null \
DEPLOY_HOST=deploy.example.test \
DEPLOY_USER=deployer \
DEPLOY_SSH_PORT=2202 \
DEPLOY_SSH_KEY="$WORK/deploy key" \
DEPLOY_PUBLIC_URL="$DEPLOY_PUBLIC_URL" \
DEPLOY_DATABASE_PATH="metadata/custom db.sqlite" \
bash scripts/deploy.sh rollback > "$WORK/host-rollback.log"
grep -Fqx '<-p>' "$SSH_LOG" || die "SSH 未接收端口选项"
grep -Fqx '<2202>' "$SSH_LOG" || die "SSH 未接收自定义端口"
grep -Fqx '<-i>' "$SSH_LOG" || die "SSH 未接收私钥选项"
grep -Fqx "<$WORK/deploy key>" "$SSH_LOG" || die "SSH 私钥路径未保持单一参数"
grep -Fq "DEPLOY_PUBLIC_URL='$DEPLOY_PUBLIC_URL'" "$SSH_LOG" || die "公网地址未透传到远端命令"
grep -Fq "DEPLOY_DATABASE_PATH='metadata/custom db.sqlite'" "$SSH_LOG" || die "数据库路径未透传到远端命令"
: > "$SSH_LOG"
if DEPLOY_ENV_FILE=/dev/null \
    DEPLOY_HOST=deploy.example.test \
    DEPLOY_USER=deployer \
    DEPLOY_PUBLIC_URL="https://artifact.example.test/'unsafe" \
    bash scripts/deploy.sh rollback > "$WORK/unsafe-url.log" 2>&1; then
    die "含单引号的公网地址未被拒绝"
fi
[ ! -s "$SSH_LOG" ] || die "非法公网地址仍发起了 SSH 连接"
ok "宿主侧：公网地址与数据库路径安全透传并拒绝单引号"

# ---------- 12. 自定义数据库路径 ----------
set_database_path "metadata/custom db.sqlite"
write_token "$TOKEN" 600
deploy "$(make_pkg 0.1.0 relative-db)" > "$WORK/relative-db.log"
grep -Fqx "$SETUP_URL" "$WORK/relative-db.log" || die "相对数据库路径未找到同目录令牌"
grep -Fqx "JIANARTIFACT_DATABASE_PATH='metadata/custom db.sqlite'" "$DEPLOY_DIR/jianartifact.env" \
    || die "相对数据库路径未写入运行环境文件"
rm -f "$TOKEN_FILE"

set_database_path "$WORK/external metadata/custom.db"
write_token "$TOKEN" 600
deploy "$(make_pkg 0.1.0 absolute-db)" > "$WORK/absolute-db.log"
grep -Fqx "$SETUP_URL" "$WORK/absolute-db.log" || die "绝对数据库路径未找到同目录令牌"
grep -Fqx "JIANARTIFACT_DATABASE_PATH='$WORK/external metadata/custom.db'" "$DEPLOY_DIR/jianartifact.env" \
    || die "绝对数据库路径未写入运行环境文件"
rm -f "$TOKEN_FILE"
set_database_path "data/jianartifact.db"
ok "数据库路径：相对与绝对路径均驱动服务配置和令牌定位"

# ---------- 13. 初始化状态门禁 ----------
write_token "$TOKEN" 600
CURL_FAIL_SETUP=1 deploy "$(make_pkg 0.1.0 status-failed)" > "$WORK/status-failed.log" 2>&1
assert_secret_absent "$WORK/status-failed.log" "状态查询失败输出" "$TOKEN"
grep -q "状态查询失败" "$WORK/status-failed.log" || die "状态查询失败未输出中文告警"
SETUP_STATUS_RESPONSE='not-json'
export SETUP_STATUS_RESPONSE
deploy "$(make_pkg 0.1.0 status-invalid)" > "$WORK/status-invalid.log" 2>&1
assert_secret_absent "$WORK/status-invalid.log" "状态响应无效输出" "$TOKEN"
grep -q "状态响应无效" "$WORK/status-invalid.log" || die "状态响应无效未输出中文告警"
SETUP_STATUS_RESPONSE='{"setup_required":true}'
export SETUP_STATUS_RESPONSE
rm -f "$TOKEN_FILE"
ok "初始化状态：请求失败或响应无效时均不输出令牌"

# ---------- 14. 缺少公网地址 ----------
write_token "$TOKEN" 600
DEPLOY_PUBLIC_URL= deploy "$(make_pkg 0.1.0 no-url)" > "$WORK/no-url.log"
assert_secret_absent "$WORK/no-url.log" "缺少公网地址提示" "$TOKEN"
grep -q "未配置 DEPLOY_PUBLIC_URL" "$WORK/no-url.log" || die "缺少公网地址时未输出中文提示"
rm -f "$TOKEN_FILE"
ok "初始化链接：缺少公网地址时仅提示且不泄露令牌"

# ---------- 15. 不安全权限 ----------
write_token "$TOKEN" 644
deploy "$(make_pkg 0.1.0 unsafe-mode)" > "$WORK/unsafe-mode.log" 2>&1
assert_secret_absent "$WORK/unsafe-mode.log" "不安全权限告警" "$TOKEN"
grep -q "权限" "$WORK/unsafe-mode.log" || die "不安全权限未输出中文告警"
rm -f "$TOKEN_FILE"
ok "初始化链接：令牌权限不是 0600 时只告警"

# ---------- 16. 非法内容与非普通文件 ----------
write_token "$INVALID_TOKEN" 600
deploy "$(make_pkg 0.1.0 invalid-token)" > "$WORK/invalid-token.log" 2>&1
assert_secret_absent "$WORK/invalid-token.log" "非法令牌告警" "$INVALID_TOKEN"
grep -q "格式" "$WORK/invalid-token.log" || die "非法令牌内容未输出中文告警"
rm -f "$TOKEN_FILE"
printf '%s' "$TOKEN" > "$WORK/token-target"
chmod 600 "$WORK/token-target"
ln -s "$WORK/token-target" "$TOKEN_FILE"
deploy "$(make_pkg 0.1.0 symlink-token)" > "$WORK/symlink-token.log" 2>&1
assert_secret_absent "$WORK/symlink-token.log" "非普通令牌文件告警" "$TOKEN"
grep -q "普通文件" "$WORK/symlink-token.log" || die "非普通令牌文件未输出中文告警"
rm -f "$TOKEN_FILE"
ok "初始化链接：非法内容与非普通文件只告警且不泄露"

# ---------- 17. 敏感令牌隔离 ----------
assert_secret_absent "$SYSTEMCTL_LOG" "systemctl 调用记录" "$TOKEN"
assert_secret_absent "$CURL_LOG" "curl 调用记录" "$TOKEN"
assert_secret_absent "$UNIT_FILE" "systemd unit" "$TOKEN"
if grep -R -Fq -- "$TOKEN" "$DEPLOY_DIR/releases"; then
    die "发布目录出现敏感内容"
fi
ok "安全隔离：systemctl、curl、unit 与发布目录均不含令牌"

echo "部署脚本逻辑测试全部通过（${PASS_COUNT} 项）"
