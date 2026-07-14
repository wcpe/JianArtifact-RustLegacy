# 功能规格：npm 协议——hosted 与 proxy（M1-FR-017~018）

> 状态：已批准 / 开发中　·　关联 PRD：M1-FR-017、M1-FR-018　·　分支：main 直接提交，按域三步工序推进（ADR-0014）

## 1. 背景与目标

属 M1，三格式中协议交互最特殊的一个：npm 客户端不走「一路径一文件」的 REST 语义，而是 `publish` 提交聚合 JSON、`install` 先取 packument（包级元数据文档）再按其中的 tarball 地址下载。本规格定义 npm 特有语义——packument 服务端权威合成、scoped package、dist-tag、不可变版本，以及 proxy 的 **tarball 地址重写与上游凭据零泄露**。鉴权、提交守卫、单飞、上游错误分类继承 `m1-repository-core.md`。迁移域（FR-027/completion 中的 npm 客户端可观察语义）以本规格为协议真源。

## 2. 需求（要什么）

**通用：包标识与路径**

- 包名支持 unscoped（`{pkg}`）与 scoped（`@{scope}/{pkg}`，URL 中 `/` 编码为 `%2f`，两种形态都要解析）。
- 端点族：`GET /repository/{仓库名}/{包名}`（packument）、`GET /repository/{仓库名}/{包名}/-/{tarball 文件名}`（tarball）、`PUT /repository/{仓库名}/{包名}`（publish）、dist-tag 端点 `GET|PUT|DELETE /repository/{仓库名}/-/package/{包名}/dist-tags[/{tag}]`。

**FR-017 npm hosted**

- **publish（write，PUT 包名）**：请求体是 npm 聚合 JSON——含 `versions`（本次发布的 version 清单，通常一个）、`_attachments`（base64 编码的 tarball，是 M1 唯一允许整体驻留内存的请求，上限 NFR-04 = 128 MiB）、`dist-tags`。服务端：解码 attachment 流式落库（内容寻址）、校验 `dist.integrity`（sha512）与 `dist.shasum`（sha1）与实际一致否则 400、登记版本与 dist-tag。
- **不可变版本**：同名同版本重复 publish → 403（npm 语义的 `cannot publish over previously published version`）；覆盖策略开关对 npm 不放开（不可变是生态硬约定）。
- **packument 服务端权威合成**（沿用已验证的权威元数据设计）：`GET {包名}` 返回由库中该包所有版本实时合成的完整 packument——`versions` 映射、`dist-tags`、`time`、`name`；每个 version 的 `dist.tarball` 指向**本仓库**的 tarball URL（绝对 URL，由请求 Host 或配置的对外基址拼装，保证 install 能取回）。
- **tarball（read，GET）**：流式返回，`Content-Type: application/octet-stream`。
- **dist-tag 端点**：GET 列出、PUT 置标签指向某版本、DELETE 删标签；`latest` 不可删除。scoped 包全程等价支持。

**FR-018 npm proxy**

- **packument——带 TTL 的可刷新缓存 + 地址重写**（默认 TTL 30 分钟，配置项，与 Maven metadata 同机制）：
  - 回源取上游 packument，**改写每个 version 的 `dist.tarball` 为本 proxy 仓库地址**（`.../{包名}/-/{文件名}`），使 install 经本服务下载并缓存——绝不让客户端直连上游；
  - 过期后回源刷新，**回源失败沿用旧缓存并 WARN**（stale-if-error）；未过期命中本地重写结果；
  - 改写后不保留上游原始 tarball 字段（防止泄露上游主机名）。
- **tarball**：按重写地址请求时单飞回源真实上游 tarball（地址由缓存的 packument→上游映射解析），2xx 校验 integrity 后按四步提交为正缓存；命中直接本地返回。tarball 内容不可变，正缓存永久有效。
- **上游凭据零泄露**：上游需认证时凭据走运行时引用（core FR-011 机制）；凭据只在回源请求头出现，**绝不写入缓存的 packument、不出现在响应、不进日志**；重写后的 packument 对下游调用方不含任何上游认证痕迹。
- 写操作（publish/dist-tag 修改）→ 405；上游错误五类映射同 Raw（404/504/502）。

## 3. 设计（怎么做）

- 模块落点：`protocol/npm`——聚合 JSON 解析、packument 合成（纯函数：版本集合+dist-tags → JSON 模型）、tarball 地址重写（纯函数）、integrity 校验；经领域服务读写，禁直连数据库（ADR-0006）。
- 对外基址：packument 里的绝对 tarball URL 优先用配置的 `public-base-url`，缺省回退请求 `Host` 头 + 仓库路径；配置项写入 core 的服务配置（不新增专表）。
- proxy packument 的「重写地址 → 上游真实地址」映射：从缓存的原始上游 packument 推导，不额外落表（tarball 文件名内含版本，可反查上游约定路径）；若上游 tarball 路径非标准，回源失败按上游 404 处理（见 §6）。
- 依赖：JSON 用已引入的 serde_json；sha512 用已引入的 sha2；无新增第三方依赖。
- dist-tag 冲突与合并规则仅在 group 语义下才复杂，本规格 hosted/proxy 内 dist-tag 为单仓库权威，合并见 `m1-group.md`。
- 三步工序：阶段 1 契约骨架（端点族 OpenAPI tag + packument/publish 模型）→ 阶段 2 无本域特有前端页面 → 阶段 3 后端实现。

## 4. 任务拆分

- [x] 阶段 1：通用格式路径 GET/HEAD/PUT/DELETE 已纳入同一 OpenAPI 并挂载 501 占位；packument/tarball/publish/dist-tag 均由该通配路径承载，具体协议模型随阶段 3 实现补全。
- [ ] 阶段 3（测试先行）：包名解析（scoped/unscoped）、packument 合成、tarball 地址重写纯函数
- [ ] 阶段 3：hosted——publish 解码与 integrity/shasum 校验、不可变 403、dist-tag 端点、scoped 全链路
- [ ] 阶段 3：proxy——packument TTL 缓存与地址重写、tarball 单飞回源与永久缓存、上游凭据零泄露断言
- [ ] 阶段 3：官方客户端协议测试与真机验收（见 §5）
- [ ] 文档同步：PRD 状态、API.md（如新增跨端点约定）、CHANGELOG

## 5. 验收标准

对照 PRD §6.2「npm hosted / npm proxy」与高风险区「凭据处理」：

- **纯函数**：scoped/unscoped 包名双向解析（含 `%2f` 编码）；packument 合成对照官方 npm registry 响应结构；地址重写正确指向本仓库。
- **hosted 协议测试**：publish 后 packument 含该版本且 `dist.tarball` 指向本仓库、tarball 可下载且字节一致；同版本重发布 403；损坏 integrity/shasum 的 publish 400；dist-tag PUT/DELETE 生效且 `latest` 不可删；scoped 包 publish/install/dist-tag 全等价通过。
- **proxy 协议测试**：install 经本服务取 packument（tarball 地址已重写为本仓库）→ 下载 tarball 回源一次并缓存（单飞 32 并发验收）→ 断开上游后缓存 install 成功；packument TTL 过期后回源可见上游新版本、上游停服沿用旧缓存并 WARN；**上游凭据零泄露断言**——回源用带认证的受控上游，检查缓存的 packument、下游响应、全量日志均不含凭据与上游主机名。
- **真机维度（需用户确认，隔离环境官方 npm）**：`npm publish`（含 scoped）与 `npm install` 全流程；proxy 首次上游 install、断开上游后的缓存 install（PRD §7.2 明列）；`.npmrc` 配 registry 指向本服务，校验和严格通过。
- 零存在性泄露与恶意路径样本在 npm 端点抽查（复用通用样本）。

## 6. 风险 / 待定

- 本规格钉死的约定（开发前可调）：packument TTL 默认 30 分钟；npm 不可变版本硬拒绝（不提供覆盖开关）。
- **对外基址来源**：优先 `public-base-url` 配置、缺省回退 `Host` 头。反代改写 Host 或多域名访问时，未配置会导致 tarball 地址错误——文档（FR-020 使用片段）与 OPERATIONS 须提示生产环境显式配置。
- **proxy tarball 上游路径反查**：依赖上游遵循标准 `{包名}/-/{文件名}` 布局；非标准上游（少见）回源失败按 404 处理，不做启发式猜测（零泄露与确定性优先）。
- `_attachments` 整体入内存是 npm 协议先天限制（对齐 NFR-04 的唯一例外条款），超大包超过 128 MiB 时调大配置即可，不改架构。
