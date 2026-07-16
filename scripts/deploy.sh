#!/usr/bin/env bash
# JianArtifact 宿主侧部署入口（M1-FR-043）
# 读取 .env（不入库）→ 上传发布包 → 经 SSH 在目标机执行 remote-install.sh。
# rootless：目标机全程用户级 systemd，无 sudo。凭据不回显、不入日志。
set -euo pipefail

# Windows Git Bash 会把远端 Linux 绝对路径参数改写为本机盘符路径，SSH/SCP 必须禁用该转换。
if [[ "${OSTYPE:-}" == msys* ]]; then
    export MSYS2_ARG_CONV_EXCL="*"
fi

cd "$(dirname "$0")/.."

MODE="${1:?用法：deploy.sh deploy | deploy.sh rollback}"

# 允许测试用 DEPLOY_ENV_FILE 覆盖配置来源
ENV_FILE="${DEPLOY_ENV_FILE:-.env}"
if [ -f "$ENV_FILE" ]; then
    set -a
    # shellcheck disable=SC1090
    . "$ENV_FILE"
    set +a
fi

missing=()
[ -z "${DEPLOY_HOST:-}" ] && missing+=(DEPLOY_HOST)
[ -z "${DEPLOY_USER:-}" ] && missing+=(DEPLOY_USER)
if [ "${#missing[@]}" -gt 0 ]; then
    echo "错误：缺少必填部署配置：${missing[*]}（请在 .env 中配置，模板见 .env.example）" >&2
    exit 1
fi
if [ -n "${DEPLOY_HEALTH_URL:-}" ] && [[ ! "$DEPLOY_HEALTH_URL" =~ ^https?://(\[[0-9A-Fa-f:]+\]|[^/:?#[:space:]]+)(:[0-9]+)?(/[^[:space:]]*)?$ ]]; then
    echo "错误：DEPLOY_HEALTH_URL 必须是完整的 http:// 或 https:// URL" >&2
    exit 1
fi

SSH_OPTS=()
SCP_OPTS=()
if [ -n "${DEPLOY_SSH_PORT:-}" ]; then
    SSH_OPTS+=(-p "$DEPLOY_SSH_PORT")
    SCP_OPTS+=(-P "$DEPLOY_SSH_PORT")
fi
if [ -n "${DEPLOY_SSH_KEY:-}" ]; then
    SSH_OPTS+=(-o IdentitiesOnly=yes -i "$DEPLOY_SSH_KEY")
    SCP_OPTS+=(-o IdentitiesOnly=yes -i "$DEPLOY_SSH_KEY")
fi
REMOTE="$DEPLOY_USER@$DEPLOY_HOST"
# 远端命令使用单引号保护配置值，显式拒绝无法安全表示的单引号。
for name in DEPLOY_DIR DEPLOY_SERVICE DEPLOY_KEEP_RELEASES DEPLOY_HEALTH_URL DEPLOY_PUBLIC_URL DEPLOY_DATABASE_PATH; do
    value="${!name:-}"
    if [[ "$value" == *"'"* ]]; then
        echo "错误：部署配置 $name 不能包含单引号" >&2
        exit 1
    fi
    if [[ "$value" == *$'\n'* || "$value" == *$'\r'* ]]; then
        echo "错误：部署配置 $name 不能包含换行" >&2
        exit 1
    fi
done
# 透传远端配置（空值由远端脚本按默认值处理）
PASS_ENV="DEPLOY_DIR='${DEPLOY_DIR:-}' DEPLOY_SERVICE='${DEPLOY_SERVICE:-}' DEPLOY_KEEP_RELEASES='${DEPLOY_KEEP_RELEASES:-}' DEPLOY_HEALTH_URL='${DEPLOY_HEALTH_URL:-}' DEPLOY_PUBLIC_URL='${DEPLOY_PUBLIC_URL:-}' DEPLOY_DATABASE_PATH='${DEPLOY_DATABASE_PATH:-}'"

case "$MODE" in
    deploy)
        version="$(tr -d '[:space:]' < VERSION)"
        pkg="jianartifact-${version}-x86_64-unknown-linux-gnu.tar.gz"
        [ -f "dist/$pkg" ] || { echo "错误：发布包 dist/$pkg 不存在，请先执行 make release" >&2; exit 1; }
        echo "==> 上传发布包到 $DEPLOY_HOST"
        remote_tmp="$(ssh "${SSH_OPTS[@]}" "$REMOTE" mktemp -d)"
        scp -q "${SCP_OPTS[@]}" "dist/$pkg" "dist/$pkg.sha256" "$REMOTE:$remote_tmp/"
        echo "==> 在目标机执行安装"
        ssh "${SSH_OPTS[@]}" "$REMOTE" "$PASS_ENV bash -s -- deploy '$remote_tmp/$pkg'" \
            < scripts/deploy/remote-install.sh
        ssh "${SSH_OPTS[@]}" "$REMOTE" "rm -rf '$remote_tmp'" || true
        ;;
    rollback)
        echo "==> 在目标机执行回滚"
        ssh "${SSH_OPTS[@]}" "$REMOTE" "$PASS_ENV bash -s -- rollback" \
            < scripts/deploy/remote-install.sh
        ;;
    *)
        echo "错误：未知模式 $MODE（支持 deploy / rollback）" >&2
        exit 1
        ;;
esac
