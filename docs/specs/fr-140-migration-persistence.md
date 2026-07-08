# 功能规格：迁移任务与报告持久化（FR-140）

> 状态：开发中　·　关联 PRD：FR-140　·　关联 ADR：ADR-0040　·　分支：feature/fr-140-migration-persistence

## 1. 背景与目标

Nexus 迁移（M4）面对上万 blob、跑数小时至数天的大迁移，现状迁移任务纯进程内（`MigrationJobs` / `TaskRegistry`）、不落库、重启即丢：历史与分仓库报告丢失、在途中断不可见、刷新网页无法恢复视图。

本功能按 **ADR-0040** 把迁移任务的**记录、节流进度快照、分仓库明细报告**经 `meta` 层落 SQLite，使进程重启后历史 / 进度 / 报告仍可查、残留在途任务标「已中断」、并可一键幂等重跑续传。属 P2 / M4，纯后端（前端恢复 UI 为 FR-142，不在本范围）。

## 2. 需求（要什么）

### 范围内
- **迁移任务表**（经 `meta` 层读写，唯一入口，其他模块不绕过）：
  - 任务记录：id（= job_id）、迁移类别 kind、统一状态、参数快照（**只存 auth_ref 引用与非敏感配置，凭据真值绝不入库**）、起 / 止 / 更新时间、错误信息。
  - 进度快照：phase + 聚合计数（total/done assets、migrated/skipped_existing/skipped）+ 聚合字节数（total/done/migrated/skipped bytes）。
  - 分仓库明细报告：每仓库源 / 目标仓库、格式、created、迁移数、增量跳过数、失败跳过数。
- **节流落库**：运行中进度快照按节流策略（最小落库间隔）定期落库，不逐 asset 写库（避免写放大）；状态变更（暂停 / 继续 / 取消 / 终态）即时落库；终态写完整报告 + 分仓库明细。
- **中断标记**：进程启动时把库中残留的 `running` / `paused` 迁移任务统一归为 `interrupted`（已中断）。
- **一键幂等重跑续传**：对已中断任务可重跑——复用现有迁移引擎的 sha256 跳过（目标已存在且一致即跳过），进度区分「跳过 vs 新搬」计数（引擎既有 `skipped_existing` vs `migrated`）。**不持久化枚举游标、不做自动断点续跑**。
- **任务列表 = 内存活跃 ∪ 库中历史**：运行态真源仍是进程内 `TaskRegistry` / `MigrationJobs`；SQLite 是历史与恢复视图。
- 新增只读端点：迁移历史列表 / 单任务历史详情（含报告）；重跑端点（对已中断任务按其参数快照重新触发一次幂等搬运）。

### 不做（范围外）
- 前端恢复 UI（FR-142）。
- 在线更新、漏洞库刷新落库（ADR-0040 明确仅迁移）。
- 持久化枚举游标 / 自动断点续跑。
- **per-asset 失败清单与原因、per-repo 字节数 / 耗时**：现有迁移引擎（`OnlinePullProgress` / `*MigrationReport`）仅产出聚合字节数与 per-repo 计数，不产出 per-asset 失败明细，也无 per-repo 字节 / 耗时（见下「设计决策」）。本期持久化引擎已产出的全部信息，不改引擎结构去新增 per-asset 明细（避免 ADR-0040 明令规避的写放大与引擎侵入）。任务级 `error` + per-repo 失败计数已落库；任务耗时由 started_at / finished_at 推导。

## 3. 设计（怎么做）

权威决策见 ADR-0040，此处不复述其正文，只记落地方案。

### 3.1 数据模型（新增迁移脚本 `migrations/0014_migration_tasks.sql`，LF 行尾，只增不改 ADR-0031；现有迁移已到 0013）

- `migration_tasks`：
  - `id` TEXT PK（= job_id）、`kind` TEXT（`online` / `offline_proxy` / `offline_hosted`）、`state` TEXT（`running`/`paused`/`succeeded`/`failed`/`cancelled`/`interrupted`）、`label` TEXT NULL。
  - `params_snapshot` TEXT NOT NULL：JSON，仅含重跑所需非敏感参数（base_url / auth_ref 引用名 / offline_path / repositories 选择 / target 名 等）；**绝不含凭据真值**。
  - 进度快照列：`phase` TEXT、`total_assets`/`done_assets`/`migrated`/`skipped_existing`/`skipped` INTEGER、`total_bytes`/`done_bytes`/`migrated_bytes`/`skipped_bytes` INTEGER。
  - `error` TEXT NULL、`started_at`/`updated_at`/`finished_at` TEXT（ISO8601，finished_at 终态才有）。
- `migration_task_repos`（分仓库明细，task_id FK ON DELETE CASCADE）：`task_id`、`source_repo`、`target_repo`、`format`、`created` INTEGER、`migrated_artifacts`/`skipped_existing_artifacts`/`skipped_artifacts` INTEGER、`position` INTEGER（稳定序）。
- 索引：`migration_tasks` 按 `started_at DESC` 列历史；`migration_task_repos` 按 `task_id`。

### 3.2 meta 层（新增 `src/meta/migrate.rs`，`mod migrate; pub use ...` 挂到 `meta/mod.rs`）

参照既有子模块（usage.rs / settings.rs）模式，在 `MetaStore` 上扩展：
- `MigrationTaskKind` / `MigrationTaskState` 枚举（入库小写字符串，未知值容错）。
- `NewMigrationTask`（起任务时 upsert 首行）、`MigrationTaskRecord`（sqlx::FromRow）、`MigrationTaskRepoRow`。
- 方法：
  - `upsert_migration_task(&self, task: &NewMigrationTask)`：起任务即时落首行（state=running）。
  - `update_migration_task_progress(&self, id, snapshot)`：节流点更新进度快照列 + updated_at（不动 repos 明细，避免写放大）。
  - `finish_migration_task(&self, id, state, snapshot, repos, error)`：**单事务内**写终态 + 完整进度快照 + 替换 repos 明细 + finished_at；失败回滚。
  - `set_migration_task_state(&self, id, state)`：暂停 / 继续即时落状态。
  - `mark_running_tasks_interrupted(&self)`：启动时把 running/paused → interrupted，返回受影响条数；updated_at / finished_at 一并置。
  - `list_migration_tasks(&self, limit)` / `get_migration_task(&self, id)`（含 repos 明细）。

### 3.3 api 层节流落库协调（新增 `src/api/migration_persist.rs`）

- `MigrationPersister`：持 `MetaStore` clone + 每任务最小落库间隔（常量，如 2s）。后台迁移任务在 asset 边界（`await_control` 附近，锁外）回调 `maybe_persist_progress(id, &progress_snapshot)`——距上次落库超阈值才写，否则跳过（节流）。
- 状态变更（pause/resume）经 `set_migration_task_state` 即时落。
- 后台任务收尾（`sync_migration_task_state` 附近）调 `finish_migration_task` 写终态 + 完整报告。
- 分层守恒：`api` → `meta`；持久化经 meta 层，不绕过。锁外做 IO（先 clone 出 `OnlinePullProgress` 快照放锁，再落库）。

### 3.4 启动中断标记（`src/main.rs`）

`MetaStore::open` 成功后、装配 AppState 前，调 `meta.mark_running_tasks_interrupted()`，INFO 日志记归一条数。

### 3.5 端点（`src/api/migrate.rs` + 路由注册）

- `GET /api/v1/migrate/tasks`（仅 Admin）：列迁移历史（库中记录，按 started_at DESC）。
- `GET /api/v1/migrate/tasks/{id}`（仅 Admin）：单任务历史 + 分仓库报告。
- `POST /api/v1/migrate/tasks/{id}/rerun`（仅 Admin）：对已中断（或终态）任务按其 `params_snapshot` 重新触发一次幂等搬运（复用既有 online/offline 搬运编排 + 单飞门）；凭据仍从 env 现解析（auth_ref 引用），不从库取。

## 4. 任务拆分

- [ ] 迁移脚本 `migrations/0014_migration_tasks.sql`（LF；两表 + 索引）；扩 `src/meta/mod.rs` 的 LF 回归测试覆盖 0014。
- [ ] meta 层 `src/meta/migrate.rs` + CRUD/事务方法 + 单测（红→绿）。
- [ ] api 层 `MigrationPersister` 节流落库 + 单测（节流间隔、状态即时落、终态完整报告）。
- [ ] 迁移端点后台任务接入持久化（online/offline proxy/hosted 四条搬运路径 + 收尾）。
- [ ] 启动中断标记接入 `main.rs`。
- [ ] 历史 / 详情 / 重跑端点 + 路由 + 集成测试（`tests/migrate_api.rs` 仿写：起迁移 → 重建 TaskRegistry/从 DB 加载 → 历史/进度/报告可查且状态 interrupted）。
- [ ] 凭据脱敏断言：参数快照不含明文密码。
- [ ] 文档同步：ARCHITECTURE（数据模型）、API（新端点）、CHANGELOG 未发布段追加一行。

## 5. 验收标准

- **重启恢复**（集成测试覆盖，对齐 PRD 验收）：起迁移 → 模拟重启（重建 `TaskRegistry` + `MigrationJobs`，`mark_running_tasks_interrupted` 跑一遍）→ `GET /migrate/tasks(/{id})` 历史 / 进度 / 报告仍可查且状态为 `interrupted`。
- **节流落库**：运行中不逐 asset 写库（断言落库次数远小于 asset 数）；状态变更即时落；终态写完整报告 + 分仓库明细。
- **幂等重跑**：对已中断任务重跑，目标已存在且 sha256 一致的计增量跳过（`skipped_existing`）、只补新增（`migrated`），计数区分正确（复用引擎既有语义，测试断言）。
- **凭据红线**（§2.6）：参数快照断言不含明文密码 / 凭据真值；仅存 auth_ref 引用。
- **元数据事务**（§2.5）：终态写「进度快照 + repos 明细」在单事务内原子完成，失败回滚不留半截（写 repos 失败则任务终态列也不更新）。
- **验证门**：`cargo test -j 2` 全绿、`cargo fmt --check` 干净、`cargo clippy --all-targets -j 2 -- -D warnings` 零告警。
- **待真机验**：真正指向真实 Nexus 的端到端「重启后续传只补新增」属 FR-142 / 整期验收，本期以集成测试（重建注册表从 DB 加载）覆盖恢复语义，真机项如实标注不冒充完成。

## 6. 风险 / 待定

- **report 深度取舍（已决策，见 §2 不做项）**：现有引擎不产出 per-asset 失败清单 / per-repo 字节 / 耗时；本期持久化引擎已产出的全部信息（任务级 error + per-repo 计数 + 聚合字节 + 起止时间推导耗时），不改引擎去新增 per-asset 明细，避免 ADR-0040 明令规避的写放大与引擎侵入。若后续确需 per-asset 失败明细，另起 FR。
- **rerun 与单飞门**：重跑复用既有搬运编排的单飞门（已有迁移在途则 409），不新造并发路径。
- **节流间隔常量**：取固定常量（如 2s），不做成配置项（YAGNI，需要再加）。
