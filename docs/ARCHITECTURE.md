# 架构设计：JianArtifact

> 系统当前真貌（HOW）。始终原地更新到现状；结构 / 机制变了就改它。
>
> 当前状态：业务实现尚未开始，本文描述的是已裁决的目标架构；实现进度以 [`ROADMAP.md`](ROADMAP.md) 阶段表为准。

## 1. 定位与边界

JianArtifact 是一个以低运维成本为目标的自托管制品仓库，统一管理 Raw、Maven、npm 三种格式的 hosted、proxy 与只读 group 仓库，并承接存量 Nexus Repository OSS 3.70.x 中上述仓库的全量迁移与切换。

第一期运行形态：

- **一个 Rust 服务进程**：管理 API、格式协议端点、存储与迁移全部在同一进程内。
- **一个 SQLite 元数据库**：元数据唯一真源，通过 sqlx（SQLite bundled）访问，无外部数据库依赖。
- **一个本地文件系统 blob 数据目录**：制品内容本体按内容寻址存放。
- **rust-embed 编译期内嵌前端产物**：React 管理端静态资源在构建时嵌入二进制，单二进制交付，部署不需要独立 Node 运行时或前端服务。
- **TLS 在反向代理终止**：服务本身只监听明文 HTTP，可置于任意反向代理之后。

边界之外（详见第 8.2 节非目标）：不做多节点集群、对象存储后端、LDAP/OIDC，Raw/Maven/npm 之外的格式进入后续分期。

## 2. 模块与依赖

### 2.1 工作区结构

pnpm workspace + Turborepo 统一编排，**`apps/*` 是唯一的可构建子工程根**；禁止新增 `packages/*`、`services/*` 等第二套子工程根目录。`docs/` 与 `scripts/` 只存正式文档和工程脚本，不属于可构建子工程。

| 子工程 | 职责 | 依赖方向 |
|---|---|---|
| `apps/server` | Rust 后端：管理 API、格式协议、存储、认证授权与迁移 | 不依赖任何前端子工程 |
| `apps/web` | React + TypeScript 管理端产品应用，只消费管理 API | 依赖 `ui`、共享配置；开发开关下可连 `devmock` |
| `apps/ui` | 共享设计系统与组件库（shadcn/ui + Tailwind，组件源码归本工程），四层：基础组件、复合组件、页面布局、业务模式 | **只允许被 `web`、`wiki` 单向依赖，禁止依赖 `web`** |
| `apps/wiki` | UI 组件与业务模式独立预览站，覆盖正常/加载/空/错误/无权限等状态，是视觉回归与无障碍验收入口 | 依赖 `ui`、共享配置 |
| `apps/devmock` | 浏览器与 Node 双端 Mock、场景包与契约模拟；**其契约必须与后端 OpenAPI 自动比对，防止漂移** | 依赖共享配置 |
| `apps/eslint-config` | 前端共享 ESLint 严格配置 | 被其余前端子工程引用 |
| `apps/typescript-config` | 前端共享 TypeScript 严格配置 | 被其余前端子工程引用 |

### 2.2 `apps/server` 内部分层

第一期保持单服务、清晰模块分层，不因未来格式扩展提前拆成微型 crate。至少划分以下模块，依赖严格单向：

```text
        ┌───────────────┐      ┌───────────────┐
        │      api      │      │    protocol   │        入口层
        │  （管理 API） │      │（Raw/Maven/npm│
        │               │      │  协议适配）   │
        └───────┬───────┘      └───────┬───────┘
                │                      │
                ▼                      ▼
   ┌─────────────────────────────────────────────┐
   │   auth  │  repository  │  domain  │migration│   领域服务层
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
| `auth` | 认证与授权：Argon2 口令校验、JWT 会话签发与撤销校验、API Token、授权判定 |
| `repository` | 仓库领域服务：仓库 CRUD、hosted/proxy/group 行为、group 成员聚合、ACL 应用 |
| `domain` | 核心领域类型与纯逻辑：仓库/制品/用户模型、路径规范化、覆盖策略、状态机 |
| `migration` | Nexus 迁移：在线/离线发现、计划、执行、断点续传、报告 |
| `persistence` | SQLite 访问层：仓储接口实现、事务、迁移脚本 |
| `storage` | blob 存储：内容寻址落盘、临时文件、提交守卫 |
| `config` | 启动配置装载与校验，只读提供给各层 |

入口层（`api`、`protocol`）只调用领域服务层（`auth`、`repository`、`domain`、`migration`）；领域服务层通过仓储接口使用 `persistence` 与 `storage`；禁止任何反向依赖与循环依赖。

## 3. 数据模型

核心表族概览（**字段与约束以数据库迁移脚本为准，schema 只向前追加**；不为后续分期加无用途占位字段）：

| 表族 | 内容 |
|---|---|
| 用户 | 账号、Argon2 密码哈希、启用状态、全局角色（Admin/User） |
| API Token | Token 哈希（明文只在签发时显示一次）、名称、创建/吊销元数据 |
| 会话撤销 | 已撤销会话的**域分隔哈希**与撤销时间，支撑退出/禁用后下一次请求必定失败 |
| 仓库 | 格式（raw/maven2/npm）、类型（hosted/proxy/group）、可见性（public/private）、状态、proxy 上游配置与凭据引用状态 |
| group 成员 | group 仓库到成员仓库的有序关系 |
| 仓库 ACL | 用户 × 仓库 × 动作（read/write/admin） |
| 制品索引 | 仓库内路径/坐标到 blob sha256 的索引、大小、校验和、时间戳 |
| 迁移任务 | 任务标识、不可变参数快照、状态机状态、节流进度、幂等信息 |
| 迁移分仓明细 | 每仓库的对象/字节计数、校验结果、凭据引用状态、有界失败样本 |

## 4. blob 存储与提交模型

沿用已验证的设计：

- **内容寻址**：blob 按 sha256 内容寻址，取哈希前两位分桶存储（如 `ab/cdef...`）；数据库只存索引与校验和，不存内容本体。相同内容天然去重。
- **写入提交顺序不变量**（所有写入路径一律遵守，禁止跳过或调换）：
  1. 流式写入临时文件，边写边计算摘要（全链路流式，缓冲上限固定）；
  2. 校验摘要与预期（如客户端声明的 checksum）；
  3. 将临时文件落定到内容寻址正式路径（原子重命名）；
  4. 写入元数据索引；
  5. 索引提交失败时回滚无引用 blob。
- **提交守卫**：写入流程由提交守卫对象包裹，含 Drop 兜底与取消安全语义——请求中断、任务取消或进程异常终止都不得留下可见的孤儿索引或孤儿 blob；临时文件可识别、可回收且不会被正常读取路径命中。
- **所有权明确**：同一 blob 的写入、索引提交与清理有明确所有权，禁止竞争删除仍被其他制品引用的内容。

## 5. 接口

详细契约见 `docs/API.md`，此处只给概览与定位：

- **管理 API**：`apps/web` 消费的 JSON API，覆盖认证、用户、Token、仓库、ACL、浏览与迁移。**utoipa 生成的 OpenAPI 是管理 API 契约唯一真源**，`devmock` 契约测试自动与其比对。
- **格式协议端点**：Raw（HTTP 上传/下载/浏览）、Maven（标准布局 + metadata + checksum sidecar）、npm（publish/packument/tarball/dist-tag）原生协议，官方客户端（curl、Maven、npm）无需理解内部管理 API。支持 Basic Auth 与 Bearer/API Token。
- **健康检查**：进程健康与基本就绪状态，响应不泄露路径、凭据或内部拓扑。

## 6. 关键机制

### 6.1 认证、会话与授权

- 密码使用内存安全参数的 Argon2 哈希。
- Web 会话采用 JWT（HS256）+ **服务端撤销状态**：沿用已验证的设计，撤销表只存域分隔哈希；退出、吊销或禁用用户提交成功后，下一次请求必定失败（会话即时撤销），不影响其他会话或 API Token。
- **授权判定为无 IO 纯函数**：输入主体（匿名/用户/Admin）、仓库可见性与 ACL、动作，输出允许/拒绝，便于穷举测试整张权限矩阵。前端不复制授权规则，权限拒绝一律以后端结果为准。

### 6.2 proxy 缓存与单飞

同一 proxy 仓库、同一路径的并发缓存未命中必须**单飞合并**：只发起一次有效上游拉取，其余请求等待并共享结果；回源内容流式落盘并走第 4 节提交流程。上游错误分类（不存在、超时、4xx/5xx、断流）与负缓存策略由 `repository` 层统一处理。

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

- 交付物为**单个 Rust 二进制**（前端产物经 rust-embed 编译期内嵌），零系统级 C 库依赖（sqlx SQLite bundled 编译期内嵌、reqwest rustls，不依赖系统 OpenSSL / sqlite3），Rust 生产代码 `forbid(unsafe)`。
- 运行时依赖仅：一个可写数据目录（SQLite 文件 + blob 目录）与启动配置。
- TLS 由反向代理终止；服务自身不管理证书。
- 首次启动通过管理员自举流程创建首个管理员，成功后入口永久关闭。
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
| `docs/adr/0011-ui-design-system.md` | shadcn/ui + Tailwind 设计系统，组件源码归 `apps/ui`，TanStack Table |
| `docs/adr/0012-api-contract-source.md` | utoipa 生成 OpenAPI 为管理 API 契约唯一真源 |
| `docs/adr/0013-single-binary-delivery.md` | rust-embed 编译期内嵌前端产物，单二进制交付 |
| `docs/adr/0014-frontend-first-delivery.md` | M1 按域三步工序：契约骨架 → 前端 + devmock → 后端业务实现，契约真源仍为 ADR-0012 |
| `docs/adr/0015-offline-native-datadir-primary.md` | 迁移首选离线直读 Nexus 原生数据目录（blob 磁盘直读 + 配置 REST/手动），在线 REST 降为备选 |

### 8.2 第一期非目标

- 不迁移 Nexus 用户、角色、权限、LDAP/OIDC 配置、任务计划、脚本、邮件、Webhook 或其他系统配置。
- 不支持 Docker/OCI、Cargo、PyPI、Go、NuGet、RubyGems、Conan、Terraform、Ansible、Pub 等协议。
- 不提供 S3/对象存储后端。
- 不提供 LDAP/OIDC 登录。
- 不提供多节点集群、分布式锁或高可用控制面。
- 不提供复杂保留策略、自动 GC、备份恢复编排、漏洞扫描、WAF、CC 挑战或使用分析面板。
- 不为后续能力在第一期数据库中加入无实际用途的占位字段。
