//! Nexus OSS 迁移入口端点（ADR-0006）：发现 / 预览 + proxy / hosted 搬运。
//!
//! - 发现 / 预览：在线 REST 入口（FR-36）+ 离线 blob store 入口（FR-37），仅枚举源系统
//!   可迁移内容与基本元数据，不搬运任何制品。
//! - 搬运：proxy 仓库配置 + 缓存制品（FR-38）、hosted 仓库配置 + 完整制品（FR-39）。
//!
//! 所有端点仅管理员可调用。handler 保持薄：解析请求、调用 `migrate` 模块编排、做错误映射，
//! 不写业务逻辑。

use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::{http::StatusCode, Extension, Json};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::meta::MigrationTaskKind;
use crate::migrate::{
    self, HttpNexusClient, JobControl, MigrateError, NexusRepoSummary, OnlinePullPhase,
    OnlinePullProgress,
};

use super::{ApiError, AppState, Identity, MigrationJobs, MigrationPersister, TaskState};

/// 把迁移入口错误映射为 HTTP 错误。
impl From<MigrateError> for ApiError {
    fn from(e: MigrateError) -> Self {
        match e {
            // 入参非法 / 凭据引用未配置：可由调用方修正，归 400
            MigrateError::Invalid(msg) => ApiError::BadRequest(msg),
            MigrateError::MissingCredential(_) => {
                ApiError::BadRequest("凭据引用未在环境变量中配置".to_string())
            }
            // 源系统侧问题（鉴权失败 / 不可用 / 响应异常）统一归 502 上游网关错误，
            // 不向调用方泄露源系统内部细节，仅记录到服务日志
            MigrateError::Status(status) => {
                tracing::warn!(状态 = status, "源 Nexus 返回错误状态");
                ApiError::BadGateway
            }
            MigrateError::Transport(err) => {
                tracing::warn!(错误 = %err, "连接源 Nexus 失败");
                ApiError::BadGateway
            }
            MigrateError::Parse(err) => {
                tracing::warn!(错误 = %err, "解析源 Nexus 响应失败");
                ApiError::BadGateway
            }
        }
    }
}

/// 迁移预览请求体。
#[derive(Debug, Deserialize)]
pub struct NexusPreviewRequest {
    /// 源 Nexus 基址（如 `https://nexus.example`）。
    pub base_url: String,
    /// 上游凭据引用（仅引用；真值走 env `JIANARTIFACT_MIGRATE_<NAME>_USERNAME/PASSWORD`，不入库）。
    /// 匿名可访问的源系统可不提供。
    #[serde(default)]
    pub auth_ref: Option<String>,
}

/// 预览源 Nexus 可迁移仓库列表（仅管理员）。
///
/// 连接在线 Nexus、枚举其仓库列表与基本元数据后返回；不搬运任何制品。
pub async fn preview_nexus_repositories(
    State(state): State<AppState>,
    identity: Identity,
    Json(req): Json<NexusPreviewRequest>,
) -> Result<Json<Vec<NexusRepoSummary>>, ApiError> {
    identity.require_admin()?;

    let base_url = req.base_url.trim().to_string();
    if base_url.is_empty() {
        return Err(ApiError::BadRequest("base_url 不能为空".to_string()));
    }

    // 复用 proxy 上游超时配置，避免慢速源系统拖垮请求线程
    // 持随 AppState 共享的出站网络热替换槽，出站时取当前 client（含运行时 PATCH 后的新代理）
    let client = HttpNexusClient::with_network_state(state.settings.network.clone());

    let repos = migrate::discover_repositories(&client, &base_url, req.auth_ref.as_deref()).await?;
    Ok(Json(repos))
}

/// 离线 blob store 预览请求体。
#[derive(Debug, Deserialize)]
pub struct NexusOfflinePreviewRequest {
    /// 本地 Nexus 文件型 blob store 根目录路径（服务进程可访问的本地文件系统路径）。
    pub path: String,
}

/// 预览本地 Nexus blob store 可迁移内容（仅管理员，FR-124 异步化）。
///
/// 仅做发现 / 预览（解析磁盘布局、按 repo 枚举可迁移 blob 及基本元数据），不读取也不搬运 blob 本体。
/// 上万 blob 的同步遍历会在前置反代后超时（504），故复用 FR-83 异步 job 基建：立即返回 `job_id`（202），
/// 后台执行枚举，结果经 `GET /migrate/jobs/{id}` 进度的 `offline_preview` 字段轮询取回。
pub async fn preview_nexus_offline(
    State(state): State<AppState>,
    Extension(jobs): Extension<Arc<MigrationJobs>>,
    identity: Identity,
    Json(req): Json<NexusOfflinePreviewRequest>,
) -> Result<(StatusCode, Json<JobCreatedDto>), ApiError> {
    identity.require_admin()?;

    let path = req.path.trim().to_string();
    if path.is_empty() {
        return Err(ApiError::BadRequest("blob store 路径不能为空".to_string()));
    }
    // 廉价同步预校验：路径必须是存在的目录——对明显错误的路径即时 400，无需起后台任务
    //（一次 stat 开销可忽略；真正高开销的 content/ 遍历 + 读上万 .properties 才放后台异步）
    if !std::path::Path::new(&path).is_dir() {
        return Err(ApiError::BadRequest(format!(
            "blob store 路径不存在或不是目录: {path}"
        )));
    }

    // 离线预览不参与迁移搬运单飞门；对外仍登记为 migration 任务，供任务中心和轮询找回。
    let job_id = Uuid::new_v4().to_string();
    state
        .tasks
        .register_migration_preview_with_id(job_id.clone(), Some("离线预览".to_string()));
    // 登记进度共享态与控制句柄（预览为单次枚举、无逐 asset 边界，控制句柄仅为统一注册表形态、不参与取消），
    // 起后台任务执行枚举，端点立即返回 job_id（202）
    let progress = Arc::new(Mutex::new(OnlinePullProgress::default()));
    let control = Arc::new(JobControl::default());
    jobs.register(job_id.clone(), progress.clone(), control);

    let tasks = state.tasks.clone();
    let task_job_id = job_id.clone();
    tokio::spawn(async move {
        // 离线枚举是同步阻塞文件 IO（遍历目录 + 读 .properties），放阻塞线程池，不占异步工作线程
        let result = tokio::task::spawn_blocking(move || {
            migrate::enumerate_blob_store(std::path::Path::new(&path))
        })
        .await;

        let final_phase = {
            let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
            match result {
                Ok(Ok(repos)) => {
                    // total_assets 记枚举到的 blob 总数，供 UI 展示规模
                    p.total_assets = repos.iter().map(|r| r.blob_count).sum();
                    p.done_assets = p.total_assets;
                    p.offline_preview = Some(repos);
                    p.phase = OnlinePullPhase::Done;
                }
                Ok(Err(e)) => {
                    p.phase = OnlinePullPhase::Failed;
                    p.error = Some(safe_migration_error(&e));
                }
                Err(_) => {
                    p.phase = OnlinePullPhase::Failed;
                    p.error = Some("离线枚举任务执行失败".to_string());
                }
            }
            p.phase
        };
        sync_migration_task_state(&tasks, &task_job_id, final_phase);
        tracing::info!(任务 = %task_job_id, "离线 blob store 预览枚举任务结束");
    });

    Ok((StatusCode::ACCEPTED, Json(JobCreatedDto { job_id })))
}

/// 离线仓库索引双通道请求体（FR-141）。
#[derive(Debug, Deserialize)]
pub struct NexusOfflineIndexRequest {
    /// 本地 Nexus 文件型 blob store 根目录路径（服务进程可访问的本地文件系统路径）。
    pub path: String,
    /// 可选：源 Nexus REST 基址。给定且可达时走权威通道取仓库配置（格式 / 类型 / 上游）；
    /// 未填或不可达时回退纯离线推断。
    #[serde(default)]
    pub base_url: Option<String>,
    /// 可选：上游凭据引用（仅引用，真值走 env `JIANARTIFACT_MIGRATE_<NAME>_USERNAME/PASSWORD`，不入库）。
    /// 匿名可访问的源系统可省略。
    #[serde(default)]
    pub auth_ref: Option<String>,
}

/// 离线仓库索引双通道枚举（仅管理员，FR-141，ADR-0041）。
///
/// 先离线扫描本地 blob store 枚举仓库（blob 数 / 总大小），再按可达性选路：给定 `base_url` 且源 Nexus REST
/// 可达时走**权威通道**（取权威格式 / 类型 / 上游，按仓库名合并）；未填或探测不可达时走**推断通道**
/// （按内容路径启发式推断格式、标「推断」来源、类型 / 上游留空）。两通道输出统一仓库索引。
///
/// 与 `/offline/preview`（blob 明细预览）并存、互不替代——本端点产出仓库级索引供向导选择。
/// 复用 FR-83 异步 job 基建：立即返回 `202 + job_id`，结果经 `GET /migrate/jobs/{id}` 进度的
/// `repo_index` 字段轮询取回。不参与迁移搬运单飞门（纯枚举）。凭据经 `auth_ref` → env，不入库 / 不进日志。
pub async fn index_nexus_offline(
    State(state): State<AppState>,
    Extension(jobs): Extension<Arc<MigrationJobs>>,
    identity: Identity,
    Json(req): Json<NexusOfflineIndexRequest>,
) -> Result<(StatusCode, Json<JobCreatedDto>), ApiError> {
    identity.require_admin()?;

    let path = req.path.trim().to_string();
    if path.is_empty() {
        return Err(ApiError::BadRequest("blob store 路径不能为空".to_string()));
    }
    // 廉价同步预校验：路径必须是存在的目录——明显错误的路径即时 400，无需起后台任务
    if !std::path::Path::new(&path).is_dir() {
        return Err(ApiError::BadRequest(format!(
            "blob store 路径不存在或不是目录: {path}"
        )));
    }

    // 规整可选 REST 参数：空白 base_url 视为未提供（回退纯离线推断）
    let base_url = req
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let auth_ref = req.auth_ref;

    // 离线索引只是枚举，不参与迁移搬运单飞门；对外仍登记为 migration 任务，供任务中心和轮询找回。
    let job_id = Uuid::new_v4().to_string();
    state
        .tasks
        .register_migration_preview_with_id(job_id.clone(), Some("离线索引".to_string()));
    let progress = Arc::new(Mutex::new(OnlinePullProgress::default()));
    let control = Arc::new(JobControl::default());
    jobs.register(job_id.clone(), progress.clone(), control);

    // 权威通道所需出站客户端：持随 AppState 共享的网络热替换槽（含运行时 PATCH 后的新代理）
    let client = HttpNexusClient::with_network_state(state.settings.network.clone());
    let tasks = state.tasks.clone();
    let task_job_id = job_id.clone();
    tokio::spawn(async move {
        // 离线枚举是同步阻塞文件 IO（遍历目录 + 读 .properties），放阻塞线程池，不占异步工作线程
        let scan_path = path.clone();
        let scan = tokio::task::spawn_blocking(move || {
            migrate::enumerate_blob_store(std::path::Path::new(&scan_path))
        })
        .await;

        let final_phase = match scan {
            Ok(Ok(offline)) => {
                // 双通道选路：给定 base_url 时尝试权威通道（不可达自动回退推断），否则纯推断
                let rest = base_url
                    .as_deref()
                    .map(|url| (&client, url, auth_ref.as_deref()));
                let index = migrate::discover_repo_index(&offline, rest).await;
                let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
                // total_assets 记枚举到的 blob 总数，供 UI 展示规模
                p.total_assets = offline.iter().map(|r| r.blob_count).sum();
                p.done_assets = p.total_assets;
                p.repo_index = Some(index);
                p.phase = OnlinePullPhase::Done;
                p.phase
            }
            Ok(Err(e)) => {
                let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
                p.phase = OnlinePullPhase::Failed;
                p.error = Some(safe_migration_error(&e));
                p.phase
            }
            Err(_) => {
                let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
                p.phase = OnlinePullPhase::Failed;
                p.error = Some("离线索引任务执行失败".to_string());
                p.phase
            }
        };
        sync_migration_task_state(&tasks, &task_job_id, final_phase);
        tracing::info!(任务 = %task_job_id, "离线仓库索引双通道枚举任务结束");
    });

    Ok((StatusCode::ACCEPTED, Json(JobCreatedDto { job_id })))
}

/// proxy 仓库配置 + 缓存制品搬运请求体（FR-38）。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NexusProxyMigrateRequest {
    /// 源离线 blob store 根目录路径：提供已缓存 proxy 制品本体（其下应含 `content/` 子目录）。
    pub offline_path: String,
    /// 可选的源仓库名白名单；未传时按离线 blob store 内全部仓库迁移。
    #[serde(default)]
    pub repositories: Option<Vec<String>>,
}

/// 执行 Nexus proxy 仓库配置创建 + 缓存制品搬运（仅管理员，FR-38）。
///
/// 仅从本地离线 blob store 推断仓库并搬运缓存制品本体，不访问源 Nexus 在线 REST，也不要求 URL / 凭据。
/// blob 先落盘校验再写索引，失败回滚不留孤儿；单制品失败不中断整批、可重入。
pub async fn migrate_nexus_proxy(
    State(state): State<AppState>,
    Extension(jobs): Extension<Arc<MigrationJobs>>,
    identity: Identity,
    Json(req): Json<NexusProxyMigrateRequest>,
) -> Result<(StatusCode, Json<JobCreatedDto>), ApiError> {
    identity.require_admin()?;

    let offline_path = req.offline_path.trim().to_string();
    if offline_path.is_empty() {
        return Err(ApiError::BadRequest("offline_path 不能为空".to_string()));
    }
    // 迁移单飞早拒（FR-131）：昂贵磁盘枚举前先快查在途迁移，已有即 409、不白跑离线扫描
    if state.tasks.migration_in_flight() {
        return Err(ApiError::Conflict("已有迁移任务在途".to_string()));
    }

    let selected_repositories = req.repositories;

    // 迁移单飞（FR-131）：原子「检查无在途迁移 → 登记统一任务」，已有在途即 409。
    let job_id = Uuid::new_v4().to_string();
    if !state
        .tasks
        .try_begin_migration(job_id.clone(), Some("离线 proxy 搬运".to_string()))
    {
        return Err(ApiError::Conflict("已有迁移任务在途".to_string()));
    }
    // 登记任务进度 + 控制句柄，起后台任务执行搬运（大库不阻塞请求、不在反代后 504，FR-125），立即返回 job_id（202）
    let progress = Arc::new(Mutex::new(OnlinePullProgress::default()));
    let control = Arc::new(JobControl::default());
    jobs.register(job_id.clone(), progress.clone(), control.clone());

    // FR-140：迁移任务落库首行 + 节流进度 ticker
    let params = offline_params_snapshot("offline_proxy", &offline_path, &selected_repositories);
    let persister = Arc::new(
        MigrationPersister::begin(
            state.meta.clone(),
            job_id.clone(),
            MigrationTaskKind::OfflineProxy,
            Some("离线 proxy 搬运".to_string()),
            params,
        )
        .await,
    );

    let meta = state.meta.clone();
    let artifacts = state.artifacts.clone();
    let formats = state.formats.clone();
    let tasks = state.tasks.clone();
    let max_size = state.config.limits.max_artifact_size;
    let task_job_id = job_id.clone();
    tokio::spawn(async move {
        let ticker = persister.clone().spawn_progress_ticker(progress.clone());
        let result = migrate::migrate_proxy_repositories_offline_with_progress(
            &meta,
            &artifacts,
            &formats,
            std::path::Path::new(&offline_path),
            selected_repositories,
            max_size,
            &progress,
            &control,
        )
        .await;
        let final_phase = {
            let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
            p.current_path = None;
            match result {
                // 被取消时 with_progress 已标 Cancelled 终态，不覆盖为 Done（FR-91）
                Ok(_) if p.phase == OnlinePullPhase::Cancelled => {}
                Ok(_) => {
                    p.phase = OnlinePullPhase::Done;
                    p.current_repo = None;
                }
                Err(e) => {
                    p.phase = OnlinePullPhase::Failed;
                    p.error = Some(safe_migration_error(&e));
                }
            }
            p.phase
        };
        finalize_migration_task(
            &tasks,
            &task_job_id,
            final_phase,
            &progress,
            &persister,
            ticker,
        )
        .await;
        tracing::info!(任务 = %task_job_id, "离线 proxy 仓库搬运后台任务结束");
    });

    Ok((StatusCode::ACCEPTED, Json(JobCreatedDto { job_id })))
}

/// hosted 仓库配置 + 完整制品搬运请求体（FR-39）。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NexusHostedMigrateRequest {
    /// 源离线 blob store 根目录路径：提供 hosted 仓库制品本体（其下应含 `content/` 子目录）。
    pub offline_path: String,
    /// 可选的源仓库名白名单；未传时按离线 blob store 内全部仓库迁移。
    #[serde(default)]
    pub repositories: Option<Vec<String>>,
}

/// 按前端所选源仓库过滤离线搬运范围；未传选择时保持旧版全量行为。
#[cfg(test)]
fn filter_selected_repositories(
    source_repos: Vec<NexusRepoSummary>,
    selected: Option<Vec<String>>,
) -> Result<Vec<NexusRepoSummary>, ApiError> {
    let Some(selected) = selected else {
        return Ok(source_repos);
    };
    let selected: Vec<String> = selected
        .into_iter()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    if selected.is_empty() {
        return Err(ApiError::BadRequest("未选择要迁移的仓库".to_string()));
    }
    let mut filtered = Vec::new();
    for repo in source_repos {
        if selected.iter().any(|name| name == &repo.name) {
            filtered.push(repo);
        }
    }
    if filtered.len() != selected.len() {
        return Err(ApiError::BadRequest("存在未知源仓库".to_string()));
    }
    Ok(filtered)
}

/// 执行 Nexus hosted 仓库配置创建 + 完整制品搬运（仅管理员，FR-39）。
///
/// 仅从本地离线 blob store 推断仓库并搬运其全部制品本体，不访问源 Nexus 在线 REST，也不要求 URL / 凭据。
/// blob 先落盘校验再写 `cached = false` 索引，失败回滚不留孤儿；单制品失败 / 不可覆盖不中断整批、可重入。
/// 超过 `limits.max_artifact_size` 的制品按跳过处理（不写半截 blob）。
pub async fn migrate_nexus_hosted(
    State(state): State<AppState>,
    Extension(jobs): Extension<Arc<MigrationJobs>>,
    identity: Identity,
    Json(req): Json<NexusHostedMigrateRequest>,
) -> Result<(StatusCode, Json<JobCreatedDto>), ApiError> {
    identity.require_admin()?;

    let offline_path = req.offline_path.trim().to_string();
    if offline_path.is_empty() {
        return Err(ApiError::BadRequest("offline_path 不能为空".to_string()));
    }
    // 迁移单飞早拒（FR-131）：昂贵磁盘枚举前先快查在途迁移，已有即 409、不白跑离线扫描
    if state.tasks.migration_in_flight() {
        return Err(ApiError::Conflict("已有迁移任务在途".to_string()));
    }

    let selected_repositories = req.repositories;

    // 登记任务进度 + 控制句柄，起后台任务执行搬运（大库不阻塞请求、不在反代后 504，FR-125），立即返回 job_id（202）
    // 迁移单飞（FR-131）：原子「检查无在途迁移 → 登记统一任务」，已有在途即 409。
    let job_id = Uuid::new_v4().to_string();
    if !state
        .tasks
        .try_begin_migration(job_id.clone(), Some("离线 hosted 搬运".to_string()))
    {
        return Err(ApiError::Conflict("已有迁移任务在途".to_string()));
    }
    let progress = Arc::new(Mutex::new(OnlinePullProgress::default()));
    let control = Arc::new(JobControl::default());
    jobs.register(job_id.clone(), progress.clone(), control.clone());

    // FR-140：迁移任务落库首行 + 节流进度 ticker
    let params = offline_params_snapshot("offline_hosted", &offline_path, &selected_repositories);
    let persister = Arc::new(
        MigrationPersister::begin(
            state.meta.clone(),
            job_id.clone(),
            MigrationTaskKind::OfflineHosted,
            Some("离线 hosted 搬运".to_string()),
            params,
        )
        .await,
    );

    let meta = state.meta.clone();
    let artifacts = state.artifacts.clone();
    let formats = state.formats.clone();
    let max_size = state.config.limits.max_artifact_size;
    let tasks = state.tasks.clone();
    let task_job_id = job_id.clone();
    tokio::spawn(async move {
        let ticker = persister.clone().spawn_progress_ticker(progress.clone());
        let result = migrate::migrate_hosted_repositories_offline_with_progress(
            &meta,
            &artifacts,
            &formats,
            std::path::Path::new(&offline_path),
            selected_repositories,
            max_size,
            &progress,
            &control,
        )
        .await;
        let final_phase = {
            let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
            p.current_path = None;
            match result {
                Ok(_) if p.phase == OnlinePullPhase::Cancelled => {}
                Ok(_) => {
                    p.phase = OnlinePullPhase::Done;
                    p.current_repo = None;
                }
                Err(e) => {
                    p.phase = OnlinePullPhase::Failed;
                    p.error = Some(safe_migration_error(&e));
                }
            }
            p.phase
        };
        finalize_migration_task(
            &tasks,
            &task_job_id,
            final_phase,
            &progress,
            &persister,
            ticker,
        )
        .await;
        tracing::info!(任务 = %task_job_id, "离线 hosted 仓库搬运后台任务结束");
    });

    Ok((StatusCode::ACCEPTED, Json(JobCreatedDto { job_id })))
}

/// 在线拉取迁移请求体（FR-82）：选中的源仓库 + 目标仓库名（无需离线 blob store）。
#[derive(Debug, Deserialize)]
pub struct NexusOnlineMigrateRequest {
    /// 源 Nexus 基址：经其 REST API 枚举仓库配置与制品。
    pub base_url: String,
    /// 上游凭据引用（仅引用，真值走 env，不入库）；匿名可访问的源系统可省略。
    #[serde(default)]
    pub auth_ref: Option<String>,
    /// 选中的源仓库及目标仓库名（`target` 省略 / 空则与源同名）。
    pub repositories: Vec<OnlineRepoSelectionDto>,
}

/// 在线拉取的单个仓库选择项。
#[derive(Debug, Deserialize)]
pub struct OnlineRepoSelectionDto {
    /// 源 Nexus 仓库名。
    pub source: String,
    /// 本系统目标仓库名（省略 / 空则与源同名，允许改名）。
    #[serde(default)]
    pub target: Option<String>,
}

/// 在线拉取迁移触发响应（FR-83）：返回 `job_id`，搬运在后台异步执行。
#[derive(Debug, Serialize)]
pub struct JobCreatedDto {
    /// 任务 id，供轮询 `GET /migrate/jobs/{id}`。
    pub job_id: String,
}

/// 单任务进度响应（FR-83）：`job_id` + 进度快照（展平）。
#[derive(Debug, Serialize)]
pub struct JobProgressDto {
    /// 任务 id。
    pub job_id: String,
    /// 进度快照。
    #[serde(flatten)]
    pub progress: OnlinePullProgress,
}

/// 任务列表项（FR-83）：供客户端重连找回活动 / 近期任务。
#[derive(Debug, Serialize)]
pub struct JobSummaryDto {
    /// 任务 id。
    pub job_id: String,
    /// 当前阶段。
    pub phase: OnlinePullPhase,
    /// 总 asset 数。
    pub total_assets: usize,
    /// 已处理 asset 数。
    pub done_assets: usize,
    /// 成功搬运数。
    pub migrated: usize,
    /// 跳过数。
    pub skipped: usize,
    /// 当前处理的源仓库。
    pub current_repo: Option<String>,
    /// 是否处于暂停态（FR-91）。
    pub paused: bool,
}

/// 触发 Nexus 在线拉取迁移（仅管理员，FR-82 + FR-83）。
///
/// 同步阶段：枚举源仓库配置、匹配所选仓库、解析凭据（失败即 400 / 502，不开任务）；随后**立即返回
/// `job_id`（202）**，实际 asset 枚举 + HTTP 流式下载 + 落地在后台 tokio 任务执行，进度经任务注册表
/// 轮询（`GET /migrate/jobs/{id}`）。Maven / Raw / npm hosted 参与（FR-82 + FR-143），其余（含 Docker/OCI
/// 走 FR-144）与 proxy 整体跳过。凭据真值走 env，绝不入库 / 不进日志。
pub async fn migrate_nexus_online(
    State(state): State<AppState>,
    Extension(jobs): Extension<Arc<MigrationJobs>>,
    identity: Identity,
    Json(req): Json<NexusOnlineMigrateRequest>,
) -> Result<(StatusCode, Json<JobCreatedDto>), ApiError> {
    identity.require_admin()?;
    if req.repositories.is_empty() {
        return Err(ApiError::BadRequest("未选择要迁移的仓库".to_string()));
    }
    let base_url = req.base_url.trim().to_string();
    if base_url.is_empty() {
        return Err(ApiError::BadRequest("base_url 不能为空".to_string()));
    }
    // 迁移单飞早拒（FR-131）：昂贵同步枚举前先快查在途迁移，已有即 409、不白跑上游
    if state.tasks.migration_in_flight() {
        return Err(ApiError::Conflict("已有迁移任务在途".to_string()));
    }

    // 持随 AppState 共享的出站网络热替换槽，出站时取当前 client（含运行时 PATCH 后的新代理）
    let client = HttpNexusClient::with_network_state(state.settings.network.clone());

    // 解析凭据（用于 components 枚举与 asset 下载）；匿名源可省略
    let credential = match req.auth_ref.as_deref() {
        Some(r) if !r.is_empty() => Some(migrate::resolve_credential(r)?),
        _ => None,
    };

    // 同步枚举源仓库列表并匹配所选 source（含格式 / 类型）；失败即同步报错，不开任务
    let source_repos =
        migrate::discover_repositories(&client, &base_url, req.auth_ref.as_deref()).await?;

    let mut selections = Vec::with_capacity(req.repositories.len());
    for r in &req.repositories {
        let source = r.source.trim();
        let Some(summary) = source_repos.iter().find(|s| s.name == source) else {
            return Err(ApiError::BadRequest(format!("源仓库不存在: {source}")));
        };
        let target = r
            .target
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(source)
            .to_string();
        selections.push(migrate::OnlinePullSelection {
            source: summary.clone(),
            target_repo: target,
        });
    }

    // 迁移单飞（FR-131）：原子「检查无在途迁移 → 登记统一任务」，已有在途即 409。
    let job_id = Uuid::new_v4().to_string();
    if !state
        .tasks
        .try_begin_migration(job_id.clone(), Some("在线拉取迁移".to_string()))
    {
        return Err(ApiError::Conflict("已有迁移任务在途".to_string()));
    }
    // 登记任务进度共享态与控制句柄，起后台任务执行枚举 + 下载，端点立即返回 job_id（202）
    let progress = Arc::new(Mutex::new(OnlinePullProgress::default()));
    let control = Arc::new(JobControl::default());
    jobs.register(job_id.clone(), progress.clone(), control.clone());

    // FR-140：迁移任务落库首行 + 节流进度 ticker。参数快照仅存 auth_ref 引用与非敏感配置，凭据真值不入库。
    let params = online_params_snapshot(&base_url, req.auth_ref.as_deref(), &req.repositories);
    let persister = Arc::new(
        MigrationPersister::begin(
            state.meta.clone(),
            job_id.clone(),
            MigrationTaskKind::Online,
            Some("在线拉取迁移".to_string()),
            params,
        )
        .await,
    );

    let meta = state.meta.clone();
    let artifacts = state.artifacts.clone();
    let formats = state.formats.clone();
    // docker registry（FR-144）：装配时 docker hosted 仓库走 registry v2 manifest + layer 图搬运
    let docker = state.docker.clone();
    let max_size = state.config.limits.max_artifact_size;
    let tasks = state.tasks.clone();
    let task_job_id = job_id.clone();
    tokio::spawn(async move {
        let ticker = persister.clone().spawn_progress_ticker(progress.clone());
        let result = migrate::migrate_online_with_progress(
            &client,
            &meta,
            &artifacts,
            &formats,
            docker.as_deref(),
            &base_url,
            credential.as_ref(),
            &selections,
            max_size,
            &progress,
            &control,
        )
        .await;
        let final_phase = {
            let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
            p.current_path = None;
            match result {
                // 被取消时 migrate_online_with_progress 已把进度标为 Cancelled 终态，
                // 此处不得覆盖为 Done（取消不算成功完成，FR-91）
                Ok(_) if p.phase == OnlinePullPhase::Cancelled => {}
                Ok(_) => {
                    p.phase = OnlinePullPhase::Done;
                    p.current_repo = None;
                }
                Err(e) => {
                    p.phase = OnlinePullPhase::Failed;
                    p.error = Some(safe_migration_error(&e));
                }
            }
            p.phase
        };
        finalize_migration_task(
            &tasks,
            &task_job_id,
            final_phase,
            &progress,
            &persister,
            ticker,
        )
        .await;
        tracing::info!(任务 = %task_job_id, "在线拉取后台任务结束");
    });

    Ok((StatusCode::ACCEPTED, Json(JobCreatedDto { job_id })))
}

/// 构造在线拉取迁移的**脱敏**参数快照 JSON（供重跑，凭据真值绝不入库，仅存 `auth_ref` 引用名）。
fn online_params_snapshot(
    base_url: &str,
    auth_ref: Option<&str>,
    repositories: &[OnlineRepoSelectionDto],
) -> String {
    let repos: Vec<serde_json::Value> = repositories
        .iter()
        .map(|r| {
            serde_json::json!({
                "source": r.source.trim(),
                "target": r.target.as_deref().map(str::trim).filter(|t| !t.is_empty()),
            })
        })
        .collect();
    serde_json::json!({
        "kind": "online",
        "base_url": base_url,
        // 仅引用名，凭据真值走 env、绝不入库
        "auth_ref": auth_ref.filter(|r| !r.is_empty()),
        "repositories": repos,
    })
    .to_string()
}

/// 构造离线搬运（proxy / hosted）的参数快照 JSON（无凭据；离线路径 + 可选仓库白名单）。
fn offline_params_snapshot(
    kind: &str,
    offline_path: &str,
    repositories: &Option<Vec<String>>,
) -> String {
    serde_json::json!({
        "kind": kind,
        "offline_path": offline_path,
        "repositories": repositories,
    })
    .to_string()
}

/// 把迁移 kind 专表的最终阶段映射并同步到统一任务注册表（FR-131）。
///
/// 终态收敛迁移单飞门：`done → succeeded` / `failed → failed`（带错误） / `cancelled → cancelled`；
/// 其余非终态阶段不更新（理论上后台任务结束时阶段已是终态之一）。
fn sync_migration_task_state(
    tasks: &Arc<super::TaskRegistry>,
    job_id: &str,
    phase: OnlinePullPhase,
) {
    match phase {
        OnlinePullPhase::Done => tasks.set_state(job_id, TaskState::Succeeded, None),
        OnlinePullPhase::Cancelled => tasks.set_state(job_id, TaskState::Cancelled, None),
        OnlinePullPhase::Failed => {
            tasks.set_state(job_id, TaskState::Failed, Some("迁移失败".to_string()))
        }
        // 非终态：后台任务正常结束不会停在此（防御性不更新，避免误把在途置终态）
        OnlinePullPhase::Enumerating | OnlinePullPhase::Downloading | OnlinePullPhase::Paused => {}
    }
}

/// 后台迁移任务收尾（FR-131 + FR-140）：同步统一注册表终态 + 停节流 ticker + 写持久化终态报告。
///
/// 先同步统一注册表释放单飞门（保持 FR-131 行为不变），再 `abort` 节流 ticker，最后据统一注册表
/// 终态与最终进度快照把完整报告 + 分仓库明细落库（ADR-0040 终态强一致）。持久化失败只记 WARN、
/// 不影响业务（历史缺一条可接受）。
async fn finalize_migration_task(
    tasks: &Arc<super::TaskRegistry>,
    job_id: &str,
    final_phase: OnlinePullPhase,
    progress: &Arc<Mutex<OnlinePullProgress>>,
    persister: &MigrationPersister,
    ticker: tokio::task::JoinHandle<()>,
) {
    // 先据 kind 专表终态同步统一注册表（FR-131），释放迁移单飞门
    sync_migration_task_state(tasks, job_id, final_phase);
    // 停节流 ticker，避免其与终态写竞态覆盖
    ticker.abort();
    // 取统一注册表的终态（succeeded / failed / cancelled）作为持久层终态来源
    let final_state = tasks
        .get(job_id)
        .map(|t| t.state)
        .unwrap_or(TaskState::Failed);
    // 锁外 clone 出最终进度快照，落终态 + 完整报告
    let snapshot = {
        let g = progress.lock().unwrap_or_else(|e| e.into_inner());
        g.clone()
    };
    persister.finish(final_state, &snapshot).await;
}

/// 查询某在线拉取任务的进度（仅管理员，FR-83）。未知 id 返回 404。
pub async fn migrate_nexus_job(
    Extension(jobs): Extension<Arc<MigrationJobs>>,
    identity: Identity,
    Path(job_id): Path<String>,
) -> Result<Json<JobProgressDto>, ApiError> {
    identity.require_admin()?;
    let progress = jobs.get(&job_id).ok_or(ApiError::NotFound)?;
    let snap = progress.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Ok(Json(JobProgressDto {
        job_id,
        progress: snap,
    }))
}

/// 列出活动 / 近期在线拉取任务（仅管理员，FR-83），供客户端重连找回。
pub async fn migrate_nexus_jobs(
    Extension(jobs): Extension<Arc<MigrationJobs>>,
    identity: Identity,
) -> Result<Json<Vec<JobSummaryDto>>, ApiError> {
    identity.require_admin()?;
    let list = jobs
        .list()
        .into_iter()
        .map(|(job_id, p)| JobSummaryDto {
            job_id,
            phase: p.phase,
            total_assets: p.total_assets,
            done_assets: p.done_assets,
            migrated: p.migrated,
            skipped: p.skipped,
            current_repo: p.current_repo,
            paused: p.paused,
        })
        .collect();
    Ok(Json(list))
}

/// 取某在线拉取任务的控制句柄；未知 id 返回 404（含已被注册表淘汰的旧任务，FR-91）。
fn job_control(jobs: &Arc<MigrationJobs>, job_id: &str) -> Result<Arc<JobControl>, ApiError> {
    jobs.control(job_id).ok_or(ApiError::NotFound)
}

/// 取消某在线拉取任务（仅管理员，FR-91）。
///
/// 置取消信号，后台循环在下一 asset 边界停止后续搬运、任务标 `cancelled`（不算失败，已搬运保留）。
/// 未知 id 返回 404；对已结束任务为幂等空操作、返回 200（`request_cancel` 自身吞掉）。
pub async fn migrate_nexus_job_cancel(
    Extension(jobs): Extension<Arc<MigrationJobs>>,
    identity: Identity,
    Path(job_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    identity.require_admin()?;
    let control = job_control(&jobs, &job_id)?;
    control.request_cancel();
    tracing::info!(任务 = %job_id, "已请求取消在线拉取任务");
    Ok(StatusCode::OK)
}

/// 暂停某在线拉取任务（仅管理员，FR-91）。
///
/// 置暂停信号，后台循环在下一 asset 边界挂起、不再推进。未知 id 返回 404；
/// 对已取消 / 已结束任务为幂等空操作、返回 200。
pub async fn migrate_nexus_job_pause(
    State(state): State<AppState>,
    Extension(jobs): Extension<Arc<MigrationJobs>>,
    identity: Identity,
    Path(job_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    identity.require_admin()?;
    let control = job_control(&jobs, &job_id)?;
    control.request_pause();
    // 仅当命中可控在途搬运任务才同步状态（离线预览等不可控任务不改写）
    if state
        .tasks
        .set_controllable_migration_state(&job_id, TaskState::Paused)
    {
        // FR-140：状态变更即时落库（暂停），失败只记 WARN 不影响业务
        let _ = state
            .meta
            .set_migration_task_state(&job_id, crate::meta::MigrationTaskState::Paused)
            .await;
    }
    tracing::info!(任务 = %job_id, "已请求暂停在线拉取任务");
    Ok(StatusCode::OK)
}

/// 继续某已暂停的在线拉取任务（仅管理员，FR-91）。
///
/// 清暂停信号并唤醒挂起的后台循环恢复搬运。未知 id 返回 404；
/// 对未暂停 / 已结束任务为幂等空操作、返回 200。
pub async fn migrate_nexus_job_resume(
    State(state): State<AppState>,
    Extension(jobs): Extension<Arc<MigrationJobs>>,
    identity: Identity,
    Path(job_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    identity.require_admin()?;
    let control = job_control(&jobs, &job_id)?;
    control.request_resume();
    // 仅当命中可控在途搬运任务才同步状态
    if state
        .tasks
        .set_controllable_migration_state(&job_id, TaskState::Running)
    {
        // FR-140：状态变更即时落库（继续），失败只记 WARN 不影响业务
        let _ = state
            .meta
            .set_migration_task_state(&job_id, crate::meta::MigrationTaskState::Running)
            .await;
    }
    tracing::info!(任务 = %job_id, "已请求继续在线拉取任务");
    Ok(StatusCode::OK)
}

/// group 仓库配置 + 成员映射迁移请求体（FR-137）。
#[derive(Debug, Deserialize)]
pub struct NexusGroupMigrateRequest {
    /// 源 Nexus 基址：经其 REST API 枚举 group 仓库配置（格式 / 成员名列表）。
    pub base_url: String,
    /// 上游凭据引用（仅引用，真值走 env，不入库）；匿名可访问的源系统可省略。
    #[serde(default)]
    pub auth_ref: Option<String>,
}

/// 执行 Nexus group 仓库配置 + 成员映射迁移（仅管理员，FR-137）。
///
/// 经在线 REST 枚举源 group 仓库配置 → 在本系统建 group 仓库 → 按成员名映射成员 id 列表并写入关联。
/// 成员仓库应已由 proxy/hosted 迁移（FR-38/39）先建好；成员缺失时记告警 + 跳过该成员（不中断整 group）。
/// group 迁移为纯元数据操作（无 blob 搬运），同步返回完整报告，无需异步 job。
pub async fn migrate_nexus_group(
    State(state): State<AppState>,
    identity: Identity,
    Json(req): Json<NexusGroupMigrateRequest>,
) -> Result<Json<migrate::GroupMigrationReport>, ApiError> {
    identity.require_admin()?;

    // 同步阶段：在线枚举源仓库配置（含 group 成员名列表）
    let client = HttpNexusClient::with_network_state(state.settings.network.clone());
    let source_repos =
        migrate::discover_repositories(&client, &req.base_url, req.auth_ref.as_deref()).await?;

    // group 迁移为纯元数据操作，同步执行并返回报告
    let report = migrate::migrate_group_repositories(&state.meta, &source_repos)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    tracing::info!(
        已迁移 = report.migrated.len(),
        已跳过 = report.skipped_repos.len(),
        "group 仓库迁移完成"
    );
    Ok(Json(report))
}

/// 迁移历史列表默认返回上限（近期任务，避免一次拉爆）。
const MIGRATION_HISTORY_LIMIT: i64 = 100;

/// 迁移历史任务的分仓库报告项（对外 DTO）。
#[derive(Debug, Serialize)]
pub struct MigrationRepoReportDto {
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
    pub total_bytes: i64,
    pub done_bytes: i64,
    pub migrated_bytes: i64,
    pub skipped_bytes: i64,
    pub duration_ms: i64,
}

/// 迁移历史中的失败资产明细。
#[derive(Debug, Serialize)]
pub struct MigrationAssetFailureDto {
    pub source_repo: String,
    pub asset: String,
    pub reason: String,
}

/// 迁移历史任务记录（对外 DTO）：列表项与详情共用的任务元数据 + 进度快照。
#[derive(Debug, Serialize)]
pub struct MigrationTaskDto {
    /// 任务 id。
    pub id: String,
    /// 迁移类别（online / offline_proxy / offline_hosted）。
    pub kind: String,
    /// 统一状态（running / paused / succeeded / failed / cancelled / interrupted）。
    pub state: String,
    /// 人类可读标签。
    pub label: Option<String>,
    /// 引擎阶段。
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
    /// 因容量限制省略的失败资产明细数。
    pub omitted_asset_failures: i64,
    /// 失败原因（失败时非空）。
    pub error: Option<String>,
    /// 起始时刻。
    pub started_at: String,
    /// 最近落库时刻。
    pub updated_at: String,
    /// 终态时刻（未结束为空）；与 started_at 之差即耗时。
    pub finished_at: Option<String>,
}

impl From<crate::meta::MigrationTaskRecord> for MigrationTaskDto {
    fn from(r: crate::meta::MigrationTaskRecord) -> Self {
        Self {
            id: r.id,
            kind: r.kind,
            state: r.state,
            label: r.label,
            phase: r.phase,
            total_assets: r.total_assets,
            done_assets: r.done_assets,
            migrated: r.migrated,
            skipped_existing: r.skipped_existing,
            skipped: r.skipped,
            total_bytes: r.total_bytes,
            done_bytes: r.done_bytes,
            migrated_bytes: r.migrated_bytes,
            skipped_bytes: r.skipped_bytes,
            omitted_asset_failures: r.omitted_asset_failures,
            error: r.error,
            started_at: r.started_at,
            updated_at: r.updated_at,
            finished_at: r.finished_at,
        }
    }
}

/// 迁移历史任务详情（DTO）：任务记录 + 分仓库报告。
#[derive(Debug, Serialize)]
pub struct MigrationTaskDetailDto {
    /// 任务记录（含进度快照）。
    #[serde(flatten)]
    pub task: MigrationTaskDto,
    /// 分仓库明细报告。
    pub repos: Vec<MigrationRepoReportDto>,
    /// 最早发生的失败资产明细。
    pub asset_failures: Vec<MigrationAssetFailureDto>,
}

/// 列出迁移历史（库中持久记录，仅 Admin，FR-140）。
///
/// 与 `GET /migrate/jobs`（进程内活跃任务）互补：本端点返回跨重启的历史 + 报告视图，
/// 含重启后被标 `interrupted` 的任务。按起始时刻倒序。
pub async fn list_migration_tasks(
    State(state): State<AppState>,
    identity: Identity,
) -> Result<Json<Vec<MigrationTaskDto>>, ApiError> {
    identity.require_admin()?;
    let records = state
        .meta
        .list_migration_tasks(MIGRATION_HISTORY_LIMIT)
        .await
        .map_err(db_internal)?;
    Ok(Json(
        records.into_iter().map(MigrationTaskDto::from).collect(),
    ))
}

/// 把 meta 层错误映射为 500，并记 WARN（不向调用方泄露 DB 内部细节）。
fn db_internal(_: crate::meta::MetaError) -> ApiError {
    tracing::warn!("迁移历史查询或落库失败");
    ApiError::Internal
}

/// 把内部迁移错误映射为稳定白名单文案，避免 URL、路径或配置值进入持久化/API。
fn safe_migration_error(error: &migrate::MigrateError) -> String {
    match error {
        migrate::MigrateError::Invalid(_) => "迁移参数或源数据无效",
        migrate::MigrateError::MissingCredential(_) => "迁移凭据未配置",
        migrate::MigrateError::Status(401 | 403) => "源系统鉴权失败",
        migrate::MigrateError::Status(_) => "源系统返回错误状态",
        migrate::MigrateError::Transport(_) => "源系统请求失败",
        migrate::MigrateError::Parse(_) => "源系统响应解析失败",
    }
    .to_string()
}

/// 查单个迁移历史任务详情 + 分仓库报告（仅 Admin，FR-140）。未知 id 返回 404。
pub async fn get_migration_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(task_id): Path<String>,
) -> Result<Json<MigrationTaskDetailDto>, ApiError> {
    identity.require_admin()?;
    let detail = state
        .meta
        .get_migration_task_detail(&task_id)
        .await
        .map_err(db_internal)?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(MigrationTaskDetailDto {
        task: MigrationTaskDto::from(detail.task),
        repos: detail
            .repos
            .into_iter()
            .map(|r| MigrationRepoReportDto {
                source_repo: r.source_repo,
                target_repo: r.target_repo,
                format: r.format,
                created: r.created != 0,
                migrated_artifacts: r.migrated_artifacts,
                skipped_existing_artifacts: r.skipped_existing_artifacts,
                skipped_artifacts: r.skipped_artifacts,
                total_bytes: r.total_bytes,
                done_bytes: r.done_bytes,
                migrated_bytes: r.migrated_bytes,
                skipped_bytes: r.skipped_bytes,
                duration_ms: r.duration_ms,
            })
            .collect(),
        asset_failures: detail
            .asset_failures
            .into_iter()
            .map(|failure| MigrationAssetFailureDto {
                source_repo: failure.source_repo,
                asset: failure.asset,
                reason: failure.reason,
            })
            .collect(),
    }))
}

/// 一键幂等重跑某历史迁移任务（仅 Admin，FR-140）。
///
/// 据任务库中**脱敏参数快照**重建原请求，委托对应触发端点重新执行一次幂等搬运——复用既有引擎的
/// sha256 跳过（目标已存在且一致即跳过），进度区分「跳过 vs 新搬」，等效断点续传。凭据仍从 env 现解析
/// （快照只存 `auth_ref` 引用），不从库取。复用既有迁移单飞门（已有在途则 409）。
///
/// 未知 id / 无参数快照返回 404 / 400；快照解析失败返回 400。重跑生成**新 job_id**（不覆盖原历史记录）。
pub async fn rerun_migration_task(
    State(state): State<AppState>,
    Extension(jobs): Extension<Arc<MigrationJobs>>,
    identity: Identity,
    Path(task_id): Path<String>,
) -> Result<(StatusCode, Json<JobCreatedDto>), ApiError> {
    identity.require_admin()?;
    let task = state
        .meta
        .get_migration_task(&task_id)
        .await
        .map_err(db_internal)?
        .ok_or(ApiError::NotFound)?;

    // 解析脱敏参数快照 → 据 kind 重建请求并委托对应触发端点（复用其校验 / 单飞 / 落库）
    let params: serde_json::Value = serde_json::from_str(&task.params_snapshot)
        .map_err(|_| ApiError::BadRequest("迁移参数快照损坏，无法重跑".to_string()))?;
    let kind = params.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
        "online" => {
            let req = rebuild_online_request(&params)?;
            migrate_nexus_online(State(state), Extension(jobs), identity, Json(req)).await
        }
        "offline_proxy" => {
            let req = NexusProxyMigrateRequest {
                offline_path: rebuild_offline_path(&params)?,
                repositories: rebuild_offline_repositories(&params),
            };
            migrate_nexus_proxy(State(state), Extension(jobs), identity, Json(req)).await
        }
        "offline_hosted" => {
            let req = NexusHostedMigrateRequest {
                offline_path: rebuild_offline_path(&params)?,
                repositories: rebuild_offline_repositories(&params),
            };
            migrate_nexus_hosted(State(state), Extension(jobs), identity, Json(req)).await
        }
        _ => Err(ApiError::BadRequest(
            "迁移参数快照类别未知，无法重跑".to_string(),
        )),
    }
}

/// 从参数快照重建在线拉取请求。
fn rebuild_online_request(
    params: &serde_json::Value,
) -> Result<NexusOnlineMigrateRequest, ApiError> {
    let base_url = params
        .get("base_url")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| ApiError::BadRequest("迁移参数快照缺 base_url".to_string()))?;
    let auth_ref = params
        .get("auth_ref")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let repositories = params
        .get("repositories")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    let source = r.get("source").and_then(|v| v.as_str())?.to_string();
                    let target = r.get("target").and_then(|v| v.as_str()).map(str::to_string);
                    Some(OnlineRepoSelectionDto { source, target })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(NexusOnlineMigrateRequest {
        base_url,
        auth_ref,
        repositories,
    })
}

/// 从参数快照重建离线路径。
fn rebuild_offline_path(params: &serde_json::Value) -> Result<String, ApiError> {
    params
        .get("offline_path")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| ApiError::BadRequest("迁移参数快照缺 offline_path".to_string()))
}

/// 从参数快照重建离线仓库白名单（可空）。
fn rebuild_offline_repositories(params: &serde_json::Value) -> Option<Vec<String>> {
    params
        .get("repositories")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
}

#[cfg(test)]
mod tests {
    use super::super::tests::{测试用状态, 读_json};
    use super::*;
    use crate::auth::hash_password;
    use crate::meta::Role;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// 在状态库内建一个指定角色用户并签发其会话 JWT。
    async fn 签发令牌(state: &AppState, name: &str, role: Role) -> String {
        let uid = state
            .meta
            .create_user(name, &hash_password("pw").unwrap(), role)
            .await
            .unwrap();
        state.jwt.issue(&uid, name, role).unwrap()
    }

    /// 便捷：带 Bearer 令牌 POST JSON 到某迁移端点。
    async fn 请求(
        state: AppState,
        path: &str,
        令牌: &str,
        body: serde_json::Value,
    ) -> axum::response::Response {
        let app = super::super::build_router(state);
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("Authorization", format!("Bearer {令牌}"))
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn 在线迁移_已有在途迁移时第二个_409_且不白跑上游() {
        // 预置一个在途迁移任务占用单飞门，触发在线迁移应在同步枚举前早拒 409
        let (state, _dir) = 测试用状态().await;
        state
            .tasks
            .try_begin_migration("in-flight".to_string(), Some("占位".to_string()));
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let resp = 请求(
            state,
            "/api/v1/migrate/nexus/online/migrate",
            &token,
            serde_json::json!({ "base_url": "http://127.0.0.1:0", "repositories": [{ "source": "a" }] }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = 读_json(resp).await;
        assert_eq!(body["error"]["message"], "已有迁移任务在途");
    }

    #[tokio::test]
    async fn 离线proxy搬运_已有在途迁移时第二个_409() {
        let (state, _dir) = 测试用状态().await;
        state
            .tasks
            .try_begin_migration("in-flight".to_string(), None);
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let resp = 请求(
            state,
            "/api/v1/migrate/nexus/proxy/migrate",
            &token,
            serde_json::json!({ "offline_path": "/tmp/x" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn 在线迁移_无在途时不被单飞门拦() {
        // 无在途迁移时单飞门放行，端点进入同步枚举阶段；本测试只断言「未被 409 早拒」
        //（base_url 指向不可达地址，枚举失败应是 502，而非 409）
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let resp = 请求(
            state,
            "/api/v1/migrate/nexus/online/migrate",
            &token,
            serde_json::json!({ "base_url": "http://127.0.0.1:0", "repositories": [{ "source": "a" }] }),
        )
        .await;
        assert_ne!(
            resp.status(),
            StatusCode::CONFLICT,
            "无在途迁移时不应被单飞门 409 拦截"
        );
    }

    #[tokio::test]
    async fn 离线预览_已有在途搬运时仍返回任务句柄() {
        // 离线预览只是枚举，不参与迁移搬运单飞门；已有搬运在途时也应允许预览。
        let (state, _dir) = 测试用状态().await;
        state
            .tasks
            .try_begin_migration("in-flight".to_string(), Some("离线搬运占位".to_string()));
        let blob_dir = tempfile::tempdir().unwrap();
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let resp = 请求(
            state,
            "/api/v1/migrate/nexus/offline/preview",
            &token,
            serde_json::json!({ "path": blob_dir.path().to_string_lossy() }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let body = 读_json(resp).await;
        assert!(body["job_id"].as_str().is_some());
    }

    fn 仓库(name: &str) -> migrate::NexusRepoSummary {
        migrate::NexusRepoSummary {
            name: name.to_string(),
            format: "maven2".to_string(),
            r#type: "proxy".to_string(),
            upstream_url: Some("https://repo.example".to_string()),
            group_members: vec![],
        }
    }

    #[test]
    fn 离线搬运_未传选择保持全量仓库() {
        let repos = vec![仓库("a"), 仓库("b")];
        let filtered = filter_selected_repositories(repos.clone(), None).unwrap();
        assert_eq!(filtered, repos);
    }

    #[test]
    fn 离线搬运_只保留所选仓库() {
        let filtered = filter_selected_repositories(
            vec![仓库("a"), 仓库("b"), 仓库("c")],
            Some(vec!["b".to_string(), "a".to_string()]),
        )
        .unwrap();
        let names: Vec<String> = filtered.into_iter().map(|repo| repo.name).collect();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn 迁移详情_dto_包含字节耗时失败明细与省略数() {
        let detail = MigrationTaskDetailDto {
            task: MigrationTaskDto {
                id: "t1".to_string(),
                kind: "online".to_string(),
                state: "failed".to_string(),
                label: None,
                phase: "failed".to_string(),
                total_assets: 1,
                done_assets: 1,
                migrated: 0,
                skipped_existing: 0,
                skipped: 1,
                total_bytes: 12,
                done_bytes: 12,
                migrated_bytes: 0,
                skipped_bytes: 12,
                omitted_asset_failures: 2,
                error: Some("迁移失败".to_string()),
                started_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:01Z".to_string(),
                finished_at: Some("2026-01-01T00:00:01Z".to_string()),
            },
            repos: vec![MigrationRepoReportDto {
                source_repo: "r1".to_string(),
                target_repo: "r1".to_string(),
                format: "maven".to_string(),
                created: true,
                migrated_artifacts: 0,
                skipped_existing_artifacts: 0,
                skipped_artifacts: 1,
                total_bytes: 12,
                done_bytes: 12,
                migrated_bytes: 0,
                skipped_bytes: 12,
                duration_ms: 34,
            }],
            asset_failures: vec![MigrationAssetFailureDto {
                source_repo: "r1".to_string(),
                asset: "a.jar".to_string(),
                reason: "源资产下载失败".to_string(),
            }],
        };
        let value = serde_json::to_value(detail).unwrap();
        assert_eq!(value["repos"][0]["duration_ms"], 34);
        assert_eq!(value["asset_failures"][0]["asset"], "a.jar");
        assert_eq!(value["omitted_asset_failures"], 2);
    }

    #[test]
    fn 离线搬运_空选择或未知仓库返回错误() {
        assert!(filter_selected_repositories(vec![仓库("a")], Some(vec![])).is_err());
        assert!(
            filter_selected_repositories(vec![仓库("a")], Some(vec!["missing".to_string()]))
                .is_err()
        );
    }
}
