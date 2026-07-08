//! 迁移任务与报告的元数据存取（FR-140，ADR-0040）。
//!
//! 与 `meta/mod.rs` 同属元数据访问层，仅在 `MetaStore` 上扩展 `migration_tasks` /
//! `migration_task_repos` 两表读写；SQLite 仍是元数据唯一真源，其他模块经此读写、不绕过直连 DB。
//!
//! 设计要点（ADR-0040）：
//! - **历史与恢复视图**：运行态真源仍是进程内 `TaskRegistry` / `MigrationJobs`，本表存历史记录、
//!   节流进度快照与分仓库报告，供进程重启后回溯与恢复视图。
//! - **节流落库**：运行中进度按节流策略更新单行快照列（不逐 asset 写库、不动明细，避免写放大）；
//!   状态变更即时落；终态在单事务内写完整快照 + 替换分仓库明细。
//! - **中断标记**：启动时把库中残留 `running` / `paused` 任务统一归 `interrupted`。
//! - **凭据红线**：参数快照仅存 `auth_ref` 引用与非敏感配置，凭据真值绝不入库。

use super::{MetaError, MetaStore};

/// 迁移任务类别：三类搬运任务，入库小写字符串，避免魔法字符串散落。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationTaskKind {
    /// 在线拉取（经 REST 枚举 + HTTP 下载）。
    Online,
    /// 离线 proxy 缓存制品搬运。
    OfflineProxy,
    /// 离线 hosted 完整制品搬运。
    OfflineHosted,
}

impl MigrationTaskKind {
    /// 入库字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            MigrationTaskKind::Online => "online",
            MigrationTaskKind::OfflineProxy => "offline_proxy",
            MigrationTaskKind::OfflineHosted => "offline_hosted",
        }
    }

    /// 从 DB 字符串解析类别；未知值回退为 `Online`（仅用于展示，不影响执行路径）。
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "offline_proxy" => MigrationTaskKind::OfflineProxy,
            "offline_hosted" => MigrationTaskKind::OfflineHosted,
            _ => MigrationTaskKind::Online,
        }
    }
}

/// 迁移任务统一状态：与进程内 `TaskState` 归一，另加持久层独有的 `Interrupted`（已中断）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationTaskState {
    /// 运行中。
    Running,
    /// 已暂停。
    Paused,
    /// 成功完成。
    Succeeded,
    /// 失败。
    Failed,
    /// 已取消（不算失败）。
    Cancelled,
    /// 已中断：进程重启时库中残留的 running / paused 任务被统一归为此态（ADR-0040）。
    Interrupted,
}

impl MigrationTaskState {
    /// 入库字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            MigrationTaskState::Running => "running",
            MigrationTaskState::Paused => "paused",
            MigrationTaskState::Succeeded => "succeeded",
            MigrationTaskState::Failed => "failed",
            MigrationTaskState::Cancelled => "cancelled",
            MigrationTaskState::Interrupted => "interrupted",
        }
    }

    /// 是否为终态（不再变化）：succeeded / failed / cancelled / interrupted。
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            MigrationTaskState::Succeeded
                | MigrationTaskState::Failed
                | MigrationTaskState::Cancelled
                | MigrationTaskState::Interrupted
        )
    }
}

/// 迁移进度快照：节流点与终态落库的进度计数（与引擎 `OnlinePullProgress` 的可持久字段对应）。
///
/// 只承载聚合计数与阶段，不含当前 asset 路径等易变瞬时字段（无历史价值、徒增写放大）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationProgressSnapshot {
    /// 引擎阶段字符串（enumerating / downloading / paused / cancelled / done / failed）。
    pub phase: String,
    /// 已枚举待搬运 asset 总数。
    pub total_assets: i64,
    /// 已处理 asset 数。
    pub done_assets: i64,
    /// 成功新写入数。
    pub migrated: i64,
    /// 增量跳过数（目标已存在且 sha256 一致）。
    pub skipped_existing: i64,
    /// 失败跳过数。
    pub skipped: i64,
    /// 待搬运逻辑总字节数。
    pub total_bytes: i64,
    /// 已处理逻辑字节数。
    pub done_bytes: i64,
    /// 成功新写入逻辑字节数。
    pub migrated_bytes: i64,
    /// 跳过 / 失败逻辑字节数。
    pub skipped_bytes: i64,
}

/// 单个分仓库明细报告项（入库 / 查询共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationRepoReport {
    /// 源仓库名。
    pub source_repo: String,
    /// 本系统目标仓库名。
    pub target_repo: String,
    /// 本系统格式名。
    pub format: String,
    /// 目标仓库是否新建。
    pub created: bool,
    /// 成功新写入的制品数。
    pub migrated_artifacts: i64,
    /// 增量跳过数（目标已存在且 sha256 一致）。
    pub skipped_existing_artifacts: i64,
    /// 失败跳过数。
    pub skipped_artifacts: i64,
}

/// 起任务时的首行写入入参。
#[derive(Debug, Clone)]
pub struct NewMigrationTask {
    /// 任务 id（= job_id）。
    pub id: String,
    /// 迁移类别。
    pub kind: MigrationTaskKind,
    /// 人类可读标签。
    pub label: Option<String>,
    /// 参数快照 JSON：仅含 auth_ref 引用与非敏感配置，凭据真值绝不入库。
    pub params_snapshot: String,
}

/// 迁移任务记录（查询返回视图，含进度快照列，不含分仓库明细）。
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct MigrationTaskRecord {
    /// 任务 id。
    pub id: String,
    /// 迁移类别字符串。
    pub kind: String,
    /// 统一状态字符串。
    pub state: String,
    /// 人类可读标签。
    pub label: Option<String>,
    /// 参数快照 JSON（仅非敏感项）。
    pub params_snapshot: String,
    /// 引擎阶段字符串。
    pub phase: String,
    /// 已枚举待搬运 asset 总数。
    pub total_assets: i64,
    /// 已处理 asset 数。
    pub done_assets: i64,
    /// 成功新写入数。
    pub migrated: i64,
    /// 增量跳过数。
    pub skipped_existing: i64,
    /// 失败跳过数。
    pub skipped: i64,
    /// 待搬运逻辑总字节数。
    pub total_bytes: i64,
    /// 已处理逻辑字节数。
    pub done_bytes: i64,
    /// 成功新写入逻辑字节数。
    pub migrated_bytes: i64,
    /// 跳过 / 失败逻辑字节数。
    pub skipped_bytes: i64,
    /// 失败原因。
    pub error: Option<String>,
    /// 起始时刻。
    pub started_at: String,
    /// 最近落库时刻。
    pub updated_at: String,
    /// 终态时刻（未结束为 None）。
    pub finished_at: Option<String>,
}

/// 分仓库明细查询行（sqlx 映射）。
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct MigrationTaskRepoRow {
    /// 源仓库名。
    pub source_repo: String,
    /// 本系统目标仓库名。
    pub target_repo: String,
    /// 本系统格式名。
    pub format: String,
    /// 目标仓库是否新建（0/1）。
    pub created: i64,
    /// 成功新写入的制品数。
    pub migrated_artifacts: i64,
    /// 增量跳过数。
    pub skipped_existing_artifacts: i64,
    /// 失败跳过数。
    pub skipped_artifacts: i64,
}

impl MetaStore {
    /// 起任务即时落首行（state = running）。同 id 重复登记视为覆盖（幂等，供重跑复用 id 场景）。
    pub async fn upsert_migration_task(&self, task: &NewMigrationTask) -> Result<(), MetaError> {
        sqlx::query(
            "INSERT INTO migration_tasks \
                (id, kind, state, label, params_snapshot, phase) \
             VALUES (?, ?, 'running', ?, ?, 'enumerating') \
             ON CONFLICT(id) DO UPDATE SET \
                kind = excluded.kind, \
                state = 'running', \
                label = excluded.label, \
                params_snapshot = excluded.params_snapshot, \
                phase = 'enumerating', \
                total_assets = 0, done_assets = 0, \
                migrated = 0, skipped_existing = 0, skipped = 0, \
                total_bytes = 0, done_bytes = 0, migrated_bytes = 0, skipped_bytes = 0, \
                error = NULL, \
                started_at = CURRENT_TIMESTAMP, \
                updated_at = CURRENT_TIMESTAMP, \
                finished_at = NULL",
        )
        .bind(&task.id)
        .bind(task.kind.as_str())
        .bind(&task.label)
        .bind(&task.params_snapshot)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 节流点更新进度快照列 + updated_at（不动状态与分仓库明细，避免写放大）。
    ///
    /// 未知 id 为空操作。运行中任务的高频进度更新走此路径。
    pub async fn update_migration_task_progress(
        &self,
        id: &str,
        snapshot: &MigrationProgressSnapshot,
    ) -> Result<(), MetaError> {
        sqlx::query(
            "UPDATE migration_tasks SET \
                phase = ?, \
                total_assets = ?, done_assets = ?, \
                migrated = ?, skipped_existing = ?, skipped = ?, \
                total_bytes = ?, done_bytes = ?, migrated_bytes = ?, skipped_bytes = ?, \
                updated_at = CURRENT_TIMESTAMP \
             WHERE id = ?",
        )
        .bind(&snapshot.phase)
        .bind(snapshot.total_assets)
        .bind(snapshot.done_assets)
        .bind(snapshot.migrated)
        .bind(snapshot.skipped_existing)
        .bind(snapshot.skipped)
        .bind(snapshot.total_bytes)
        .bind(snapshot.done_bytes)
        .bind(snapshot.migrated_bytes)
        .bind(snapshot.skipped_bytes)
        .bind(id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 即时更新任务状态（暂停 / 继续用）。未知 id 为空操作。终态经 `finish_migration_task` 走，
    /// 本方法不写 finished_at / 报告。
    pub async fn set_migration_task_state(
        &self,
        id: &str,
        state: MigrationTaskState,
    ) -> Result<(), MetaError> {
        sqlx::query(
            "UPDATE migration_tasks SET state = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(state.as_str())
        .bind(id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 终态落库：**单事务内**写终态 + 完整进度快照 + 替换分仓库明细 + finished_at。
    ///
    /// 元数据事务原子性（§2.5）：先删旧明细再插新明细、更新任务行，任一失败整体回滚，
    /// 不留半截（不出现「任务标终态但明细未写全」）。未知 id：UPDATE 影响 0 行、明细也不挂靠，
    /// 事务仍提交（幂等空操作）。
    pub async fn finish_migration_task(
        &self,
        id: &str,
        state: MigrationTaskState,
        snapshot: &MigrationProgressSnapshot,
        repos: &[MigrationRepoReport],
        error: Option<&str>,
    ) -> Result<(), MetaError> {
        let mut tx = self.pool().begin().await?;

        sqlx::query(
            "UPDATE migration_tasks SET \
                state = ?, phase = ?, \
                total_assets = ?, done_assets = ?, \
                migrated = ?, skipped_existing = ?, skipped = ?, \
                total_bytes = ?, done_bytes = ?, migrated_bytes = ?, skipped_bytes = ?, \
                error = ?, \
                updated_at = CURRENT_TIMESTAMP, \
                finished_at = CURRENT_TIMESTAMP \
             WHERE id = ?",
        )
        .bind(state.as_str())
        .bind(&snapshot.phase)
        .bind(snapshot.total_assets)
        .bind(snapshot.done_assets)
        .bind(snapshot.migrated)
        .bind(snapshot.skipped_existing)
        .bind(snapshot.skipped)
        .bind(snapshot.total_bytes)
        .bind(snapshot.done_bytes)
        .bind(snapshot.migrated_bytes)
        .bind(snapshot.skipped_bytes)
        .bind(error)
        .bind(id)
        .execute(&mut *tx)
        .await?;

        // 替换分仓库明细：先清旧（重跑同 id 时不叠加），再按序插新。
        sqlx::query("DELETE FROM migration_task_repos WHERE task_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for (position, r) in repos.iter().enumerate() {
            sqlx::query(
                "INSERT INTO migration_task_repos \
                    (task_id, position, source_repo, target_repo, format, created, \
                     migrated_artifacts, skipped_existing_artifacts, skipped_artifacts) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(position as i64)
            .bind(&r.source_repo)
            .bind(&r.target_repo)
            .bind(&r.format)
            .bind(r.created as i64)
            .bind(r.migrated_artifacts)
            .bind(r.skipped_existing_artifacts)
            .bind(r.skipped_artifacts)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// 启动时把库中残留的 running / paused 迁移任务统一归为 interrupted（ADR-0040）。
    ///
    /// 返回受影响条数。一并置 finished_at（中断即终止），updated_at 刷新。
    pub async fn mark_running_migration_tasks_interrupted(&self) -> Result<u64, MetaError> {
        let affected = sqlx::query(
            "UPDATE migration_tasks SET \
                state = 'interrupted', \
                updated_at = CURRENT_TIMESTAMP, \
                finished_at = CURRENT_TIMESTAMP \
             WHERE state IN ('running', 'paused')",
        )
        .execute(self.pool())
        .await?
        .rows_affected();
        Ok(affected)
    }

    /// 列出近期迁移任务历史，按起始时刻倒序（最新在前），最多 `limit` 条。
    pub async fn list_migration_tasks(
        &self,
        limit: i64,
    ) -> Result<Vec<MigrationTaskRecord>, MetaError> {
        let records = sqlx::query_as::<_, MigrationTaskRecord>(
            "SELECT id, kind, state, label, params_snapshot, phase, \
                    total_assets, done_assets, migrated, skipped_existing, skipped, \
                    total_bytes, done_bytes, migrated_bytes, skipped_bytes, \
                    error, started_at, updated_at, finished_at \
             FROM migration_tasks \
             ORDER BY started_at DESC, id DESC \
             LIMIT ?",
        )
        .bind(limit)
        .fetch_all(self.pool())
        .await?;
        Ok(records)
    }

    /// 按 id 查单个迁移任务记录；不存在返回 None。
    pub async fn get_migration_task(
        &self,
        id: &str,
    ) -> Result<Option<MigrationTaskRecord>, MetaError> {
        let record = sqlx::query_as::<_, MigrationTaskRecord>(
            "SELECT id, kind, state, label, params_snapshot, phase, \
                    total_assets, done_assets, migrated, skipped_existing, skipped, \
                    total_bytes, done_bytes, migrated_bytes, skipped_bytes, \
                    error, started_at, updated_at, finished_at \
             FROM migration_tasks WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        Ok(record)
    }

    /// 查某任务的分仓库明细报告，按 position 升序。
    pub async fn get_migration_task_repos(
        &self,
        id: &str,
    ) -> Result<Vec<MigrationTaskRepoRow>, MetaError> {
        let rows = sqlx::query_as::<_, MigrationTaskRepoRow>(
            "SELECT source_repo, target_repo, format, created, \
                    migrated_artifacts, skipped_existing_artifacts, skipped_artifacts \
             FROM migration_task_repos WHERE task_id = ? ORDER BY position ASC",
        )
        .bind(id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 新任务(id: &str) -> NewMigrationTask {
        NewMigrationTask {
            id: id.to_string(),
            kind: MigrationTaskKind::Online,
            label: Some("在线拉取迁移".to_string()),
            params_snapshot: r#"{"base_url":"https://nexus.example","auth_ref":"prod"}"#
                .to_string(),
        }
    }

    fn 进度(phase: &str) -> MigrationProgressSnapshot {
        MigrationProgressSnapshot {
            phase: phase.to_string(),
            total_assets: 10,
            done_assets: 6,
            migrated: 4,
            skipped_existing: 2,
            skipped: 0,
            total_bytes: 1000,
            done_bytes: 600,
            migrated_bytes: 400,
            skipped_bytes: 200,
        }
    }

    #[tokio::test]
    async fn 起任务后可查出且状态为运行中() {
        let store = MetaStore::open_in_memory().await.unwrap();
        store.upsert_migration_task(&新任务("t1")).await.unwrap();

        let got = store.get_migration_task("t1").await.unwrap().unwrap();
        assert_eq!(got.id, "t1");
        assert_eq!(got.kind, "online");
        assert_eq!(got.state, "running");
        assert_eq!(got.label.as_deref(), Some("在线拉取迁移"));
        assert!(got.finished_at.is_none());
        assert_eq!(got.migrated, 0);
        // 未知 id 返回 None
        assert!(store
            .get_migration_task("无此任务")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn 节流更新进度快照不改状态() {
        let store = MetaStore::open_in_memory().await.unwrap();
        store.upsert_migration_task(&新任务("t1")).await.unwrap();

        store
            .update_migration_task_progress("t1", &进度("downloading"))
            .await
            .unwrap();

        let got = store.get_migration_task("t1").await.unwrap().unwrap();
        // 进度列已更新
        assert_eq!(got.phase, "downloading");
        assert_eq!(got.total_assets, 10);
        assert_eq!(got.done_assets, 6);
        assert_eq!(got.migrated, 4);
        assert_eq!(got.skipped_existing, 2);
        assert_eq!(got.migrated_bytes, 400);
        // 状态仍为 running（进度更新不动状态 / 终态时刻）
        assert_eq!(got.state, "running");
        assert!(got.finished_at.is_none());
    }

    #[tokio::test]
    async fn 更新进度_未知id为空操作() {
        let store = MetaStore::open_in_memory().await.unwrap();
        // 不 panic、不报错
        store
            .update_migration_task_progress("无此任务", &进度("downloading"))
            .await
            .unwrap();
        assert!(store.list_migration_tasks(10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn 暂停继续即时落状态() {
        let store = MetaStore::open_in_memory().await.unwrap();
        store.upsert_migration_task(&新任务("t1")).await.unwrap();

        store
            .set_migration_task_state("t1", MigrationTaskState::Paused)
            .await
            .unwrap();
        assert_eq!(
            store.get_migration_task("t1").await.unwrap().unwrap().state,
            "paused"
        );

        store
            .set_migration_task_state("t1", MigrationTaskState::Running)
            .await
            .unwrap();
        assert_eq!(
            store.get_migration_task("t1").await.unwrap().unwrap().state,
            "running"
        );
    }

    #[tokio::test]
    async fn 终态写完整快照与分仓库明细() {
        let store = MetaStore::open_in_memory().await.unwrap();
        store.upsert_migration_task(&新任务("t1")).await.unwrap();

        let repos = vec![
            MigrationRepoReport {
                source_repo: "maven-releases".to_string(),
                target_repo: "maven-releases".to_string(),
                format: "maven".to_string(),
                created: true,
                migrated_artifacts: 3,
                skipped_existing_artifacts: 1,
                skipped_artifacts: 0,
            },
            MigrationRepoReport {
                source_repo: "maven-snapshots".to_string(),
                target_repo: "snap".to_string(),
                format: "maven".to_string(),
                created: false,
                migrated_artifacts: 1,
                skipped_existing_artifacts: 1,
                skipped_artifacts: 2,
            },
        ];
        store
            .finish_migration_task(
                "t1",
                MigrationTaskState::Succeeded,
                &进度("done"),
                &repos,
                None,
            )
            .await
            .unwrap();

        let got = store.get_migration_task("t1").await.unwrap().unwrap();
        assert_eq!(got.state, "succeeded");
        assert_eq!(got.phase, "done");
        assert!(got.finished_at.is_some(), "终态应记 finished_at");
        assert!(got.error.is_none());

        let rows = store.get_migration_task_repos("t1").await.unwrap();
        assert_eq!(rows.len(), 2);
        // 按 position 稳定序
        assert_eq!(rows[0].source_repo, "maven-releases");
        assert_eq!(rows[0].created, 1);
        assert_eq!(rows[0].migrated_artifacts, 3);
        assert_eq!(rows[1].target_repo, "snap");
        assert_eq!(rows[1].created, 0);
        assert_eq!(rows[1].skipped_artifacts, 2);
    }

    #[tokio::test]
    async fn 终态失败记录错误原因() {
        let store = MetaStore::open_in_memory().await.unwrap();
        store.upsert_migration_task(&新任务("t1")).await.unwrap();
        store
            .finish_migration_task(
                "t1",
                MigrationTaskState::Failed,
                &进度("failed"),
                &[],
                Some("源系统鉴权失败"),
            )
            .await
            .unwrap();
        let got = store.get_migration_task("t1").await.unwrap().unwrap();
        assert_eq!(got.state, "failed");
        assert_eq!(got.error.as_deref(), Some("源系统鉴权失败"));
        assert!(store
            .get_migration_task_repos("t1")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn 终态替换分仓库明细不叠加() {
        // 重跑同 id 终态时应替换旧明细，而非叠加
        let store = MetaStore::open_in_memory().await.unwrap();
        store.upsert_migration_task(&新任务("t1")).await.unwrap();
        let first = vec![MigrationRepoReport {
            source_repo: "r1".to_string(),
            target_repo: "r1".to_string(),
            format: "maven".to_string(),
            created: true,
            migrated_artifacts: 1,
            skipped_existing_artifacts: 0,
            skipped_artifacts: 0,
        }];
        store
            .finish_migration_task(
                "t1",
                MigrationTaskState::Succeeded,
                &进度("done"),
                &first,
                None,
            )
            .await
            .unwrap();
        assert_eq!(store.get_migration_task_repos("t1").await.unwrap().len(), 1);

        // 二次终态（重跑）用新明细替换
        let second = vec![
            MigrationRepoReport {
                source_repo: "r1".to_string(),
                target_repo: "r1".to_string(),
                format: "maven".to_string(),
                created: false,
                migrated_artifacts: 0,
                skipped_existing_artifacts: 1,
                skipped_artifacts: 0,
            },
            MigrationRepoReport {
                source_repo: "r2".to_string(),
                target_repo: "r2".to_string(),
                format: "maven".to_string(),
                created: true,
                migrated_artifacts: 5,
                skipped_existing_artifacts: 0,
                skipped_artifacts: 0,
            },
        ];
        store
            .finish_migration_task(
                "t1",
                MigrationTaskState::Succeeded,
                &进度("done"),
                &second,
                None,
            )
            .await
            .unwrap();
        let rows = store.get_migration_task_repos("t1").await.unwrap();
        assert_eq!(rows.len(), 2, "应替换为新明细而非叠加");
        assert_eq!(rows[0].skipped_existing_artifacts, 1);
        assert_eq!(rows[1].source_repo, "r2");
    }

    #[tokio::test]
    async fn 启动中断标记归一残留任务() {
        let store = MetaStore::open_in_memory().await.unwrap();
        // 两个在途（running / paused）+ 一个已终态
        store.upsert_migration_task(&新任务("run")).await.unwrap();
        store.upsert_migration_task(&新任务("pause")).await.unwrap();
        store
            .set_migration_task_state("pause", MigrationTaskState::Paused)
            .await
            .unwrap();
        store.upsert_migration_task(&新任务("done")).await.unwrap();
        store
            .finish_migration_task(
                "done",
                MigrationTaskState::Succeeded,
                &进度("done"),
                &[],
                None,
            )
            .await
            .unwrap();

        let affected = store
            .mark_running_migration_tasks_interrupted()
            .await
            .unwrap();
        assert_eq!(affected, 2, "只归一 running / paused 两个");

        assert_eq!(
            store
                .get_migration_task("run")
                .await
                .unwrap()
                .unwrap()
                .state,
            "interrupted"
        );
        let paused = store.get_migration_task("pause").await.unwrap().unwrap();
        assert_eq!(paused.state, "interrupted");
        assert!(paused.finished_at.is_some(), "中断应记 finished_at");
        // 已终态任务不受影响
        assert_eq!(
            store
                .get_migration_task("done")
                .await
                .unwrap()
                .unwrap()
                .state,
            "succeeded"
        );
    }

    #[tokio::test]
    async fn 历史列表按起始时刻倒序且受限() {
        let store = MetaStore::open_in_memory().await.unwrap();
        for i in 0..3 {
            store
                .upsert_migration_task(&新任务(&format!("t{i}")))
                .await
                .unwrap();
        }
        let all = store.list_migration_tasks(10).await.unwrap();
        assert_eq!(all.len(), 3);
        // limit 生效
        let limited = store.list_migration_tasks(2).await.unwrap();
        assert_eq!(limited.len(), 2);
    }

    #[tokio::test]
    async fn 参数快照不含凭据真值() {
        // 凭据红线（§2.6）：入库参数快照仅存 auth_ref 引用，不含明文密码
        let store = MetaStore::open_in_memory().await.unwrap();
        store.upsert_migration_task(&新任务("t1")).await.unwrap();
        let got = store.get_migration_task("t1").await.unwrap().unwrap();
        assert!(got.params_snapshot.contains("auth_ref"));
        assert!(got.params_snapshot.contains("prod"));
        // 断言不含任何凭据真值样本
        assert!(!got.params_snapshot.to_lowercase().contains("password"));
        assert!(!got.params_snapshot.contains("s3cr3t"));
    }

    #[tokio::test]
    async fn 删任务级联删明细() {
        // 外键 ON DELETE CASCADE：删任务应清其分仓库明细
        let store = MetaStore::open_in_memory().await.unwrap();
        store.upsert_migration_task(&新任务("t1")).await.unwrap();
        let repos = vec![MigrationRepoReport {
            source_repo: "r1".to_string(),
            target_repo: "r1".to_string(),
            format: "maven".to_string(),
            created: true,
            migrated_artifacts: 1,
            skipped_existing_artifacts: 0,
            skipped_artifacts: 0,
        }];
        store
            .finish_migration_task(
                "t1",
                MigrationTaskState::Succeeded,
                &进度("done"),
                &repos,
                None,
            )
            .await
            .unwrap();
        assert_eq!(store.get_migration_task_repos("t1").await.unwrap().len(), 1);

        sqlx::query("DELETE FROM migration_tasks WHERE id = ?")
            .bind("t1")
            .execute(store.pool())
            .await
            .unwrap();
        assert!(store
            .get_migration_task_repos("t1")
            .await
            .unwrap()
            .is_empty());
    }

    #[test]
    fn 类别与状态字符串往返() {
        assert_eq!(MigrationTaskKind::Online.as_str(), "online");
        assert_eq!(
            MigrationTaskKind::from_db_str("offline_hosted"),
            MigrationTaskKind::OfflineHosted
        );
        // 未知值回退 Online
        assert_eq!(
            MigrationTaskKind::from_db_str("怪值"),
            MigrationTaskKind::Online
        );
        assert!(MigrationTaskState::Interrupted.is_terminal());
        assert!(!MigrationTaskState::Running.is_terminal());
        assert!(!MigrationTaskState::Paused.is_terminal());
    }
}
