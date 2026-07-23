# 功能规格：在线 hosted 搬运扩 Docker/OCI（FR-144）

> 状态：开发中　·　关联 PRD：FR-144　·　分支：feature/fr-144-online-docker

## 1. 背景与目标

FR-82 建立了「源 Nexus 在线时经其 REST API 枚举 + HTTP 流式下载搬运 hosted 制品」的在线迁移路径，
FR-143 把它扩到 Raw / npm。这些格式的 asset 都是「普通文件」：一个 downloadUrl → 一段字节 → 按仓库内
路径落为 hosted 制品。**Docker/OCI 不是普通文件**：一个镜像由 manifest（按 digest 内容寻址）+ 其引用的
config blob 与各 layer blob（均按 digest 内容寻址）构成一张图，tag 是指向 manifest 的可变指针。直接套
「downloadUrl → 落文件」无法保证迁移后 `docker pull` 可用。

本功能（P2，M4）把在线迁移扩展到 **Docker/OCI 一种格式**：经 **Docker Registry v2 协议**按
「tag 清单 → manifest → config + layer blob」搬运镜像，逐级校验 digest，按本系统既有 docker 存储布局
落库，使迁移后 `docker pull` 从本系统成功、digest 一致。

## 2. 需求（要什么）

范围内：
- 对每个选中的**源 docker hosted 仓库**：经 registry v2 枚举其镜像与 tag → 拉每个 tag 的 manifest
  （校验 digest）→ 解析 manifest 图取出 config + 各 layer 的 digest → 逐个拉取 blob（按 digest 内容
  寻址、校验 sha256）→ 按本系统 docker 布局（`{image}/blobs/sha256:...`、`{image}/manifests/sha256:...`、
  `{image}/tags/{tag}`）**blob 先落盘校验再写索引、失败回滚**落库 → 建 / 复用目标 docker hosted 仓库。
- `_catalog` 与 `/tags/list` 必须完整跟随 Registry v2 `Link: <...>; rel="next"` 分页；续页 URL 作为服务端游标整体跟随，兼容 Nexus 的 `last` 参数，不得只搬首屏。分页只允许同源且仍在目标仓库 v2 前缀内，重复 URL / 循环 / 超过 10,000 页必须失败，跨页重复 image/tag 去重并保留首次顺序。
- 支持 manifest list / OCI image index（多架构）：index 引用的子 manifest 按 digest 递归搬运，index 自身
  按 digest + tag 落库。
- 幂等重跑：目标已存在且 digest 一致的 blob / manifest 跳过（复用既有内容寻址去重），进度区分「新搬 vs 跳过」。
- 损坏 / 未实现路径：manifest 拉取失败 / digest 不符 / layer 缺失 → 该镜像（或该 tag）记失败跳过、
  不留半截（blob 先落盘校验再写索引的既有不变量保证不留孤儿索引），不中断整批。
- 凭据经 `auth_ref` → env 解析（Basic Auth 注入 registry v2 请求），不入库 / 不进日志；锁外做 IO；流式下载。

不做（范围外）：
- 其他格式（Maven / Raw / npm 已由 FR-82/143 覆盖；Cargo / PyPI 等后续阶段格式不碰）。
- proxy 仓库在线搬运缓存（沿用现状：proxy 在线只迁配置不搬缓存，可回源重建）。
- registry v2 的 token 挑战应答流对**源** Nexus 的适配：源 Nexus docker 仓库经 Basic Auth 直接访问
  （既有 `download_asset` 同款），不实现对源的 bearer 令牌换取（源多为 OSS Nexus，支持 Basic）。

## 3. 设计（怎么做）

属 FR-82 模式的**格式扩展**，无新架构决策 → **不写 ADR**（与 FR-143 一致；docker 存储布局与 registry v2
写入路径均由既有 FR-16 / ADR-0013 覆盖，本功能只在迁移侧复用之）。

新增 / 改动：
- `src/migrate/mod.rs`：新增 `DockerRegistryClient` trait（registry v2 **源侧读**抽象：`list_tags` /
  `fetch_manifest` / `download_blob`），与既有 `NexusClient`（REST 枚举）并列、互不干扰（不改 `NexusClient`
  签名，既有 mock 不受影响）。生产实现 `HttpNexusClient` 同时实现二者。新增 manifest 图解析纯函数
  `parse_manifest_refs`（从 manifest / index 字节取出 config + layers / 子 manifest 的 digest 列表）。
- `src/migrate/http.rs`：`HttpNexusClient` 实现 `DockerRegistryClient`——按 `{base}/repository/{repo}/v2/{image}/...`
  拼 registry v2 URL，Basic Auth 注入，流式下载 blob。
- `src/migrate/online_docker.rs`（新文件）：docker 特有搬运编排——`migrate_online_docker_repo`，对单个源
  docker 仓库做「枚举 image/tag → 拉 manifest → 图遍历拉 blob → 按布局落库」，复用 `OnlinePullProgress` /
  `JobControl`（取消 / 暂停 / 进度）与 `OnlineRepoMigrationOutcome`。**docker 搬运逻辑单独封装，不塞进
  raw/npm 的普通文件路径**（`online.rs` 的 `pull_one_asset` 不变）。
- `src/format/docker_registry.rs`：`DockerRegistry` 新增迁移入口 `ingest_blob`（从 reader 流式落 blob、
  校验声明 digest、幂等写索引，供迁移逐 blob 搬运——与 `finish_upload` 共用「blob 先落盘校验再写索引、
  失败回滚」不变量，区别是字节来自迁移下载流而非上传会话）。`put_manifest` 已幂等（内容寻址），直接复用。
- `src/migrate/online.rs`：`migrate_online_with_progress` 分派——docker hosted 走新 docker 编排（需
  `DockerRegistry`），其余格式走既有普通文件路径。为此 `migrate_online_with_progress` 增加一个
  `docker: Option<&DockerRegistry>` 参数（None 时 docker 仓库整体跳过并计入 skipped_repos，兼容无 docker
  装配的调用点）；`SUPPORTED_ONLINE_FORMATS` 增加 `docker`。
- `src/api/migrate.rs`：`migrate_nexus_online` 后台任务把 `state.docker` 传入。
- `src/api/mod.rs`：路由装配处已持有 `docker`，迁移任务共用。

真源约束不变：SQLite 是索引唯一真源，blob 本体在文件系统；docker manifest 亦作内容寻址 blob 存储；
每个 blob / manifest 先落盘校验 sha256 再写索引，失败回滚不留孤儿。

## 4. 任务拆分

- [ ] 测试先行：mock `DockerRegistryClient` 返回 manifest + layers，断言 manifest/digest 校验、
      layer 逐个拉取校验、按布局落库、幂等重跑跳过、损坏 manifest / layer 缺失回滚不留半截、manifest list 递归。
- [ ] `DockerRegistryClient` trait + `parse_manifest_refs` 纯函数 + 单测（含 schema2 / OCI / manifest list / 缺字段）。
- [ ] `DockerRegistry::ingest_blob` + 单测（幂等 / digest 不符回滚 / 超限）。
- [ ] `online_docker.rs` 编排 + 单测（枚举 → 图遍历 → 落库 → 进度 → 取消）。
- [x] `HttpNexusClient` 实现 `DockerRegistryClient`（registry v2 URL 拼接 + Basic Auth + 流式），并完整处理 catalog/tags 的 `Link rel=next` 分页、同源前缀校验、去重与循环上限。
- [ ] 分派接入 `migrate_online_with_progress` + API 传 `state.docker`。
- [ ] 文档同步：PRD 状态、ARCHITECTURE（若机制变化）、API（端点无新增、说明覆盖格式扩到 docker）、CHANGELOG。

## 5. 验收标准

- `cargo test -j 2` 全绿、`cargo fmt --check` 干净、`cargo clippy --all-targets -j 2 -- -D warnings` 零告警。
- 单元 / 集成测试覆盖（mock registry v2）：
  - manifest 拉取后校验 digest；digest 不符即该镜像失败跳过、不落库。
  - config + 各 layer blob 逐个按 digest 内容寻址拉取并校验；layer 缺失 → 失败跳过、不留半截。
  - 按本系统 docker 布局落库：blob / manifest / tag 指针三类索引正确，`get_manifest` / `get_blob` 可读回。
  - tag → manifest 映射正确；manifest list 递归搬运子 manifest。
  - 幂等重跑：二次跑同源 `migrated=0`、增量跳过=N。
  - 取消 / 暂停在 asset 边界生效（复用 FR-91）。
  - catalog 与 tags 至少两页时，后续页的 image/tag 也进入搬运；`last` 编码原样跟随，坏 Link、跨域、越前缀、循环和页数上限均有自动化测试。
- **真机验收（待真机验）**：对真实 Nexus docker hosted 仓库在线迁移后，配置本系统为 registry，
  `docker pull {本系统}/{目标仓库}/{image}:{tag}` 成功、拉下镜像各层 digest 与源一致。用 mock 覆盖 OCI
  搬运逻辑，真机维度标「待真机验」，需用户提供真实 Nexus + docker 环境确认。

## 6. 风险 / 待定

- 源 Nexus docker 仓库的 registry v2 基址：Nexus OSS 3.x 的 docker 仓库经
  `{base}/repository/{repo}/v2/...` 暴露 registry v2（与其 REST 仓库同名子路径）；不同版本 / 反代前缀可能
  异，真机验收时确认。若源仅暴露 REST components（不暴露 registry v2），本路径不适用——但 FR-144 明确
  按 registry v2 协议搬运，故以 registry v2 为准。
- 镜像枚举：registry v2 的 `_catalog` 列 image、`/tags/list` 列 tag。Nexus 的 `_catalog` 返回本仓库下
  全部 image。用 `_catalog` + 逐 image `/tags/list` 枚举。
