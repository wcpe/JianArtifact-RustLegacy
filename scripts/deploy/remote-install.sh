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
DEPLOY_PUBLIC_URL="${DEPLOY_PUBLIC_URL:-}"
DEPLOY_DATABASE_PATH="${DEPLOY_DATABASE_PATH:-data/jianartifact.db}"
# 健康确认的重试参数（逻辑测试会调小以加速）
HEALTH_RETRIES="${HEALTH_RETRIES:-10}"
HEALTH_INTERVAL="${HEALTH_INTERVAL:-2}"
UNIT_FILE="$HOME/.config/systemd/user/${DEPLOY_SERVICE}.service"
# 非登录 SSH 会话通常缺 XDG_RUNTIME_DIR，systemctl --user 需要它
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

fail() { echo "$@" >&2; exit 1; }

if [[ "$DEPLOY_DATABASE_PATH" == *"'"* || "$DEPLOY_DATABASE_PATH" == *$'\n'* || "$DEPLOY_DATABASE_PATH" == *$'\r'* ]]; then
    fail "错误：DEPLOY_DATABASE_PATH 包含不安全字符"
fi
if [ -n "$DEPLOY_HEALTH_URL" ] && [[ ! "$DEPLOY_HEALTH_URL" =~ ^https?://(\[[0-9A-Fa-f:]+\]|[^/:?#[:space:]]+)(:[0-9]+)?(/[^[:space:]]*)?$ ]]; then
    fail "错误：DEPLOY_HEALTH_URL 必须是完整的 http:// 或 https:// URL"
fi

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

warn_setup_link() { echo "警告：$1，已禁止输出初始化链接" >&2; }

resolved_database_path() {
    if [[ "$DEPLOY_DATABASE_PATH" = /* ]]; then
        printf '%s\n' "$DEPLOY_DATABASE_PATH"
    else
        printf '%s\n' "$DEPLOY_DIR/$DEPLOY_DATABASE_PATH"
    fi
}

setup_status_required() {
    local body compact
    if ! body="$(curl --fail --silent --show-error "${DEPLOY_PUBLIC_URL%/}/api/v1/setup/status")"; then
        warn_setup_link "初始化状态查询失败"
        return 1
    fi
    compact="${body//[$' \t\r\n']/}"
    if [ "$compact" = '{"setup_required":true}' ]; then
        return 0
    fi
    if [ "$compact" != '{"setup_required":false}' ]; then
        warn_setup_link "初始化状态响应无效"
    fi
    return 1
}

# 仅在部署健康确认后读取一次性令牌，并只向当前 SSH 标准输出展示链接。
show_initial_setup_link() {
    local token_file token permissions size
    token_file="$(dirname -- "$(resolved_database_path)")/initial-admin.token"
    if [ ! -e "$token_file" ] && [ ! -L "$token_file" ]; then
        return 0
    fi
    if [ ! -f "$token_file" ] || [ -L "$token_file" ]; then
        warn_setup_link "初始化令牌不是普通文件"
        return 0
    fi
    permissions="$(stat --format='%a' -- "$token_file" 2>/dev/null)" || permissions=""
    if [ "$permissions" != "600" ]; then
        warn_setup_link "初始化令牌权限不是 0600"
        return 0
    fi
    size="$(stat --format='%s' -- "$token_file" 2>/dev/null)" || size=""
    token="$(< "$token_file")"
    if [ "$size" != "64" ] || [[ ! "$token" =~ ^[0-9a-f]{64}$ ]]; then
        warn_setup_link "初始化令牌格式无效"
        return 0
    fi
    if [ -z "$DEPLOY_PUBLIC_URL" ]; then
        echo "提示：检测到待使用的初始化令牌，但未配置 DEPLOY_PUBLIC_URL；请在受保护的目标机上按运维流程完成初始化"
        return 0
    fi
    setup_status_required || return 0
    echo "提醒：请尽快在受控浏览器中完成首个管理员初始化；一次性链接如下"
    printf '%s\n' "${DEPLOY_PUBLIC_URL%/}/setup#token=$token"
}

write_runtime_environment() {
    local tmp="$DEPLOY_DIR/jianartifact.env.tmp.$$"
    mkdir -p "$DEPLOY_DIR"
    printf "JIANARTIFACT_DATABASE_PATH='%s'\n" "$DEPLOY_DATABASE_PATH" > "$tmp"
    chmod 600 "$tmp"
    mv "$tmp" "$DEPLOY_DIR/jianartifact.env"
}

# 安装/更新 unit：工作目录、环境文件与启动路径改写为实际部署目录；内容有变化才重新加载
install_unit() {
    local src="$1" tmp
    mkdir -p "$(dirname "$UNIT_FILE")"
    write_runtime_environment
    tmp="$(mktemp)"
    sed -e "s|^WorkingDirectory=.*|WorkingDirectory=$DEPLOY_DIR|" \
        -e "s|^EnvironmentFile=.*|EnvironmentFile=-$DEPLOY_DIR/jianartifact.env|" \
        -e "s|^ExecStart=.*|ExecStart=$DEPLOY_DIR/current/bin/jianartifact|" \
        "$src" > "$tmp"
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
        show_initial_setup_link
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
