#!/usr/bin/env bash
# JianArtifact 远端安装/回滚脚本（M1-FR-043）
# 由 deploy.sh 经 SSH 以 bash -s 方式在目标机执行（逻辑测试在容器内直接调用）。
# rootless：全程用户级 systemd，无任何 sudo。
# 用法：remote-install.sh deploy <tar 路径> | remote-install.sh rollback
set -euo pipefail

MODE="${1:?用法：remote-install.sh deploy <tar 路径> | remote-install.sh rollback}"
DEPLOY_DIR="${DEPLOY_DIR:-$HOME/jianartifact}"
DEPLOY_SERVICE="${DEPLOY_SERVICE:-jianartifact}"
DEPLOY_KEEP_RELEASES="${DEPLOY_KEEP_RELEASES:-3}"
DEPLOY_HEALTH_URL="${DEPLOY_HEALTH_URL:-}"
# 健康确认的重试参数（逻辑测试会调小以加速）
HEALTH_RETRIES="${HEALTH_RETRIES:-10}"
HEALTH_INTERVAL="${HEALTH_INTERVAL:-2}"
UNIT_FILE="$HOME/.config/systemd/user/${DEPLOY_SERVICE}.service"
# 非登录 SSH 会话通常缺 XDG_RUNTIME_DIR，systemctl --user 需要它
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

fail() { echo "$@" >&2; exit 1; }

# linger 未开启时服务会随登出停止——只警告不阻断（开启需管理员一次性执行）
check_linger() {
    if ! loginctl show-user "$(id -un)" --property=Linger 2>/dev/null | grep -q 'Linger=yes'; then
        echo "警告：当前用户未开启 linger，服务将随登出停止；请管理员执行一次 loginctl enable-linger $(id -un)"
    fi
}

current_target() { readlink -f "$DEPLOY_DIR/current" 2>/dev/null || true; }

# 符号链接原子切换：先建临时链接再 mv -T 覆盖
switch_current() {
    local tmp="$DEPLOY_DIR/current.tmp.$$"
    ln -sfn "$1" "$tmp"
    mv -T "$tmp" "$DEPLOY_DIR/current"
}

restart_service() {
    systemctl --user enable "$DEPLOY_SERVICE" > /dev/null 2>&1 || true
    systemctl --user restart "$DEPLOY_SERVICE"
}

# 健康确认：服务须 active；配置了探活地址时再请求就绪自检接口（仅看状态码）
service_healthy() {
    local i
    for ((i = 1; i <= HEALTH_RETRIES; i++)); do
        systemctl --user is-active --quiet "$DEPLOY_SERVICE" && break
        sleep "$HEALTH_INTERVAL"
    done
    systemctl --user is-active --quiet "$DEPLOY_SERVICE" || return 1
    if [ -z "$DEPLOY_HEALTH_URL" ]; then
        echo "探活降级为仅服务状态检查（待 M1-FR-021 落地后配置 DEPLOY_HEALTH_URL）"
        return 0
    fi
    for ((i = 1; i <= HEALTH_RETRIES; i++)); do
        curl --fail --silent --output /dev/null "$DEPLOY_HEALTH_URL" && return 0
        sleep "$HEALTH_INTERVAL"
    done
    return 1
}

# 安装/更新 unit：ExecStart 改写为实际部署目录；内容有变化才复制并 daemon-reload
install_unit() {
    local src="$1" tmp
    mkdir -p "$(dirname "$UNIT_FILE")"
    tmp="$(mktemp)"
    sed "s|^ExecStart=.*|ExecStart=$DEPLOY_DIR/current/bin/jianartifact|" "$src" > "$tmp"
    if [ ! -f "$UNIT_FILE" ] || ! cmp -s "$tmp" "$UNIT_FILE"; then
        cp "$tmp" "$UNIT_FILE"
        systemctl --user daemon-reload
        echo "已安装/更新 systemd unit：$UNIT_FILE"
    fi
    rm -f "$tmp"
}

# 按目录名（版本-时间戳）排序，取 current 的前一个发布
previous_release() {
    local cur rel prev=""
    cur="$(current_target)"
    [ -n "$cur" ] || return 1
    while IFS= read -r rel; do
        if [ "$DEPLOY_DIR/releases/$rel" = "$cur" ]; then
            [ -n "$prev" ] || return 1
            echo "$DEPLOY_DIR/releases/$prev"
            return 0
        fi
        prev="$rel"
    done < <(ls -1 "$DEPLOY_DIR/releases" | sort)
    return 1
}

# 保留清理：从最旧删起，永不删活动发布及其上一版
cleanup_releases() {
    local cur prev rel count
    cur="$(current_target)"
    prev="$(previous_release || true)"
    count="$(ls -1 "$DEPLOY_DIR/releases" | wc -l)"
    while IFS= read -r rel; do
        [ "$count" -le "$DEPLOY_KEEP_RELEASES" ] && break
        local full="$DEPLOY_DIR/releases/$rel"
        if [ "$full" != "$cur" ] && [ "$full" != "$prev" ]; then
            rm -rf "$full"
            count=$((count - 1))
            echo "已清理过期发布：$rel"
        fi
    done < <(ls -1 "$DEPLOY_DIR/releases" | sort)
}

do_deploy() {
    local tar_path="${2:?用法：remote-install.sh deploy <tar 路径>}"
    [ -f "$tar_path" ] || fail "错误：发布包不存在：$tar_path"
    [ -f "$tar_path.sha256" ] || fail "错误：缺少 sha256 伴随文件：$tar_path.sha256"

    echo "==> 校验发布包完整性"
    (cd "$(dirname "$tar_path")" && sha256sum --check --quiet "$(basename "$tar_path").sha256") \
        || fail "错误：发布包 sha256 校验失败，拒绝部署"

    echo "==> 解压新发布"
    local extract_tmp version rel_base rel_dir suffix=0
    extract_tmp="$(mktemp -d)"
    tar -xzf "$tar_path" --strip-components=1 -C "$extract_tmp"
    version="$(tr -d '[:space:]' < "$extract_tmp/VERSION")"
    rel_base="$DEPLOY_DIR/releases/${version}-$(date +%Y%m%d%H%M%S)"
    rel_dir="$rel_base"
    # 同秒重复部署时追加 -N 后缀，保持目录名可排序且不逃出 releases/
    while [ -e "$rel_dir" ]; do
        suffix=$((suffix + 1))
        rel_dir="${rel_base}-${suffix}"
    done
    mkdir -p "$DEPLOY_DIR/releases"
    mv "$extract_tmp" "$rel_dir"

    check_linger
    local prev
    prev="$(current_target)"
    install_unit "$rel_dir/systemd/jianartifact.service"

    echo "==> 切换到新发布并重启服务"
    switch_current "$rel_dir"
    restart_service

    if service_healthy; then
        cleanup_releases
        echo "部署成功：current -> $rel_dir"
        return 0
    fi

    if [ -n "$prev" ]; then
        echo "健康确认失败，执行自动回滚到上一发布：$prev" >&2
        switch_current "$prev"
        restart_service || true
        fail "错误：本次部署失败，已自动回滚到上一发布"
    fi
    fail "错误：首次部署健康确认失败，且无上一发布可回滚"
}

do_rollback() {
    local prev
    prev="$(previous_release)" || fail "错误：没有更早的发布可回滚"
    check_linger
    echo "==> 回滚到上一发布：$prev"
    switch_current "$prev"
    restart_service
    service_healthy || fail "错误：回滚后健康确认仍失败，请人工介入"
    echo "回滚成功：current -> $prev"
}

case "$MODE" in
    deploy) do_deploy "$@" ;;
    rollback) do_rollback ;;
    *) fail "错误：未知模式 $MODE（支持 deploy / rollback）" ;;
esac
