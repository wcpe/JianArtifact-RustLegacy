//! Nexus **在线拉取**制品迁移（FR-82，ADR-0006 在线入口「读取并搬运」的补齐实现）。
//!
//! 源 Nexus 在线时，经其 REST `components` API 枚举某 hosted 仓库的全部 asset，按各 asset 的
//! `downloadUrl` HTTP 流式下载，经既有 [`ArtifactService::ingest_hosted`] 落为本系统 hosted
//! 制品——**无需离线 blob store 目录**。落定后比对 sha256 保证文件字节一致；单 asset 失败记
//! WARN 跳过、不中断整批。
//!
//! 范围（scope-discipline）：**Maven（`maven2`）/ Raw / npm 的 hosted** 仓库（FR-82 + FR-143）；
//! 其余格式（含 Docker/OCI，走 FR-144）/ 类型整体跳过。proxy 在线只迁配置不搬缓存（可回源重建）。
//! 与离线目录搬运（FR-39，见 [`super::hosted`]）互补：本路径制品本体来自 HTTP，而非磁盘 blob。
//!
//! 关键约束：凭据不入库、不进日志；锁外做 IO；下载流式经 `ingest_hosted`，不整体载入内存。

use crate::format::{
    ArtifactCoordinates, ArtifactService, DockerRegistry, Format, FormatRegistry, ServiceError,
};
use crate::meta::{MetaStore, RepositoryRecord};
use crate::proxy::Upstream;
use crate::storage::BlobStore;

use super::hosted::ensure_hosted_repo;
use super::online_docker::migrate_online_docker_repo;
use super::{
    map_nexus_format, parse_components, saturating_millis, DockerRegistryClient, MigrateError,
    NexusAsset, NexusClient, NexusCredential, NexusRepoSummary,
};

/// 单个 asset 下载 / 写入的最大尝试次数（含首次）：网络中断 / 流式解码失败等瞬时错误自动重试。
const MAX_ASSET_ATTEMPTS: u32 = 3;
/// 每个任务保留的最早失败资产明细上限。
const MAX_ASSET_FAILURES: usize = 1_000;
/// 失败资产标识最大 Unicode 字符数。
const MAX_FAILURE_ASSET_CHARS: usize = 1_024;
/// 稳定失败原因最大 Unicode 字符数。
const MAX_FAILURE_REASON_CHARS: usize = 512;

/// 在线拉取当前支持搬运的 hosted 格式集合（FR-82 Maven + FR-143 Raw / npm + FR-144 Docker）。
///
/// 集合即白名单，避免用 if-else 按格式名堆逻辑。`docker` 走 registry v2 manifest + layer 图搬运
/// （[`super::online_docker`]，需装配 [`DockerRegistry`]），其余格式走普通文件路径。
const SUPPORTED_ONLINE_FORMATS: &[&str] = &["maven", "raw", "npm", "docker"];

/// 把 Nexus 原样格式名映射为在线拉取当前支持搬运的本系统格式名；不支持则返回 None。
///
/// 复用 [`map_nexus_format`] 做统一格式映射，再据 [`SUPPORTED_ONLINE_FORMATS`] 白名单过滤——
/// 仅放行在线搬运已实现的格式（Maven / Raw / npm），其余（含尚未支持的 Docker/OCI）整体跳过。
fn online_hosted_format(nexus_format: &str) -> Option<&'static str> {
    let mapped = map_nexus_format(nexus_format)?;
    SUPPORTED_ONLINE_FORMATS
        .iter()
        .find(|f| **f == mapped)
        .copied()
}

/// 在线拉取的网络错误是否为可重试的瞬时错误（传输失败 / 源系统 5xx；4xx 等确定性错误不重试）。
fn is_transient_migrate(e: &MigrateError) -> bool {
    match e {
        MigrateError::Transport(_) => true,
        MigrateError::Status(s) => *s >= 500,
        _ => false,
    }
}

/// 写入错误是否为可重试的瞬时错误（流式下载中断在写入读流时浮现为存储 IO 失败）。
fn is_transient_service(e: &ServiceError) -> bool {
    matches!(e, ServiceError::Storage(_))
}

/// 重试前的指数退避等待 + 中文 WARN 日志（不打印凭据 / URL 明文）。
async fn retry_backoff(repo: &RepositoryRecord, path: &str, attempt: u32, stage: &str) {
    let backoff = std::time::Duration::from_millis(200u64 * 2u64.pow(attempt - 1));
    tracing::warn!(仓库 = %repo.name, 路径 = %path, 第 = attempt, 阶段 = stage, "asset 瞬时失败，退避后重试");
    tokio::time::sleep(backoff).await;
}

/// 在线拉取的单个仓库选择项：源仓库摘要 + 目标仓库名（可自定义，默认与源同名）。
#[derive(Debug, Clone)]
pub struct OnlinePullSelection {
    /// 源 Nexus 仓库摘要（来自 discover）。
    pub source: NexusRepoSummary,
    /// 本系统目标仓库名（允许与源不同名；默认应由上层填为源名）。
    pub target_repo: String,
}

/// 单个失败资产的安全明细。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MigrationAssetFailure {
    /// 源仓库名。
    pub source_repo: String,
    /// 资产路径或镜像 tag 标识。
    pub asset: String,
    /// 稳定、脱敏的中文失败分类。
    pub reason: String,
}

/// 单个仓库的在线拉取结果明细。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct OnlineRepoMigrationOutcome {
    /// 源仓库名。
    pub source_repo: String,
    /// 本系统目标仓库名。
    pub target_repo: String,
    /// 本系统格式名（映射后的白名单格式：`maven` / `raw` / `npm`）。
    pub format: String,
    /// 目标仓库是否新建（false 表示同名仓库已存在、复用）。
    pub created: bool,
    /// 成功新写入的 asset 数（首次搬运或内容变化后覆盖写入）。
    pub migrated_artifacts: usize,
    /// 增量跳过数（FR-134）：目标已存在且 sha256 一致，本次幂等重入跳过落盘。
    pub skipped_existing_artifacts: usize,
    /// 失败跳过数（路径非法、下载失败、sha256 不符、不可覆盖、写入失败等，均不中断整批）。
    pub skipped_artifacts: usize,
    /// 源枚举可知的逻辑总字节数。
    pub total_bytes: u64,
    /// 已处理完成的逻辑字节数。
    pub done_bytes: u64,
    /// 本次新写入目标存储的落定字节数。
    pub migrated_bytes: u64,
    /// 幂等命中或失败跳过的逻辑字节数。
    pub skipped_bytes: u64,
    /// 本仓库处理耗时。
    pub duration_ms: u64,
}

/// 整批在线拉取报告。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct OnlineMigrationReport {
    /// 各仓库的在线拉取结果明细。
    pub repos: Vec<OnlineRepoMigrationOutcome>,
    /// 因非 hosted / 格式不在在线搬运白名单（范围外）而整体跳过的源仓库名列表。
    pub skipped_repos: Vec<String>,
}

/// 在线拉取任务的阶段（FR-83 / FR-91）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnlinePullPhase {
    /// 正在枚举某仓库的 asset 列表。
    #[default]
    Enumerating,
    /// 正在下载 / 落地 asset。
    Downloading,
    /// 已被运维暂停，后台循环挂起等待继续（FR-91）。
    Paused,
    /// 已被运维取消，后台循环在 asset 边界停止后续搬运（不算失败，FR-91）。
    Cancelled,
    /// 任务成功完成。
    Done,
    /// 任务失败（枚举 / 鉴权 / 网络等致命错误）。
    Failed,
}

/// 在线拉取任务的进程内控制信号（FR-91）：随任务与控制端点共享。
///
/// 用 std `AtomicBool` + 既有 `tokio::sync::Notify` 实现协作式取消与暂停 / 继续，不引入新依赖：
/// 后台循环在每个 asset 边界轮询标志，暂停时 `await` 在 `notify` 上挂起，继续 / 取消时唤醒。
#[derive(Debug, Default)]
pub struct JobControl {
    /// 取消请求标志：置真后后台循环在下一 asset 边界收尾退出（标 `Cancelled`）。
    cancel: std::sync::atomic::AtomicBool,
    /// 暂停请求标志：置真后后台循环在下一 asset 边界挂起，直至继续 / 取消。
    paused: std::sync::atomic::AtomicBool,
    /// 唤醒挂起的后台循环（继续 / 取消时触发）。
    notify: tokio::sync::Notify,
}

impl JobControl {
    /// 请求取消任务：置取消标志并唤醒可能挂起的后台循环。对已结束任务为幂等空操作
    /// （后台循环已退出，标志置真无副作用）。
    pub fn request_cancel(&self) {
        self.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        // 同时清暂停标志，确保被唤醒后不再回到暂停等待
        self.paused
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// 请求暂停任务：置暂停标志。后台循环在下一 asset 边界自行挂起。已取消则不覆盖。
    pub fn request_pause(&self) {
        if self.cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        self.paused.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// 请求继续任务：清暂停标志并唤醒挂起的后台循环。
    pub fn request_resume(&self) {
        self.paused
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// 是否已请求取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 是否已请求暂停。
    pub fn is_paused(&self) -> bool {
        self.paused.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// 在线拉取任务的进度快照（FR-83）：任务执行期间持续更新，`GET jobs/{id}` 直接序列化之。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct OnlinePullProgress {
    /// 当前阶段。
    pub phase: OnlinePullPhase,
    /// 已枚举到的待搬运 asset 总数（各仓库枚举完成后累加）。
    pub total_assets: usize,
    /// 已处理 asset 数（migrated + skipped_existing + skipped）。
    pub done_assets: usize,
    /// 待搬运 asset 的逻辑总字节数；未知大小的条目按 0 计入。
    pub total_bytes: u64,
    /// 已处理 asset 的逻辑字节数（migrated_bytes + skipped_bytes）。
    pub done_bytes: u64,
    /// 成功新写入的逻辑字节数。
    pub migrated_bytes: u64,
    /// 跳过或失败的逻辑字节数（含增量跳过与失败跳过）。
    pub skipped_bytes: u64,
    /// 成功新写入数（首次搬运或内容变化后覆盖写入）。
    pub migrated: usize,
    /// 增量跳过数（FR-134）：目标已存在且 sha256 一致，本次幂等重入跳过落盘。
    pub skipped_existing: usize,
    /// 失败跳过数（路径非法 / 读本体失败 / 写入失败 / 不可覆盖等）。
    pub skipped: usize,
    /// 当前正在处理的源仓库名。
    pub current_repo: Option<String>,
    /// 当前正在处理的 asset 路径。
    pub current_path: Option<String>,
    /// 是否处于暂停态（FR-91）：暂停期间为真，继续后置假。
    pub paused: bool,
    /// 各仓库完成结果明细。
    pub repos: Vec<OnlineRepoMigrationOutcome>,
    /// 因非 hosted / 格式不在在线搬运白名单整体跳过的源仓库名。
    pub skipped_repos: Vec<String>,
    /// 最早发生的有界失败资产明细。
    pub asset_failures: Vec<MigrationAssetFailure>,
    /// 超出失败明细容量而省略的条数。
    pub omitted_asset_failures: usize,
    /// 失败原因（`phase == failed` 时）。
    pub error: Option<String>,
    /// 离线 blob store 预览枚举结果（FR-124）：仅离线预览任务在 `phase == done` 时填充；
    /// 在线拉取等其余任务为 `None`、不序列化（不污染在线进度结构）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offline_preview: Option<Vec<super::OfflineRepoSummary>>,
    /// 离线仓库索引双通道结果（FR-141）：仅离线索引任务在 `phase == done` 时填充；
    /// 其余任务为 `None`、不序列化（不污染其他进度结构）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_index: Option<Vec<super::RepoIndexEntry>>,
}

/// 取进度锁（容忍中毒：恢复内部数据继续，不让一次 panic 永久毒死进度查询）。
fn lock_progress(
    p: &std::sync::Mutex<OnlinePullProgress>,
) -> std::sync::MutexGuard<'_, OnlinePullProgress> {
    p.lock().unwrap_or_else(|e| e.into_inner())
}

/// 按 Unicode 字符边界安全截断文本。
fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

/// 记录一条有界失败明细；锁内只更新内存，不执行 IO。
pub(crate) fn record_asset_failure(
    progress: &std::sync::Mutex<OnlinePullProgress>,
    source_repo: &str,
    asset: &str,
    reason: &str,
) {
    let mut p = lock_progress(progress);
    if p.asset_failures.len() >= MAX_ASSET_FAILURES {
        p.omitted_asset_failures = p.omitted_asset_failures.saturating_add(1);
        return;
    }
    p.asset_failures.push(MigrationAssetFailure {
        source_repo: source_repo.to_string(),
        asset: truncate_chars(asset, MAX_FAILURE_ASSET_CHARS),
        reason: truncate_chars(reason, MAX_FAILURE_REASON_CHARS),
    });
}

/// 仓库开始处理时的任务累计计数快照。
#[derive(Clone, Copy)]
pub(crate) struct RepoProgressSnapshot {
    migrated: usize,
    skipped_existing: usize,
    skipped: usize,
    total_bytes: u64,
    done_bytes: u64,
    migrated_bytes: u64,
    skipped_bytes: u64,
}

pub(crate) fn repo_progress_snapshot(
    progress: &std::sync::Mutex<OnlinePullProgress>,
) -> RepoProgressSnapshot {
    let p = lock_progress(progress);
    RepoProgressSnapshot {
        migrated: p.migrated,
        skipped_existing: p.skipped_existing,
        skipped: p.skipped,
        total_bytes: p.total_bytes,
        done_bytes: p.done_bytes,
        migrated_bytes: p.migrated_bytes,
        skipped_bytes: p.skipped_bytes,
    }
}

/// 仓库处理发生致命错误时，先保留耗时与截至失败点的仓库增量计数。
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_fatal_repo_outcome(
    progress: &std::sync::Mutex<OnlinePullProgress>,
    source_repo: &str,
    target_repo: &str,
    format: &str,
    created: bool,
    known_total_bytes: u64,
    started: std::time::Instant,
    before: RepoProgressSnapshot,
) {
    let after = repo_progress_snapshot(progress);
    lock_progress(progress)
        .repos
        .push(OnlineRepoMigrationOutcome {
            source_repo: source_repo.to_string(),
            target_repo: target_repo.to_string(),
            format: format.to_string(),
            created,
            migrated_artifacts: after.migrated.saturating_sub(before.migrated),
            skipped_existing_artifacts: after
                .skipped_existing
                .saturating_sub(before.skipped_existing),
            skipped_artifacts: after.skipped.saturating_sub(before.skipped),
            total_bytes: after
                .total_bytes
                .saturating_sub(before.total_bytes)
                .max(known_total_bytes),
            done_bytes: after.done_bytes.saturating_sub(before.done_bytes),
            migrated_bytes: after.migrated_bytes.saturating_sub(before.migrated_bytes),
            skipped_bytes: after.skipped_bytes.saturating_sub(before.skipped_bytes),
            duration_ms: saturating_millis(started.elapsed().as_millis()),
        });
}

/// （无进度上报的便捷入口）执行在线拉取迁移——内部用一次性进度委托
/// [`migrate_online_with_progress`]；同步调用 / 测试用。仅 hosted 且格式在白名单
/// （`maven` / `raw` / `npm` / `docker`）内的仓库参与。
///
/// 便捷入口不装配 docker registry（`docker = None`）：docker 仓库整体跳过、计入 `skipped_repos`。
/// 需搬运 docker 的调用点走 [`migrate_online_with_progress`] 并传入 [`DockerRegistry`]。
#[allow(clippy::too_many_arguments)]
pub async fn migrate_online_repositories<C, S, U>(
    client: &C,
    meta: &MetaStore,
    artifacts: &ArtifactService<S, U>,
    formats: &FormatRegistry,
    base_url: &str,
    credential: Option<&NexusCredential>,
    selections: &[OnlinePullSelection],
    max_size: Option<u64>,
) -> Result<OnlineMigrationReport, MigrateError>
where
    C: NexusClient + DockerRegistryClient,
    S: BlobStore,
    U: Upstream,
{
    let progress = std::sync::Mutex::new(OnlinePullProgress::default());
    // 便捷入口无控制需求：用一次性默认控制（从不取消 / 暂停）委托
    let control = JobControl::default();
    // 便捷入口不装配 docker registry：docker 仓库整体跳过
    let docker: Option<&DockerRegistry<S>> = None;
    migrate_online_with_progress(
        client, meta, artifacts, formats, docker, base_url, credential, selections, max_size,
        &progress, &control,
    )
    .await
}

/// 执行 Nexus 在线拉取迁移并持续上报进度（FR-83，异步任务用）。
///
/// 形参同上，另加 `docker` 可选 docker registry（装配时 docker hosted 仓库走 registry v2 manifest +
/// layer 图搬运 FR-144，`None` 时 docker 仓库整体跳过）与 `progress` 进度共享态。仅 hosted 且格式在
/// 白名单（`maven` / `raw` / `npm` / `docker`）内参与，其余计入 `skipped_repos`。进度锁临界区只更新
/// 内存态、不持锁做 IO（锁外做 IO）。
#[allow(clippy::too_many_arguments)]
pub async fn migrate_online_with_progress<C, S, U>(
    client: &C,
    meta: &MetaStore,
    artifacts: &ArtifactService<S, U>,
    formats: &FormatRegistry,
    docker: Option<&DockerRegistry<S>>,
    base_url: &str,
    credential: Option<&NexusCredential>,
    selections: &[OnlinePullSelection],
    max_size: Option<u64>,
    progress: &std::sync::Mutex<OnlinePullProgress>,
    control: &JobControl,
) -> Result<OnlineMigrationReport, MigrateError>
where
    C: NexusClient + DockerRegistryClient,
    S: BlobStore,
    U: Upstream,
{
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err(MigrateError::Invalid(
            "源系统 base URL 不能为空".to_string(),
        ));
    }

    let mut report = OnlineMigrationReport::default();
    for sel in selections {
        // 取消优先：已请求取消则不再开始新仓库（FR-91）
        if control.is_cancelled() {
            mark_cancelled(progress);
            return Ok(report);
        }

        let src = &sel.source;
        // 范围：仅 hosted 且格式在在线搬运白名单（Maven / Raw / npm / Docker）内；其余格式 / 类型整体
        // 跳过，不越界建仓（proxy 在线只迁配置不搬缓存）
        let Some(target_format) = (if src.r#type == "hosted" {
            online_hosted_format(&src.format)
        } else {
            None
        }) else {
            tracing::info!(
                仓库 = %src.name, 源格式 = %src.format, 类型 = %src.r#type,
                "非受支持 hosted 格式，跳过在线拉取迁移"
            );
            report.skipped_repos.push(src.name.clone());
            lock_progress(progress).skipped_repos.push(src.name.clone());
            continue;
        };

        // docker 需装配 registry v2 存储服务；未装配（如便捷入口 / 无 docker 后端）则整体跳过，
        // 计入 skipped_repos（不越界建 docker 仓库却无法落库）
        if target_format == "docker" && docker.is_none() {
            tracing::info!(
                仓库 = %src.name,
                "未装配 docker registry，跳过 docker 仓库在线拉取迁移"
            );
            report.skipped_repos.push(src.name.clone());
            lock_progress(progress).skipped_repos.push(src.name.clone());
            continue;
        }

        {
            let mut p = lock_progress(progress);
            p.phase = OnlinePullPhase::Enumerating;
            p.current_repo = Some(src.name.clone());
            p.current_path = None;
        }

        let repo_started = std::time::Instant::now();
        let repo_before = repo_progress_snapshot(progress);
        // 建 / 复用目标 hosted 仓库；致命错误也先留下本仓库耗时明细。
        let (repo, created) = match ensure_hosted_repo(meta, &sel.target_repo, target_format).await
        {
            Ok(result) => result,
            Err(error) => {
                record_fatal_repo_outcome(
                    progress,
                    &src.name,
                    &sel.target_repo,
                    target_format,
                    false,
                    0,
                    repo_started,
                    repo_before,
                );
                return Err(error);
            }
        };

        // 分派：docker 走 registry v2 manifest + layer 图搬运（FR-144），其余走普通文件路径。
        // docker 特有逻辑单独封装于 online_docker，不塞进普通文件路径。
        let pulled = if target_format == "docker" {
            let registry = docker.expect("docker registry 已在前置守卫确认装配");
            migrate_online_docker_repo(
                client, registry, base, &src.name, &repo, credential, progress, control,
            )
            .await
            .map(RepoPullResult::from)
        } else {
            pull_repo_assets(
                client, artifacts, formats, base, &src.name, &repo, credential, max_size, progress,
                control,
            )
            .await
        };
        let pulled = match pulled {
            Ok(result) => result,
            Err(error) => {
                record_fatal_repo_outcome(
                    progress,
                    &src.name,
                    &sel.target_repo,
                    target_format,
                    created,
                    0,
                    repo_started,
                    repo_before,
                );
                return Err(error);
            }
        };

        tracing::info!(
            源仓库 = %src.name, 目标仓库 = %sel.target_repo, 格式 = %target_format,
            新建 = created, 已搬运 = pulled.migrated, 增量跳过 = pulled.skipped_existing,
            失败跳过 = pulled.skipped, 已取消 = pulled.cancelled,
            "hosted 仓库在线拉取迁移结束"
        );
        let outcome = OnlineRepoMigrationOutcome {
            source_repo: src.name.clone(),
            target_repo: sel.target_repo.clone(),
            format: target_format.to_string(),
            created,
            migrated_artifacts: pulled.migrated,
            skipped_existing_artifacts: pulled.skipped_existing,
            skipped_artifacts: pulled.skipped,
            total_bytes: pulled.total_bytes,
            done_bytes: pulled.done_bytes,
            migrated_bytes: pulled.migrated_bytes,
            skipped_bytes: pulled.skipped_bytes,
            duration_ms: saturating_millis(repo_started.elapsed().as_millis()),
        };
        lock_progress(progress).repos.push(outcome.clone());
        report.repos.push(outcome);

        // 仓库内被取消：标记已取消并停止后续仓库（不算失败，已搬运保留）
        if pulled.cancelled {
            mark_cancelled(progress);
            return Ok(report);
        }
    }

    Ok(report)
}

/// 把进度标记为「已取消」终态（FR-91）：清当前项、置 `Cancelled` 阶段、清暂停标志。
///
/// `pub(crate)` 供离线搬运（FR-125）复用同一取消终态语义。
pub(crate) fn mark_cancelled(progress: &std::sync::Mutex<OnlinePullProgress>) {
    let mut p = lock_progress(progress);
    p.phase = OnlinePullPhase::Cancelled;
    p.paused = false;
    p.current_path = None;
    tracing::info!("在线拉取任务已按请求取消，停止后续搬运");
}

/// 单仓库在线拉取的结果：成功 / 跳过计数 + 是否在搬运中途被取消（FR-91）。
#[derive(Default)]
struct RepoPullResult {
    /// 成功新写入的 asset 数。
    migrated: usize,
    /// 目标已存在且 sha256 一致的增量跳过数。
    skipped_existing: usize,
    /// 跳过 / 失败的 asset 数。
    skipped: usize,
    /// 源枚举逻辑总字节数。
    total_bytes: u64,
    /// 已处理逻辑字节数。
    done_bytes: u64,
    /// 新写入落定字节数。
    migrated_bytes: u64,
    /// 幂等或失败跳过逻辑字节数。
    skipped_bytes: u64,
    /// 是否在 asset 边界被取消（取消则提前结束本仓库、未搬完）。
    cancelled: bool,
}

impl From<super::online_docker::DockerRepoPullResult> for RepoPullResult {
    fn from(result: super::online_docker::DockerRepoPullResult) -> Self {
        Self {
            migrated: result.migrated,
            skipped_existing: result.skipped_existing,
            skipped: result.skipped,
            total_bytes: result.total_bytes,
            done_bytes: result.done_bytes,
            migrated_bytes: result.migrated_bytes,
            skipped_bytes: result.skipped_bytes,
            cancelled: result.cancelled,
        }
    }
}

/// 单个在线 asset 的落定结果。
enum PullOneOutcome {
    /// 成功新写入及落定字节数。
    Migrated(u64),
    /// 目标已存在且 sha256 一致，增量跳过。
    SkippedExisting,
}

/// 枚举某源仓库的全部 components（已知总数）再逐 asset 下载搬运，边搬边上报进度。
///
/// 枚举失败向上冒泡（整仓无法继续）；单 asset 失败计跳过、不中断整批。每个 asset 处理前检查
/// 控制信号（FR-91）：取消则提前结束（`cancelled=true`）；暂停则挂起等待继续 / 取消。
#[allow(clippy::too_many_arguments)]
async fn pull_repo_assets<C, S, U>(
    client: &C,
    artifacts: &ArtifactService<S, U>,
    formats: &FormatRegistry,
    base_url: &str,
    source_repo: &str,
    target_repo: &RepositoryRecord,
    credential: Option<&NexusCredential>,
    max_size: Option<u64>,
    progress: &std::sync::Mutex<OnlinePullProgress>,
    control: &JobControl,
) -> Result<RepoPullResult, MigrateError>
where
    C: NexusClient,
    S: BlobStore,
    U: Upstream,
{
    let Some(format) = formats.get(&target_repo.format) else {
        tracing::warn!(仓库 = %target_repo.name, 格式 = %target_repo.format, "格式处理器未注册，跳过在线拉取");
        return Ok(RepoPullResult::default());
    };
    let assets = enumerate_assets(client, base_url, source_repo, credential).await?;
    let total_bytes = assets.iter().fold(0u64, |sum, asset| {
        sum.saturating_add(asset.size.unwrap_or(0))
    });
    {
        let mut p = lock_progress(progress);
        p.total_assets += assets.len();
        p.total_bytes = p.total_bytes.saturating_add(total_bytes);
        p.phase = OnlinePullPhase::Downloading;
    }

    let mut result = RepoPullResult {
        total_bytes,
        ..Default::default()
    };
    for asset in &assets {
        if await_control(control, progress).await {
            result.cancelled = true;
            return Ok(result);
        }
        lock_progress(progress).current_path = Some(asset.path.clone());
        let outcome = pull_one_asset(
            client,
            artifacts,
            format,
            target_repo,
            asset,
            credential,
            max_size,
        )
        .await;
        apply_asset_outcome(progress, source_repo, asset, outcome, &mut result);
    }
    Ok(result)
}

/// 分页枚举一个仓库的全部资产。
async fn enumerate_assets<C: NexusClient>(
    client: &C,
    base_url: &str,
    source_repo: &str,
    credential: Option<&NexusCredential>,
) -> Result<Vec<NexusAsset>, MigrateError> {
    let mut assets = Vec::new();
    let mut token = None;
    loop {
        let body = client
            .fetch_components(base_url, source_repo, token.as_deref(), credential)
            .await?;
        let page = parse_components(&body)?;
        assets.extend(page.assets);
        match page.continuation_token {
            Some(next) => token = Some(next),
            None => return Ok(assets),
        }
    }
}

/// 把单资产三态结果同时推进任务级与仓库级计数。
fn apply_asset_outcome(
    progress: &std::sync::Mutex<OnlinePullProgress>,
    source_repo: &str,
    asset: &NexusAsset,
    outcome: Result<PullOneOutcome, &'static str>,
    result: &mut RepoPullResult,
) {
    let logical = asset.size.unwrap_or(0);
    let mut failure = None;
    let mut p = lock_progress(progress);
    p.done_assets += 1;
    p.done_bytes = p.done_bytes.saturating_add(logical);
    result.done_bytes = result.done_bytes.saturating_add(logical);
    match outcome {
        Ok(PullOneOutcome::Migrated(written)) => {
            result.migrated += 1;
            result.migrated_bytes = result.migrated_bytes.saturating_add(written);
            p.migrated += 1;
            p.migrated_bytes = p.migrated_bytes.saturating_add(written);
        }
        Ok(PullOneOutcome::SkippedExisting) => {
            result.skipped_existing += 1;
            result.skipped_bytes = result.skipped_bytes.saturating_add(logical);
            p.skipped_existing += 1;
            p.skipped_bytes = p.skipped_bytes.saturating_add(logical);
        }
        Err(reason) => {
            result.skipped += 1;
            result.skipped_bytes = result.skipped_bytes.saturating_add(logical);
            p.skipped += 1;
            p.skipped_bytes = p.skipped_bytes.saturating_add(logical);
            failure = Some(reason);
        }
    }
    drop(p);
    if let Some(reason) = failure {
        record_asset_failure(progress, source_repo, &asset.path, reason);
    }
}

/// 在 asset 边界响应控制信号（FR-91）。返回 `true` 表示应取消（提前结束本仓库）。
///
/// 取消优先：已取消立即返回 `true`，不进入暂停等待。暂停时把进度标 `Paused`，`await` 在 `notify`
/// 上挂起，直至被继续 / 取消唤醒；醒来复核——取消则返回 `true`，继续则清暂停标志、恢复
/// `Downloading` 后返回 `false`。等待在进度锁外（不持锁阻塞）。
pub(crate) async fn await_control(
    control: &JobControl,
    progress: &std::sync::Mutex<OnlinePullProgress>,
) -> bool {
    if control.is_cancelled() {
        return true;
    }
    if !control.is_paused() {
        return false;
    }

    // 进入暂停态：标记进度后在锁外挂起等待唤醒
    {
        let mut p = lock_progress(progress);
        p.paused = true;
        p.phase = OnlinePullPhase::Paused;
    }
    loop {
        // 先创建并 enable() 等待者再复核标志：消除「复核后、await 前」继续 / 取消唤醒丢失的竞态
        // （`notify_waiters` 对未注册的等待者不留存通知，故须先注册）
        let notified = control.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        if control.is_cancelled() {
            return true;
        }
        if !control.is_paused() {
            // 已继续：恢复下载态并清暂停标志
            let mut p = lock_progress(progress);
            p.paused = false;
            p.phase = OnlinePullPhase::Downloading;
            return false;
        }
        // 仍处暂停：挂起直至下一次唤醒，醒来再复核
        notified.await;
    }
}

/// 拉取并落定单个 asset：解析路径 → 流式下载 → `ingest_hosted` → 比对 sha256（不符回滚）。
///
/// 任一步失败返回 `Err(())`（已记日志），由调用方计跳过、不中断整批。
async fn pull_one_asset<C, S, U>(
    client: &C,
    artifacts: &ArtifactService<S, U>,
    format: &dyn Format,
    repo: &RepositoryRecord,
    asset: &NexusAsset,
    credential: Option<&NexusCredential>,
    max_size: Option<u64>,
) -> Result<PullOneOutcome, &'static str>
where
    C: NexusClient,
    S: BlobStore,
    U: Upstream,
{
    // 归一化并校验路径：非法路径（穿越 / 空）跳过
    let coords: ArtifactCoordinates = match format.parse_path(&asset.path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(仓库 = %repo.name, 路径 = %asset.path, 错误 = %e, "asset 路径非法，跳过");
            return Err("制品路径非法");
        }
    };

    // 下载 + 流式写入；网络中断 / 流式解码失败等瞬时错误自动重试（指数退避），
    // 确定性失败（不可覆盖 / 超限 / 路径非法等）不重试。
    let mut attempt = 0u32;
    let outcome = loop {
        attempt += 1;

        // 流式下载（不整体载入内存）
        let reader = match client.download_asset(&asset.download_url, credential).await {
            Ok(r) => r,
            Err(e) => {
                if is_transient_migrate(&e) && attempt < MAX_ASSET_ATTEMPTS {
                    retry_backoff(repo, &asset.path, attempt, "下载").await;
                    continue;
                }
                tracing::warn!(仓库 = %repo.name, 路径 = %asset.path, 错误 = %e, "asset 下载失败，跳过");
                return Err("源资产下载失败");
            }
        };

        // 流式写入：下载流中断会在读流时浮现为存储 IO 失败
        match artifacts
            .ingest_hosted(repo, format, &coords, reader, max_size)
            .await
        {
            Ok(outcome) => break outcome,
            // 确定性策略错误不重试。
            Err(ServiceError::OverwriteForbidden) => {
                tracing::info!(仓库 = %repo.name, 路径 = %asset.path, "同坐标制品已存在且不可覆盖，跳过");
                return Err("目标制品不可覆盖");
            }
            Err(ServiceError::TooLarge) => {
                tracing::warn!(仓库 = %repo.name, 路径 = %asset.path, "制品超过大小限制，跳过");
                return Err("制品超过大小限制");
            }
            Err(e) => {
                if is_transient_service(&e) && attempt < MAX_ASSET_ATTEMPTS {
                    retry_backoff(repo, &asset.path, attempt, "写入").await;
                    continue;
                }
                tracing::warn!(仓库 = %repo.name, 路径 = %asset.path, 错误 = %e, "asset 写入失败，跳过");
                return Err("目标制品写入失败");
            }
        }
    };

    let written = outcome.is_written();
    let record = outcome.into_record();

    // 文件一致：落定 sha256 须与源报告一致，否则视为下载损坏，回滚该制品并跳过
    if let Some(expected) = &asset.sha256 {
        if !record.sha256.eq_ignore_ascii_case(expected) {
            tracing::warn!(仓库 = %repo.name, 路径 = %asset.path, "下载内容 sha256 与源报告不符，回滚该制品并跳过");
            let _ = artifacts.delete(repo, &coords).await;
            return Err("资产校验失败");
        }
    }

    if written {
        Ok(PullOneOutcome::Migrated(
            u64::try_from(record.size).unwrap_or(0),
        ))
    } else {
        Ok(PullOneOutcome::SkippedExisting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    use tokio::io::AsyncReadExt;

    use crate::format::{ArtifactCoordinates, MavenFormat, NpmFormat, RawFormat};
    use crate::proxy::{Upstream, UpstreamBody, UpstreamError};
    use crate::storage::LocalFsStore;

    /// 永不被触达的 mock 上游：在线拉取不走 proxy 回源。
    struct NeverUpstream;
    impl Upstream for NeverUpstream {
        async fn fetch(&self, _b: &str, _p: &str) -> Result<UpstreamBody, UpstreamError> {
            panic!("在线拉取不应触发上游回源");
        }
    }

    /// 为普通文件路径的 mock 客户端补一份「永不触达」的 [`DockerRegistryClient`] 实现——
    /// 这些用例只覆盖 maven/raw/npm 普通文件路径，不迁移 docker（docker 图搬运的正确性在
    /// [`super::super::online_docker`] 单独覆盖），故 docker 方法为 unreachable 占位。
    macro_rules! never_docker_client {
        ($t:ty) => {
            impl DockerRegistryClient for $t {
                async fn list_images(
                    &self,
                    _b: &str,
                    _r: &str,
                    _c: Option<&NexusCredential>,
                ) -> Result<Vec<String>, MigrateError> {
                    unimplemented!("普通文件路径用例不迁移 docker")
                }
                async fn list_tags(
                    &self,
                    _b: &str,
                    _r: &str,
                    _i: &str,
                    _c: Option<&NexusCredential>,
                ) -> Result<Vec<String>, MigrateError> {
                    unimplemented!("普通文件路径用例不迁移 docker")
                }
                async fn fetch_manifest(
                    &self,
                    _b: &str,
                    _r: &str,
                    _i: &str,
                    _rf: &str,
                    _c: Option<&NexusCredential>,
                ) -> Result<crate::migrate::FetchedManifest, MigrateError> {
                    unimplemented!("普通文件路径用例不迁移 docker")
                }
                async fn download_blob(
                    &self,
                    _b: &str,
                    _r: &str,
                    _i: &str,
                    _d: &str,
                    _c: Option<&NexusCredential>,
                ) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>, MigrateError> {
                    unimplemented!("普通文件路径用例不迁移 docker")
                }
            }
        };
    }

    /// mock Nexus 客户端：按 token 序返回预置 components 页，按 downloadUrl 返回预置字节。
    struct MockOnline {
        /// components 页（JSON 文本）按页序排列；token 即页下标字符串。
        pages: Vec<String>,
        /// downloadUrl → 字节；缺失即下载失败。
        assets: HashMap<String, Vec<u8>>,
    }

    impl NexusClient for MockOnline {
        async fn fetch_repositories(
            &self,
            _base_url: &str,
            _credential: Option<&NexusCredential>,
        ) -> Result<String, MigrateError> {
            unimplemented!("在线拉取用例不调用 fetch_repositories")
        }

        async fn fetch_components(
            &self,
            _base_url: &str,
            _repository: &str,
            continuation_token: Option<&str>,
            _credential: Option<&NexusCredential>,
        ) -> Result<String, MigrateError> {
            let idx: usize = continuation_token.and_then(|t| t.parse().ok()).unwrap_or(0);
            self.pages
                .get(idx)
                .cloned()
                .ok_or_else(|| MigrateError::Parse("页越界".to_string()))
        }

        async fn download_asset(
            &self,
            download_url: &str,
            _credential: Option<&NexusCredential>,
        ) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>, MigrateError> {
            match self.assets.get(download_url) {
                Some(bytes) => Ok(Box::new(std::io::Cursor::new(bytes.clone()))),
                None => Err(MigrateError::Transport(format!("404 {download_url}"))),
            }
        }
    }
    never_docker_client!(MockOnline);

    async fn 新建() -> (
        MetaStore,
        ArtifactService<LocalFsStore, NeverUpstream>,
        FormatRegistry,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let meta = MetaStore::open_in_memory().await.unwrap();
        let store = LocalFsStore::new(dir.path().join("blobs")).await.unwrap();
        let svc = ArtifactService::new(store, meta.clone(), NeverUpstream);
        let mut formats = FormatRegistry::new();
        formats.register(Box::new(RawFormat));
        formats.register(Box::new(MavenFormat));
        formats.register(Box::new(NpmFormat));
        (meta, svc, formats, dir)
    }

    fn src(name: &str, format: &str, r#type: &str) -> NexusRepoSummary {
        NexusRepoSummary {
            name: name.to_string(),
            format: format.to_string(),
            r#type: r#type.to_string(),
            upstream_url: None,
            group_members: vec![],
        }
    }

    fn sel(source: NexusRepoSummary, target: &str) -> OnlinePullSelection {
        OnlinePullSelection {
            source,
            target_repo: target.to_string(),
        }
    }

    /// 固定载荷与其已知 sha256（与 tests/upload_api.rs 同一对照值）。
    const FIXTURE: &[u8] = b"sidecar-maven-fixture-v1";
    const FIXTURE_SHA256: &str = "68e9e9702196267c9832e413cb66fe40836d9388c7b5296302b7e571f5e062c9";

    async fn read_artifact(
        svc: &Arc<ArtifactService<LocalFsStore, NeverUpstream>>,
        repo: &RepositoryRecord,
        path: &str,
    ) -> Option<Vec<u8>> {
        let coords = ArtifactCoordinates {
            path: path.to_string(),
        };
        let (mut h, _) = svc.get(repo, &MavenFormat, &coords).await.ok()?;
        let mut buf = Vec::new();
        h.blob.read_to_end(&mut buf).await.unwrap();
        Some(buf)
    }

    #[tokio::test]
    async fn 在线拉取建仓并按asset落制品字节一致且校验sha256() {
        let (meta, svc, formats, _d) = 新建().await;
        let url = "https://nx/repository/r3d/com/foo/lib/1.0/lib-1.0.jar";
        let page = format!(
            r#"{{ "items": [ {{ "assets": [
                {{ "path": "com/foo/lib/1.0/lib-1.0.jar", "downloadUrl": "{url}", "checksum": {{ "sha256": "{FIXTURE_SHA256}" }} }}
            ]}} ], "continuationToken": null }}"#
        );
        let client = MockOnline {
            pages: vec![page],
            assets: HashMap::from([(url.to_string(), FIXTURE.to_vec())]),
        };

        let report = migrate_online_repositories(
            &client,
            &meta,
            &svc,
            &formats,
            "https://nx/",
            None,
            &[sel(src("r3d", "maven2", "hosted"), "r3d")],
            None,
        )
        .await
        .unwrap();

        assert_eq!(report.repos.len(), 1);
        let o = &report.repos[0];
        assert!(o.created);
        assert_eq!(o.format, "maven");
        assert_eq!(o.migrated_artifacts, 1);
        assert_eq!(o.skipped_artifacts, 0);

        // 目标仓库为 hosted，制品字节一致
        let repo = meta.get_repository_by_name("r3d").await.unwrap().unwrap();
        assert_eq!(repo.r#type, "hosted");
        let svc = Arc::new(svc);
        let got = read_artifact(&svc, &repo, "com/foo/lib/1.0/lib-1.0.jar")
            .await
            .unwrap();
        assert_eq!(got, FIXTURE);
    }

    #[tokio::test]
    async fn 在线拉取二次同sha256计入增量跳过而非新搬() {
        let (meta, svc, formats, _d) = 新建().await;
        let url = "https://nx/repository/r3d/com/foo/lib/1.0/lib-1.0.jar";
        let page = format!(
            r#"{{ "items": [ {{ "assets": [
                {{ "path": "com/foo/lib/1.0/lib-1.0.jar", "downloadUrl": "{url}", "checksum": {{ "sha256": "{FIXTURE_SHA256}" }} }}
            ]}} ], "continuationToken": null }}"#
        );
        let client = MockOnline {
            pages: vec![page],
            assets: HashMap::from([(url.to_string(), FIXTURE.to_vec())]),
        };
        let selection = [sel(src("r3d", "maven2", "hosted"), "r3d")];

        let progress1 = std::sync::Mutex::new(OnlinePullProgress::default());
        let control1 = JobControl::default();
        let first = migrate_online_with_progress(
            &client,
            &meta,
            &svc,
            &formats,
            None,
            "https://nx",
            None,
            &selection,
            None,
            &progress1,
            &control1,
        )
        .await
        .unwrap();
        assert_eq!(first.repos[0].migrated_artifacts, 1);
        assert_eq!(first.repos[0].skipped_existing_artifacts, 0);

        let progress2 = std::sync::Mutex::new(OnlinePullProgress::default());
        let control2 = JobControl::default();
        let second = migrate_online_with_progress(
            &client,
            &meta,
            &svc,
            &formats,
            None,
            "https://nx",
            None,
            &selection,
            None,
            &progress2,
            &control2,
        )
        .await
        .unwrap();

        assert_eq!(second.repos[0].migrated_artifacts, 0, "二次跑无新写入");
        assert_eq!(
            second.repos[0].skipped_existing_artifacts, 1,
            "二次跑应计入增量跳过"
        );
        assert_eq!(second.repos[0].skipped_artifacts, 0);
        let p = progress2.lock().unwrap();
        assert_eq!(p.total_assets, 1);
        assert_eq!(p.done_assets, 1);
        assert_eq!(p.migrated, 0);
        assert_eq!(p.skipped_existing, 1);
        assert_eq!(p.skipped, 0);
        assert_eq!(p.repos[0].migrated_artifacts, 0);
        assert_eq!(p.repos[0].skipped_existing_artifacts, 1);
    }

    #[tokio::test]
    async fn sha256与源报告不符时回滚并跳过() {
        let (meta, svc, formats, _d) = 新建().await;
        let url = "https://nx/repository/r3d/a.jar";
        let page = format!(
            r#"{{ "items": [ {{ "assets": [
                {{ "path": "com/foo/a/1.0/a-1.0.jar", "downloadUrl": "{url}", "checksum": {{ "sha256": "00000000000000000000000000000000000000000000000000000000deadbeef" }} }}
            ]}} ], "continuationToken": null }}"#
        );
        let client = MockOnline {
            pages: vec![page],
            assets: HashMap::from([(url.to_string(), FIXTURE.to_vec())]),
        };

        let report = migrate_online_repositories(
            &client,
            &meta,
            &svc,
            &formats,
            "https://nx",
            None,
            &[sel(src("r3d", "maven2", "hosted"), "r3d")],
            None,
        )
        .await
        .unwrap();

        assert_eq!(report.repos[0].migrated_artifacts, 0);
        assert_eq!(report.repos[0].skipped_artifacts, 1);
        // 回滚后制品不可取回
        let repo = meta.get_repository_by_name("r3d").await.unwrap().unwrap();
        let svc = Arc::new(svc);
        assert!(read_artifact(&svc, &repo, "com/foo/a/1.0/a-1.0.jar")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn 目标仓库可改名落于新名仓库() {
        let (meta, svc, formats, _d) = 新建().await;
        let url = "https://nx/repository/r3d/x.jar";
        let page = format!(
            r#"{{ "items": [ {{ "assets": [ {{ "path": "x/1.0/x-1.0.jar", "downloadUrl": "{url}", "checksum": {{}} }} ]}} ], "continuationToken": null }}"#
        );
        let client = MockOnline {
            pages: vec![page],
            assets: HashMap::from([(url.to_string(), b"xx".to_vec())]),
        };
        let report = migrate_online_repositories(
            &client,
            &meta,
            &svc,
            &formats,
            "https://nx",
            None,
            &[sel(src("r3d", "maven2", "hosted"), "r3d-copy")],
            None,
        )
        .await
        .unwrap();

        assert_eq!(report.repos[0].source_repo, "r3d");
        assert_eq!(report.repos[0].target_repo, "r3d-copy");
        // 落于改名后的仓库；源名仓库未建
        assert!(meta
            .get_repository_by_name("r3d-copy")
            .await
            .unwrap()
            .is_some());
        assert!(meta.get_repository_by_name("r3d").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn proxy或未支持格式整体跳过不建仓() {
        let (meta, svc, formats, _d) = 新建().await;
        let client = MockOnline {
            pages: vec![],
            assets: HashMap::new(),
        };
        let report = migrate_online_repositories(
            &client,
            &meta,
            &svc,
            &formats,
            "https://nx",
            None,
            &[
                // maven proxy（非 hosted）：在线只迁配置不搬缓存，此路径整体跳过
                sel(src("maven-central", "maven2", "proxy"), "maven-central"),
                // cargo hosted（在线搬运未支持格式，FR-143 只扩 raw/npm）：整体跳过
                sel(src("cargo-hosted", "cargo", "hosted"), "cargo-hosted"),
            ],
            None,
        )
        .await
        .unwrap();

        assert!(report.repos.is_empty());
        assert!(report.skipped_repos.contains(&"maven-central".to_string()));
        assert!(report.skipped_repos.contains(&"cargo-hosted".to_string()));
        assert!(meta
            .get_repository_by_name("maven-central")
            .await
            .unwrap()
            .is_none());
        assert!(meta
            .get_repository_by_name("cargo-hosted")
            .await
            .unwrap()
            .is_none());
    }

    /// docker hosted 在未装配 docker registry（便捷入口 `docker = None`）时整体跳过、不建仓（FR-144）。
    #[tokio::test]
    async fn docker_未装配_registry_时整体跳过不建仓() {
        let (meta, svc, formats, _d) = 新建().await;
        let client = MockOnline {
            pages: vec![],
            assets: HashMap::new(),
        };
        // 便捷入口 migrate_online_repositories 不装配 docker registry
        let report = migrate_online_repositories(
            &client,
            &meta,
            &svc,
            &formats,
            "https://nx",
            None,
            &[sel(
                src("docker-hosted", "docker", "hosted"),
                "docker-hosted",
            )],
            None,
        )
        .await
        .unwrap();

        assert!(report.repos.is_empty());
        assert!(report.skipped_repos.contains(&"docker-hosted".to_string()));
        // 不越界建 docker 仓库（无法落库时不建仓）
        assert!(meta
            .get_repository_by_name("docker-hosted")
            .await
            .unwrap()
            .is_none());
    }

    /// raw hosted 在线拉取：枚举 → 流式下载 → sha256 校验 → 落为 raw hosted 制品，
    /// 结果 format 如实为 raw（不再恒为 maven）。
    #[tokio::test]
    async fn raw_hosted_在线拉取落制品并校验sha256() {
        let (meta, svc, formats, _d) = 新建().await;
        let url = "https://nx/repository/raw3d/dir/file.bin";
        let page = format!(
            r#"{{ "items": [ {{ "assets": [
                {{ "path": "dir/file.bin", "downloadUrl": "{url}", "checksum": {{ "sha256": "{FIXTURE_SHA256}" }} }}
            ]}} ], "continuationToken": null }}"#
        );
        let client = MockOnline {
            pages: vec![page],
            assets: HashMap::from([(url.to_string(), FIXTURE.to_vec())]),
        };

        let report = migrate_online_repositories(
            &client,
            &meta,
            &svc,
            &formats,
            "https://nx/",
            None,
            &[sel(src("raw3d", "raw", "hosted"), "raw3d")],
            None,
        )
        .await
        .unwrap();

        assert_eq!(report.repos.len(), 1);
        let o = &report.repos[0];
        assert!(o.created);
        assert_eq!(o.format, "raw", "结果格式应如实为 raw");
        assert_eq!(o.migrated_artifacts, 1);
        assert_eq!(o.skipped_artifacts, 0);

        // 目标仓库按 raw 格式建为 hosted，制品字节一致可读回
        let repo = meta.get_repository_by_name("raw3d").await.unwrap().unwrap();
        assert_eq!(repo.r#type, "hosted");
        assert_eq!(repo.format, "raw");
        let coords = ArtifactCoordinates {
            path: "dir/file.bin".to_string(),
        };
        let svc = Arc::new(svc);
        let (mut h, _) = svc.get(&repo, &RawFormat, &coords).await.unwrap();
        let mut buf = Vec::new();
        h.blob.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, FIXTURE);
    }

    /// npm hosted 在线拉取：tarball 与 packument 两类 asset 均按普通仓库内路径搬入，
    /// 结果 format 如实为 npm。
    #[tokio::test]
    async fn npm_hosted_在线拉取落tarball与packument() {
        let (meta, svc, formats, _d) = 新建().await;
        let tarball_url = "https://nx/repository/npm3d/lib/-/lib-1.0.0.tgz";
        let packument_url = "https://nx/repository/npm3d/lib";
        let page = format!(
            r#"{{ "items": [ {{ "assets": [
                {{ "path": "lib/-/lib-1.0.0.tgz", "downloadUrl": "{tarball_url}", "checksum": {{}} }},
                {{ "path": "lib", "downloadUrl": "{packument_url}", "checksum": {{}} }}
            ]}} ], "continuationToken": null }}"#
        );
        let client = MockOnline {
            pages: vec![page],
            assets: HashMap::from([
                (tarball_url.to_string(), b"tarball-bytes".to_vec()),
                (packument_url.to_string(), b"{\"name\":\"lib\"}".to_vec()),
            ]),
        };

        let report = migrate_online_repositories(
            &client,
            &meta,
            &svc,
            &formats,
            "https://nx",
            None,
            &[sel(src("npm3d", "npm", "hosted"), "npm3d")],
            None,
        )
        .await
        .unwrap();

        let o = &report.repos[0];
        assert_eq!(o.format, "npm", "结果格式应如实为 npm");
        assert_eq!(o.migrated_artifacts, 2, "tarball 与 packument 均应搬入");
        assert_eq!(o.skipped_artifacts, 0);

        let repo = meta.get_repository_by_name("npm3d").await.unwrap().unwrap();
        assert_eq!(repo.format, "npm");
        assert!(meta
            .list_artifacts_by_repo(&repo.id)
            .await
            .unwrap()
            .iter()
            .any(|a| a.path == "lib/-/lib-1.0.0.tgz"));
    }

    /// raw hosted 二次同 sha256：计增量跳过而非新搬（复用 FR-134 幂等引擎）。
    #[tokio::test]
    async fn raw_hosted_二次同sha256增量跳过() {
        let (meta, svc, formats, _d) = 新建().await;
        let url = "https://nx/repository/raw3d/a.bin";
        let page = format!(
            r#"{{ "items": [ {{ "assets": [
                {{ "path": "a.bin", "downloadUrl": "{url}", "checksum": {{ "sha256": "{FIXTURE_SHA256}" }} }}
            ]}} ], "continuationToken": null }}"#
        );
        let client = MockOnline {
            pages: vec![page],
            assets: HashMap::from([(url.to_string(), FIXTURE.to_vec())]),
        };
        let selection = [sel(src("raw3d", "raw", "hosted"), "raw3d")];

        let first = migrate_online_repositories(
            &client,
            &meta,
            &svc,
            &formats,
            "https://nx",
            None,
            &selection,
            None,
        )
        .await
        .unwrap();
        assert_eq!(first.repos[0].migrated_artifacts, 1);
        assert_eq!(first.repos[0].skipped_existing_artifacts, 0);

        let second = migrate_online_repositories(
            &client,
            &meta,
            &svc,
            &formats,
            "https://nx",
            None,
            &selection,
            None,
        )
        .await
        .unwrap();
        assert_eq!(second.repos[0].migrated_artifacts, 0, "二次跑无新写入");
        assert_eq!(
            second.repos[0].skipped_existing_artifacts, 1,
            "二次跑应计增量跳过"
        );
    }

    #[tokio::test]
    async fn 分页枚举跨页搬运全部asset() {
        let (meta, svc, formats, _d) = 新建().await;
        let u1 = "https://nx/repository/r3d/p1.jar";
        let u2 = "https://nx/repository/r3d/p2.jar";
        // 第 0 页 continuationToken=1 指向第 1 页；第 1 页 null 收尾
        let p0 = format!(
            r#"{{ "items": [ {{ "assets": [ {{ "path": "a/1.0/a-1.0.jar", "downloadUrl": "{u1}", "checksum": {{}} }} ]}} ], "continuationToken": "1" }}"#
        );
        let p1 = format!(
            r#"{{ "items": [ {{ "assets": [ {{ "path": "b/1.0/b-1.0.jar", "downloadUrl": "{u2}", "checksum": {{}} }} ]}} ], "continuationToken": null }}"#
        );
        let client = MockOnline {
            pages: vec![p0, p1],
            assets: HashMap::from([
                (u1.to_string(), b"aa".to_vec()),
                (u2.to_string(), b"bb".to_vec()),
            ]),
        };
        let report = migrate_online_repositories(
            &client,
            &meta,
            &svc,
            &formats,
            "https://nx",
            None,
            &[sel(src("r3d", "maven2", "hosted"), "r3d")],
            None,
        )
        .await
        .unwrap();
        assert_eq!(report.repos[0].migrated_artifacts, 2);
        assert_eq!(report.repos[0].skipped_artifacts, 0);
    }

    #[tokio::test]
    async fn 单asset下载失败不中断整批() {
        let (meta, svc, formats, _d) = 新建().await;
        let ok_url = "https://nx/repository/r3d/ok.jar";
        let bad_url = "https://nx/repository/r3d/missing.jar";
        let page = format!(
            r#"{{ "items": [ {{ "assets": [
                {{ "path": "ok/1.0/ok-1.0.jar", "downloadUrl": "{ok_url}", "checksum": {{}} }},
                {{ "path": "bad/1.0/bad-1.0.jar", "downloadUrl": "{bad_url}", "checksum": {{}} }}
            ]}} ], "continuationToken": null }}"#
        );
        // bad_url 不在 assets 映射 → 下载失败
        let client = MockOnline {
            pages: vec![page],
            assets: HashMap::from([(ok_url.to_string(), b"ok".to_vec())]),
        };
        let report = migrate_online_repositories(
            &client,
            &meta,
            &svc,
            &formats,
            "https://nx",
            None,
            &[sel(src("r3d", "maven2", "hosted"), "r3d")],
            None,
        )
        .await
        .unwrap();
        assert_eq!(report.repos[0].migrated_artifacts, 1);
        assert_eq!(report.repos[0].skipped_artifacts, 1);
    }

    #[tokio::test]
    async fn 仓库枚举致命错误先保留_repo_outcome() {
        let (meta, svc, formats, _d) = 新建().await;
        let client = MockOnline {
            pages: vec![],
            assets: HashMap::new(),
        };
        let progress = std::sync::Mutex::new(OnlinePullProgress::default());
        let selection = [sel(src("r3d", "maven2", "hosted"), "r3d")];

        let error = migrate_online_with_progress(
            &client,
            &meta,
            &svc,
            &formats,
            None,
            "https://nx",
            None,
            &selection,
            None,
            &progress,
            &JobControl::default(),
        )
        .await;

        assert!(error.is_err());
        let p = progress.lock().unwrap();
        assert_eq!(p.repos.len(), 1);
        assert_eq!(p.repos[0].source_repo, "r3d");
        assert_eq!(p.repos[0].target_repo, "r3d");
        assert_eq!(p.repos[0].done_bytes, 0);
    }

    #[tokio::test]
    async fn 空_base_url_被拒() {
        let (meta, svc, formats, _d) = 新建().await;
        let client = MockOnline {
            pages: vec![],
            assets: HashMap::new(),
        };
        let err = migrate_online_repositories(
            &client,
            &meta,
            &svc,
            &formats,
            "   ",
            None,
            &[sel(src("r3d", "maven2", "hosted"), "r3d")],
            None,
        )
        .await;
        assert!(matches!(err, Err(MigrateError::Invalid(_))));
    }

    /// 「先失败 N 次再成功」的下载 mock：验证瞬时失败自动重试后恢复。
    struct FlakyClient {
        page: String,
        url: String,
        bytes: Vec<u8>,
        fail_times: u32,
        count: std::sync::Mutex<u32>,
    }

    impl NexusClient for FlakyClient {
        async fn fetch_repositories(
            &self,
            _b: &str,
            _c: Option<&NexusCredential>,
        ) -> Result<String, MigrateError> {
            unimplemented!()
        }
        async fn fetch_components(
            &self,
            _b: &str,
            _r: &str,
            _t: Option<&str>,
            _c: Option<&NexusCredential>,
        ) -> Result<String, MigrateError> {
            Ok(self.page.clone())
        }
        async fn download_asset(
            &self,
            download_url: &str,
            _c: Option<&NexusCredential>,
        ) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>, MigrateError> {
            assert_eq!(download_url, self.url);
            let mut n = self.count.lock().unwrap();
            *n += 1;
            if *n <= self.fail_times {
                // 模拟流式下载瞬时中断（传输类错误，应被重试）
                Err(MigrateError::Transport(format!("瞬时中断 第{n}次")))
            } else {
                Ok(Box::new(std::io::Cursor::new(self.bytes.clone())))
            }
        }
    }
    never_docker_client!(FlakyClient);

    #[tokio::test]
    async fn 下载瞬时失败重试后成功() {
        let (meta, svc, formats, _d) = 新建().await;
        let url = "https://nx/repository/r3d/flaky.jar";
        let page = format!(
            r#"{{ "items": [ {{ "assets": [ {{ "path": "f/1.0/f-1.0.jar", "downloadUrl": "{url}", "checksum": {{}} }} ]}} ], "continuationToken": null }}"#
        );
        // 前 2 次下载失败、第 3 次成功；MAX_ASSET_ATTEMPTS=3 应恰好恢复
        let client = FlakyClient {
            page,
            url: url.to_string(),
            bytes: b"flaky-ok".to_vec(),
            fail_times: 2,
            count: std::sync::Mutex::new(0),
        };
        let report = migrate_online_repositories(
            &client,
            &meta,
            &svc,
            &formats,
            "https://nx",
            None,
            &[sel(src("r3d", "maven2", "hosted"), "r3d")],
            None,
        )
        .await
        .unwrap();
        // 重试后成功搬运、未计跳过；恰好尝试 3 次
        assert_eq!(report.repos[0].migrated_artifacts, 1);
        assert_eq!(report.repos[0].skipped_artifacts, 0);
        assert_eq!(*client.count.lock().unwrap(), 3);
    }

    #[tokio::test]
    async fn 进度随枚举与下载推进() {
        let (meta, svc, formats, _d) = 新建().await;
        let u1 = "https://nx/repository/r3d/a.jar";
        let u2 = "https://nx/repository/r3d/b.jar";
        let bad = "https://nx/repository/r3d/missing.jar";
        let page = format!(
            r#"{{ "items": [ {{ "assets": [
                {{ "path": "a/1.0/a-1.0.jar", "downloadUrl": "{u1}", "checksum": {{}} }},
                {{ "path": "b/1.0/b-1.0.jar", "downloadUrl": "{u2}", "checksum": {{}} }},
                {{ "path": "c/1.0/c-1.0.jar", "downloadUrl": "{bad}", "checksum": {{}} }}
            ]}} ], "continuationToken": null }}"#
        );
        let client = MockOnline {
            pages: vec![page],
            assets: HashMap::from([
                (u1.to_string(), b"a".to_vec()),
                (u2.to_string(), b"b".to_vec()),
            ]),
        };
        let progress = std::sync::Mutex::new(OnlinePullProgress::default());
        let control = JobControl::default();
        let report = migrate_online_with_progress(
            &client,
            &meta,
            &svc,
            &formats,
            None,
            "https://nx",
            None,
            &[sel(src("r3d", "maven2", "hosted"), "r3d")],
            None,
            &progress,
            &control,
        )
        .await
        .unwrap();

        assert_eq!(report.repos[0].migrated_artifacts, 2);
        assert_eq!(report.repos[0].skipped_artifacts, 1);
        // 进度终态：总 3、完成 3（2 迁 + 1 跳）；Done 由 api 任务在返回后置，故此处仍为 downloading
        let p = progress.lock().unwrap();
        assert_eq!(p.total_assets, 3);
        assert_eq!(p.done_assets, 3);
        assert_eq!(p.migrated, 2);
        assert_eq!(p.skipped, 1);
        assert_eq!(p.phase, OnlinePullPhase::Downloading);
        assert_eq!(p.repos.len(), 1);
        assert_eq!(p.current_repo.as_deref(), Some("r3d"));
    }

    #[tokio::test]
    async fn 未支持格式仓库计入进度的skipped_repos() {
        let (meta, svc, formats, _d) = 新建().await;
        let client = MockOnline {
            pages: vec![],
            assets: HashMap::new(),
        };
        let progress = std::sync::Mutex::new(OnlinePullProgress::default());
        let control = JobControl::default();
        // cargo hosted 属在线搬运未支持格式（FR-143 只扩 raw/npm）→ 整体跳过、计入 skipped_repos
        let _ = migrate_online_with_progress(
            &client,
            &meta,
            &svc,
            &formats,
            None,
            "https://nx",
            None,
            &[sel(src("cargo-hosted", "cargo", "hosted"), "cargo-hosted")],
            None,
            &progress,
            &control,
        )
        .await
        .unwrap();
        let p = progress.lock().unwrap();
        assert!(p.skipped_repos.contains(&"cargo-hosted".to_string()));
        assert_eq!(p.total_assets, 0);
    }

    // ---------- FR-91：任务控制（取消 / 暂停 / 继续）----------

    /// 受控下载 mock：每次 `download_asset` 进入时计数 + 通知测试，再在信号量上阻塞，
    /// 直至测试为该 asset「放行」一个许可。借此把后台搬运卡在确切的 asset 边界，
    /// 让测试在边界处注入取消 / 暂停信号并断言时序，无需 sleep。
    struct GatedClient {
        page: String,
        /// downloadUrl → 字节。
        assets: HashMap<String, Vec<u8>>,
        /// 已进入下载的次数。
        entered: Arc<std::sync::atomic::AtomicUsize>,
        /// 每进入一次下载即通知测试。
        entered_notify: Arc<tokio::sync::Notify>,
        /// 放行许可：测试 `add_permits(1)` 放行一个 asset 下载。
        release: Arc<tokio::sync::Semaphore>,
    }

    impl NexusClient for GatedClient {
        async fn fetch_repositories(
            &self,
            _b: &str,
            _c: Option<&NexusCredential>,
        ) -> Result<String, MigrateError> {
            unimplemented!()
        }
        async fn fetch_components(
            &self,
            _b: &str,
            _r: &str,
            _t: Option<&str>,
            _c: Option<&NexusCredential>,
        ) -> Result<String, MigrateError> {
            Ok(self.page.clone())
        }
        async fn download_asset(
            &self,
            download_url: &str,
            _c: Option<&NexusCredential>,
        ) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>, MigrateError> {
            self.entered
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.entered_notify.notify_waiters();
            // 等测试放行该 asset 的下载
            let permit = self.release.acquire().await.unwrap();
            permit.forget();
            match self.assets.get(download_url) {
                Some(bytes) => Ok(Box::new(std::io::Cursor::new(bytes.clone()))),
                None => Err(MigrateError::Transport(format!("404 {download_url}"))),
            }
        }
    }
    never_docker_client!(GatedClient);

    /// 构造 N 个 asset 的单仓库 components 页 + 受控客户端句柄。
    fn gated_client(
        n: usize,
    ) -> (
        GatedClient,
        Arc<tokio::sync::Semaphore>,
        Arc<tokio::sync::Notify>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let mut asset_lines = Vec::new();
        let mut assets = HashMap::new();
        for i in 0..n {
            let url = format!("https://nx/repository/r3d/a{i}.jar");
            asset_lines.push(format!(
                r#"{{ "path": "g/1.0/a{i}-1.0.jar", "downloadUrl": "{url}", "checksum": {{}} }}"#
            ));
            assets.insert(url, format!("bytes-{i}").into_bytes());
        }
        let page = format!(
            r#"{{ "items": [ {{ "assets": [ {} ]}} ], "continuationToken": null }}"#,
            asset_lines.join(",")
        );
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let entered_notify = Arc::new(tokio::sync::Notify::new());
        let entered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let client = GatedClient {
            page,
            assets,
            entered: entered.clone(),
            entered_notify: entered_notify.clone(),
            release: release.clone(),
        };
        (client, release, entered_notify, entered)
    }

    /// 取消在途任务：循环在 asset 边界停止后续搬运、任务标 Cancelled，且已搬运的保留。
    #[tokio::test]
    async fn 取消后停止后续搬运并标_cancelled() {
        let (meta, svc, formats, _d) = 新建().await;
        let (client, release, entered_notify, entered) = gated_client(3);
        let progress = Arc::new(std::sync::Mutex::new(OnlinePullProgress::default()));
        let control = Arc::new(JobControl::default());

        // 后台跑迁移
        let (meta2, formats2, progress2, control2) =
            (meta.clone(), formats, progress.clone(), control.clone());
        let svc = Arc::new(svc);
        let svc2 = svc.clone();
        let handle = tokio::spawn(async move {
            migrate_online_with_progress(
                &client,
                &meta2,
                &svc2,
                &formats2,
                None,
                "https://nx",
                None,
                &[sel(src("r3d", "maven2", "hosted"), "r3d")],
                None,
                &progress2,
                &control2,
            )
            .await
            .unwrap()
        });

        // 放行第 0 个 asset 并等其确实下载完进入第 1 个 asset 的边界
        release.add_permits(1);
        wait_entered(&entered_notify, &entered, 1).await;
        // 请求取消：第 1 个 asset 在边界处应被拦下，不再下载
        control.request_cancel();
        // 放行余下许可（即便放行，被取消后循环也不会再进入下载）
        release.add_permits(2);

        let report = handle.await.unwrap();
        // 仅第 0 个 asset 被搬运，后续在边界被取消
        assert_eq!(
            entered.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "取消后不应再进入下载"
        );
        assert_eq!(report.repos[0].migrated_artifacts, 1);
        let p = progress.lock().unwrap();
        assert_eq!(p.phase, OnlinePullPhase::Cancelled);
        assert_eq!(p.migrated, 1);
        assert!(!p.paused);
    }

    /// 暂停在途任务：循环挂起不推进；继续后恢复并搬完。
    #[tokio::test]
    async fn 暂停后不推进继续后恢复() {
        let (meta, svc, formats, _d) = 新建().await;
        let (client, release, entered_notify, entered) = gated_client(2);
        let progress = Arc::new(std::sync::Mutex::new(OnlinePullProgress::default()));
        let control = Arc::new(JobControl::default());

        let (meta2, formats2, progress2, control2) =
            (meta.clone(), formats, progress.clone(), control.clone());
        let svc = Arc::new(svc);
        let svc2 = svc.clone();
        let handle = tokio::spawn(async move {
            migrate_online_with_progress(
                &client,
                &meta2,
                &svc2,
                &formats2,
                None,
                "https://nx",
                None,
                &[sel(src("r3d", "maven2", "hosted"), "r3d")],
                None,
                &progress2,
                &control2,
            )
            .await
            .unwrap()
        });

        // 放行第 0 个 asset，等其完成进入第 1 个 asset 边界
        release.add_permits(1);
        wait_entered(&entered_notify, &entered, 1).await;
        // 请求暂停：第 1 个 asset 边界应挂起
        control.request_pause();
        // 即便放行第 1 个许可，暂停态下循环不应进入下载——轮询直到进度标 paused
        release.add_permits(1);
        wait_paused(&progress).await;
        assert_eq!(
            entered.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "暂停期间不应推进下载"
        );
        {
            let p = progress.lock().unwrap();
            assert!(p.paused);
            assert_eq!(p.phase, OnlinePullPhase::Paused);
        }

        // 继续：应恢复并搬完第 1 个 asset
        control.request_resume();
        let report = handle.await.unwrap();
        assert_eq!(
            entered.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "继续后应搬完全部 asset"
        );
        assert_eq!(report.repos[0].migrated_artifacts, 2);
        let p = progress.lock().unwrap();
        assert!(!p.paused);
    }

    /// 控制信号对已结束任务为幂等空操作（不 panic、不改变标志语义）。
    #[test]
    fn 控制信号幂等() {
        let control = JobControl::default();
        // 取消后再请求暂停应被取消优先吞掉，不会回到暂停
        control.request_cancel();
        assert!(control.is_cancelled());
        control.request_pause();
        assert!(!control.is_paused(), "已取消不应再被置为暂停");
        // 继续对已取消任务不改变取消标志
        control.request_resume();
        assert!(control.is_cancelled());
    }

    /// 轮询直至下载进入次数达到目标（避免 sleep；mock 在每次进入时 notify）。
    async fn wait_entered(
        notify: &tokio::sync::Notify,
        entered: &std::sync::atomic::AtomicUsize,
        target: usize,
    ) {
        loop {
            if entered.load(std::sync::atomic::Ordering::SeqCst) >= target {
                return;
            }
            let n = notify.notified();
            tokio::pin!(n);
            n.as_mut().enable();
            if entered.load(std::sync::atomic::Ordering::SeqCst) >= target {
                return;
            }
            // 设超时兜底，避免用例在回归失败时永久挂起
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), n).await;
        }
    }

    #[test]
    fn 失败明细容量截断与省略计数() {
        let progress = std::sync::Mutex::new(OnlinePullProgress::default());
        for i in 0..1002 {
            record_asset_failure(
                &progress,
                "源仓库",
                &format!("{}尾", "资".repeat(1024 + i % 2)),
                &format!("{}尾", "原".repeat(512 + i % 2)),
            );
        }
        let p = progress.lock().unwrap();
        assert_eq!(p.asset_failures.len(), 1000);
        assert_eq!(p.omitted_asset_failures, 2);
        assert!(p.asset_failures[0].asset.chars().count() <= 1024);
        assert!(p.asset_failures[0].reason.chars().count() <= 512);
        assert_eq!(p.asset_failures[0].source_repo, "源仓库");
    }

    /// 轮询直至进度进入暂停态。
    async fn wait_paused(progress: &std::sync::Mutex<OnlinePullProgress>) {
        for _ in 0..500 {
            if progress.lock().unwrap().paused {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("等待暂停态超时");
    }
}
