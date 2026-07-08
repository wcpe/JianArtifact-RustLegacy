#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [ -f ".env" ]; then
  set -a
  # shellcheck disable=SC1091
  . ".env"
  set +a
fi

DEPLOY_PORT="${DEPLOY_PORT:-22}"
DEPLOY_USER="${DEPLOY_USER:-root}"
SSH_KEY_PATH="${SSH_KEY_PATH:-$HOME/.ssh/jianartifact_deploy_ed25519}"
SERVICE_NAME="${SERVICE_NAME:-jianartifact}"
SERVICE_USER="${SERVICE_USER:-jianartifact}"
INSTALL_DIR="${INSTALL_DIR:-/opt/jianartifact}"
CONFIG_DIR="${CONFIG_DIR:-/etc/jianartifact}"
DATA_DIR="${DATA_DIR:-/var/lib/jianartifact}"
LISTEN_ADDR="${LISTEN_ADDR:-0.0.0.0}"
PORT="${PORT:-9999}"
PUBLIC_BASE_URL="${PUBLIC_BASE_URL:-}"
ARTIFACT_TARBALL="${ARTIFACT_TARBALL:-}"
BINARY_NAME="${BINARY_NAME:-}"
JIANARTIFACT_ADMIN_USERNAME="${JIANARTIFACT_ADMIN_USERNAME:-}"
JIANARTIFACT_ADMIN_PASSWORD="${JIANARTIFACT_ADMIN_PASSWORD:-}"

log() {
  printf '[部署] %s\n' "$1"
}

die() {
  printf '[部署] 错误：%s\n' "$1" >&2
  exit 1
}

require_non_empty() {
  local name="$1"
  local value="$2"
  if [ -z "$value" ]; then
    die "请先填写 ${name}。"
  fi
}

escape_quoted_value() {
  local value="$1"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  printf '%s' "$value"
}

write_env_line() {
  local key="$1"
  local value="$2"
  if [ -n "$value" ]; then
    printf '%s="%s"\n' "$key" "$(escape_quoted_value "$value")"
  fi
}

require_non_empty "DEPLOY_HOST" "${DEPLOY_HOST:-}"
require_non_empty "DEPLOY_USER" "$DEPLOY_USER"
require_non_empty "SSH_KEY_PATH" "$SSH_KEY_PATH"
require_non_empty "ARTIFACT_TARBALL" "$ARTIFACT_TARBALL"
require_non_empty "BINARY_NAME" "$BINARY_NAME"

case "$DEPLOY_PORT" in
  ''|*[!0-9]*) die "DEPLOY_PORT 必须是数字。" ;;
esac

case "$PORT" in
  ''|*[!0-9]*) die "PORT 必须是数字。" ;;
esac

[ -f "$SSH_KEY_PATH" ] || die "未找到 SSH 私钥：${SSH_KEY_PATH}"
[ -f "$ARTIFACT_TARBALL" ] || die "未找到部署包：${ARTIFACT_TARBALL}"

mkdir -p .tmp
LOCAL_TMP="$(mktemp -d .tmp/deploy.XXXXXX)"
REMOTE_TMP=""

cleanup() {
  rm -rf "$LOCAL_TMP"
  if [ -n "$REMOTE_TMP" ]; then
    ssh -i "$SSH_KEY_PATH" -p "$DEPLOY_PORT" \
      -o BatchMode=yes \
      -o PasswordAuthentication=no \
      -o IdentitiesOnly=yes \
      "${DEPLOY_USER}@${DEPLOY_HOST}" \
      "rm -rf $(printf '%q' "$REMOTE_TMP")" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

CONFIG_TEMPLATE="$LOCAL_TMP/config.toml"
RUNTIME_ENV="$LOCAL_TMP/jianartifact.env"

{
  printf '[server]\n'
  printf 'listen_addr = "%s"\n' "$(escape_quoted_value "$LISTEN_ADDR")"
  printf 'port = %s\n\n' "$PORT"
  printf '[data]\n'
  printf 'data_dir = "%s"\n' "$(escape_quoted_value "$DATA_DIR")"
} > "$CONFIG_TEMPLATE"

{
  write_env_line "JIANARTIFACT_ADMIN_USERNAME" "$JIANARTIFACT_ADMIN_USERNAME"
  write_env_line "JIANARTIFACT_ADMIN_PASSWORD" "$JIANARTIFACT_ADMIN_PASSWORD"
  write_env_line "JIANARTIFACT_SERVER_PUBLIC_BASE_URL" "$PUBLIC_BASE_URL"
} > "$RUNTIME_ENV"
chmod 600 "$RUNTIME_ENV"

SSH_OPTS=(
  -i "$SSH_KEY_PATH"
  -p "$DEPLOY_PORT"
  -o BatchMode=yes
  -o PasswordAuthentication=no
  -o IdentitiesOnly=yes
  -o StrictHostKeyChecking=accept-new
)
SCP_OPTS=(
  -i "$SSH_KEY_PATH"
  -P "$DEPLOY_PORT"
  -o BatchMode=yes
  -o PasswordAuthentication=no
  -o IdentitiesOnly=yes
  -o StrictHostKeyChecking=accept-new
)
REMOTE="${DEPLOY_USER}@${DEPLOY_HOST}"

shell_quote() {
  local value="$1"
  printf "'"
  printf '%s' "$value" | sed "s/'/'\\\\''/g"
  printf "'"
}

remote_var() {
  local name="$1"
  local value="$2"
  printf '%s=%s ' "$name" "$(shell_quote "$value")"
}

log "检查 SSH 连接。"
REMOTE_TMP="$(ssh "${SSH_OPTS[@]}" "$REMOTE" 'mktemp -d /tmp/jianartifact-deploy.XXXXXX')"
REMOTE_TARBALL="$REMOTE_TMP/jianartifact.tar.gz"
REMOTE_CONFIG="$REMOTE_TMP/config.toml"
REMOTE_ENV="$REMOTE_TMP/jianartifact.env"

log "上传构建产物与运行配置。"
scp "${SCP_OPTS[@]}" "$ARTIFACT_TARBALL" "$REMOTE:$REMOTE_TARBALL" >/dev/null
scp "${SCP_OPTS[@]}" "$CONFIG_TEMPLATE" "$REMOTE:$REMOTE_CONFIG" >/dev/null
scp "${SCP_OPTS[@]}" "$RUNTIME_ENV" "$REMOTE:$REMOTE_ENV" >/dev/null

REMOTE_ENV_PREFIX="$(
  remote_var SERVICE_NAME "$SERVICE_NAME"
  remote_var SERVICE_USER "$SERVICE_USER"
  remote_var INSTALL_DIR "$INSTALL_DIR"
  remote_var CONFIG_DIR "$CONFIG_DIR"
  remote_var DATA_DIR "$DATA_DIR"
  remote_var BINARY_NAME "$BINARY_NAME"
  remote_var PORT "$PORT"
  remote_var REMOTE_TMP "$REMOTE_TMP"
  remote_var REMOTE_TARBALL "$REMOTE_TARBALL"
  remote_var REMOTE_CONFIG "$REMOTE_CONFIG"
  remote_var REMOTE_ENV "$REMOTE_ENV"
)"

log "远端安装并重启 systemd 服务。"
ssh "${SSH_OPTS[@]}" "$REMOTE" "${REMOTE_ENV_PREFIX}bash -s" <<'REMOTE_SCRIPT'
set -euo pipefail

log_remote() {
  printf '[远端部署] %s\n' "$1"
}

die_remote() {
  printf '[远端部署] 错误：%s\n' "$1" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die_remote "远端缺少命令：$1"
}

if [ "$(id -u)" -eq 0 ]; then
  SUDO=()
else
  need_cmd sudo
  SUDO=(sudo -n)
fi

need_cmd systemctl
need_cmd tar
need_cmd mktemp

WORK_DIR="$(mktemp -d /tmp/jianartifact-unpack.XXXXXX)"
cleanup_remote() {
  rm -rf "$WORK_DIR"
  rm -rf "$REMOTE_TMP"
}
trap cleanup_remote EXIT

if id -u "$SERVICE_USER" >/dev/null 2>&1; then
  log_remote "服务用户已存在：$SERVICE_USER"
else
  NOLOGIN=/usr/sbin/nologin
  if [ ! -x "$NOLOGIN" ]; then
    NOLOGIN=/sbin/nologin
  fi
  "${SUDO[@]}" useradd --system --home-dir "$DATA_DIR" --shell "$NOLOGIN" "$SERVICE_USER"
fi

"${SUDO[@]}" install -d -m 755 "$INSTALL_DIR" "$CONFIG_DIR"
"${SUDO[@]}" install -d -m 750 -o "$SERVICE_USER" -g "$SERVICE_USER" "$DATA_DIR"

tar -xzf "$REMOTE_TARBALL" -C "$WORK_DIR"
if [ ! -f "$WORK_DIR/$BINARY_NAME" ]; then
  die_remote "部署包内未找到二进制：$BINARY_NAME"
fi

"${SUDO[@]}" systemctl stop "$SERVICE_NAME" >/dev/null 2>&1 || true

if [ -f "$INSTALL_DIR/jianartifact" ]; then
  "${SUDO[@]}" install -m 755 "$INSTALL_DIR/jianartifact" "$INSTALL_DIR/jianartifact.bak"
fi
"${SUDO[@]}" install -m 755 "$WORK_DIR/$BINARY_NAME" "$INSTALL_DIR/jianartifact"
"${SUDO[@]}" chown root:root "$INSTALL_DIR/jianartifact"

if [ -f "$CONFIG_DIR/config.toml" ]; then
  log_remote "配置文件已存在，保持不覆盖：$CONFIG_DIR/config.toml"
else
  "${SUDO[@]}" install -m 644 "$REMOTE_CONFIG" "$CONFIG_DIR/config.toml"
  "${SUDO[@]}" chown root:root "$CONFIG_DIR/config.toml"
fi

"${SUDO[@]}" install -m 600 "$REMOTE_ENV" "$CONFIG_DIR/jianartifact.env"
"${SUDO[@]}" chown root:root "$CONFIG_DIR/jianartifact.env"
"${SUDO[@]}" chown "$SERVICE_USER:$SERVICE_USER" "$DATA_DIR"

UNIT_TMP="$WORK_DIR/${SERVICE_NAME}.service"
cat > "$UNIT_TMP" <<UNIT
[Unit]
Description=JianArtifact artifact repository
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=${SERVICE_USER}
Group=${SERVICE_USER}
WorkingDirectory=${INSTALL_DIR}
EnvironmentFile=-${CONFIG_DIR}/jianartifact.env
ExecStart=${INSTALL_DIR}/jianartifact --config ${CONFIG_DIR}/config.toml --data-dir ${DATA_DIR}
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
UNIT

"${SUDO[@]}" install -m 644 "$UNIT_TMP" "/etc/systemd/system/${SERVICE_NAME}.service"
"${SUDO[@]}" systemctl daemon-reload
"${SUDO[@]}" systemctl enable "$SERVICE_NAME" >/dev/null
"${SUDO[@]}" systemctl restart "$SERVICE_NAME"
"${SUDO[@]}" systemctl is-active --quiet "$SERVICE_NAME"

if command -v curl >/dev/null 2>&1; then
  HEALTH_OK=0
  for _ in $(seq 1 30); do
    if curl -fsS "http://127.0.0.1:${PORT}/health" >/dev/null 2>&1; then
      HEALTH_OK=1
      break
    fi
    sleep 1
  done
  if [ "$HEALTH_OK" -ne 1 ]; then
    die_remote "服务已启动，但 /health 未在超时时间内就绪。"
  fi
  log_remote "健康检查通过。"
else
  log_remote "远端未安装 curl，已仅完成 systemd active 检查。"
fi
REMOTE_SCRIPT

REMOTE_TMP=""
log "部署完成：${SERVICE_NAME} 已在 ${DEPLOY_HOST} 上运行。"
