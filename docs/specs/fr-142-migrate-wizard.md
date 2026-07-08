# 功能规格：迁移向导重构（前端）

> 状态：开发中　·　关联 PRD：FR-142（增强 FR-81，依赖 FR-140/141）　·　分支：feature/fr-142-migrate-wizard

## 1. 背景与目标

Nexus 迁移前端（B0 已落 `MigrationPage.tsx`）已是「源配置 → 预览 → 勾选执行 → 报告」的三步 Stepper，并具备异步任务轮询、URL/localStorage/任务表三路续看、暂停/继续/取消控制。M4 重做补齐了两块后端能力，前端需消费它们并把体验收敛为**统一迁移向导**：

- **FR-141 离线仓库索引双通道**（ADR-0041）：离线入口不再只枚举 blob 明细，而是产出**仓库级索引**（每仓库含格式及来源〔权威/推断〕、类型、blob 数、总大小、上游）。推断项的类型/上游需管理员在向导内补填。
- **FR-140 迁移任务与报告持久化**（ADR-0040）：迁移记录/进度快照/分仓库报告落 SQLite，进程重启后历史与报告可查、在途任务标「已中断」、可一键幂等重跑。

属 P2 / M4。目标：在既有向导基础上**重构为统一向导 + 索引双通道选择 + 持久化历史/恢复**，不从零重写。

## 2. 需求（要什么）

### 范围内（纯前端）

1. **统一向导步骤机**：源配置 → 仓库索引加载（异步 job）→ 仓库多选（目标改名 / 冲突提示 / 推断项补填类型与上游）→ **执行计划确认** → 分步执行进度 → 报告页。
2. **离线索引双通道消费**：离线入口调用 `POST /migrate/nexus/offline/index`（可选带 `base_url` + `auth_ref`），轮询 job 进度的 `repo_index` 字段回填仓库索引；每仓库展示 `name / format / format_source / type / blob_count / total_size / upstream_url`。
3. **推断项补填**：`format_source === 'inferred'` 的仓库，`type` 与 `upstream_url` 为空，向导在选择步骤要求/允许管理员补填仓库类型（proxy/hosted 决定走哪个搬运端点）与上游 URL（proxy 才需要）；权威项（`authoritative`）直接沿用后端值。
4. **执行计划确认步骤**：搬运前汇总「将迁哪些仓库、目标名、方式（在线拉取 / proxy / hosted）、推断补填结果」，管理员确认后才发起搬运。
5. **执行中恢复**：执行中刷新网页 / 离开回来，经迁移页（URL `?job_id=` / localStorage 存档 / `listTasks` 活跃任务）或任务中心恢复到进度视图（B0 已具备，向导重构后保持）。
6. **完成后历史报告可回看**：新增迁移历史区，消费 `GET /migrate/tasks` 列表与 `GET /migrate/tasks/{id}` 详情（含分仓库报告）；对 `interrupted` / 终态任务展示「一键重跑」调用 `POST /migrate/tasks/{id}/rerun`（返新 job_id，进入进度续看）。
7. **凭据脱敏红线不变**：源凭据仅以引用名 `auth_ref` 输入（口令型输入框），明文不回显、不持久化。

### 不做（范围外）

- 不动 `src/` Rust 后端、不改任何 API 契约（后端端点已就绪）。
- 不实现 per-asset 失败明细展示（属 FR-145，后端尚未产出该数据）。
- 不做在线入口的索引双通道（在线入口仍用 `/migrate/nexus/preview` 拿权威列表）。
- 不改 `docs/API.md`（前端不新增/改后端接口约定）、一般不动 `docs/ARCHITECTURE.md`。

## 3. 设计（怎么做）

涉及模块（均 `frontend/src/`）：`api/types.ts`、`api/endpoints.ts`、`pages/MigrationPage.tsx`、`i18n/locales/zh-CN/migration.ts`、`test/mocks/handlers.ts`（+ store）、`pages/MigrationPage.test.tsx`、新增 Playwright `e2e/migration.spec.ts`。无架构决策，**不新增 ADR**（消费既有 ADR-0040 / ADR-0041）。

### 3.1 类型层（对齐后端真实 JSON）

- `FormatSource = 'authoritative' | 'inferred'`。
- `RepoIndexEntry`：`{ name: string; format: string; format_source: FormatSource; type: string | null; blob_count: number; total_size: number; upstream_url: string | null }`（对齐 `src/migrate/index.rs::RepoIndexEntry`，`r#type` serde 为 `type`）。
- `OnlinePullJob` 增可选 `repo_index?: RepoIndexEntry[]`（对齐 `OnlinePullProgress.repo_index`，与 `offline_preview` 同为终态填充的可选字段）。
- `NexusOfflineIndexRequest`：`{ path: string; base_url?: string | null; auth_ref?: string | null }`。
- FR-140 历史 DTO：
  - `MigrationTaskKind = 'online' | 'offline_proxy' | 'offline_hosted'`；`MigrationHistoryState = TaskState | 'interrupted'`。
  - `MigrationTaskDto`：任务展平字段（`id/kind/state/label?/phase/total_assets/done_assets/migrated/skipped_existing/skipped/total_bytes/done_bytes/migrated_bytes/skipped_bytes/error?/started_at/updated_at/finished_at?`，时间为字符串）。
  - `MigrationRepoReportDto`：`{ source_repo; target_repo; format; created; migrated_artifacts; skipped_existing_artifacts; skipped_artifacts }`。
  - `MigrationTaskDetailDto = MigrationTaskDto & { repos: MigrationRepoReportDto[] }`。

### 3.2 端点层（新增封装，路径对齐 `docs/API.md`）

- `indexNexusOffline(req: NexusOfflineIndexRequest): Promise<MigrationJobCreated>` → `POST /migrate/nexus/offline/index`（202 + job_id）。
- `listMigrationTasks(): Promise<MigrationTaskDto[]>` → `GET /migrate/tasks`。
- `getMigrationTask(id): Promise<MigrationTaskDetailDto>` → `GET /migrate/tasks/{id}`。
- `rerunMigrationTask(id): Promise<MigrationJobCreated>` → `POST /migrate/tasks/{id}/rerun`（202）。

### 3.3 向导状态机与 UI（`MigrationPage.tsx`）

步骤（Stepper）：`①源配置` → `②仓库多选（含推断补填）` → `③执行计划确认` → `④执行进度 / 报告`。

- **①源配置**：在线/离线切换保持。离线入口新增可选「源 Nexus 地址 + 凭据引用」两输入（给定即走权威通道）；点「加载仓库索引」→ `indexNexusOffline` → 轮询 job 至终态 → 用 `repo_index` 回填选择行。在线入口仍走 `previewNexusRepositories`。
- **②仓库多选**：
  - 每行展示格式 + 来源徽章（权威=绿、推断=橙「推断」）、类型、blob 数、总大小、上游。
  - 在线入口：勾选 + 目标改名（B0 已有）。
  - 离线入口权威项：类型/上游只读沿用。
  - 离线入口推断项：必须由管理员选类型（proxy/hosted）；选 proxy 时可填上游 URL。未补填类型的勾选项不允许进入执行计划（冲突提示）。
- **③执行计划确认**：只读汇总所选仓库的「源名 → 目标名 / 方式 / 类型 / 上游」。管理员点「开始迁移」发起搬运；离线按每仓库类型分派 proxy/hosted 搬运端点（B0 的 proxy/hosted 按钮收敛为按补填类型自动分派），在线走 `migrateNexusOnline`。发起前做前端单飞检查（`findActiveMigrationTask`）。
- **④执行进度 / 报告**：复用 B0 的 `OnlineJobPanel` 轮询 + 控制（暂停/继续/取消）+ 续看/恢复逻辑。
- **迁移历史区**（页内独立区块或标签）：加载 `listMigrationTasks`，列出含 `interrupted` 的历史；点开经 `getMigrationTask` 展示分仓库报告；`interrupted` / 终态项提供「重跑」→ `rerunMigrationTask` → 进入 ④ 续看。

> 索引推断补填后的搬运仍走既有 `proxy/migrate` / `hosted/migrate`（离线目录 + 仓库白名单）——推断的类型只决定前端选哪个端点，后端仍按自身推断建仓，前端补填是**选择意图**而非改后端契约。上游 URL 补填在 P1 端点不接收（`NexusMigrateRequest` 无上游字段），故上游补填仅作计划展示与运维提示，不下发；如此不越界、不改契约。

### 3.4 Mock（有状态，ADR-0035）

- 新增 `POST /migrate/nexus/offline/index` handler：造带 `repo_index`（含权威 + 推断样本，覆盖 maven/npm/docker/raw）的离线 job，返 202 + job_id，轮询回填。
- 新增 `GET /migrate/tasks`、`GET /migrate/tasks/{id}`、`POST /migrate/tasks/{id}/rerun` handler：从 `state.migrationJobs` 派生历史 DTO（含一个 `interrupted` 样本）；rerun 造新 job 返新 job_id。

## 4. 任务拆分

- [ ] 类型层：新增 `RepoIndexEntry` / `FormatSource` / `NexusOfflineIndexRequest` / FR-140 历史 DTO，`OnlinePullJob` 加 `repo_index?`。
- [ ] 端点层：`indexNexusOffline` / `listMigrationTasks` / `getMigrationTask` / `rerunMigrationTask`。
- [ ] Mock：offline/index、migrate/tasks 系列 handler（有状态，含推断样本 + interrupted 样本）。
- [ ] 测试先行：vitest 用例（红→绿）覆盖索引双通道回填、推断补填冲突、执行计划确认、历史报告回看与重跑、执行中恢复。
- [ ] 实现 `MigrationPage.tsx` 向导重构（四步 + 历史区）。
- [ ] i18n：新增双通道 / 补填 / 计划确认 / 历史 / 重跑文案（中文）。
- [ ] Playwright `e2e/migration.spec.ts`：源配置→索引加载→选仓库→执行→报告，执行中刷新恢复。
- [ ] 文档同步：PRD 状态（本 spec §3 已改 `计划→开发中`）、CHANGELOG 未发布段追加一行。

## 5. 验收标准

- `npm test -- --run` 全绿（含新增/改写的 MigrationPage 用例，MSW 有状态强断言）。
- `npm run lint`（eslint + prettier）干净。
- Playwright `e2e/migration.spec.ts` 通过：全流程（源配置→索引加载→选仓库→执行→报告）+ 执行中刷新恢复到进度视图。
- 推断项未补填类型时不可进入执行计划 / 发起搬运（冲突提示）；权威项类型/上游沿用后端值。
- 历史区能列出含 `interrupted` 的历史任务，点开展示分仓库报告，`interrupted` / 终态项可一键重跑并进入进度续看。
- **真机全流程**（前端指向真实后端 + 真实 Nexus 走完源配置→索引→选仓库→执行→报告、执行中刷新恢复、重启后历史可查）属整期验收维度，单元/E2E（Mock）全绿不替代它，**交付时标「待真机验」如实报告**。

## 6. 风险 / 待定

- 上游 URL 补填在 P1 搬运端点无接收字段：本 spec 决定上游补填仅作计划展示/运维提示、不下发（守不改契约、不越界）。若后续需要下发需先扩后端契约（另开 FR）。
- B0 的 proxy/hosted 双执行按钮收敛为「按补填/权威类型自动分派」，需保证既有 proxy/hosted 搬运测试语义仍被覆盖（改写而非删除既有断言）。
- 历史区与任务中心（TaskCenterPage）职责有重叠：历史区聚焦「迁移专属持久化报告 + 重跑」，任务中心是跨 kind 活跃/近期视图；二者消费不同端点（`/migrate/tasks` vs `/tasks`），不合并。
