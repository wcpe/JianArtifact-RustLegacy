# 运维手册：JianArtifact

> 运行形态、构建、质量门、部署与回滚、数据目录与日志约定。业务实现尚未开始；运维方式变化时更新本文件。

## 1. 运行形态

- 一个 Rust 服务进程，单一可执行文件 `jianartifact`；React 前端静态产物经 rust-embed 内嵌，部署时不需要独立 Node 运行时。
- 一个 SQLite 元数据库文件（元数据唯一真源）。
- 一个本地文件系统 blob 数据目录（制品与代理缓存本体）。
- TLS 由前置反向代理终止，服务本身不直接持有证书。
- 配置加载顺序：代码默认值 → 工作目录 `config.toml`（可选）→ `JIANARTIFACT_` 前缀环境变量（最高优先）。当前字段：`bind`（监听地址，默认 `0.0.0.0:8080`）、`database-path`（SQLite 文件，默认 `data/jianartifact.db`）、`data-dir`（blob 目录，默认 `data/blobs`）、`log-level`（日志级别，默认 `info`）。首次启动还识别 `JIANARTIFACT_ADMIN_USER` / `JIANARTIFACT_ADMIN_PASSWORD`，仅在用户表为空且两项均完整时优先创建首个 Admin；变量值不得写入配置文件、日志或故障报告。
- 辅助模式：`jianartifact openapi` 打印管理 API 的 OpenAPI 契约 JSON 后退出（不启动服务）；`jianartifact admin reset-password <用户名>` 仅打开配置指向的既有数据库并重置本地用户口令（M1-FR-047，开发中）。

## 2. 构建

全部构建在 Docker 容器内执行，宿主机禁止直接运行 cargo / pnpm。

发布构建入口：`make release`（执行 `scripts/build-release.sh` 于容器内）。流程：VERSION 与 Cargo workspace 版本一致性校验 → 前端构建（子工程未落地时跳过）→ `cargo build --release` → ELF x86_64 产物校验 → 打包 `dist/jianartifact-<版本>-x86_64-unknown-linux-gnu.tar.gz`（内含二进制、用户级 systemd unit 模板、VERSION 与根级 `THIRD_PARTY_NOTICES`）及 `.sha256` 伴随文件。`make clean` 清空 `dist/`。

`THIRD_PARTY_NOTICES` 是发布包的许可证 notice 清单。新增、升级或移除需要 notice 的依赖，或调整 `deny.toml` 的包级许可证例外时，发布维护者必须同步核对并更新该文件；不得只放宽许可证门禁而遗漏随包分发的 notice。

构建顺序硬约束：必须先产出前端 dist，再构建后端二进制（rust-embed 在编译期打包前端静态产物，顺序颠倒会得到缺失前端的二进制或构建失败）；该顺序已由 `scripts/build-release.sh` 固化。

## 3. 测试与质量门

唯一入口：`make check`（等价于 `scripts/check.sh`，Windows 亦可用 `scripts/check.ps1`）。三者均经 `docker compose run --rm dev bash scripts/quality-gate.sh` 在容器内依次执行：Rust 格式检查、clippy 静态分析、Rust 测试、部署脚本逻辑测试、Prettier 格式检查（只检不改）与前端四工程 typecheck / lint / test / build（Turborepo 编排与缓存）。CI 使用同一脚本，各端行为一致。

## 4. 部署与回滚（rootless）

- 入口：`make deploy`（自动依赖 `make release`，防复用陈旧包）与 `make rollback`；配置来自项目根 `.env`（模板 `.env.example`，`.env` 不入库）。
- 目标机要求：Linux + systemd + GNU coreutils，部署用户**无需 root/sudo**；一次性前置为管理员执行 `loginctl enable-linger <部署用户>`（否则服务随登出停止，脚本会中文警告）。
- 目标机布局：`~/jianartifact/releases/<版本>-<时间戳>/` 存放各发布，`current` 符号链接指向活动发布并被用户级 unit 的 `ExecStart` 引用；保留份数由 `DEPLOY_KEEP_RELEASES` 控制（默认 3，活动发布与其上一版永不清理）。
- 数据库路径：`DEPLOY_DATABASE_PATH` 默认 `data/jianartifact.db`，相对路径以 `DEPLOY_DIR` 为基准，也可使用绝对路径。部署脚本把该值写入权限为 `0600` 的 `DEPLOY_DIR/jianartifact.env`，由用户级 unit 通过 `JIANARTIFACT_DATABASE_PATH` 加载；首启令牌始终从该数据库文件同目录定位。
- 健康确认与自动回滚：部署/回滚后服务必须进入 `active`，配置 `DEPLOY_HEALTH_URL` 时必须使用指向 `/readyz` 的完整 `http://` 或 `https://` URL，再按状态码探活；相对路径会在连接目标机前被拒绝。健康确认失败自动切回上一发布并非零退出。
- 初始化链接：配置 `DEPLOY_PUBLIC_URL` 后，部署成功只在 `GET <公网地址>/api/v1/setup/status` 明确返回 `{"setup_required":true}` 且令牌文件为权限 `0600` 的普通有效文件时输出一次性 fragment 链接。状态请求失败、响应无效、入口已关闭或令牌异常时均不输出令牌。
- 逻辑测试：`scripts/deploy/test-remote-install.sh` 已纳入质量门，用桩 systemd 覆盖切换、回滚、保留清理、数据库路径、初始化状态门禁与敏感令牌隔离。

## 5. 首个管理员与本地口令恢复

### 5.1 首次启动管理员初始化

1. **自动化部署优先使用环境变量自举**：仅在用户表为空时，同时提供 `JIANARTIFACT_ADMIN_USER` 与 `JIANARTIFACT_ADMIN_PASSWORD`；用户名必须非空，口令按 Unicode 字符数计至少 12 个字符。创建成功后用户表非空，后续启动即使仍保留变量也不会再次创建用户。
2. **未完整提供变量时使用一次性设置页**：服务先在 SQLite 数据库文件同目录完整写入并同步权限为 `0600` 的临时文件，再原子发布 `initial-admin.token`；正式文件内容为 32 字节随机数的 64 位小写十六进制表示，不会暴露半写状态。运维人员从受保护的主机文件读取令牌，在浏览器访问 `https://<服务地址>/setup#token=<令牌>`；不得把真实令牌粘贴到工单、聊天、截图、shell 历史或监控标签中。
3. 设置页先调用 `GET /api/v1/setup/status` 判断入口是否开放，再把 fragment 中的令牌、非空用户名和至少 12 个 Unicode 字符的口令提交到 `POST /api/v1/setup`。成功响应为 204；空请求、损坏 JSON 或错误内容类型返回 400 / `SETUP_REQUEST_INVALID`，用户名为空返回 400 / `SETUP_USERNAME_REQUIRED`，口令过短返回 400 / `SETUP_PASSWORD_TOO_SHORT`；令牌无效、入口已关闭、重复使用或并发被抢先时统一返回 404 / `SETUP_UNAVAILABLE`，并发初始化仅一个请求成功。
4. 创建成功后服务立即删除 `initial-admin.token`，用户表非空使环境变量与设置页两条入口永久关闭。若状态已关闭，不得删除用户表、复制旧令牌文件或修改数据库来重新开放；应使用正常用户管理或 §5.2 的本地重置命令。

一次性令牌只允许出现在 URL fragment 与 setup POST 请求体中。fragment 不随初始 HTTP 请求发送，但浏览器历史、截图和扩展仍可能读取页面地址，因此初始化应在受控终端完成并在成功后关闭页面。服务端日志、反向代理日志与错误响应不得记录令牌、口令或数据库路径。

### 5.2 本地口令重置 CLI

- 交互使用：`jianartifact admin reset-password <用户名>`。命令在终端隐藏输入新口令并要求连续输入两次；两次不一致或少于 12 个 Unicode 字符时非零退出。
- 自动化使用：`jianartifact admin reset-password <用户名> --password-stdin`。标准输入必须来自受控的秘密管理器或等价安全通道，并且只包含单个 UTF-8 行；无效 UTF-8、空输入或多行输入会被拒绝。
- 禁止使用 `--password`，命令也不提供任何等价的命令行明文口令参数，避免口令进入进程列表与 shell 历史。
- 命令使用正常配置解析得到的数据库路径，但只打开既有 JianArtifact SQLite 数据库；路径不存在、数据库无效或用户不存在时非零退出，不创建数据库、用户或首启令牌。
- 成功后只更换 Argon2 口令哈希并将 `token_version` 递增 1：既有 JWT 会话立即失效，用户的角色、启用状态与 API Token 保持不变。标准输出、标准错误和日志不得包含口令、哈希或 Token。

## 6. 数据目录与备份

| 备份对象 | 说明 |
|---|---|
| SQLite 元数据库文件 | 含主文件与 `-wal` / `-shm` 伴随文件，必须作为一组一致快照处理 |
| blob 数据目录 | 制品与代理缓存本体，必须与元数据库同一时点备份，保证索引与内容一致 |

只备份其一会造成索引与内容漂移，恢复后出现悬空索引或孤儿 blob。具体目录路径由部署配置决定，默认布局随实现落地后在此登记。

## 7. 日志约定

- 全部日志使用中文。
- 严格分级：ERROR / WARN / INFO / DEBUG，不得混用。
- 禁止输出密码、Token、JWT 密钥、Nexus 凭据等敏感值；凭据仅以引用形态出现在日志与报告中。
- 日志携带必要上下文（请求标识、仓库名、迁移任务标识），避免无意义刷屏。
