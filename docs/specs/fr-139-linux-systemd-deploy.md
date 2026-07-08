# FR-139 规格：Make + Docker 交叉编译 + Linux systemd 自动部署

> 状态：实现完成（待 Docker 联网构建与真机验收） · 关联 PRD：FR-139 · 分支：当前工作区

## 1. 背景与目标

JianArtifact 已提供 Linux 单文件产物，实际部署仍需运维手动完成构建、上传、配置、创建 systemd 服务、启动与健康检查。为降低首部署与后续更新门槛，本功能提供一套基于 Make + Docker 的交叉编译与部署流程：用户本地执行 `make deploy`，Make 调用项目内 Docker 交叉编译环境产出 Linux 包，再通过 SSH key 上传到服务器、安装二进制、写入远端配置与 systemd unit，并启动或重启服务。

目标：把「交叉编译 Linux x86_64 / arm64 产物 → 上传 → systemd 部署」收敛为一条可重复执行的 `make deploy` 流程，同时保持现有架构不变：单一二进制 + `config.toml` + 数据目录 + 环境变量覆盖。

## 2. 需求（要什么）

### 2.1 范围内

- 新增根目录 `Makefile`，提供 `make deploy` 一条命令完成 Docker 交叉编译、上传与 systemd 部署。
- 新增项目内 `Dockerfile.cross`，作为可复现的 Linux 交叉编译环境。
- 支持 Linux 双架构产物：`x86_64-unknown-linux-musl` 与 `aarch64-unknown-linux-musl`。
- 生成一对本地部署专用 SSH 公私钥，默认 Ed25519、无 passphrase。
- 创建本地未跟踪 `.env`，供用户填写服务器连接、目标架构、远端路径、运行端口、首启管理员等部署变量。
- 新增部署脚本，通过 SSH key 连接 Linux systemd 服务器并部署项目。
- 部署脚本默认部署 `make` 刚交叉编译出的目标架构 Linux 包，也允许通过 `.env` 指定现有包路径。
- 远端自动创建 / 更新 systemd service，并使服务开机自启。
- 脚本可重复执行：重复部署不破坏已有数据目录，已有 `config.toml` 默认保留，二进制替换前备份旧文件。
- 明确区分本地部署变量与远端运行环境变量，避免把 SSH 连接信息上传到远端。

### 2.2 不做（范围外）

- 不支持 SSH 密码登录。
- 不自动配置 TLS、域名、反向代理、CDN、WAF 或防火墙。
- 不做 Docker 运行时部署 / Kubernetes / Windows 服务部署；Docker 仅作为本地交叉编译环境。
- 不把 `.env`、SSH 私钥、真实管理员口令或服务器地址提交入库。
- 不扩大应用自更新权限；systemd 部署下的二进制更新以脚本替换为主路径。

## 3. 设计（怎么做）

### 3.1 本地文件与入库边界

| 文件 | 是否入库 | 用途 |
|---|---:|---|
| `Makefile` | 是 | 对外统一入口，提供 `make deploy` / `make cross-build` 等目标 |
| `Dockerfile.cross` | 是 | Linux 双架构交叉编译环境 |
| `scripts/deploy-linux-systemd.sh` | 是 | 部署脚本，负责上传、安装与 systemd 操作 |
| `.env` | 否 | 用户本地填写部署变量，已被 `.gitignore` 忽略 |
| `~/.ssh/jianartifact_deploy_ed25519` | 否 | 本地部署私钥 |
| `~/.ssh/jianartifact_deploy_ed25519.pub` | 否 | 需复制到服务器的公钥 |
| `output/linux/*.tar.gz` | 否 | 本地构建产物，不入库 |

若未来需要可入库的环境模板，应另建 `.env.deploy.example` 并经用户确认；本 FR 默认只创建本地未跟踪 `.env`。

### 3.2 `.env` 变量

`.env` 同时供 `Makefile` 与部署脚本读取，示例键：

```bash
# SSH 连接
DEPLOY_HOST=
DEPLOY_PORT=22
DEPLOY_USER=root

# 构建与目标架构：amd64 → x86_64-unknown-linux-musl；arm64 → aarch64-unknown-linux-musl
DEPLOY_ARCH=amd64
CROSS_IMAGE=jianartifact-cross:local
BUILD_VERSION=0.5.0

# 本地产物；留空时由 make 按 DEPLOY_ARCH 自动推导
ARTIFACT_TARBALL=
BINARY_NAME=

# 远端布局
SERVICE_NAME=jianartifact
SERVICE_USER=jianartifact
INSTALL_DIR=/opt/jianartifact
CONFIG_DIR=/etc/jianartifact
DATA_DIR=/var/lib/jianartifact

# 应用运行
LISTEN_ADDR=0.0.0.0
PORT=9999
PUBLIC_BASE_URL=

# 首启管理员引导（空库首次启动才使用）
JIANARTIFACT_ADMIN_USERNAME=
JIANARTIFACT_ADMIN_PASSWORD=
```

`Makefile` 必须校验 `DEPLOY_ARCH` 只能是 `amd64` 或 `arm64`。部署脚本必须校验 `DEPLOY_HOST`、`DEPLOY_USER`、默认私钥路径、最终产物路径等必填项。敏感项不可打印到日志。

### 3.3 Make 与 Docker 交叉编译流程

根目录 `Makefile` 提供以下目标：

| 目标 | 行为 |
|---|---|
| `make cross-image` | 构建本地交叉编译镜像 `Dockerfile.cross` |
| `make cross-build` | 在 Docker 中构建 `DEPLOY_ARCH` 对应 Linux 包 |
| `make deploy` | 默认入口：先 `cross-build`，再调用部署脚本上传并部署 |
| `make ssh-key` | 生成本地部署 SSH key，已存在则不覆盖 |
| `make env` | 创建本地 `.env`，已存在则不覆盖 |

架构映射：

| `DEPLOY_ARCH` | Rust target | 输出包名建议 |
|---|---|---|
| `amd64` | `x86_64-unknown-linux-musl` | `output/linux/jianartifact-${BUILD_VERSION}-linux-x86_64.tar.gz` |
| `arm64` | `aarch64-unknown-linux-musl` | `output/linux/jianartifact-${BUILD_VERSION}-linux-aarch64.tar.gz` |

`make deploy` 的顺序固定为：

```text
读取 .env
  → 校验 DEPLOY_ARCH
  → docker build Dockerfile.cross
  → docker run 挂载项目目录执行前端构建 + cargo build --release --target <target>
  → 打包 output/linux/*.tar.gz
  → scripts/deploy-linux-systemd.sh 上传并部署该包
```

`Dockerfile.cross` 仅用于构建，不作为运行镜像。镜像内需包含 Rust、目标 Rust target、Node/pnpm、Linux 交叉编译所需工具链。前端构建仍使用项目现有 `frontend/package.json` 与 `frontend/pnpm-lock.yaml`，Rust 构建仍使用 `Cargo.lock`，不引入新的运行时依赖。

### 3.4 SSH 与权限模型

- SSH 客户端强制使用密钥认证：`BatchMode=yes`、`PasswordAuthentication=no`、`IdentitiesOnly=yes`。
- 远端特权执行支持两种路径：
  - `DEPLOY_USER=root`：直接执行特权命令。
  - 非 root：使用 `sudo -n` 执行 `useradd` / `install` / `systemctl` 等命令；若服务器需要交互式 sudo 密码，脚本失败并给出明确提示。
- 脚本不接收、保存或回显服务器密码。

### 3.5 远端安装与幂等策略

远端操作顺序：

1. 创建临时目录并上传 tar.gz。
2. 创建 system 用户（已存在则复用）。
3. 创建目录：`INSTALL_DIR`、`CONFIG_DIR`、`DATA_DIR`。
4. 解包得到 `BINARY_NAME`，安装为 `${INSTALL_DIR}/jianartifact`，权限 `755`。
5. 若旧二进制存在，替换前备份为 `${INSTALL_DIR}/jianartifact.bak`。
6. 若 `${CONFIG_DIR}/config.toml` 不存在，则写入最小配置；若已存在则默认不覆盖。
7. 写入 `${CONFIG_DIR}/jianartifact.env`，权限 `600`，只包含 `JIANARTIFACT_*` 运行环境变量。
8. 写入 `/etc/systemd/system/${SERVICE_NAME}.service`。
9. 执行 `systemctl daemon-reload`、`systemctl enable ${SERVICE_NAME}`、`systemctl restart ${SERVICE_NAME}`。
10. 检查 `systemctl is-active ${SERVICE_NAME}`，并尽量请求 `http://127.0.0.1:${PORT}/health` 验证就绪。

重复执行必须满足：数据目录不删除、不清空；已有配置不被覆盖；新二进制替换失败不应删除旧备份。

### 3.6 systemd unit

生成的 unit 采用以下结构：

```ini
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
```

权限要求：

- `${INSTALL_DIR}`：`755`。
- `${INSTALL_DIR}/jianartifact`：`755`。
- `${CONFIG_DIR}/config.toml`：`644`。
- `${CONFIG_DIR}/jianartifact.env`：`600`。
- `${DATA_DIR}`：归属 `${SERVICE_USER}:${SERVICE_USER}`，服务用户可读写。

### 3.7 远端最小配置

脚本仅在缺失时写入最小 `config.toml`：

```toml
[server]
listen_addr = "${LISTEN_ADDR}"
port = ${PORT}
# public_base_url 可为空；用户需要对外链接时再填写

[data]
data_dir = "${DATA_DIR}"
```

敏感启动项走 `${CONFIG_DIR}/jianartifact.env`，例如：

```bash
JIANARTIFACT_ADMIN_USERNAME=...
JIANARTIFACT_ADMIN_PASSWORD=...
JIANARTIFACT_SERVER_PUBLIC_BASE_URL=...
```

首启后建议用户登录并修改管理员密码，再按运维策略清理或轮换 bootstrap 变量。

## 4. 任务拆分

- [x] 更新 PRD：新增 FR-139 及验收标准。
- [x] 更新 ARCHITECTURE / OPERATIONS / CHANGELOG。
- [x] 新增 `Dockerfile.cross`，支持 Linux amd64 / arm64 交叉编译环境。
- [x] 新增根目录 `Makefile`，提供 `make deploy` 一条命令入口。
- [x] 生成本地 SSH 密钥，输出公钥给用户复制到服务器。
- [x] 创建本地 `.env` 模板；若已存在则不覆盖并提示。
- [x] 新增 `scripts/deploy-linux-systemd.sh`。
- [x] 本地验证 Make 目标、脚本语法与敏感文件不入库。
- [ ] 如用户提供服务器并放置公钥，执行真实部署验证。

## 5. 验收标准

- `make env` 创建本地 `.env` 成功且不被 git 跟踪，内容只含占位值与中文说明，不含真实密钥。
- `make ssh-key` 生成 `~/.ssh/jianartifact_deploy_ed25519(.pub)` 成功；同名密钥已存在时不覆盖。
- `make cross-build DEPLOY_ARCH=amd64` 可经 Docker 产出 Linux x86_64 tar.gz。
- `make cross-build DEPLOY_ARCH=arm64` 可经 Docker 产出 Linux arm64 tar.gz。
- `make deploy` 会先交叉编译当前 `DEPLOY_ARCH`，再调用部署脚本上传该包并部署。
- 部署脚本只使用 SSH key 认证，禁用 SSH 密码认证。
- 脚本能上传 `make` 产出的 tar.gz 并安装包内二进制为远端 `${INSTALL_DIR}/jianartifact`。
- 远端 systemd service 创建成功，`ExecStart` 使用 `--config` 和 `--data-dir` 指向约定路径。
- 重复执行脚本不会清空数据目录，不覆盖已有 `config.toml`，旧二进制会备份。
- 本地验证：`bash -n scripts/deploy-linux-systemd.sh` 通过；`make -n deploy` 能展示预期命令链。
- 若本机安装 `shellcheck`，额外跑 `shellcheck scripts/deploy-linux-systemd.sh`；未安装不阻塞交付。
- 真实服务器验收（需用户提供环境）：`make deploy` 退出码 0，`systemctl is-active ${SERVICE_NAME}` 为 `active`，`GET /health` 成功。

## 6. 风险 / 待定

- Docker 交叉编译需要本机可用 Docker，并需要首次联网拉取基础镜像与下载依赖；离线环境无法首次构建交叉编译镜像。
- `aarch64-unknown-linux-musl` 交叉编译若遇到底层原生依赖编译差异，需在 `Dockerfile.cross` 中补齐对应交叉工具链；本 FR 验收以 Docker 内构建成功为准。
- 服务器若禁用 `sudo -n` 或缺少 systemd，脚本无法完成特权部署；需用户改用 root 或配置免密码 sudo。
- 服务器若没有 `curl`，健康检查可降级为仅检查 `systemctl is-active`，脚本应给出说明。
- systemd 下应用进程通常无权替换 root 拥有的二进制；在线自更新可能不可用，本 FR 以脚本部署更新为准。
- `.env` 作为本地未跟踪文件满足用户“供填写”的要求；是否新增可入库 `.env.deploy.example` 需用户单独确认。
