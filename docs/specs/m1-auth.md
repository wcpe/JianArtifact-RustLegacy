# 功能规格：认证与用户管理（M1-FR-001~006）

> 状态：草拟　·　关联 PRD：M1-FR-001~006　·　分支：main 直接提交，按域三步工序推进（ADR-0014）

## 1. 背景与目标

属 M1，后端第一个业务域。交付本地用户的完整认证链路：管理员自举、用户名密码登录、可刷新可撤销的会话、API Token、CLI 鉴权与用户管理。授权判定（能否做，read/write/admin ACL）不在本规格，见 `m1-repository-core.md`；本规格只回答「是谁、凭据是否有效、账号是否可用」。决策依据 ADR-0003（认证与会话）。

## 2. 需求（要什么）

**FR-001 管理员自举**

- 仅当用户表为空时，服务启动过程读取环境变量 `JIANARTIFACT_ADMIN_USER` / `JIANARTIFACT_ADMIN_PASSWORD` 创建首个 Admin（口令按 Argon2 入库）；创建成功后该入口永久关闭（表非空即忽略变量并记一条中文 WARN，不重复创建、不报错退出）。
- 空库且未提供变量：服务正常运行但无任何用户（管理 API 一律 401，格式端点仅匿名可读 public），等待下次带变量启动。
- 变量值不得出现在任何日志、错误信息与响应中。不提供交互式创建流程（服务无 TTY，YAGNI）。

**FR-002 用户名密码登录**

- `POST /api/v1/auth/login`：Argon2id 校验（参数钉死：memory 19456 KiB、iterations 2、parallelism 1，OWASP 推荐档），成功签发 JWT HS256 会话令牌（Authorization Bearer 头携带，不用 Cookie）。
- 会话 TTL 默认 8 小时（配置项）；JWT claims 至少含 sub（用户标识）、jti（会话标识）、iat/exp。
- HS256 密钥来自配置/环境，启动时缺失即拒绝启动并给中文错误；密钥不落库、不进日志。
- 登录失败返回统一 401，不区分「用户不存在/密码错误」（防枚举）；不做失败锁定与限流（M4 能力，范围外）。

**FR-003 会话刷新与退出**

- `POST /api/v1/auth/refresh`：持有效会话令牌换发新令牌（新 jti），旧 jti 立即写入撤销表——同一会话不产生并行有效令牌。
- `POST /api/v1/auth/logout`：吊销当前 jti；提交成功后的下一次请求必定 401，不影响该用户其他会话与 API Token。
- 撤销表只存域分隔哈希（`sha256("session-revoke:" + jti)`）与过期时间，过期记录可清理；不存原始 jti。

**FR-004 API Token**

- `POST /api/v1/tokens`（登录用户）：生成 `jat_` 前缀 + 32 字节随机数的令牌，**明文仅在本次响应出现一次**；持久化只存域分隔哈希（`sha256("api-token:" + 明文)`）与元数据（名称、创建时间）。
- `GET /api/v1/tokens`：仅本人的 Token 元数据列表（分页按 NFR-16），永不含明文或哈希。
- `DELETE /api/v1/tokens/{id}`：吊销（删行），下一次使用该 Token 的请求必定 401。

**FR-005 CLI 鉴权**

- 管理 API 与格式端点统一支持：
  - `Authorization: Bearer <值>`——`jat_` 前缀走 API Token 哈希查验，否则按 JWT 会话验签；
  - `Authorization: Basic`——password 字段既可为登录口令（Argon2 校验），也可为 API Token（先按 Token 匹配，未命中再走口令），适配 mvn/npm/curl 的原生凭据形态。
- 口令型 Basic 的校验结果带 60 秒进程内缓存（键为域分隔哈希，不存明文），避免高并发下逐请求 Argon2 拖垮服务（对照 NFR-07 验收负载）。
- **错误凭据不得回退为已认证或匿名身份**：凭据存在但无效 → 一律 401，即使目标是匿名可读的 public 资源。
- 范围内不含 npm 专用 token 端点与 Docker Bearer 质询（分别随 npm 协议规格与 M2）。

**FR-006 用户管理**

- Admin 专属：`GET /api/v1/users`（分页）、`POST /api/v1/users`（用户名+初始口令+角色）、`PATCH /api/v1/users/{id}`（启用/禁用、调整 Admin/User 角色）。
- **每个受保护请求都校验用户启用状态**（会话与 API Token 两条路径一致）：禁用提交成功后，该用户的下一次管理 API 或格式请求必定失败。
- 保护规则：禁止禁用或降级**最后一名启用的 Admin**（防止实例锁死），违规返回明确中文错误。
- 范围外：删除用户、用户自改口令、管理员重置口令（PRD 未登记，需要时走需求登记流程）。

## 3. 设计（怎么做）

- 模块落点（ARCHITECTURE §2.2）：`auth`（口令哈希、JWT 签发/验签、Token 生成/查验、鉴权提取器）、`domain/用户服务`（自举、用户管理、启用校验）、`persistence`（表与仓储）、`api`（handler + utoipa 注解）。协议层经统一鉴权提取器取得「已认证身份」，不直连数据库。
- 数据表草案（权威以迁移脚本为准，只向前追加）：`users`（id、username 唯一、password_hash、role、enabled、created_at）、`api_tokens`（id、user_id、name、token_hash 唯一、created_at）、`revoked_sessions`（jti_hash 唯一、expires_at）。
- 服务基座随本域首个后端任务一并落地（auth 是第一个后端域）：TOML 配置加载（env 覆盖，遵循 `config-files.md`）、中文分级日志初始化、sqlx 迁移框架、统一错误结构与 request_id（API.md §3）、utoipa OpenAPI 集成与导出。
- 首批后端依赖（按规则 15 经本规格评审确认；全 rustls/RustCrypto，零系统级 C 库）：axum 0.8、tokio、sqlx（sqlite bundled）、argon2、jsonwebtoken、serde/serde_json、utoipa、figment + toml、tracing + tracing-subscriber（中文日志）、rand、sha2、时间库（jiff 或 time，实现时定其一）。引入后把 cargo-audit 与许可证检查接入质量门（ROADMAP 阶段 0 遗留项）。
- 三步工序（ADR-0014）：
  1. **阶段 1 契约骨架**：上述端点 handler 空实现 + utoipa 注解 + 请求/响应模型，OpenAPI 可导出；
  2. **阶段 2 前端对 mock**：登录页、用户管理页、Token 管理页对 devmock 开发（devmock 按 OpenAPI 类型实现本域场景）；
  3. **阶段 3 后端实现**：测试先行补齐业务逻辑，web 切真连。

## 4. 任务拆分

- [ ] 阶段 1：服务基座（配置/日志/迁移框架/错误结构/OpenAPI）+ 本域契约骨架与模型
- [ ] 阶段 2：devmock 认证场景（正常/权限受限）+ 登录、用户、Token 三页对 mock
- [ ] 阶段 3：测试先行实现——自举、登录、刷新/退出、Token 签发/吊销、Basic/Bearer 提取器、用户管理与启用校验
- [ ] 阶段 3：高风险区测试（见 §5）全绿；cargo-audit 与许可证检查接入质量门
- [ ] 文档同步：PRD 状态、ARCHITECTURE §3 数据模型落定、API.md（如出现新的跨端点约定）、CHANGELOG

## 5. 验收标准

对照 PRD §6.1 与 `testing-and-quality.md` §2「授权矩阵」「凭据处理」高风险区：

- **凭据穷举矩阵**（集成测试）：{无凭据、坏口令、坏 Token、过期会话、已吊销会话、已吊销 Token、被禁用用户的有效会话/Token} × {管理 API、格式端点} 全组合断言 401（或匿名语义），且坏凭据不降级为匿名。
- **即时失效**：logout / 吊销 Token / 禁用用户，提交成功后的下一次请求必定失败（三条路径各有测试）。
- **刷新语义**：refresh 后旧令牌立即失效、新令牌可用；并发使用旧令牌返回 401。
- **自举三态**：空库+变量创建成功且可登录；表非空+变量不创建并告警；空库无变量服务可运行、管理 API 全 401。
- **凭据零泄露**（自动断言）：签发响应之外全库检索不到 Token 明文；数据库中口令列仅 Argon2 PHC 串、Token 列仅哈希；日志采样不含口令/Token/JWT 密钥；撤销表无原始 jti。
- **Argon2 参数断言**：入库 PHC 串参数等于钉死值。
- **最后 Admin 保护**：禁用/降级最后一名启用 Admin 被拒绝。
- **真机维度**（需用户确认）：curl 以 Basic（口令与 Token 两种）访问受保护端点成功；部署实例上完成登录→刷新→退出全流程。
- 本域完成同时解锁 FR-021 `/readyz` 所需的服务常驻基座 → 触发 FR-043 真机验收补验（见 `m1-build-deploy.md` §5）。

## 6. 风险 / 待定

- 本规格钉死的约定（开发前可调）：会话 TTL 默认 8h、Argon2id 19456/2/1、Basic 口令校验缓存 60 秒、Token 前缀 `jat_`、自举环境变量名。
- Basic「password 可为 API Token」是对原生客户端最友好的形态，但意味着 Token 与口令共享一条 Basic 通道——文档与使用片段（FR-020）须引导用户优先用 Token。
- 时间库选型（jiff/time）与 utoipa 版本随实现敲定，不影响契约。
- 登录失败锁定、限流、审计日志均为 M3/M4 能力，本域明确不做；出现相关诱惑即停（scope-discipline）。
