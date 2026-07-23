# FR-145：迁移报告明细与分仓库指标

> 状态：开发中  
> 关联：FR-140、FR-144、ADR-0040  
> 目标：在不破坏现有迁移幂等、控制语义与节流持久化的前提下，补齐 per-asset 失败明细、per-repo 字节与耗时。

## 1. 范围

本规格覆盖：

- 普通在线 hosted（Maven / Raw / npm）迁移；
- 离线 hosted / proxy 迁移；
- Docker / OCI 在线迁移；
- 迁移运行模型、SQLite 持久化、历史详情 API 与前端展示。

不在本需求范围：

- 不把运行中 per-asset 明细持续写入 SQLite；
- 不改变任务注册表、暂停 / 继续 / 取消语义；
- 不新增依赖或修改构建配置；
- 不把原始 URL、绝对路径、响应正文、错误链或凭据写入失败原因。

## 2. 统一指标模型

### 2.1 任务级指标

`OnlinePullProgress` 继续维护：

- `total_bytes`
- `done_bytes`
- `migrated_bytes`
- `skipped_bytes`
- `asset_failures`
- `omitted`

字节口径：

- `total_bytes`：已枚举且已知大小的资产声明字节；未知大小按 0；
- `done_bytes`：已完成成功、幂等跳过或失败处理的已知字节；
- `migrated_bytes`：本次确实写入目标的新字节；
- `skipped_bytes`：幂等已存在、策略跳过或失败跳过的已知字节；
- 终态满足 `done_bytes <= total_bytes`，未知大小不虚构字节。

### 2.2 分仓库指标

`OnlineRepoMigrationOutcome` 与持久化报告均包含：

- `total_bytes`
- `done_bytes`
- `migrated_bytes`
- `skipped_bytes`
- `duration_ms`

`duration_ms` 从该仓库开始处理到仓库终态，使用饱和转换，避免溢出。

### 2.3 失败资产明细

失败明细结构：

```text
MigrationAssetFailure {
  asset: String,
  reason: String,
}
```

约束：

- 每任务保留最早 1,000 条；
- 超额只累加 `omitted` / `omitted_asset_failures`；
- `asset` 最多 1,024 个 Unicode 字符；
- `reason` 最多 512 个 Unicode 字符；
- 截断必须按字符边界，禁止产生非法 UTF-8；
- 原因使用稳定、脱敏的中文分类；
- 不记录凭据、查询参数、原始 URL、绝对路径、响应正文与完整错误链。

稳定分类至少覆盖：

- `下载失败`
- `校验失败`
- `路径非法`
- `读取源文件失败`
- `写入目标仓库失败`
- `目标策略拒绝覆盖`
- `制品超过大小限制`
- `Docker 清单读取失败`
- `Docker 清单解析失败`
- `Docker 内容摘要不匹配`
- `Docker blob 下载失败`
- `Docker blob 写入失败`
- `Docker manifest 写入失败`

## 3. Nexus 普通资产大小

`NexusAsset` 墐 `size: Option<u64>`，从 `fileSize` 解析：

- 非负整数：`Some(value)`；
- 缺失：`None`；
- 负数：`None`；
- 字符串、浮点数或其他非法类型：`None`；
- 单个非法值不得中断整页解析。

普通在线迁移失败时，若声明大小已知，计入 `done_bytes + skipped_bytes`；未知大小按 0。

## 4. 离线 hosted / proxy

- `total_bytes` 使用 `.properties` 中声明的 `size`；
- 声明缺失或非法时使用已可知的 entry size；仍未知则按 0；
- 成功新写：推进 done + migrated；
- 幂等已存在、覆盖拒绝、超限或失败：推进 done + skipped；
- 单制品失败追加有界失败明细，不中断整仓库；
- hosted / proxy 的非法路径、源读取、策略拒绝、超限与目标写入使用相邻一致的稳定中文分类；
- proxy 所有搬运入口必须显式传递 `max_size`，禁止以硬编码 `None` 绕过限制；
- 取消后的未处理资产不得推进 done。

## 5. Docker / OCI 统计与幂等

### 5.1 写入结果

`DockerRegistry::ingest_blob` 与 `put_manifest` 返回：

```text
DockerWriteOutcome {
  written: bool,
  payload_written: bool,
  size: u64,
  digest: String,
}
```

语义：

- `size` 是本次输入实体大小；
- `digest` 是规范化内容摘要；
- `payload_written` 仅表示本次新增内容寻址 payload，纯索引或 tag 修复为 `false`；
- 首次写入为 `written=true`；
- 同内容且索引状态完全一致为 `written=false`；
- manifest 内容相同但 digest 索引缺失、tag 缺失或 tag 指向变化时为 `written=true`；
- 保持共享 blob guard、失败清理与 manifest 索引原子性。

### 5.2 Docker 计数单元

- blob：按实体字节统计；
- manifest / manifest list / 子 manifest：按原始响应字节统计；
- tag 指针：0 字节；
- manifest payload 新写或已存在时统计 manifest 逻辑 payload；payload 已存在而仅新增 tag、补 tag 或改变 tag 指向时，四类 bytes 均不重复统计 manifest；
- 同 digest 重复引用按每次处理结果推进任务 / 仓库进度，`written=false` 计 skipped；
- 同一 tag 内失败最多追加一条失败明细；
- manifest / blob 的真实 `DigestMismatch` 统一分类为 `Docker 内容摘要不匹配`；
- manifest digest 不匹配、blob 下载或写入失败时，按已知实体大小计 done + skipped，未知按 0。

## 6. 持久化

新增只追加迁移：

```text
migrations/0016_migration_report_details.sql
```

Schema：

- `migration_tasks` 增 `omitted_asset_failures INTEGER NOT NULL DEFAULT 0`；
- `migration_task_repos` 墋 `total_bytes`、`done_bytes`、`migrated_bytes`、`skipped_bytes`，均 `INTEGER NOT NULL DEFAULT 0`；
- 新表 `migration_task_asset_failures`：`id`、`task_id`、`source_repo`、`asset`、`reason`、`occurred_at`；
- 为 `task_id` 增索引；
- 旧任务读取时新增数字字段为 0，失败明细为空列表。

### 6.1 终态事务

`finish_migration_task` 在同一事务内：

1. 更新任务终态与聚合快照；
2. 删除该任务旧失败明细；
3. 插入新失败明细；
4. 替换分仓库报告；
5. 提交事务。

任一步失败均整体回滚。

### 6.2 重跑与运行中更新

- upsert / rerun reset 清理旧仓库报告和旧失败明细；
- 运行中节流更新只写任务聚合快照，不写 per-repo 与 per-asset 明细；
- 暂停 / 继续不写失败详情；
- 仓库处理开始后，ensure、枚举、Docker 图拉取等致命错误须先追加含耗时和截至失败点计数的仓库结果，再把任务置为失败；
- 终态收口一次性写完整报告。

## 7. API

### 7.1 列表

任务列表可返回：

- 任务级四类 bytes；
- `duration_ms`；
- `omitted_asset_failures`。

列表不得为每条任务查询 failures，避免 N+1；不返回失败明细数组。

### 7.2 详情

详情返回：

- `repos[]`：计数、四类 bytes 与 `duration_ms`；
- `asset_failures[]`：`source_repo`、`asset`、`reason`；
- `omitted_asset_failures`。

仓库报告与失败明细应在同一只读事务快照中读取。

## 8. 前端

迁移当前任务报告与历史详情展示：

- 总字节、已处理字节、迁移字节、跳过字节；
- 仓库耗时；
- 失败仓库 / 资产 / 原因；
- “另有 N 条失败明细未展示”的 omitted 提示。

要求：

- 字节复用统一 `formatBytes`；
- 耗时使用统一格式化函数；
- 失败文案按普通文本渲染；
- 当前任务进入任一终态后始终显示失败资产区，空数组显示统一空态；
- `omitted_asset_failures` 在前端任务 DTO 中为必填数字字段；
- 禁止 `dangerouslySetInnerHTML`；
- 类型、MSW、i18n 与测试同步。

## 9. 实现 Checklist

- [x] 测试先行：fileSize、失败容量 / 截断、三类迁移字节与 Docker 幂等 outcome。
- [x] 扩展运行模型与普通在线、离线 hosted/proxy、Docker/OCI 计数。
- [x] Docker 写入 outcome 保持共享 blob guard 与原子 manifest 索引。
- [x] 新增 0016 迁移、meta 终态事务与旧记录兼容。
- [x] 扩展历史详情 API、前端类型、MSW、页面与测试。
- [x] 收口 hosted/proxy 超限与失败分类、Docker 六类安全分类及 tag-only 字节口径。
- [x] 致命仓库错误先保留仓库 outcome；耗时与持久化整数使用饱和转换。
- [x] 当前任务终态失败区、空态、四类 bytes 与 omitted 契约补齐代码级测试。
- [x] 同步 PRD、ARCHITECTURE、API、CHANGELOG。

## 10. 质量门

- [x] `cargo test --lib migrate:: --jobs 1`、`cargo test --lib format:: --jobs 1`、`cargo test --test migrate_api --jobs 1`。
- [x] `cargo check --jobs 1` 与 `cargo fmt --all -- --check`。
- [x] 前端 `npm test -- --run`、`npm run lint`、`npm run build`。
- [ ] 依赖真实 Nexus / Docker 客户端的迁移后 pull 真机验收。

## 11. 验收

- [ ] 普通在线 hosted 迁移报告显示 per-repo 四类字节、耗时与失败资产原因。
- [ ] 离线 hosted/proxy 对成功、幂等、失败与取消部分进度计数正确。
- [ ] Docker/OCI 按 blob + manifest 统计，tag 指针为 0，同 tag 失败仅一条明细。
- [ ] 历史详情重启后仍可读取；重跑不叠加旧仓库报告和失败明细。
- [ ] 列表无 failures N+1；旧任务显示 0 / 空列表。
- [ ] 前端安全展示字节、耗时、失败明细与 omitted 提示。
