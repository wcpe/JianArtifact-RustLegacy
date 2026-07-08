# 功能规格：离线仓库索引双通道

> 状态：开发中　·　关联 PRD：FR-141　·　分支：feature/fr-141-offline-index

## 1. 背景与目标

Nexus 迁移重做（M4）要求离线迁移先获取**仓库索引**（仓库列表及其格式 / 类型 / 上游配置），
展示给管理员选择后再搬运。障碍在于源 Nexus OSS 3.70.x 的仓库配置存在嵌入式 OrientDB，
Rust 无读取库、无法解析；而 blob store 的 `.properties` 只含仓库名 / 内容路径 / sha1 / 大小，
**不含**格式、类型（hosted/proxy）、上游 URL 等权威配置。

本 FR（后端）实现 ADR-0041 决策的**双通道仓库索引获取**：权威通道（REST）优先、纯离线推断回退，
两通道输出同一份统一索引，供 FR-142 前端向导消费。属 P2 / M4。

## 2. 需求（要什么）

范围内（纯后端）：

- **通道一（权威，优先）**：向导可选填源 Nexus 的 REST `base_url` + 凭据引用 `auth_ref`（真值走
  环境变量，绝不入库 / 日志）。可达时经 `/service/rest/v1/repositories` 取权威仓库配置
  （名称 / 格式 / 类型 / 上游 URL），与离线 blob store 扫描结果**按仓库名对齐合并**——
  配置（格式 / 类型 / 上游）取 REST，blob 数与总大小取扫描。
- **通道二（推断，回退）**：未填地址或探测不可达时，纯离线扫描 blob store 枚举仓库，按内容路径
  **启发式推断格式**（见 §3 规则表），推断项在索引中**标注来源为「推断」**，仓库类型与上游 URL 留空。
- **统一索引结构**：两通道输出同一份仓库索引（名称、格式及其来源〔权威 / 推断〕、类型、blob 数、
  总大小、上游 URL），供向导多选与后续搬运消费。
- 新增一个 API 端点：触发离线仓库索引双通道枚举（仅 Admin），复用既有异步 job 基建
  （返回 `202 + job_id`，结果经 `GET /migrate/jobs/{id}` 进度字段轮询取回）。

不做（范围外）：

- 前端向导消费（FR-142）——不动 `frontend/`。
- 解析 OrientDB / H2 数据库文件（ADR-0041 已排除）。
- 制品搬运（FR-38/39 已交付）；本 FR 只产出索引，不搬运。
- 不改既有 `/offline/preview` 端点行为（其为 blob 明细预览，本 FR 是仓库级索引，两者并存）。

## 3. 设计（怎么做）

依 ADR-0041，不新写 ADR。涉及模块：`src/migrate/`（新增索引逻辑）、`src/api/migrate.rs`（端点编排）、
`src/api/mod.rs`（路由）。依赖方向 `api → migrate → offline/online` 单向，不破坏分层。

### 3.1 统一索引结构

```
RepoIndexEntry {
    name: String,              // 仓库名（两通道均取 blob store 扫描的仓库名为基准）
    format: String,            // 本系统格式名（maven/npm/docker/raw/...）
    format_source: 权威 | 推断, // 格式来源
    r#type: Option<String>,    // hosted/proxy（权威通道有；推断留空 None）
    blob_count: usize,         // 取自离线扫描
    total_size: u64,           // 该仓库 blob size 之和（取自离线扫描；缺失按 0）
    upstream_url: Option<String>, // proxy 上游（权威通道有；推断留空 None）
}
```

序列化 `format_source` 为小写字符串 `"authoritative"` / `"inferred"`。

### 3.2 格式推断启发式规则表（纯函数，便于穷举）

以离线扫描出的某仓库全部 blob 路径为输入，按内容路径特征打分，取最高分格式；均无命中兜底 raw。
标注来源恒为「推断」。规则（对齐 ADR-0041 四格式 + raw 兜底）：

| 格式 | 命中特征（blob 逻辑名路径，大小写不敏感） |
|---|---|
| maven | 含 `maven-metadata.xml`，或 group/artifact/version/file 布局且文件名以 `.pom/.jar/.war/.ear/.module` 结尾 |
| npm | 含 `/-/` 且文件名以 `.tgz` 结尾（npm tarball 布局），或路径含 `package.json` |
| docker | docker v2 内容寻址布局：路径含 `blobs/sha256`、`manifests/` 或 `_uploads/` |
| raw | 以上均不命中时的兜底 |

推断以「仓库内是否存在命中某格式的 blob」累计计分（每命中一次加权），取最高分；全零则 raw。

### 3.3 双通道选路与合并

```
discover_repo_index(offline_scan, rest: Option<(base_url, auth_ref)>) -> Vec<RepoIndexEntry>
```

1. 先对 blob store 做离线扫描（复用既有 `enumerate_blob_store`），按仓库聚合出
   `{name -> (blob_count, total_size, blob 路径集合)}`。
2. 若给定 REST 且探测**可达**（`discover_repositories` 成功返回）：进通道一——
   以扫描仓库集合为基准，按名匹配 REST 权威配置：
   - 命中：`format`/`type`/`upstream_url` 取 REST（格式经 `map_nexus_format` 归一，未实现格式回退推断），
     `format_source = 权威`；`blob_count`/`total_size` 取扫描。
   - REST 有但扫描无该仓库：不纳入索引（索引以「本地有 blob 可迁」为准）。
   - 扫描有但 REST 无：按通道二推断该仓库格式，`format_source = 推断`、类型 / 上游留空。
3. 若未给定 REST 或探测**不可达**（`discover_repositories` 报错）：进通道二——
   全部仓库走推断，类型 / 上游留空，`format_source = 推断`。可达性探测失败仅记 WARN、回退，不使整次失败。

「可达性探测」即调用 `discover_repositories`：成功=可达，任何 `MigrateError`=不可达回退。
凭据经 `auth_ref` → env 解析（`resolve_credential`），绝不入库 / 日志。

### 3.4 API 端点

`POST /api/v1/migrate/nexus/offline/index`（仅 Admin）：

- 请求体 `{ "path", "base_url"?, "auth_ref"? }`——`path` 必填（本地 blob store 根目录）；
  `base_url` / `auth_ref` 可选（给定则尝试权威通道，不可达自动回退推断）。
- 同步阶段廉价校验（`path` 非空、是目录）后立即返回 `202 + job_id`；后台异步执行扫描 + 双通道，
  结果经 `GET /migrate/jobs/{id}` 进度的新字段 `repo_index` 取回（复用 FR-83 任务注册表）。
- 不参与迁移搬运单飞门（与 `/offline/preview` 一致，纯枚举）。

## 4. 任务拆分

- [ ] `src/migrate/offline.rs`（或新子模块）：`RepoIndexEntry` + `FormatSource` 结构、格式推断纯函数、按仓库聚合
- [ ] `src/migrate/mod.rs`：`discover_repo_index` 双通道选路 + 合并；导出新类型
- [ ] `OnlinePullProgress` 增 `repo_index` 字段（`skip_serializing_if = None`）
- [ ] `src/api/migrate.rs`：新端点 handler + 请求体
- [ ] `src/api/mod.rs`：注册路由
- [ ] 单元测试（migrate 模块，红→绿）：格式推断穷举、合并规则、不可达回退、结构字段
- [ ] 集成测试（tests/migrate_api.rs）：鉴权矩阵（匿名 401 / 非 Admin 403 / Admin 202）、路径校验 400
- [ ] 文档同步：PRD 状态、API.md、CHANGELOG

## 5. 验收标准

- 纯离线通道：含 maven/npm/docker/raw 样本路径的临时 blob store，格式推断正确并标「推断」，类型 / 上游留空。
- 权威通道：mock REST 返回仓库配置，与扫描按名合并——配置取 REST、blob 数 / 大小取扫描、`format_source = 权威`。
- 不可达回退：给定 base_url 但探测失败时回退纯离线推断，不报错。
- 统一索引结构字段正确（`name`/`format`/`format_source`/`type`/`blob_count`/`total_size`/`upstream_url`）。
- 端点鉴权：仅 Admin；匿名 401、非 Admin 403、Admin `202 + job_id`。
- `cargo test -j 2` 全绿、`cargo fmt --check` 干净、`cargo clippy --all-targets -j 2 -- -D warnings` 零告警。
- **真机验收（待用户确认）**：指向真实 Nexus OSS 3.70.x 库，权威通道取回权威配置、离线通道推断正确。
  本批用临时目录 + mock REST 覆盖逻辑正确性，真机维度标「待真机验」如实报告。

## 6. 风险 / 待定

- 推断通道对 blob store 目录布局有隐式依赖（既有离线枚举已有此风险），跨 Nexus 版本需维护。
- 格式推断存在误判可能（如混合内容仓库），由 FR-142 向导侧人工修正兜底。
- 权威通道格式经 `map_nexus_format` 归一；REST 报告的未实现格式（如 rubygems）回退推断而非硬报错。
