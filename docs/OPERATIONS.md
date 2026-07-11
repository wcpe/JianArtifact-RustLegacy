# 运维手册：JianArtifact

> 运行形态、构建、质量门、部署与回滚、数据目录与日志约定。业务实现尚未开始；运维方式变化时更新本文件。

## 1. 运行形态

- 一个 Rust 服务进程，单一可执行文件 `jianartifact`；React 前端静态产物经 rust-embed 内嵌，部署时不需要独立 Node 运行时。
- 一个 SQLite 元数据库文件（元数据唯一真源）。
- 一个本地文件系统 blob 数据目录（制品与代理缓存本体）。
- TLS 由前置反向代理终止，服务本身不直接持有证书。

## 2. 构建

全部构建在 Docker 容器内执行，宿主机禁止直接运行 cargo / pnpm。

发布构建入口：`make release`（执行 `scripts/build-release.sh` 于容器内）。流程：VERSION 与 Cargo workspace 版本一致性校验 → 前端构建（子工程未落地时跳过）→ `cargo build --release` → ELF x86_64 产物校验 → 打包 `dist/jianartifact-<版本>-x86_64-unknown-linux-gnu.tar.gz`（内含二进制、用户级 systemd unit 模板与 VERSION）及 `.sha256` 伴随文件。`make clean` 清空 `dist/`。

构建顺序硬约束：必须先产出前端 dist，再构建后端二进制（rust-embed 在编译期打包前端静态产物，顺序颠倒会得到缺失前端的二进制或构建失败）；该顺序已由 `scripts/build-release.sh` 固化。

## 3. 测试与质量门

唯一入口：`make check`（等价于 `scripts/check.sh`，Windows 亦可用 `scripts/check.ps1`）。三者均经 `docker compose run --rm dev bash scripts/quality-gate.sh` 在容器内依次执行 Rust 格式检查、clippy 静态分析、测试，以及前端 typecheck / lint / test / build（前端子工程尚未落地、仓库无 `pnpm-lock.yaml` 时自动跳过前端段）。CI 使用同一脚本，各端行为一致。

## 4. 部署与回滚（rootless）

- 入口：`make deploy`（自动依赖 `make release`，防复用陈旧包）与 `make rollback`；配置来自项目根 `.env`（模板 `.env.example`，`.env` 不入库）。
- 目标机要求：Linux + systemd + GNU coreutils，部署用户**无需 root/sudo**；一次性前置为管理员执行 `loginctl enable-linger <部署用户>`（否则服务随登出停止，脚本会中文警告）。
- 目标机布局：`~/jianartifact/releases/<版本>-<时间戳>/` 存放各发布，`current` 符号链接指向活动发布并被用户级 unit 的 `ExecStart` 引用；保留份数由 `DEPLOY_KEEP_RELEASES` 控制（默认 3，活动发布与其上一版永不清理）。
- 健康确认与自动回滚：部署/回滚后服务必须进入 `active`，配置 `DEPLOY_HEALTH_URL`（指向 `/readyz` 就绪自检）后再按状态码探活；健康确认失败自动切回上一发布并非零退出。
- 逻辑测试：`scripts/deploy/test-remote-install.sh` 已纳入质量门，用桩 systemd 覆盖切换、回滚、保留清理与校验失败路径。

## 5. 数据目录与备份

| 备份对象 | 说明 |
|---|---|
| SQLite 元数据库文件 | 含主文件与 `-wal` / `-shm` 伴随文件，必须作为一组一致快照处理 |
| blob 数据目录 | 制品与代理缓存本体，必须与元数据库同一时点备份，保证索引与内容一致 |

只备份其一会造成索引与内容漂移，恢复后出现悬空索引或孤儿 blob。具体目录路径由部署配置决定，默认布局随实现落地后在此登记。

## 6. 日志约定

- 全部日志使用中文。
- 严格分级：ERROR / WARN / INFO / DEBUG，不得混用。
- 禁止输出密码、Token、JWT 密钥、Nexus 凭据等敏感值；凭据仅以引用形态出现在日志与报告中。
- 日志携带必要上下文（请求标识、仓库名、迁移任务标识），避免无意义刷屏。
