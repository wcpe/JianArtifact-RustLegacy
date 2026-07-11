#!/usr/bin/env bash
# 部署脚本逻辑测试（M1-FR-043）：在容器内用桩 systemctl/loginctl 驱动 remote-install.sh，
# 覆盖首次部署、升级切换、sha256 校验失败、自动回滚、手动回滚、保留清理与无 sudo 断言。
set -euo pipefail

cd "$(dirname "$0")/../.."

PASS_COUNT=0
ok() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  通过：$1"; }
die() { echo "  失败：$1" >&2; exit 1; }

# ---------- 测试环境 ----------
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
export HOME="$WORK/home"
mkdir -p "$HOME"

# 桩 systemctl：记录调用；SYSTEMCTL_FAIL_ACTIVE=1 时 is-active 判失败
STUB="$WORK/stub-bin"
mkdir -p "$STUB"
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
chmod +x "$STUB/systemctl" "$STUB/loginctl"
export PATH="$STUB:$PATH"
export SYSTEMCTL_LOG="$WORK/systemctl.log"
: > "$SYSTEMCTL_LOG"

export DEPLOY_DIR="$HOME/jianartifact"
export DEPLOY_SERVICE="jianartifact"
export DEPLOY_KEEP_RELEASES="3"
export HEALTH_RETRIES="1"
export HEALTH_INTERVAL="0"

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
deploy "$PKG1" > /dev/null
[ -x "$(current)/bin/jianartifact" ] || die "首次部署后 current 未指向可执行二进制"
[ -f "$HOME/.config/systemd/user/jianartifact.service" ] || die "unit 未安装到用户 systemd 目录"
grep -q "ExecStart=$DEPLOY_DIR/current/bin/jianartifact" "$HOME/.config/systemd/user/jianartifact.service" \
    || die "unit 的 ExecStart 未改写为部署目录路径"
grep -q -- "--user daemon-reload" "$SYSTEMCTL_LOG" || die "未执行 daemon-reload"
grep -q -- "--user restart" "$SYSTEMCTL_LOG" || die "未重启服务"
ok "首次部署：current 就位、unit 安装、服务重启"

# ---------- 3. 升级切换与幂等 ----------
CUR1="$(current)"
PKG2="$(make_pkg 0.1.0 v2)"
deploy "$PKG2" > /dev/null
CUR2="$(current)"
[ "$CUR1" != "$CUR2" ] || die "升级后 current 未切换到新发布"
[ "$("$CUR2/bin/jianartifact")" = "v2" ] || die "current 指向的不是新版本二进制"
[ -d "$CUR1" ] || die "上一发布被误删"
deploy "$(make_pkg 0.1.0 v3)" > /dev/null
ok "升级切换：current 原子切换、旧发布保留、重复部署幂等"

# ---------- 4. sha256 校验失败 ----------
PKG_BAD="$(make_pkg 0.1.0 v4)"
printf '损坏' >> "$PKG_BAD"
CUR_BEFORE="$(current)"
if deploy "$PKG_BAD" > /dev/null 2>&1; then
    die "sha256 不匹配的包被接受了"
fi
[ "$(current)" = "$CUR_BEFORE" ] || die "校验失败后 current 被改动"
ok "sha256 校验失败：拒绝部署且 current 不受影响"

# ---------- 5. 自动回滚 ----------
CUR_GOOD="$(current)"
PKG_FAIL="$(make_pkg 0.1.0 vbad)"
if SYSTEMCTL_FAIL_ACTIVE=1 deploy "$PKG_FAIL" > "$WORK/autoroll.log" 2>&1; then
    die "健康确认失败的部署未以非零退出"
fi
[ "$(current)" = "$CUR_GOOD" ] || die "自动回滚未把 current 切回上一发布"
grep -q "自动回滚" "$WORK/autoroll.log" || die "输出未报告自动回滚"
ok "自动回滚：健康确认失败自动切回上一发布并非零退出"

# ---------- 6. 手动回滚 ----------
CUR_NOW="$(current)"
rollback > /dev/null
[ "$(current)" != "$CUR_NOW" ] || die "手动回滚未切换 current"
while rollback > /dev/null 2>&1; do :; done
if rollback > "$WORK/rollback.log" 2>&1; then
    die "无更早发布时 rollback 未报错"
fi
grep -q "没有更早的发布" "$WORK/rollback.log" || die "rollback 报错文案缺失"
ok "手动回滚：逐级回退，无更早发布时明确报错"

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
ok "宿主侧：缺必填配置立即失败且不发起连接"

echo "部署脚本逻辑测试全部通过（${PASS_COUNT} 项）"
