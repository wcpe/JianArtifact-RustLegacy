# 功能规格：Makefile 构建入口与 SSH+systemd 部署（M1-FR-042 / 043）

> 状态：草拟　·　关联 PRD：M1-FR-042、M1-FR-043　·　分支：feature/build-deploy（042 与 043 分别独立提交）

## 1. 背景与目标

属 M1，ROADMAP 阶段 0 工程基座。为项目提供统一的构建与部署链路：Makefile 作为唯一人机入口（薄封装 Docker），在容器内完成 x86_64 Linux 发布构建并打出可部署的 tar 包；部署脚本经 SSH 把发布包装到 Linux 主机的 systemd 服务。宿主机禁直接运行 cargo/pnpm 的硬约束不变（`.claude/rules/architecture-invariants.md` §7）。

## 2. 需求（要什么）

**FR-042 构建入口与发布构建**

- 根 `Makefile` 提供目标：`help`（默认，列出各目标用途）、`check`（质量门）、`release`（发布构建与打包）、`deploy`（部署，见 FR-043）、`clean`（清发布输出目录）。
- 每个目标都是 Docker 的薄封装：`check` 直接调用既有 `docker compose run --rm dev bash scripts/quality-gate.sh` 链路，与 `scripts/check.sh` / CI 完全同源；`release` 在同一 dev 镜像容器内执行构建脚本。
- `release` 产出（落宿主根 `dist/` 目录，已被 `.gitignore` 的 `dist/` 规则覆盖）：
  - `jianartifact-<版本>-x86_64-unknown-linux-gnu.tar.gz`：内含 `bin/jianartifact`（release 二进制）、`systemd/jianartifact.service`（unit 模板）、`VERSION` 文件；
  - 同名 `.sha256` 校验和伴随文件。
- 版本一致性校验：`release` 开始时校验根 `VERSION` 与 Cargo workspace `package.version` 一致，不一致立即失败（中文错误）；二进制内版本使用 `CARGO_PKG_VERSION`，发版流程负责同步两处（`docs/CONTRIBUTING.md` §8）。
- 构建顺序硬约束：rust-embed 接入后（ROADMAP 阶段 3 起），`release` 必须先产出前端 dist 再执行 `cargo build --release`；接入前 `release` 为纯 Rust 构建，本规格的脚本结构须预留该顺序位（一个先行的前端构建步骤，当前以"前端未落地，跳过"分支存在）。
- 目标架构仅 x86_64（dev 镜像即 amd64 Debian，容器内构建即目标平台构建，无交叉工具链）。
- 范围内：Makefile、容器内构建脚本、tar 打包、unit 模板文件。
- 不做（范围外）：arm64 等其他架构；Docker 运行时镜像交付；快照/Release CI 流水线（随首次发布落地）；cargo-audit/许可证检查接线（阶段 0 独立事项，随首个 Rust 依赖引入）。

**FR-043 部署脚本**

- `make deploy` 调用宿主侧 `scripts/deploy.sh`（bash，经宿主 OpenSSH 客户端执行 ssh/scp；构建部分仍在 Docker 内）。
- 配置全部来自项目根 `.env`（已被 `.gitignore` 排除），提供 `.env.example` 模板（每个变量中文注释、无真实值）：
  - `DEPLOY_HOST`（必填）、`DEPLOY_USER`（必填）、`DEPLOY_SSH_KEY`（可选，缺省走 ssh agent / 默认密钥）、`DEPLOY_DIR`（默认 `/opt/jianartifact`）、`DEPLOY_SERVICE`（默认 `jianartifact`）、`DEPLOY_HEALTH_URL`（可选；未配置时探活步骤警告跳过）。
- 部署流程（幂等，重复执行安全）：
  1. 目标依赖链保证先完成 `release`（防复用陈旧包——上一代踩过的坑）；
  2. 上传 tar 与 `.sha256` 到目标机临时路径，远端校验 sha256，不匹配即失败；
  3. 解压；若 `DEPLOY_DIR/bin/jianartifact` 已存在，先保留为 `jianartifact.prev`（单份，供手动回滚），再原子替换；
  4. 安装/更新 systemd unit（内容有变化才复制并 `daemon-reload`）、`systemctl enable --now` 或 `restart`；
  5. 探活：请求 `DEPLOY_HEALTH_URL` 重试若干次，全失败则非零退出；变量未配置时打印「跳过探活（待 M1-FR-021 健康检查落地后配置）」并成功返回。
- 凭据与主机信息不得进入构建产物与仓库；脚本禁用 `set -x`，输出不回显私钥路径以外的敏感值；SSH 失败、校验失败、服务启动失败均中文报错并非零退出。
- 范围内：deploy 脚本、`.env.example`、unit 模板的安装逻辑。
- 不做（范围外）：多机/滚动部署；自动回滚编排；秘钥管理与分发；TLS 配置（反代职责，见 `docs/OPERATIONS.md`）。

## 3. 设计（怎么做）

- 涉及文件：根 `Makefile`（新增）、`scripts/build-release.sh`（新增，容器内执行）、`scripts/deploy.sh`（新增，宿主执行）、`scripts/deploy/jianartifact.service`（新增，unit 模板）、`.env.example`（新增）、`docs/OPERATIONS.md`（构建与部署章节同步）。
- 不新增子工程与依赖；不改动 `scripts/quality-gate.sh` 与 compose 编排（`release` 复用 dev 服务另跑一条命令）。
- unit 模板要点：`Type=simple`、专用运行用户（文档说明由管理员创建）、`WorkingDirectory=DEPLOY_DIR`、重启策略 `on-failure`；数据目录与配置路径待 config 域落地后在模板中补注释占位说明（不加无用配置项）。
- 换行符：新增 `Makefile`、`*.sh`、unit 模板均须落在 `.gitattributes` 的 LF 规则内（`Makefile` 需补一行 `Makefile text eol=lf`）。
- 相关裁决引用：ADR-0010（质量门与 Docker 化验证）、ADR-0013（单二进制交付与构建顺序）。

## 4. 任务拆分

- [ ] FR-042：Makefile（help/check/release/clean）+ `scripts/build-release.sh` + unit 模板 + 版本一致性校验 + `.gitattributes` 补 Makefile 行
- [ ] FR-042 验收：容器内产出 tar 与 sha256，`file` 验证 ELF x86_64，宿主零构建产物（空挂载点豁免）
- [ ] FR-043：`scripts/deploy.sh` + `.env.example` + Makefile `deploy` 目标（依赖 `release`）
- [ ] FR-043 验收：真机部署 Linux 主机，幂等重跑，失败路径逐项验证；探活项后置（见 §5）
- [ ] 文档同步：PRD FR-042/043 状态流转、`docs/OPERATIONS.md` 构建与部署章节改为 Makefile 入口、CHANGELOG 未发布段各 +1 行

## 5. 验收标准

**FR-042**

- `make check` 与 `scripts/check.sh`、CI 执行同一 `quality-gate.sh`，三端行为一致（对比执行日志）。
- `make release` 后：`dist/` 存在 tar 与 `.sha256`；解包含 `bin/jianartifact`、`systemd/jianartifact.service`、`VERSION`；容器内 `file` 输出证明二进制为 ELF 64-bit x86-64（实机证据，需用户确认）。
- 全程宿主无 cargo/pnpm 进程；宿主目录除命名卷空挂载点与 `dist/` 产物外零新增（对照 ROADMAP 阶段 0 门禁口径）。
- 把 `VERSION` 临时改为不一致值，`make release` 立即失败并给中文错误（红→绿证据）。
- `make clean` 清空 `dist/` 且不触碰其他目录。

**FR-043**

- `.env.example` 覆盖 §2 全部变量、中文注释、无真实值；`.env` 确认被 git 忽略。
- 缺失必填变量时 `make deploy` 立即失败并指出缺哪项（不发起任何连接）。
- 真机（Linux + systemd 主机）验收，需用户确认通过：首次部署后服务 `active (running)`；改动二进制重跑 deploy 平滑更新且 `jianartifact.prev` 为上一版；连续两次 deploy 幂等成功。
- 失败路径：错误主机/端口（SSH 不通）、篡改 tar（sha256 不匹配）、损坏二进制（服务启动失败）→ 均非零退出、中文错误、不留半装状态（unit 与旧二进制不被破坏）。
- 输出与日志全程不出现口令/密钥内容；`DEPLOY_HEALTH_URL` 未配置时明确提示跳过探活。
- **探活后置项**：`/healthz`（M1-FR-021）落地后配置 `DEPLOY_HEALTH_URL` 补验「部署后健康检查通过」，此前 FR-043 不得标记完成验收（对应 ROADMAP 阶段 0 注记）。

## 6. 风险 / 待定

- 宿主需具备 GNU make 与 OpenSSH 客户端（Windows 自带 ssh/scp；make 由用户环境提供）——环境缺失属机器问题，不在仓库内兜底。
- 本机曾出现失效的 `DOCKER_HOST` 残留环境变量导致 docker 不可用（会话内清空可解）；属机器特定问题，Makefile 不为其写补偿逻辑。
- unit 模板中的数据目录、配置文件路径依赖 config 域设计（阶段 3），当前模板仅含运行所需最小字段；config 落地后同步更新模板并在 CHANGELOG 记录。
- 默认部署目录 `/opt/jianartifact`、服务名 `jianartifact`、单份 `.prev` 手动回滚为本规格选定的约定（均可经 `.env` 覆盖）；如需调整在开发前提出。
