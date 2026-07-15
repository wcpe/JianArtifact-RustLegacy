# 功能规格：Maven 协议——hosted 与 proxy（M1-FR-015~016）

> 状态：已交付@v0.2.0　·　关联 PRD：M1-FR-015、M1-FR-016　·　分支：main 直接提交，按域三步工序推进（ADR-0014）

## 1. 背景与目标

属 M1。实现 Maven2 标准仓库布局的 hosted 与 proxy，官方 `mvn` 客户端可直接 deploy 与解析依赖。鉴权、提交守卫、单飞、上游错误分类等继承 `m1-repository-core.md`；本规格定义 Maven 特有语义——布局、checksum sidecar、两级 metadata 的**服务端权威生成**、release/snapshot 写入语义与 proxy 的 metadata 时效。迁移域（FR-025 完成定义中的 Maven 客户端可观察语义）以本规格为协议真源。

## 2. 需求（要什么）

**通用：布局与版本策略**

- 路径即 Maven2 标准布局：`{groupId 点转斜杠}/{artifactId}/{version}/{artifactId}-{version}[-{classifier}].{extension}`；两级 metadata 路径 `.../{artifactId}/maven-metadata.xml` 与 `.../{version}/maven-metadata.xml`（仅 SNAPSHOT 版本目录）。
- Maven hosted 仓库带版本策略属性 `version-policy: release | snapshot | mixed`（默认 mixed；迁移按 Nexus 源仓库策略映射）：release 库拒绝 `-SNAPSHOT` 版本路径写入，snapshot 库拒绝非 SNAPSHOT 写入，均返回 400 与中文错误。

**FR-015 Maven hosted**

- 构件上传（write 权限，PUT）：POM/JAR/附属文件（classifier/extension 任意组合）流式入库（通用四步守卫）。
- **release 不可变语义**：release 路径的覆盖遵循仓库覆盖策略，Maven hosted 默认 `deny`（重发布同坐标 → 409）；`-SNAPSHOT` 版本目录内的 timestamped 文件不受覆盖策略限制（snapshot 重复部署天然产生新 timestamped 文件）。
- **checksum sidecar 不落盘、动态生成**（沿用已验证的权威元数据设计）：
  - `GET *.sha1 / *.md5 / *.sha256`：从制品索引的校验和实时生成（写入时单遍流式已算好，见核心规格 FR-010）；
  - 客户端 `PUT *.sha1 / *.md5`：与库中值比对——一致则接受（200，不另存），不一致返回 400 拒绝（防止传输损坏落库）。
- **两级 maven-metadata.xml 由服务端权威生成**。客户端 PUT 的 metadata 以及该 metadata 的 `.sha1/.md5/.sha256` sidecar 一律接受但忽略内容（不落盘、不参与校验；并发 deploy 下客户端各自生成的 metadata 会互相丢失更新，服务端推导才可靠）：
  - artifact 级：versions 列表、latest、release（最新非 SNAPSHOT）、lastUpdated，由该坐标下已有版本实时推导；
  - snapshot 版本级：timestamp、buildNumber、snapshotVersions（按 extension/classifier），由该版本目录下 timestamped 文件实时推导；
  - 版本排序遵循 Maven ComparableVersion 规则（qualifier 语义），实现自带对照官方样例的测试集。
- metadata 的 sidecar 同样动态生成（对生成结果实时计算）。

**FR-016 Maven proxy**

- 构件（含 timestamped snapshot 文件）坐标不可变 → 正缓存**永久有效**，命中直接本地返回，缓存内容与校验和一致（sidecar 按本地缓存实时生成，与内容必然一致）。
- **maven-metadata.xml 特例——带 TTL 的可刷新缓存**（默认 30 分钟，配置项）：过期后回源刷新；**回源失败沿用旧缓存并记中文 WARN**（stale-if-error，上游抖动不打断构建）；未过期期间命中本地。这是 proxy 能看见上游新版本的关键，不做则上游发新版对使用方永久不可见。
- 上游 sidecar 处理：回源构件时一并核验上游 `.sha1`（存在时），不一致按「上游数据损坏」类别报错且不落缓存；对外 sidecar 一律本地动态生成。
- 写操作 → 405；上游错误五类映射同 Raw（404 / 504 / 502）。

## 3. 设计（怎么做）

- 模块落点：`protocol/maven`——布局解析（坐标 ↔ 路径的纯函数，穷举测试）、metadata 推导（纯函数：版本集合/文件集合 → XML 模型）、sidecar 核验；经领域服务读写，禁直连数据库（ADR-0006）。
- `artifacts` 表沿用核心规格草案，不为 Maven 增设专表——metadata 与 sidecar 均为读时推导，坐标信息由路径解析获得（不加冗余列，需要时再议）。
- 新增依赖：`quick-xml`（metadata XML 序列化/解析，纯 Rust；按规则 15 经本规格评审确认）。版本排序自实现（无成熟纯 Rust 现成件），范围为 ComparableVersion 的数字/qualifier 常用全集并以官方测试样例锁行为。
- metadata TTL 缓存的过期判断基于缓存条目 `updated_at`，不引入后台刷新任务（读时惰性刷新，YAGNI）。
- 三步工序：阶段 1 契约骨架（格式端点 OpenAPI tag）→ 阶段 2 无本域特有前端页面 → 阶段 3 后端实现（依赖核心机理与 Raw 联调经验）。

## 4. 任务拆分

- [x] 阶段 1：通用格式路径 GET/HEAD/PUT/DELETE 已纳入同一 OpenAPI 并挂载 501 占位；Maven metadata/sidecar 端点族由同一通配路径承载，具体协议语义随阶段 3 实现。
- [x] 阶段 3（测试先行）：布局解析与 ComparableVersion 纯函数 + 官方样例测试集
- [x] 阶段 3：hosted——版本策略、release 覆盖语义、sidecar 上传核验、两级 metadata 权威生成（含并发 deploy 场景）
- [x] 阶段 3：proxy——构件永久缓存、metadata TTL 与 stale-if-error、上游 sidecar 核验
- [x] 阶段 3：官方客户端协议测试与真机验收（2026-07-15）
- [x] 文档同步：PRD 状态、API.md（如新增跨端点约定）、CHANGELOG

## 5. 验收标准

对照 PRD §6.2「Maven hosted / Maven proxy」与高风险区：

- **纯函数**：布局解析双向穷举（含 classifier/多段 extension/非法路径拒绝）；ComparableVersion 对照官方样例全绿。
- **hosted 协议测试**：release deploy 全套文件（jar/pom/sidecar/metadata）入库后 GET 逐一一致；同坐标重发布 409（默认 deny）、改 allow 后 200；snapshot 两次 deploy 产生递增 buildNumber 的 timestamped 文件且版本级 metadata 推导正确；并发 deploy 后 artifact 级 metadata versions 无丢失；PUT 损坏 sidecar 400；policy 三值行为各自正确。
- **proxy 协议测试**：构件缓存命中不回源；metadata 过期后回源刷新可见上游新版本、上游停服时沿用旧缓存并 WARN；上游 sidecar 不一致按损坏类别拒绝且缓存无残留；单飞与五类错误继承验收（core 已覆盖，此处按 Maven 路径抽查）。
- **真机维度（需用户确认，隔离环境官方 mvn）**：`mvn deploy` release 与 snapshot 各一轮、`mvn dependency:get` 与项目构建解析回包成功，且客户端校验和严格模式（`-C`）通过；proxy 以受控上游全套验收 + 对公开中央仓库抽样解析一次；断开上游后已缓存依赖可继续构建。
- 零存在性泄露与恶意路径样本在 maven 端点抽查（复用通用样本）。

## 6. 风险 / 待定

- 本规格钉死的约定（开发前可调）：metadata TTL 默认 30 分钟、Maven hosted 覆盖策略默认 deny、version-policy 默认 mixed。
- 客户端 PUT 的 maven-metadata.xml 被忽略是与「客户端写入即真源」的传统行为的有意分歧（并发正确性优先）；极端场景（客户端刻意伪造 metadata）不受影响，因为读取端永远看到服务端推导结果。
- ComparableVersion 自实现范围以官方样例集为锁；样例外的罕见 qualifier 组合若在真机验收暴露差异，按缺陷回归补样例。
- proxy 不实现 Nexus 式「构件也带 TTL」——M1 坐标不可变假设下无必要；若未来需要强制重新核验，按新需求登记。
