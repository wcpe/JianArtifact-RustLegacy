# 架构不变量（防架构漂移）

> 以下是本项目锁定的架构约束（依据 `docs/ARCHITECTURE.md` 与 `docs/adr/`）。**违反任一条即为架构漂移。**
> 确需改变某条 → 先写新 ADR 取代旧决策、经确认后再改；**禁止在代码里静默违背**。

## 1. 工作区与模块边界

- 工作区恰有两个可构建子工程根（ADR-0016）：`apps/`（可运行入口：server、web、wiki）与 `packages/`（共享库：ui、devmock、eslint-config、typescript-config）；禁止再新增 `services/`、`libs/` 等第三套子工程根目录。`packages/*` 禁止依赖 `apps/*`。
- 后端模块依赖严格单向：`api` / `protocol`（入口层）→ `auth` / `repository` / `domain` / `migration`（领域服务层）→ `persistence` / `storage`（基础设施层）；`config` 仅被各层只读引用。模块、crate、前端包之间一律禁止循环依赖。
- `protocol` 协议适配层严禁直接读写数据库（含直接使用 sqlx / SQL），必须经领域服务与仓储接口。
- `packages/ui` 禁止依赖 `apps/web`；`ui` 只允许被 `web`、`wiki` 单向依赖。
- 前端禁止自行实现鉴权判定替代后端授权：所有权限拒绝以后端结果为准，前端隐藏入口只是体验优化，不是安全边界。

## 2. 简单优先（禁用重型件）

- 第一期禁止引入：外部数据库、Redis 等中间件、消息队列、S3/对象存储、分布式锁、多节点集群与高可用组件；需要时先走 ADR。
- 单服务清晰分层，不提前拆微型 crate；不以未使用字段、空接口或占位模块为后续分期预留实现。
- 数据库不加无实际用途的占位字段。
- 保持零系统级 C 库依赖（sqlx SQLite bundled、reqwest rustls，不依赖系统 OpenSSL / sqlite3）；新增第三方依赖须先说明理由并经确认。

## 3. 真源与一致性

- SQLite 是元数据唯一真源；blob 目录只存内容本体，数据库只记录索引、状态、大小与校验信息，二者不得互为权威。
- 数据库迁移脚本只向前追加，禁止改写已发布迁移；破坏性 Schema 变更必须先写 ADR 与迁移方案。
- utoipa 生成的 OpenAPI 是管理 API 契约唯一真源；`devmock` 契约必须与其自动比对，禁止手工长期漂移。
- blob 写入提交顺序不变量（所有写入路径一律遵守，禁止跳过或调换）：流式写临时文件边写边算摘要 → 校验 → 落定内容寻址正式路径 → 提交元数据索引 → 索引失败回滚无引用 blob；由提交守卫（含 Drop 兜底、取消安全）保证任何中断都不留孤儿索引或孤儿 blob。

## 4. 技术栈锁定

- 后端：Rust + axum 0.8 + tokio + sqlx（SQLite bundled）+ argon2 + JWT HS256（服务端撤销状态）+ reqwest（rustls）。
- 前端：React + TypeScript strict + Vite + shadcn/ui + Tailwind（组件源码归 `packages/ui`）+ TanStack Table；pnpm workspace + Turborepo。
- 交付形态：rust-embed 编译期内嵌前端产物，单二进制交付。
- 换框架、换存储后端、换交付形态均为架构决策 → 先走新 ADR，不擅自更换。

## 5. 安全不变量

- Rust 生产代码 crate 级 `forbid(unsafe)`。
- 密码、API Token、JWT 密钥、上游与迁移凭据一律不得明文落库、不得进入日志、任务参数快照、前端响应或迁移报告；持久化只存哈希或安全运行时引用。
- 会话撤销表只存域分隔哈希，不存原始会话标识。
- 生产代码禁止临时打印（`println!`、`console.log` 等）替代正式日志；日志与调试输出必须使用中文，禁止英文日志，禁止输出敏感值。

## 6. 并发与性能不变量

- 大文件上传、下载、代理回源与迁移全链路流式处理，缓冲上限固定，不随对象大小线性增长；禁止整体读入内存。
- 持锁期间禁止执行网络、磁盘或数据库等长耗时 IO 操作。
- 同一 proxy 仓库、同一路径的并发缓存未命中必须单飞合并，只产生一次有效上游拉取。
- 禁止在循环中逐项查询数据库（N+1）或逐项创建 HTTP 客户端。
- 迁移任务有限并发，不得挤占正常上传、下载和代理请求资源。

## 7. 工程与流程不变量

- 构建、测试、lint、typecheck 与协议验收全部在 Docker 中执行；宿主机禁止直接运行 `cargo`、`pnpm`、`npm` 等构建/测试命令。
- 宿主项目目录不得出现 `target`、`node_modules`、包管理器缓存或大型测试产物。
- 禁止创建 git worktree 作为并行开发或测试手段。

## 红线（出现即停止并先确认）

新增第二套子工程根 · 模块反向/循环依赖 · protocol 层直连数据库 · `ui` 依赖 `web` · 前端自行鉴权判定 · 引入被禁重型件 · 破坏 SQLite 真源或改写已发布迁移 · 跳过 blob 提交顺序 · 大文件整体读入内存 · 持锁做长耗时 IO · 凭据明文落库或进日志 · 生产代码 `unsafe` · 宿主机跑构建命令或宿主目录出现 `target`/`node_modules` · 创建 git worktree · 循环内逐项查库/建客户端 · 数据库加占位字段 · 临时打印或英文日志 · 静默违背任一已接受 ADR。
