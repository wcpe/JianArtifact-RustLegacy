# 功能规格：认证与用户管理（M1-FR-001~006、045~047）

> 状态：M1-FR-001~006、045 已交付@v0.2.0；M1-FR-046~047 开发中　·　关联 PRD：M1-FR-001~006、M1-FR-045~047　·　分支：main 直接提交，按域三步工序推进（ADR-0014）

## 1. 背景与目标

属 M1，后端第一个业务域。交付本地用户的完整认证链路：管理员自举、用户名密码登录、可刷新可撤销的会话、API Token、CLI 鉴权与用户管理；M1-FR-046~047 在既有认证模型上补齐无预置环境变量时的首启管理员初始化，以及仅访问本地既有数据库的紧急口令重置 CLI。授权判定（能否做，read/write/admin ACL）不在本规格，见 `m1-repository-core.md`；本规格只回答「是谁、凭据是否有效、账号是否可用」。决策依据仍为 ADR-0003（认证与会话），不新增 ADR。

## 2. 需求（要什么）

**FR-001 管理员自举**

- 仅当用户表为空时允许创建首个 Admin；服务启动先检查 `JIANARTIFACT_ADMIN_USER` / `JIANARTIFACT_ADMIN_PASSWORD`，两项均完整提供时优先走环境变量自举，用户名必须非空、口令按 Unicode 字符数计至少 12 个字符并以 Argon2 入库。
- 环境变量自举成功后入口永久关闭；表非空时忽略后续自举变量并记一条中文 WARN，不重复创建、不报错退出。
- 空库且未完整提供两项变量时，不再保持无用户等待下次启动，而是进入 FR-046 的一次性令牌初始化流程。变量值不得出现在任何日志、错误信息与响应中。

**FR-002 用户名密码登录**

- `POST /api/v1/auth/login`：Argon2id 校验（参数钉死：memory 19456 KiB、iterations 2、parallelism 1，OWASP 推荐档），成功签发 JWT HS256 会话令牌（Authorization Bearer 头携带，不用 Cookie）。
- 会话 TTL 默认 8 小时（配置项）；JWT claims 含 sub（用户标识）、sid（会话家族）、jti（单令牌标识）、ver（用户会话版本）、iat/exp。
- HS256 密钥来自配置/环境，启动时缺失即拒绝启动并给中文错误；密钥不落库、不进日志。
- 登录失败返回统一 401，不区分「用户不存在/密码错误」（防枚举）；不做失败锁定与限流（M4 能力，范围外）。

**FR-003 会话刷新与退出**

- `POST /api/v1/auth/refresh`：持有效会话令牌换发新令牌（新 jti、沿用 sid 会话家族），旧 jti 经原子单次消费立即写入撤销表——同一旧令牌并发刷新仅一个成功。
- `POST /api/v1/auth/logout`：吊销当前 sid 会话家族；提交成功后的下一次请求必定 401，不影响该用户其他会话家族与 API Token；与刷新并发时不得遗留有效新令牌。
- 撤销表只存域分隔哈希（单令牌 `sha256("session-revoke:" + jti)`、家族 `sha256("session-family-revoke:" + sid)`）与过期时间，过期记录可清理；不存原始 jti/sid。

**FR-004 API Token**

- `POST /api/v1/tokens`（登录用户）：生成 `jat_` 前缀 + 32 字节随机数的令牌，**明文仅在本次响应出现一次**；持久化只存域分隔哈希（`sha256("api-token:" + 明文)`）与元数据（名称、创建时间）。
- `GET /api/v1/tokens`：仅本人的 Token 元数据列表（分页按 NFR-16），永不含明文或哈希。
- `DELETE /api/v1/tokens/{id}`：吊销（删行），下一次使用该 Token 的请求必定 401。

**FR-005 CLI 鉴权**

- 统一鉴权提取器已在管理 API 落地，并为后续格式端点提供同一能力；格式路由接入后补齐协议矩阵验收：
  - `Authorization: Bearer <值>`——`jat_` 前缀走 API Token 哈希查验，否则按 JWT 会话验签；
  - `Authorization: Basic`——password 字段既可为登录口令（Argon2 校验），也可为 API Token（先按 Token 匹配，未命中再走口令），适配 mvn/npm/curl 的原生凭据形态。
- 口令型 Basic 的校验结果带 60 秒进程内缓存（键为域分隔哈希，不存明文），避免高并发下逐请求 Argon2 拖垮服务（对照 NFR-07 验收负载）。
- **错误凭据不得回退为已认证或匿名身份**：凭据存在但无效 → 一律 401，即使目标是匿名可读的 public 资源。
- 范围内不含 npm 专用 token 端点与 Docker Bearer 质询（分别随 npm 协议规格与 M2）。

**FR-006 用户管理**

- Admin 专属：`GET /api/v1/users`（分页）、`POST /api/v1/users`（用户名+初始口令+角色）、`PATCH /api/v1/users/{id}`（启用/禁用、调整 Admin/User 角色）。
- **每个受保护请求都校验用户启用状态**（会话与 API Token 两条路径一致）：禁用提交成功后，该用户的下一次管理 API 或格式请求必定失败。
- 保护规则：禁止禁用或降级**最后一名启用的 Admin**（防止实例锁死），违规返回明确中文错误。
- 范围外：删除用户（PRD 未登记，需要时走需求登记流程）。
- 口令修改与重置已于 2026-07-13 登记为 **M1-FR-045**（用户体验反馈驱动）：`POST /api/v1/auth/password`（旧口令验证后自改）+ `PATCH /api/v1/users/{id}` 扩展可选 `password` 字段（Admin 重置）；新旧口令不进日志与响应，提交成功后旧口令立即失效；管理端提供自改模态框（顶栏用户区）与用户管理行内重置模态框。

**FR-046 首启管理员初始化**

- 环境变量自举优先。仅当用户表为空且 `JIANARTIFACT_ADMIN_USER` / `JIANARTIFACT_ADMIN_PASSWORD` 未完整提供时，服务在 SQLite 数据库文件同目录生成 `initial-admin.token`；令牌取 32 字节密码学安全随机数并编码为 64 个小写十六进制字符，文件权限固定为 `0600`，日志与响应不得输出令牌或文件内容。
- 浏览器入口为 `/setup#token=<令牌>`。令牌只放在 URL fragment，浏览器不会把 fragment 随初始 HTTP 请求发送；设置页从 fragment 读取令牌后通过请求体提交，不把令牌放入 query、路径、日志或持久化数据。
- `GET /api/v1/setup/status` 无需鉴权，返回 200 与 `{"setup_required": true|false}`；只有用户表为空且一次性设置入口有效时为 `true`，响应不包含令牌、数据库路径或其他内部信息。
- `POST /api/v1/setup` 无需既有账号鉴权，请求体为 `{"token":"…","username":"…","password":"…"}`；用户名去除首尾空白后必须非空，口令按 Unicode 字符数而非 UTF-8 字节数计至少 12 个字符。令牌有效且首个 Admin 原子创建成功时返回 204，无响应体。
- 错误使用统一结构：请求体为空、JSON 损坏或内容类型错误返回 400 / `SETUP_REQUEST_INVALID`；用户名为空返回 400 / `SETUP_USERNAME_REQUIRED`，口令不足 12 个 Unicode 字符返回 400 / `SETUP_PASSWORD_TOO_SHORT`；令牌无效、入口已关闭、令牌重复使用或并发请求被其他请求抢先完成时，均统一返回 404 / `SETUP_UNAVAILABLE`。错误消息不得回显令牌、用户名之外的敏感输入或数据库路径，也不得暴露具体不可用原因。
- 首个用户创建与“仅空表可创建”的判定必须置于同一 SQLite 原子边界；多个有效并发提交仅一个返回 204，其余统一返回 404 / `SETUP_UNAVAILABLE`。成功后以用户表非空作为永久关闭的权威条件，并立即删除 `initial-admin.token`；后续重启、状态查询与提交都不得重新开放入口。

**FR-047 本地口令重置 CLI**

- 命令固定为 `jianartifact admin reset-password <用户名>`，直接使用正常配置解析得到的数据库路径，但只允许打开既有 SQLite 数据库；数据库文件不存在、不是可用的 JianArtifact 数据库或目标用户不存在时，以中文错误写入标准错误并非零退出，不得创建数据库、用户或首启令牌。
- 默认从终端隐藏读取新口令并要求连续输入两次；两次不一致、用户名为空或口令按 Unicode 字符数计少于 12 个字符时拒绝修改。非交互场景仅允许 `--password-stdin`，从标准输入读取单个 UTF-8 行作为口令；无效 UTF-8、空输入或多行输入均拒绝。
- 禁止提供 `--password` 或任何把口令放入命令行参数的等价选项，避免口令进入 shell 历史与进程列表；口令、哈希及标准输入内容不得进入日志或错误信息。
- 成功时只更新目标用户的 Argon2 口令哈希并原子递增 `token_version`，使既有 JWT 会话失效；不得改变 `role`、`enabled` 或任何 API Token 记录。成功信息只包含用户名与结果，不包含口令、哈希或令牌。

## 3. 设计（怎么做）

- 模块落点（ARCHITECTURE §2.2）：`auth`（口令哈希、JWT 签发/验签、Token 生成/查验、鉴权提取器）、`domain/用户服务`（环境变量自举、一次性令牌初始化、用户管理、口令重置、启用校验）、`persistence`（表、仓储与空用户表原子判定）、`api`（setup handler + utoipa 注解）、CLI 入口（参数与安全输入）。HTTP 与 CLI 共享同一用户领域服务，不复制口令规则；协议层经统一鉴权提取器取得「已认证身份」，不直连数据库。
- 数据表草案（权威以迁移脚本为准，只向前追加）：`users`（id、username 唯一、password_hash、role、enabled、token_version、created_at）、`api_tokens`（id、user_id、name、token_hash 唯一、created_at）、`revoked_sessions`（jti_hash 唯一、expires_at）。`initial-admin.token` 是数据库同目录的一次性本地文件，不进入 SQLite schema；生成时先完整写入并同步同目录临时文件，再原子发布正式路径，正式文件不得出现半写内容。
- 启动编排顺序固定为：打开并迁移既有配置指向的数据库 → 检查用户表 → 空表时优先尝试完整环境变量自举 → 未完整提供时确保一次性设置令牌可用 → 挂载 setup 页面与 API。用户表非空是关闭 setup 的权威状态，令牌文件本身不是重新开放依据。
- setup 的“校验令牌 + 确认用户表为空 + 创建 Admin”与 CLI 的“查找用户 + 更新哈希 + 递增 `token_version`”分别使用短 SQLite 原子事务；口令 Argon2 计算在事务外完成，事务内只做最终条件核对与写入，避免持有数据库写锁执行高开销计算。
- 服务基座随本域首个后端任务一并落地（auth 是第一个后端域）：TOML 配置加载（env 覆盖，遵循 `config-files.md`）、中文分级日志初始化、sqlx 迁移框架、统一错误结构与 request_id（API.md §3）、utoipa OpenAPI 集成与导出。
- 首批后端依赖（按规则 15 经本规格评审确认；全 rustls/RustCrypto，零系统级 C 库）：axum 0.8、tokio、sqlx（sqlite bundled）、argon2、jsonwebtoken、serde/serde_json、utoipa、figment + toml、tracing + tracing-subscriber（中文日志）、rand、sha2、时间库（jiff 或 time，实现时定其一）。引入后把 cargo-audit 与许可证检查接入质量门（ROADMAP 阶段 0 遗留项）。
- 三步工序（ADR-0014）：
  1. **阶段 1 契约骨架**：上述端点 handler 空实现 + utoipa 注解 + 请求/响应模型，OpenAPI 可导出；
  2. **阶段 2 前端对 mock**：登录页、用户管理页、Token 管理页对 devmock 开发（devmock 按 OpenAPI 类型实现本域场景）；
  3. **阶段 3 后端实现**：测试先行补齐业务逻辑，web 切真连。

## 4. 任务拆分

- [x] 阶段 1：服务基座（配置/日志/迁移框架/错误结构/OpenAPI）+ 本域契约骨架与模型（2026-07-12：登录/刷新/退出/Token/用户端点 501 占位 + healthz/readyz 真实现 + 迁移 0001_auth + 7 项契约集成测试；cargo-audit 与许可证检查接入质量门）
- [x] 阶段 2：devmock 认证场景（正常/权限受限）+ 登录、用户、Token 三页对 mock（2026-07-13：登录跳转/失败展示、用户增改与最后 Admin 保护提示、Token 一次性明文展示，测试对 default 场景全绿）
- [x] 阶段 2 增补（FR-045）：`POST /api/v1/auth/password` 契约 + devmock 改口令/重置行为 + 顶栏自改模态框与用户管理行内重置模态框（2026-07-13：单测覆盖成功/旧口令错/两次不一致，E2E 改密后同一页面会话新口令重登全绿）
- [x] 阶段 3：测试先行实现——自举、登录、刷新/退出、Token 签发/吊销、Basic/Bearer 提取器、用户管理与启用校验（含 FR-045 改口令/重置真实现；2026-07-13：新增 `/auth/me` 权威身份，JWT 版本失效、原子刷新、最后 Admin/自举并发保护）
- [x] 阶段 3：认证域高风险测试——管理 API 凭据矩阵、即时失效、并发刷新、口令全会话撤销、自举三态装配、凭据零泄露、Argon2 参数与最后 Admin 保护全绿；Argon2 计算已移交阻塞线程池，不存在用户也执行同参数固定哈希校验；格式端点鉴权随 Raw/Maven/npm 协议与官方客户端验收通过
- [x] 文档同步：ARCHITECTURE 认证数据模型与机制、API.md 鉴权约定、ROADMAP 阶段 3 域状态、CHANGELOG
- [ ] FR-046：实现环境变量优先的一次性令牌初始化、setup 页面与两个 setup API；补齐令牌文件权限/删除、错误码、凭据零泄露及并发仅一成功测试。
- [ ] FR-047：实现本地 `admin reset-password` CLI、安全输入与仅打开既有数据库约束；补齐交互双输、stdin 单行、参数拒绝、字段不变与 `token_version` 递增测试。
- [ ] FR-046~047 实现完成后同步 OpenAPI/devmock（仅 setup API）、运维示例与真实二进制验收；在全部验收证据齐备前保持“开发中”。

## 5. 验收标准

对照 PRD §6.1 与 `testing-and-quality.md` §2「授权矩阵」「凭据处理」高风险区：

- **凭据穷举矩阵**（集成测试）：管理 API 已覆盖 {无凭据、坏口令、坏 Token、过期会话、已吊销会话、已吊销 Token、被禁用用户的有效会话/Token}，全组合断言 401 且坏凭据不降级为匿名；格式端点矩阵在 Raw/Maven/npm 协议路由落地时按同一集合补验。
- **即时失效**：logout / 吊销 Token / 禁用用户，提交成功后的下一次请求必定失败（三条路径各有测试）。
- **刷新语义**：refresh 后旧令牌立即失效、新令牌可用；并发使用旧令牌返回 401。
- **自举三态**：空库+变量创建成功且可登录；表非空+变量不创建并告警；空库无变量服务可运行、管理 API 全 401。
- **凭据零泄露**（自动断言）：签发响应之外全库检索不到 Token 明文；数据库中口令列仅 Argon2 PHC 串、Token 列仅哈希；日志采样不含口令/Token/JWT 密钥；撤销表无原始 jti。
- **Argon2 参数断言**：入库 PHC 串参数等于钉死值。
- **最后 Admin 保护**：禁用/降级最后一名启用 Admin 被拒绝。
- **真机维度**：已在 Docker 内启动真实 Rust 服务并用 curl 完成登录 → `/auth/me` → 刷新（旧令牌 401、新令牌 200）→ 退出（新令牌 401），并验证 Basic 口令访问受保护管理端点成功；Basic API Token 与格式协议端点随对应格式域落地补验。
- **FR-046 环境变量优先**：空库且两项自举变量完整时直接创建首个 Admin，不生成可用 setup 入口；变量缺项时不创建用户，改为生成一次性令牌；用户表非空时两条首启路径均永久关闭，且所有日志、响应与数据库均不含口令或令牌明文。
- **FR-046 令牌文件与 URL**：空库无完整变量时，数据库同目录出现 `initial-admin.token`，内容为 32 字节随机数的 64 位小写十六进制表示，权限为 `0600`；设置入口使用 `/setup#token=<令牌>`，服务访问日志不出现 fragment 或令牌，GET 状态响应不泄露令牌与路径。
- **FR-046 API 与并发**：覆盖 setup 状态 true/false、204 成功、四类稳定错误码、空/损坏/错误内容类型请求、空用户名、11/12 个 Unicode 字符边界；分别验证令牌无效、入口已关闭、令牌重复使用均返回 404 / `SETUP_UNAVAILABLE`，且响应不可区分具体原因。至少 16 个携带同一有效令牌的并发提交中恰有一个 204，其余均为 404 / `SETUP_UNAVAILABLE`，数据库仅有一个 Admin。成功后令牌文件被删除，服务重启后状态仍为 false，旧令牌永远不能复用。
- **FR-047 CLI 安全输入**：真实二进制验证隐藏双输成功与不一致拒绝，`--password-stdin` 仅接受单个 UTF-8 行，`--password` 明确拒绝；进程参数、标准输出、标准错误与日志均不含口令。缺失数据库、无效数据库、未知用户、空用户名及 11/12 个 Unicode 字符边界均有非零/成功断言。
- **FR-047 字段不变量**：重置前后目标用户的角色、启用状态与 API Token 记录完全一致，仅口令哈希变化且 `token_version` 恰好递增 1；旧口令与既有 JWT 立即失效，新口令可登录，既有 API Token 仍按原启用状态工作。
- 本域完成同时解锁 FR-021 `/readyz` 所需的服务常驻基座 → 触发 FR-043 真机验收补验（见 `m1-build-deploy.md` §5）。

## 6. 风险 / 待定

- 本规格钉死的约定（开发前可调）：会话 TTL 默认 8h、Argon2id 19456/2/1、Basic 口令校验缓存 60 秒、Token 前缀 `jat_`、自举环境变量名。
- FR-046~047 已批准并固定的安全边界：环境变量优先；令牌为 32 字节随机十六进制且文件权限 `0600`；setup 仅在空用户表开放；初始口令与 CLI 新口令均至少 12 个 Unicode 字符；CLI 不接受命令行明文口令且不创建数据库。实现不得以配置开关重新开放 setup，也不得为便利放宽这些边界。
- 本地重置 CLI 直接持有 SQLite 文件访问权，属于主机运维权限边界而非远程管理 API；运维侧必须限制数据库文件与命令执行权限。该能力不新增网络端点、不改变角色/启用状态/API Token，也不引入新的架构决策。
- Basic「password 可为 API Token」是对原生客户端最友好的形态，但意味着 Token 与口令共享一条 Basic 通道——文档与使用片段（FR-020）须引导用户优先用 Token。
- 时间库选型（jiff/time）与 utoipa 版本随实现敲定，不影响契约。
- 登录失败锁定、限流、审计日志均为 M3/M4 能力，本域明确不做；出现相关诱惑即停（scope-discipline）。
