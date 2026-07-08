# 功能规格：在线 hosted 搬运扩 Raw + npm

> 状态：开发中　·　关联 PRD：FR-143（增强 FR-82）　·　分支：feature/fr-143-online-raw-npm

## 1. 背景与目标

FR-82 已落地「源 Nexus 在线时经其 REST `components` API 枚举 hosted 仓库 asset、按 `downloadUrl`
HTTP 流式下载、经既有 `ingest_hosted` 落为本系统 hosted 制品」的在线拉取迁移，但**当前仅放行
Maven（`maven2`）hosted**，其余格式仓库整体跳过（`src/migrate/online.rs` 的格式过滤）。

离线 hosted 搬运（FR-39，`src/migrate/hosted.rs`）早已用「格式可映射即放行 + `format.parse_path`
+ `ingest_hosted`」的通用模式覆盖 raw / npm / maven 等全部已实现格式。本 FR 把**在线拉取**这条路径
的格式过滤同样放宽到 **Raw + npm**，复用同一枚举 / 下载 / sha256 校验 / 幂等重跑引擎，补齐在线与离线
两条搬运通道的格式覆盖一致性。属 P2、里程碑 M4。

## 2. 需求（要什么）

- 范围内：
  - 在线拉取迁移（`migrate_online_with_progress`）除 `maven2` hosted 外，**额外放行 `raw` 与 `npm`
    的 hosted 仓库**；目标仓库按映射格式（raw / npm / maven）建 / 复用。
  - raw / npm asset 走与 maven **完全相同**的路径：REST 枚举 → 按 `downloadUrl` 流式下载 →
    `ingest_hosted`（blob 先落盘校验 sha256 再写索引）→ 与源报告 sha256 比对不符则回滚跳过。
  - 复用既有幂等 / 重试 / 增量跳过（FR-134）/ 取消暂停（FR-91）/ 进度上报（FR-83）语义，不为本 FR
    新造机制。
  - 各仓库结果明细的 `format` 字段如实反映映射后的格式（不再恒为 `maven`）。
  - proxy 仓库在线**只迁配置、不搬缓存**——沿用现状：在线拉取只处理 hosted，proxy / group 整体跳过
    （在线 proxy 缓存可回源重建，不搬）。
- 不做（范围外）：
  - Docker/OCI 在线搬运（FR-144，另行）。
  - 其余未实现格式（cargo / pypi / nuget / go 等）在线搬运——仍整体跳过。
  - 在线 proxy 缓存搬运、group 在线搬运——不在本 FR。
  - 任何新端点 / 新请求参数 / 新数据表。

## 3. 设计（怎么做）

**唯一改动点集中在 `src/migrate/online.rs`**，把「仅 maven」的硬编码替换为「格式可映射到受支持
hosted 格式集合即放行、按映射格式建仓、结果 `format` 用映射值」：

1. 格式过滤：把 `map_nexus_format(&src.format) != Some("maven")` 改为「映射结果不在
   {`maven`, `raw`, `npm`} 集合内则整体跳过」。新增一个私有判定函数
   `online_hosted_format(nexus_format) -> Option<&'static str>`，内部复用 `map_nexus_format`
   并只认在线支持的三种格式（单一职责、不用 if-else 按格式名堆逻辑，集合即白名单）。
2. 建仓：`ensure_hosted_repo(meta, &sel.target_repo, "maven")` 的硬编码 `"maven"` 改为传映射后的
   格式名。
3. 结果 `format`：`OnlineRepoMigrationOutcome.format` 从字面量 `"maven"` 改为映射后的格式名；
   进度结构同源。
4. 下载 / 落地 / sha256 校验 / 重试逻辑（`pull_repo_assets` / `pull_one_asset`）**不改**——它们已按
   `target_repo.format` 从注册表取处理器、经 `format.parse_path` + `ingest_hosted` 泛化处理，raw / npm
   直接受益。

`src/api/migrate.rs` 的 `migrate_nexus_online` 编排**不改逻辑**，仅更新其「仅 Maven hosted 参与」的
文档注释为「Maven / Raw / npm hosted 参与」。

不涉及新 ADR（在 ADR-0006「随已实现格式逐期扩展」框架内）；不涉及数据模型 / Schema / 新接口。

## 4. 任务拆分

- [x] 测试先行（红）：在 `online.rs` mock 用例中新增 raw hosted、npm hosted 在线拉取断言
      （枚举 → 下载 → sha256 校验 → 落库、结果 `format` 正确、幂等重跑跳过）。
- [x] 测试先行（红）：断言未支持格式（如 cargo hosted）与 proxy 仍整体跳过、不建仓。
- [x] 实现：`online.rs` 格式过滤放宽 + 建仓格式 + 结果 format 泛化。
- [x] 更新 `api/migrate.rs` 在线拉取 handler 文档注释（Maven/Raw/npm）。
- [x] 文档同步：PRD 状态、ARCHITECTURE（在线搬运格式覆盖）、API（在线迁移端点格式覆盖说明）、CHANGELOG。

## 5. 验收标准

- `cargo test -j 2` 全绿，含新增 raw / npm 在线拉取用例（§2.2 格式协议、§2.4 流式、§2.5 事务边界）。
- `cargo fmt --check` 干净、`cargo clippy --all-targets -j 2 -- -D warnings` 零告警。
- mock 覆盖：raw hosted 与 npm hosted 各自「枚举 → 流式下载 → sha256 与源报告一致 → 落为 hosted
  制品」成功；二次同 sha256 计增量跳过而非新搬；下载内容 sha256 与源报告不符时回滚跳过。
- 未支持格式（cargo 等）与 proxy 在线仍整体跳过、不建仓。
- 真机（整期真机验收，标「待真机验」）：对真实源 Nexus 的 raw hosted / npm hosted 仓库在线迁移后，
  `curl GET`（raw）/ `npm install`（npm）从本系统成功且 sha256 与源一致。用 mock 覆盖逻辑，真机维度
  待用户确认。

## 6. 风险 / 待定

- npm 在 Nexus 侧 asset 既含 tarball（`{包名}/-/{文件}`）也含 packument / `package.json` 元数据路径；
  二者对本系统 npm 格式都是普通仓库内路径，`parse_path` 均按归一化路径处理，`can_overwrite` 已据
  `/-/` 段区分 tarball（不可覆盖）与 packument（可更新）——沿用既有语义，本 FR 不新增区分逻辑。
- 真机 npm packument 的 URL 重写（把上游 tarball 地址改写为本系统地址）不在在线**搬运**职责内，属
  npm 格式服务既有行为；本 FR 只负责把源 asset 字节搬入，不做二次改写。
