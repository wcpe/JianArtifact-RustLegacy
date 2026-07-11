# 运维手册：JianArtifact

> 运行形态、构建、质量门、数据目录与日志约定。业务实现尚未开始，部署 / 升级 / 回滚的细化步骤随首个可运行版本补充；运维方式变化时更新本文件。

## 1. 运行形态

- 一个 Rust 服务进程，单一可执行文件 `jianartifact`；React 前端静态产物经 rust-embed 内嵌，部署时不需要独立 Node 运行时。
- 一个 SQLite 元数据库文件（元数据唯一真源）。
- 一个本地文件系统 blob 数据目录（制品与代理缓存本体）。
- TLS 由前置反向代理终止，服务本身不直接持有证书。

## 2. 构建

全部构建在 Docker 容器内执行，宿主机禁止直接运行 cargo / pnpm。

构建顺序硬约束：必须先产出前端 dist，再构建后端二进制（rust-embed 在编译期打包前端静态产物，顺序颠倒会得到缺失前端的二进制或构建失败）：

1. 容器内执行 `pnpm install --frozen-lockfile` 与 `pnpm turbo build`，产出各前端子工程 dist。
2. 容器内执行 `cargo build --release`，产出内嵌前端的单二进制。

## 3. 测试与质量门

唯一入口：`scripts/check.sh`（Windows 用 `scripts/check.ps1`）。该入口经 `docker compose run --rm dev bash scripts/quality-gate.sh` 在容器内依次执行 Rust 格式检查、clippy 静态分析、测试，以及前端 typecheck / lint / test / build（前端子工程尚未落地、仓库无 `pnpm-lock.yaml` 时自动跳过前端段）。CI 使用同一脚本，两端行为一致。

## 4. 数据目录与备份

| 备份对象 | 说明 |
|---|---|
| SQLite 元数据库文件 | 含主文件与 `-wal` / `-shm` 伴随文件，必须作为一组一致快照处理 |
| blob 数据目录 | 制品与代理缓存本体，必须与元数据库同一时点备份，保证索引与内容一致 |

只备份其一会造成索引与内容漂移，恢复后出现悬空索引或孤儿 blob。具体目录路径由部署配置决定，默认布局随实现落地后在此登记。

## 5. 日志约定

- 全部日志使用中文。
- 严格分级：ERROR / WARN / INFO / DEBUG，不得混用。
- 禁止输出密码、Token、JWT 密钥、Nexus 凭据等敏感值；凭据仅以引用形态出现在日志与报告中。
- 日志携带必要上下文（请求标识、仓库名、迁移任务标识），避免无意义刷屏。
