# 架构设计：JianArtifact

> 系统当前真貌（HOW）。始终原地更新到现状；结构 / 机制变了就改它。
>
> 当前状态：阶段 3 已完成认证与用户、仓库权限/配置生命周期、BlobStore 与制品索引、Raw hosted/proxy、仓库浏览搜索及就绪探针真实后端；Maven、npm、group、使用片段与迁移仍按路线图逐域实现。实现进度以 [`ROADMAP.md`](ROADMAP.md) 阶段表为准。

## 1. 定位与边界

JianArtifact 是一个以低运维成本为目标的自托管制品仓库，统一管理 Raw、Maven、npm 三种格式的 hosted、proxy 与只读 group 仓库，并承接存量 Nexus Repository OSS 3.70.x 中上述仓库的全量迁移与切换。

第一期目标运行形态（阶段 3 当前已落地 Rust 管理 API、SQLite 认证与仓库配置元数据、就绪探针及独立前端构建）：

- **一个 Rust 服务进程**：当前承载认证、用户、仓库配置/ACL、制品索引与浏览管理 API、Raw hosted/proxy 协议及健康探针；其他格式与迁移随对应业务域接入同一进程。
- **一个 SQLite 元数据库**：当前已作为认证、用户、仓库配置、ACL、group 成员关系与制品索引的唯一真源，通过 sqlx（SQLite bundled）访问，无外部数据库依赖。
- **一个本地文件系统 blob 数据目录**：当前按 SHA-256 内容寻址落盘，并由 `/readyz` 验证挂载可写；SQLite 索引是引用关系唯一真源。
- **前端最终内嵌交付**：当前 React 管理端独立构建，尚未接入 Rust 静态资源路由；后续以编译期内嵌方式收敛为单二进制交付。
- **TLS 在反向代理终止**：服务本身只监听明文 HTTP，可置于任意反向代理之后。

边界之外（详见第 8.2 节非目标）：不做多节点集群、对象存储后端、LDAP/OIDC，Raw/Maven/npm 之外的格式进入后续分期。

## 2. 模块与依赖

### 2.1 工作区结构

pnpm workspace + Turborepo 统一编排，工作区**恰有两个可构建子工程根**（ADR-0016）：`apps/`（可运行入口）与 `packages/`（共享库）；禁止再新增 `services/`、`libs/` 等第三套子工程根目录。`docs/` 与 `scripts/` 只存正式文档和工程脚本，不属于可构建子工程。

| 子工程 | 职责 | 依赖方向 |
|---|---|---|
| `apps/server` | Rust 后端：管理 API、格式协议、存储、认证授权与迁移 | 不依赖任何前端子工程 |
| `apps/web` | React + TypeScript 管理端产品应用，只消费管理 API；默认开发经 Vite 代理直连 Rust，只有 `VITE_MOCK=true` 才启用 devmock；独立 Playwright 配置可启动隔离 SQLite/Rust 后端做真连验收 | 依赖 `ui`、共享配置；显式开发开关下可连 `devmock` |
| `apps/wiki` | UI 组件与业务模式独立预览站，覆盖正常/加载/空/错误/无权限等状态，是视觉回归与无障碍验收入口 | 依赖 `ui`、共享配置 |
| `packages/ui` | 共享设计系统与组件库（shadcn/ui + Tailwind，组件源码归本工程），四层：基础组件、复合组件、页面布局、业务模式 | **只允许被 `web`、`wiki` 单向依赖，禁止依赖 `apps/*`** |
| `packages/devmock` | 浏览器与 Node 双端 Mock、场景包与契约模拟；**其契约必须与后端 OpenAPI 自动比对，防止漂移** | 依赖共享配置 |
| `packages/eslint-config` | 前端共享 ESLint 严格配置 | 被前端子工程引用 |
| `packages/typescript-config` | 前端共享 TypeScript 严格配置 | 被前端子工程引用 |

### 2.2 `apps/server` 内部分层

当前 Rust crate 已落地 `api`、`protocol`、`auth`、`artifact`、`repository`、`domain`、`persistence`、`storage` 等模块。`api::formats` 只承担 axum 提取与 utoipa 契约，Raw 行为位于 `protocol::raw`，并经 `artifact` / `repository` 服务访问基础设施；协议层不直连数据库。以下分层继续供 Maven、npm 与 group 复用，不因格式扩展拆成微型 crate：

```text
        ┌───────────────┐      ┌───────────────┐
        │      api      │      │    protocol   │        入口层
        │  （管理 API） │      │（Raw/Maven/npm│
        │               │      │  协议适配）   │
        └───────┬───────┘      └───────┬───────┘
                │                      │
                ▼                      ▼
   ┌─────────────────────────────────────────────┐
   │ auth │ artifact │ repository │ domain │migration│ 领域服务层
   └───────────────────┬─────────────────────────┘
                       │
            ┌──────────┴──────────┐
            ▼                     ▼
   ┌────────────────┐    ┌────────────────┐
   │  persistence   │    │    storage     │           基础设施层
   │   （SQLite）   │    │ （blob 存储）  │
   └────────────────┘    └────────────────┘

   config：静态配置，被各层只读引用，不反向依赖任何模块
```

| 模块 | 职责 |
|---|---|
| `api` | 管理 API 路由与 handler（axum），鉴权提取、请求校验、统一错误映射；utoipa 注解在此生成 OpenAPI |
| `protocol` | Raw、Maven、npm 格式协议适配：路径解析、协议元数据生成、checksum sidecar、packument 合并等；**严禁直接读写数据库，必须经领域服务与仓储接口** |
| `auth` | 认证与授权：Argon2 口令校验、JWT 会话签发与撤销校验、API Token、首启一次性令牌校验、本地口令重置、授权判定 |
| `artifact` | 共享制品服务：索引原子提交、带租约读取、引用安全回收、proxy 单飞与长生命周期上游客户端 |
| `repository` | 仓库领域服务：仓库 CRUD、配置失效、浏览搜索、group 成员关系与 ACL 应用 |
| `domain` | 核心领域类型与纯逻辑：仓库/制品/用户模型、路径规范化、覆盖策略、状态机 |
| `migration` | Nexus 迁移：在线/离线发现、计划、执行、断点续传、报告 |
| `persistence` | SQLite 访问层：仓储接口实现、事务、迁移脚本 |
| `storage` | blob 存储：内容寻址落盘、临时文件、提交守卫、读写租约、删除许可与启动恢复 |
| `config` | 启动配置装载与校验，只读提供给各层 |

入口层（`api`、`protocol`）只调用领域服务层（`auth`、`artifact`、`repository`、`domain`、`migration`）；领域服务层使用 `persistence` 与 `storage`，协议层不得直接访问 SQL；禁止任何反向依赖与循环依赖。

## 3. 数据模型

核心表族概览（**字段与约束以数据库迁移脚本为准，schema 只向前追加**；不为后续分期加无用途占位字段）：

| 表族 | 内容 |
|---|---|
| 用户 | 账号、Argon2id 密码哈希、启用状态、全局角色（Admin/User）、会话版本 `token_version`；改密或重置时原子递增版本，使全部既有 JWT 失效 |
| API Token | Token 域分隔哈希（明文只在签发时显示一次）、名称、创建元数据；吊销即删除记录 |
| 会话撤销 | 已撤销会话标识的**域分隔哈希**与过期时间，支撑退出与刷新旧会话的即时失效 |
| 仓库 | 格式（raw/maven2/npm）、类型（hosted/proxy/group）、可见性（public/private）、状态、proxy 上游配置与凭据引用状态 |
| group 成员 | group 仓库到成员仓库的有序关系 |
| 仓库 ACL | 用户 × 仓库 × 动作（read/write/admin） |
| 制品索引 | `repo_id + path` 唯一映射到 SHA-256 内容，记录 SHA-1/MD5/SHA-512、大小、正缓存标记与创建/更新时间；blob 引用数按 SHA-256 聚合，不另设计数表 |
| blob 回收意图 | 以 SHA-256 为主键持久化待回收内容及正整数 `generation`；代次隔离同摘要的新旧任务，陈旧任务不得误删文件或清除新意图 |
| 迁移任务 | 任务标识、不可变参数快照、状态机状态、节流进度、幂等信息 |
| 迁移分仓明细 | 每仓库的对象/字节计数、校验结果、凭据引用状态、有界失败样本 |

M1-FR-046 的 `initial-admin.token` 不属于数据表：它是仅在用户表为空且未完整提供环境变量自举凭据时生成的数据库同目录一次性文件，保存 32 字节随机数的十六进制表示，权限固定为 `0600`。创建时先把完整内容写入并同步同目录 `0600` 临时文件，再以同文件系统硬链接原子发布正式路径，避免崩溃留下半写令牌。用户表非空是首启入口永久关闭的权威状态，不能以令牌文件存在与否反向开启功能。

## 4. blob 存储与提交模型

沿用已验证的设计：

- **内容寻址**：blob 按 SHA-256 内容寻址，取哈希前两位分桶存储（如 `ab/cdef...`）；数据库只存索引与校验和，不存内容本体。相同内容天然去重。
- **写入提交顺序不变量**（hosted 上传与 proxy 回源共用）：
  1. 随机排他创建 `.part`，流式写入并同步计算 SHA-256/SHA-1/SHA-512/MD5 与实际大小；
  2. 执行大小、磁盘阈值与路径校验；
  3. 先持久化提交守卫，再以同文件系统硬链接原子发布内容寻址正式路径；已存在同摘要时复用正式 blob；
  4. 在 SQLite 短事务内提交或覆盖制品索引；
  5. 索引成功后移除守卫；索引失败时取得删除许可、重新核对全局引用并立即清理，无引用清理失败则由启动恢复兜底。
- **提交守卫与启动恢复**：请求中断时 `.part` 由 Drop 清理；正式 blob 已发布但索引尚未提交时保留守卫。启动会删除残留 `.part`，并只删除“有守卫且无数据库引用”的正式 blob，保证 SQLite 始终是引用真源。
- **租约与删除许可**：读取、落定与回收按 SHA-256 协调。活跃租约阻止物理删除；删除请求建立后阻止新租约并等待旧租约释放，取得许可后再次批量核对引用。多摘要回收按稳定顺序获取许可，避免交叉回收死锁；空闲协调状态即时释放，避免按对象数增长。
- **持久化回收意图**：索引覆盖、单文件删除、proxy URL 缓存失效与仓库 `mode=data` 删除都在删除/替换索引的同一 SQLite 事务内登记回收意图；`mode=config` 在删除配置与索引前同事务取消相关意图，明确保留 orphan blob。每次重复登记递增 `generation`，回收器只按匹配代次核对引用和完成意图，陈旧任务不得误删或误清新任务。
- **取消安全与恢复顺序**：事务提交后即使请求在等待租约、删除文件或清理意图时被取消，意图仍保留在数据库。启动先收口 `.part` 与提交守卫，再在开放路由前处理全部持久化回收意图；文件已删除但意图未清时按不存在幂等完成，普通无守卫且无回收意图的 orphan blob 保持不动。

## 5. 接口

详细契约见 `docs/API.md`，此处只给概览与定位：

- **管理 API**：`apps/web` 消费的 JSON API；认证、用户、Token、仓库配置生命周期、ACL、制品统计、目录浏览与路径搜索已真实现，首启管理员状态/提交接口（M1-FR-046）、使用片段及迁移按各自开发状态推进。setup 页面只从 `/setup#token=<令牌>` 的 fragment 取值并向 `/api/v1/setup` 请求体提交，令牌不进入请求路径或 query。仓库列表在 SQL 中按 read 权限过滤并聚合制品数/总大小，私有无权与不存在返回一致错误，group 详情仅返回调用方可读成员。`devmock` 双向契约测试只比较 `/api/v1` 管理业务路径。
- **格式协议端点**：Raw hosted/proxy 已由 `protocol::raw` 实现 GET/HEAD/PUT/DELETE 与 JSON 目录索引，`api::formats` 只负责提取请求和 OpenAPI。Maven（标准布局 + metadata + checksum sidecar）、npm（publish/packument/tarball/dist-tag）与 group 后续按域接入同一 `ArtifactService`，协议层始终不得直连数据库。
- **健康检查**：`/healthz` 只报告进程存活；`/readyz` 以临时事务写入并回滚探测 SQLite，以随机且排他创建的临时文件探测 blob 目录，运行期挂载丢失时不自愈重建，响应不泄露路径、凭据或内部拓扑。

## 6. 关键机制

### 6.1 认证、会话与授权

- 密码使用 Argon2id，参数固定为 memory 19456 KiB、iterations 2、parallelism 1。
- Web 会话采用 JWT（HS256）+ **服务端撤销状态 + 用户会话版本**：每次请求同时校验签名、到期时间、jti/sid 撤销哈希、`token_version` 与用户启用状态；刷新原子消费当前 jti 并沿用 sid，退出撤销整个 sid 会话家族，改密/重置口令原子递增版本并撤销该用户全部既有会话。同一旧令牌并发刷新仅一个请求成功，刷新与退出竞态不得遗留有效新令牌。
- API Token 仅持久化 `sha256("api-token:" + 明文)`；Bearer `jat_` 与 Basic 口令位均可使用，吊销或用户禁用后下一次请求立即失败。
- Basic 口令校验仅缓存 Argon2 正向结果 60 秒，缓存键绑定口令、当前 PHC 与 `token_version`；身份、角色和启用状态仍逐请求查库。
- 管理端通过 `GET /api/v1/auth/me` 获取权威用户角色，不按用户名猜测；前端隐藏入口仅为体验优化。
- **首启管理员初始化（M1-FR-046，开发中）**：服务打开数据库并检查用户表后，优先使用完整的 `JIANARTIFACT_ADMIN_USER` / `JIANARTIFACT_ADMIN_PASSWORD` 创建首个 Admin；空表且变量不完整时生成数据库同目录的 `initial-admin.token`，由 `/setup#token=<令牌>` 页面调用无鉴权 status/submit API。令牌校验、空表复核与 Admin 写入使用同一 SQLite 原子边界，保证并发仅一个成功；成功后删除令牌文件，用户表非空使两条入口永久关闭。
- **本地口令重置（M1-FR-047，开发中）**：CLI 参数解析与隐藏终端/标准输入读取位于进程入口，口令校验、Argon2 哈希与用户更新复用认证领域服务。CLI 只打开配置指向的既有数据库；短事务只更新 `password_hash` 并递增 `token_version`，角色、启用状态与 API Token 表不参与写入。Argon2 计算在事务外执行，避免持有 SQLite 写锁进行高开销计算。
- **授权判定为无 IO 纯函数**：输入主体（匿名/用户/Admin）、仓库可见性与 ACL、动作，输出允许/拒绝，便于穷举测试整张权限矩阵。前端不复制授权规则，权限拒绝一律以后端结果为准。

### 6.2 proxy 缓存与单飞

同一 proxy 仓库、同一路径、同一规范化 `upstream_url` 配置快照的并发缓存未命中由 `ArtifactService` 进程内注册表**单飞合并**：URL 更新前后的请求进入独立 flight，不互相等待；锁内只登记任务，网络、磁盘和数据库 IO 全在锁外。领导任务脱离首个客户端生命周期执行，等待者共享同一成功或错误结果；领导者取得资格后再次检查本地索引，消除“先 miss、后登记”的竞态。

正缓存提交携带预期 `upstream_url`，并在同一 SQLite 事务内通过条件写入核对仓库仍存在、类型仍为 proxy 且 URL 与快照一致。若配置更新先提交，旧回源的缓存提交被拒绝且不会生成索引；协议层随后重新读取仓库与授权状态，按新配置最多透明重试一次，重试前仍先查本地缓存。若第二次提交期间配置再次变化，则返回稳定 503，避免无限循环。

Raw proxy 使用进程内长生命周期 reqwest/rustls 客户端，连接超时 10 秒、首响应 30 秒、分块空闲 60 秒，禁止重定向与客户端自动重试；上述一次重试只针对本地仓库配置竞态，不重试上游传输。上游凭据只持久化环境变量引用，运行时仅接受完整 Basic/Bearer Authorization 并只注入该头。2xx 内容完整流式落盘并提交正缓存后才从本地 blob 响应；不实现负缓存。404、连接/超时、3xx、其他 4xx、5xx 与断流映射为稳定类别，失败不产生索引。`upstream_url` 变化、正缓存索引删除与回收意图登记在同一 SQLite 事务内完成；只改凭据引用不失效缓存。

### 6.3 迁移状态机与持久化

- 迁移任务异步执行，状态机：`queued / running / pausing / paused / cancelling / cancelled / succeeded / partial / failed / interrupted`。仅零显式放弃、零未迁移受支持对象、零未解决配置项时可标记 `succeeded`。
- 沿用已验证的设计：**任务表 + 分仓库明细表**持久化任务参数快照、节流进度、对象检查点与有界失败样本；服务重启后在途任务标记为 `interrupted`（已中断），可一键续传。
- 暂停/取消只在单对象原子提交边界生效；取消清理未提交临时文件，但不删除已成功且被引用的数据。

### 6.4 幂等键与增量续传

以**稳定源标识 + 目标路径 + 内容校验**构成幂等键。重跑只处理未完成、失败或变化的对象；已一致对象精确计为跳过，不产生重复索引或孤儿 blob。

### 6.5 迁移来源与一致性

迁移来源分三类（ADR-0015 调整了默认与构成）：

- **离线直读 Nexus 原生数据目录（首选）**：数百 GB 的 blob 大头从磁盘上的 Nexus 文件型 blob store（`.bytes` + `.properties`）直接读取与归属，不逐个 HTTP 拉；少量仓库配置（proxy 上游、group 成员）经 REST 一次性拉取（Nexus 可达时）或由管理员在向导手动补（Nexus 已下线时）。不直读 Nexus 内嵌配置库（H2/OrientDB 为 JVM 私有格式，纯 Rust 不可行）。原生数据目录一致性由「迁移前停 Nexus 或取文件系统快照后再拷/挂载」保证。
- **在线 REST（备选）**：数据不在本机但 Nexus 可达时，走 REST + 格式端点 + blob 下载。采用「**首轮固化快照 → 源端停写窗口 → 最终增量扫描**」两阶段一致性：只有最终清单零未处理漂移（新增/修改/删除）时才能标记全量成功；默认不自动传播源端删除，删除项显式列入切换报告由管理员决定。
- **自有离线迁移包（可选归档）**：可移植、带整体校验的自有格式（ADR-0008），用于异地交付/审计。

裸 blob store（仅 blob、无 REST 也无手动配置）为降级路径：hosted 与已缓存内容从磁盘尽力恢复，proxy 上游/group 成员缺失的仓库标「配置待补充」，任务总状态不得全量成功。

### 6.6 迁移并发控制

迁移任务有限并发执行，不得挤占正常上传、下载和代理请求资源；运行中按配置的磁盘剩余阈值停止接收新对象，当前原子对象完成后进入可恢复失败态。

## 7. 部署

- 目标交付物为**单个 Rust 二进制**（前端产物编译期内嵌）；当前阶段 Rust 服务与前端仍分别构建，静态资源内嵌待后续部署域落地。后端保持零系统级 C 库依赖（sqlx SQLite bundled，不依赖系统 sqlite3），Rust 生产代码 `forbid(unsafe)`。
- 当前运行时依赖为 SQLite 文件、就绪探针使用的可写数据目录与启动配置；blob 目录语义随存储域落地。
- TLS 由反向代理终止；服务自身不管理证书。
- 首次启动先尝试环境变量管理员自举；空用户库且变量不完整时，以数据库同目录 `initial-admin.token` 和 `/setup#token=<令牌>` 完成首个管理员初始化。任一方式成功后用户表非空，入口永久关闭并删除令牌文件。
- 主机管理员可执行 `jianartifact admin reset-password <用户名>` 重置本地用户口令；命令只打开既有数据库，不提供命令行明文口令参数，也不改变角色、启用状态与 API Token。
- 构建、测试、lint、typecheck 与协议验收统一在 Docker 中执行，宿主项目目录不产生 `target`、`node_modules` 或大型测试产物。

## 8. 关键裁决与不做项

### 8.1 关键裁决（详见对应 ADR）

| ADR | 裁决 |
|---|---|
| `docs/adr/0001-tech-stack-and-workspace.md` | 后端 axum 0.8 + tokio + sqlx + reqwest(rustls)，前端 React + TS strict + Vite，pnpm workspace + Turborepo，`apps/*` 唯一子工程根 |
| `docs/adr/0002-sqlite-metadata-and-blob-storage.md` | SQLite 元数据唯一真源 + 本地文件系统内容寻址 blob |
| `docs/adr/0003-authentication-and-session.md` | Argon2 口令哈希，JWT HS256 会话 + 服务端撤销状态，API Token 只存哈希 |
| `docs/adr/0004-authorization-model.md` | Admin/User 全局角色 + 仓库 read/write/admin ACL，授权判定为无 IO 纯函数 |
| `docs/adr/0005-repository-types.md` | hosted/proxy/只读 group 三类仓库模型与聚合语义 |
| `docs/adr/0006-protocol-adapter-boundary.md` | 协议适配层禁直连数据库，必须经领域服务与仓储接口 |
| `docs/adr/0007-nexus-migration-strategy.md` | Nexus OSS 3.70.x 迁移策略、全量成功完成定义与两阶段一致性（入口/默认经 0015 调整） |
| `docs/adr/0008-offline-migration-package.md` | 自有离线迁移包契约与裸 blob 尽力恢复降级（0015 后降为可选归档） |
| `docs/adr/0009-migration-task-persistence.md` | 任务表 + 分仓明细表持久化，重启在途任务标已中断 |
| `docs/adr/0010-quality-gates-and-docker.md` | 质量硬门禁与 Docker 容器化验证约束 |
| `docs/adr/0011-ui-design-system.md` | shadcn/ui + Tailwind 设计系统，组件源码归 `packages/ui`，TanStack Table |
| `docs/adr/0012-api-contract-source.md` | utoipa 生成同一 OpenAPI 作为服务端 HTTP 契约唯一真源；devmock 只双向比对 `/api/v1` 管理业务路径 |
| `docs/adr/0013-single-binary-delivery.md` | rust-embed 编译期内嵌前端产物，单二进制交付 |
| `docs/adr/0014-frontend-first-delivery.md` | M1 按域三步工序：契约骨架 → 前端 + devmock → 后端业务实现，契约真源仍为 ADR-0012 |
| `docs/adr/0015-offline-native-datadir-primary.md` | 迁移首选离线直读 Nexus 原生数据目录（blob 磁盘直读 + 配置 REST/手动），在线 REST 降为备选 |
| `docs/adr/0016-apps-packages-two-roots.md` | 工作区改为 apps/（入口）+ packages/（共享库）双根，取代 ADR-0001 的单一 apps/ 根 |

### 8.2 第一期非目标

- 不迁移 Nexus 用户、角色、权限、LDAP/OIDC 配置、任务计划、脚本、邮件、Webhook 或其他系统配置。
- 不支持 Docker/OCI、Cargo、PyPI、Go、NuGet、RubyGems、Conan、Terraform、Ansible、Pub 等协议。
- 不提供 S3/对象存储后端。
- 不提供 LDAP/OIDC 登录。
- 不提供多节点集群、分布式锁或高可用控制面。
- 不提供复杂保留策略、自动 GC、备份恢复编排、漏洞扫描、WAF、CC 挑战或使用分析面板。
- 不为后续能力在第一期数据库中加入无实际用途的占位字段。
