//! 迁移任务节流落库协调（FR-140，ADR-0040）。
//!
//! 后台迁移搬运任务在 asset 边界回调本协调器，把进程内 `OnlinePullProgress` 快照按**节流策略**
//! 落到 `meta` 层的 `migration_tasks` 表；状态变更（暂停 / 继续）即时落；终态在单事务内写完整
//! 报告 + 分仓库明细。运行态真源仍是进程内 `MigrationJobs`，本表是历史与恢复视图。
//!
//! 分层：`api` → `meta`，持久化经 meta 层唯一入口，不绕过直连 DB。落库前先在进度锁外 clone 出
//! 快照，再异步写库（锁外做 IO）。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use crate::meta::{
    MetaStore, MigrationProgressSnapshot, MigrationRepoReport, MigrationTaskKind,
    MigrationTaskState, NewMigrationTask,
};
use crate::migrate::{OnlinePullPhase, OnlinePullProgress};

use super::TaskState;

/// 运行中进度的最小落库间隔（毫秒）：距上次落库不足此值的进度更新被节流跳过，避免逐 asset 写放大。
const MIN_PERSIST_INTERVAL_MS: i64 = 2_000;

/// 单个迁移任务的节流落库协调器：绑定一个 `task_id`，随后台搬运任务共享。
///
/// 起任务时落首行；运行中经 [`Self::maybe_persist_progress`] 节流落进度；暂停 / 继续经
/// [`Self::persist_state`] 即时落；结束经 [`Self::finish`] 写终态 + 报告。
pub struct MigrationPersister {
    meta: MetaStore,
    task_id: String,
    /// 上次落库时刻（Unix 毫秒）。`AtomicI64` 供 `&self` 并发下无锁更新。
    last_persist_ms: AtomicI64,
}

/// 取当前 Unix 毫秒（系统时钟异常时回 0，不 panic）。
fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 把引擎阶段枚举转为入库字符串（与 serde snake_case 一致）。
fn phase_str(phase: OnlinePullPhase) -> &'static str {
    match phase {
        OnlinePullPhase::Enumerating => "enumerating",
        OnlinePullPhase::Downloading => "downloading",
        OnlinePullPhase::Paused => "paused",
        OnlinePullPhase::Cancelled => "cancelled",
        OnlinePullPhase::Done => "done",
        OnlinePullPhase::Failed => "failed",
    }
}

/// 把进程内进度快照映射为可持久的进度计数（丢弃当前 asset 路径等瞬时字段）。
fn to_progress_snapshot(p: &OnlinePullProgress) -> MigrationProgressSnapshot {
    MigrationProgressSnapshot {
        phase: phase_str(p.phase).to_string(),
        total_assets: p.total_assets as i64,
        done_assets: p.done_assets as i64,
        migrated: p.migrated as i64,
        skipped_existing: p.skipped_existing as i64,
        skipped: p.skipped as i64,
        total_bytes: p.total_bytes as i64,
        done_bytes: p.done_bytes as i64,
        migrated_bytes: p.migrated_bytes as i64,
        skipped_bytes: p.skipped_bytes as i64,
    }
}

/// 把进度中的分仓库明细映射为可持久的报告项列表。
fn to_repo_reports(p: &OnlinePullProgress) -> Vec<MigrationRepoReport> {
    p.repos
        .iter()
        .map(|r| MigrationRepoReport {
            source_repo: r.source_repo.clone(),
            target_repo: r.target_repo.clone(),
            format: r.format.clone(),
            created: r.created,
            migrated_artifacts: r.migrated_artifacts as i64,
            skipped_existing_artifacts: r.skipped_existing_artifacts as i64,
            skipped_artifacts: r.skipped_artifacts as i64,
        })
        .collect()
}

/// 把统一注册表的终态映射为持久层迁移状态。非终态（running/paused）不由此路径落库。
fn terminal_state(state: TaskState) -> Option<MigrationTaskState> {
    match state {
        TaskState::Succeeded => Some(MigrationTaskState::Succeeded),
        TaskState::Failed => Some(MigrationTaskState::Failed),
        TaskState::Cancelled => Some(MigrationTaskState::Cancelled),
        TaskState::Running | TaskState::Paused => None,
    }
}

impl MigrationPersister {
    /// 起一个迁移任务：即时落首行（state = running），返回协调器供后台任务复用。
    ///
    /// `params_snapshot` 须为已脱敏 JSON（仅 auth_ref 引用与非敏感配置，不含凭据真值），由调用方
    /// 构造。落首行失败只记 WARN、不阻断迁移（持久化失败不影响业务，历史缺一条可接受）。
    pub async fn begin(
        meta: MetaStore,
        task_id: String,
        kind: MigrationTaskKind,
        label: Option<String>,
        params_snapshot: String,
    ) -> Self {
        let persister = Self {
            meta,
            task_id,
            last_persist_ms: AtomicI64::new(now_unix_ms()),
        };
        if let Err(e) = persister
            .meta
            .upsert_migration_task(&NewMigrationTask {
                id: persister.task_id.clone(),
                kind,
                label,
                params_snapshot,
            })
            .await
        {
            tracing::warn!(任务 = %persister.task_id, 原因 = %e, "迁移任务起始落库失败，历史将缺此记录");
        }
        persister
    }

    /// 节流落进度：距上次落库超过 [`MIN_PERSIST_INTERVAL_MS`] 才写，否则跳过（避免写放大）。
    ///
    /// 由后台搬运任务在 asset 边界调用；入参为进度锁外 clone 出的快照（不持进度锁做 IO）。
    /// 落库失败只记 DEBUG（下次节流点会带上更新的累计值补上），不影响业务。
    pub async fn maybe_persist_progress(&self, snapshot: &OnlinePullProgress) {
        let now = now_unix_ms();
        let last = self.last_persist_ms.load(Ordering::Relaxed);
        if now - last < MIN_PERSIST_INTERVAL_MS {
            return;
        }
        // 抢占落库权：CAS 成功者落库，避免并发重复写（本任务后台单线程，防御性处理）
        if self
            .last_persist_ms
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let progress = to_progress_snapshot(snapshot);
        if let Err(e) = self
            .meta
            .update_migration_task_progress(&self.task_id, &progress)
            .await
        {
            tracing::debug!(任务 = %self.task_id, 原因 = %e, "迁移进度节流落库失败，下次节流点补上");
        }
    }

    /// 终态落库：据统一注册表终态 + 最终进度快照，在单事务内写终态 + 完整报告 + 分仓库明细。
    ///
    /// `final_state` 为非终态时不落库（防御，正常后台任务结束时已是终态之一）。落库失败只记 WARN。
    pub async fn finish(&self, final_state: TaskState, snapshot: &OnlinePullProgress) {
        let Some(state) = terminal_state(final_state) else {
            return;
        };
        let progress = to_progress_snapshot(snapshot);
        let repos = to_repo_reports(snapshot);
        let error = snapshot.error.as_deref();
        if let Err(e) = self
            .meta
            .finish_migration_task(&self.task_id, state, &progress, &repos, error)
            .await
        {
            tracing::warn!(任务 = %self.task_id, 原因 = %e, "迁移任务终态落库失败，历史报告将缺失");
        }
    }

    /// 起后台节流落库 ticker：每隔节流间隔读一次共享进度快照（锁外落库），落进度到库。
    ///
    /// 与迁移搬运任务并发跑；搬运结束后调用方 `abort()` 停之，再由 [`Self::finish`] 写终态。
    /// ticker 只落进度、不落终态，避免与终态写竞态。读进度时锁内只 clone 立即放锁（锁外做 IO）。
    pub fn spawn_progress_ticker(
        self: Arc<Self>,
        progress: Arc<Mutex<OnlinePullProgress>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let interval = std::time::Duration::from_millis(MIN_PERSIST_INTERVAL_MS as u64);
            loop {
                tokio::time::sleep(interval).await;
                // 锁内只 clone 快照立即放锁，落库在锁外
                let snapshot = {
                    let g = progress.lock().unwrap_or_else(|e| e.into_inner());
                    g.clone()
                };
                self.maybe_persist_progress(&snapshot).await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::OnlineRepoMigrationOutcome;

    fn 进度(phase: OnlinePullPhase) -> OnlinePullProgress {
        OnlinePullProgress {
            phase,
            total_assets: 10,
            done_assets: 6,
            migrated: 4,
            skipped_existing: 2,
            skipped: 0,
            total_bytes: 1000,
            done_bytes: 600,
            migrated_bytes: 400,
            skipped_bytes: 200,
            repos: vec![OnlineRepoMigrationOutcome {
                source_repo: "r1".to_string(),
                target_repo: "r1".to_string(),
                format: "maven".to_string(),
                created: true,
                migrated_artifacts: 4,
                skipped_existing_artifacts: 2,
                skipped_artifacts: 0,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn 阶段字符串与_serde_一致() {
        // 与 OnlinePullPhase 的 serde snake_case 对齐，前端可复用同一取值
        assert_eq!(phase_str(OnlinePullPhase::Enumerating), "enumerating");
        assert_eq!(phase_str(OnlinePullPhase::Downloading), "downloading");
        assert_eq!(phase_str(OnlinePullPhase::Done), "done");
        assert_eq!(phase_str(OnlinePullPhase::Failed), "failed");
        assert_eq!(phase_str(OnlinePullPhase::Cancelled), "cancelled");
        assert_eq!(phase_str(OnlinePullPhase::Paused), "paused");
    }

    #[test]
    fn 进度快照映射保留聚合计数() {
        let snap = to_progress_snapshot(&进度(OnlinePullPhase::Downloading));
        assert_eq!(snap.phase, "downloading");
        assert_eq!(snap.total_assets, 10);
        assert_eq!(snap.migrated, 4);
        assert_eq!(snap.skipped_existing, 2);
        assert_eq!(snap.migrated_bytes, 400);
    }

    #[test]
    fn 分仓库报告映射() {
        let reports = to_repo_reports(&进度(OnlinePullPhase::Done));
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].source_repo, "r1");
        assert!(reports[0].created);
        assert_eq!(reports[0].migrated_artifacts, 4);
    }

    #[test]
    fn 终态映射区分终态与非终态() {
        assert_eq!(
            terminal_state(TaskState::Succeeded),
            Some(MigrationTaskState::Succeeded)
        );
        assert_eq!(
            terminal_state(TaskState::Cancelled),
            Some(MigrationTaskState::Cancelled)
        );
        assert_eq!(
            terminal_state(TaskState::Failed),
            Some(MigrationTaskState::Failed)
        );
        // 运行 / 暂停不由终态路径落库
        assert_eq!(terminal_state(TaskState::Running), None);
        assert_eq!(terminal_state(TaskState::Paused), None);
    }

    #[tokio::test]
    async fn 起任务落首行且节流生效() {
        let meta = MetaStore::open_in_memory().await.unwrap();
        let persister = MigrationPersister::begin(
            meta.clone(),
            "t1".to_string(),
            MigrationTaskKind::Online,
            Some("在线拉取迁移".to_string()),
            r#"{"auth_ref":"prod"}"#.to_string(),
        )
        .await;

        // 首行已落，state = running
        let got = meta.get_migration_task("t1").await.unwrap().unwrap();
        assert_eq!(got.state, "running");

        // 紧接着的进度更新距首行落库不足节流间隔 → 被跳过（phase 仍为初始 enumerating）
        persister
            .maybe_persist_progress(&进度(OnlinePullPhase::Downloading))
            .await;
        let got = meta.get_migration_task("t1").await.unwrap().unwrap();
        assert_eq!(got.phase, "enumerating", "节流内进度更新应被跳过");

        // 把上次落库时刻回拨超过节流间隔 → 下次进度更新落库
        persister.last_persist_ms.store(
            now_unix_ms() - MIN_PERSIST_INTERVAL_MS - 1,
            Ordering::Relaxed,
        );
        persister
            .maybe_persist_progress(&进度(OnlinePullPhase::Downloading))
            .await;
        let got = meta.get_migration_task("t1").await.unwrap().unwrap();
        assert_eq!(got.phase, "downloading", "超节流间隔应落库");
        assert_eq!(got.migrated, 4);
    }

    #[tokio::test]
    async fn 终态落完整报告() {
        let meta = MetaStore::open_in_memory().await.unwrap();
        let persister = MigrationPersister::begin(
            meta.clone(),
            "t1".to_string(),
            MigrationTaskKind::Online,
            None,
            "{}".to_string(),
        )
        .await;

        persister
            .finish(TaskState::Succeeded, &进度(OnlinePullPhase::Done))
            .await;
        let got = meta.get_migration_task("t1").await.unwrap().unwrap();
        assert_eq!(got.state, "succeeded");
        assert!(got.finished_at.is_some());
        let repos = meta.get_migration_task_repos("t1").await.unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].migrated_artifacts, 4);
    }
}
