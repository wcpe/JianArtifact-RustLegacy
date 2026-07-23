//! Nexus 迁移管理 API：来源发现、计划快照、任务控制与可核实报告。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::PageQuery;
use crate::api::extractor::RequireAdmin;
use crate::domain::migration::policy::{
    ConflictObservation, MigrationPolicyError, TemporaryCredentials, validate_conflict_decision,
    validate_temporary_credentials, validate_unique_source_decisions,
};
use crate::domain::migration::state::{TaskEvent, TaskState, TaskStateError, transition};
pub use crate::domain::migration::{
    ConflictAction, ObjectConflictDecision, SourceKind, TaskPhase, TaskStatus,
};
use crate::domain::migration::{
    DiscoveredRepository, DiscoveryIssue, DiscoverySnapshot, RepositorySourceConfig,
};
use crate::error::ApiError;
use crate::migration::MigrationRuntime;
use crate::persistence::migration::{
    self, MigrationDiscoveryInput, MigrationPlanInput, MigrationRepositoryInput,
    MigrationTaskInput, MigrationTaskRecord,
};
use crate::state::AppState;

use super::repos::{Format, RepoType};

const DISK_ESTIMATE_NUMERATOR: u64 = 11;
const DISK_ESTIMATE_DENOMINATOR: u64 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlanObject {
    source_repository: String,
    source_object_key: String,
    sha256: String,
    size: u64,
}

struct PlanInventory {
    objects: Vec<PlanObject>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ExecutableScope {
    object_count: u64,
    total_bytes: u64,
    configuration_incomplete_count: u64,
    supported_objects_not_migrated: u64,
}

pub type OptionalJson<T> = Result<Option<Json<T>>, JsonRejection>;

/// 迁移源配置。账号口令只用于本次请求及进程内任务凭据槽。
#[derive(Deserialize, ToSchema)]
pub struct MigrationSource {
    pub kind: SourceKind,
    pub path: Option<String>,
    pub url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl MigrationSource {
    pub fn take_temporary_credentials(
        &mut self,
    ) -> Result<Option<TemporaryCredentials>, MigrationPolicyError> {
        validate_temporary_credentials(self.username.take(), self.password.take())
    }
}

/// 发现到的源仓库。
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DiscoveredRepo {
    pub name: String,
    pub format: Format,
    pub repo_type: RepoType,
    pub upstream_url: Option<String>,
    pub has_credential_ref: bool,
    pub members: Option<Vec<String>>,
    pub object_count: i64,
    pub total_bytes: i64,
    pub config_complete: bool,
}

/// 发现结果。
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DiscoveryResult {
    pub discovery_id: String,
    pub snapshot_time: String,
    pub repositories: Vec<DiscoveredRepo>,
    pub unsupported: Vec<UnsupportedItem>,
    pub degraded_bare_blob: bool,
}

/// 不支持项。
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UnsupportedItem {
    pub name: String,
    pub category: String,
    pub reason: String,
}

/// 单仓库迁移映射。
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RepoMapping {
    pub source: String,
    pub target: Option<String>,
    pub excluded: bool,
    pub upstream_credential_ref: Option<String>,
    pub manual_upstream_url: Option<String>,
    pub manual_members: Option<Vec<String>>,
    #[serde(default)]
    pub conflict_decisions: Vec<ObjectConflictDecision>,
}

/// 计划请求体。
#[derive(Debug, Deserialize, ToSchema)]
pub struct PlanRequest {
    pub discovery_id: String,
    pub mappings: Vec<RepoMapping>,
}

/// 冲突条目。
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PlanConflict {
    pub target: String,
    pub path: String,
    pub category: String,
    pub decision: Option<ObjectConflictDecision>,
    pub detail: String,
}

/// 迁移计划预览。
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MigrationPlan {
    pub plan_id: String,
    pub mappings: Vec<RepoMapping>,
    pub conflicts: Vec<PlanConflict>,
    pub blocked: bool,
    pub configuration_incomplete_count: i64,
    pub supported_objects_not_migrated: i64,
    pub unresolved_conflict_count: i64,
    pub object_count: i64,
    pub total_bytes: i64,
    pub estimated_disk_bytes: i64,
    pub disk_sufficient: bool,
    pub unsupported: Vec<UnsupportedItem>,
}

/// 任务信息与进度。
#[derive(Debug, Serialize, ToSchema)]
pub struct TaskInfo {
    pub task_id: String,
    pub status: TaskStatus,
    pub phase: TaskPhase,
    pub plan_id: String,
    pub done_objects: i64,
    pub total_objects: i64,
    pub done_bytes: i64,
    pub total_bytes: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// 任务列表分页响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct TaskListResponse {
    pub items: Vec<TaskInfo>,
    pub page: u32,
    pub page_size: u32,
    pub total: i64,
}

/// 单仓库迁移报告。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RepoReport {
    pub source: String,
    pub target: String,
    pub total: i64,
    pub done: i64,
    pub migrated: i64,
    pub skipped: i64,
    pub failed: i64,
    pub total_bytes: i64,
    pub done_bytes: i64,
    pub duration_ms: i64,
    pub credential_status: String,
}

/// 校验计数。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ValidationCounts {
    pub checked: i64,
    pub passed: i64,
    pub failed: i64,
}

/// 迁移结果校验汇总。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MigrationValidation {
    pub proxy_positive_cache: ValidationCounts,
    pub blob: ValidationCounts,
    pub protocol_metadata: ValidationCounts,
}

/// 在线迁移期间的源端漂移汇总。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct OnlineDriftSummary {
    pub added: i64,
    pub modified: i64,
    pub deleted: i64,
    pub unresolved_conflict_count: i64,
    pub conflict_decisions: Vec<ObjectConflictDecision>,
}

/// 失败明细。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct FailureSample {
    pub repo: String,
    pub object_key: String,
    pub category: String,
    pub message: String,
}

/// 迁移任务报告。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MigrationReport {
    pub task_id: String,
    pub status: TaskStatus,
    pub configuration_incomplete_count: i64,
    pub supported_objects_not_migrated: i64,
    pub validation: MigrationValidation,
    pub online_drift: OnlineDriftSummary,
    pub repositories: Vec<RepoReport>,
    pub failures: Vec<FailureSample>,
    pub failures_omitted: i64,
    pub unsupported: Vec<UnsupportedItem>,
    pub excluded: Vec<String>,
}

#[derive(Deserialize)]
pub struct CreateTaskQuery {
    plan_id: String,
}

#[derive(Deserialize)]
struct TaskSnapshot {
    plan: MigrationPlan,
}

#[derive(Deserialize, ToSchema)]
pub struct RetryTaskRequest {
    pub username: Option<String>,
    pub password: Option<String>,
}

impl RetryTaskRequest {
    pub fn take_temporary_credentials(
        &mut self,
    ) -> Result<Option<TemporaryCredentials>, MigrationPolicyError> {
        validate_temporary_credentials(self.username.take(), self.password.take())
    }
}

#[utoipa::path(post, path = "/api/v1/migration/discover", tag = "migration", request_body = MigrationSource, responses((status = 200, body = DiscoveryResult), (status = 400, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)))]
pub async fn discover(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Extension(runtime): Extension<Arc<MigrationRuntime>>,
    body: Result<Json<MigrationSource>, JsonRejection>,
) -> Result<Json<DiscoveryResult>, ApiError> {
    let Json(mut source) = body.map_err(invalid_json)?;
    let credentials = source
        .take_temporary_credentials()
        .map_err(policy_bad_request)?;
    let snapshot = discover_snapshot(&state, &source, credentials.as_ref()).await?;
    let discovery_id = crate::auth::random_hex(16);
    let discovery = serde_json::to_value(&snapshot)
        .map_err(|_| ApiError::internal("序列化迁移发现结果失败"))?;
    migration::create_migration_discovery(
        &state.pool,
        MigrationDiscoveryInput {
            id: discovery_id.clone(),
            source_kind: snapshot.source_kind,
            source_instance_id: snapshot.source_instance_id.clone(),
            snapshot_id: snapshot.snapshot_id.clone(),
            snapshot_time: snapshot.snapshot_time.clone(),
            source_locator: snapshot.source_locator.clone(),
            discovery,
        },
    )
    .await?;
    if let Some(credentials) = credentials {
        runtime.store_discovery(discovery_id.clone(), credentials);
    }
    Ok(Json(discovery_result(discovery_id, &snapshot)?))
}

#[utoipa::path(post, path = "/api/v1/migration/plan", tag = "migration", request_body = PlanRequest, responses((status = 200, body = MigrationPlan), (status = 400, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)))]
pub async fn plan(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Extension(runtime): Extension<Arc<MigrationRuntime>>,
    body: Result<Json<PlanRequest>, JsonRejection>,
) -> Result<Json<MigrationPlan>, ApiError> {
    let Json(request) = body.map_err(invalid_json)?;
    let record = migration::load_migration_discovery(&state.pool, &request.discovery_id)
        .await?
        .ok_or_else(|| invalid_request("引用的迁移发现结果不存在"))?;
    let snapshot: DiscoverySnapshot = serde_json::from_value(record.discovery)
        .map_err(|_| ApiError::internal("迁移发现快照无效"))?;
    if snapshot.snapshot_id != record.snapshot_id || snapshot.source_kind != record.source_kind {
        return Err(ApiError::internal("迁移发现记录与快照不一致"));
    }
    let inventory = enumerate_plan_objects(&snapshot, &runtime, &request.discovery_id).await?;
    let plan = build_plan(&state, &snapshot, &inventory.objects, request.mappings).await?;
    let excluded_count = plan.mappings.iter().filter(|item| item.excluded).count();
    migration::create_migration_plan(
        &state.pool,
        MigrationPlanInput {
            id: plan.plan_id.clone(),
            discovery_id: request.discovery_id,
            plan: serde_json::to_value(&plan)
                .map_err(|_| ApiError::internal("序列化迁移计划失败"))?,
            blocked: plan.blocked,
            total_objects: to_u64(plan.object_count)?,
            total_bytes: to_u64(plan.total_bytes)?,
            configuration_incomplete_count: to_u64(plan.configuration_incomplete_count)?,
            supported_objects_not_migrated: to_u64(plan.supported_objects_not_migrated)?,
            excluded_repository_count: u64::try_from(excluded_count)
                .map_err(|_| ApiError::internal("迁移排除计数溢出"))?,
            unresolved_conflict_count: to_u64(plan.unresolved_conflict_count)?,
        },
    )
    .await?;
    Ok(Json(plan))
}

#[utoipa::path(post, path = "/api/v1/migration/tasks", tag = "migration", params(("plan_id" = String, Query)), responses((status = 202, body = TaskInfo), (status = 400, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)))]
pub async fn create_task(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Extension(runtime): Extension<Arc<MigrationRuntime>>,
    query: Result<Query<CreateTaskQuery>, QueryRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Query(query) = query.map_err(invalid_query)?;
    let plan_record = migration::load_migration_plan(&state.pool, &query.plan_id)
        .await?
        .ok_or_else(|| invalid_request("迁移计划不存在"))?;
    if plan_record.blocked {
        return Err(invalid_request("迁移计划存在阻断冲突"));
    }
    if migration::migration_task_exists_for_plan(&state.pool, &query.plan_id).await? {
        return Err(ApiError::conflict(
            "MIGRATION_PLAN_ALREADY_USED",
            "迁移计划已创建任务",
        ));
    }
    let discovery_record =
        migration::load_migration_discovery(&state.pool, &plan_record.discovery_id)
            .await?
            .ok_or_else(|| ApiError::internal("迁移计划引用的发现结果不存在"))?;
    let discovery: DiscoverySnapshot = serde_json::from_value(discovery_record.discovery)
        .map_err(|_| ApiError::internal("迁移发现快照无效"))?;
    let plan: MigrationPlan = serde_json::from_value(plan_record.plan)
        .map_err(|_| ApiError::internal("迁移计划快照无效"))?;
    let task_id = crate::auth::random_hex(16);
    let repositories = task_repositories(&discovery, &plan)?;
    let state_value = TaskState::new(
        discovery.source_kind,
        TaskStatus::Queued,
        TaskPhase::InitialTransfer,
        None,
    )
    .map_err(|_| ApiError::internal("迁移任务初始状态无效"))?;
    let snapshot = serde_json::to_value(TaskSnapshotRef {
        discovery: &discovery,
        plan: &plan,
        credentials_required: runtime.discovery_has_credentials(&plan_record.discovery_id),
    })
    .map_err(|_| ApiError::internal("序列化迁移任务快照失败"))?;
    let task = migration::create_migration_task(
        &state.pool,
        MigrationTaskInput {
            id: task_id.clone(),
            discovery_id: plan_record.discovery_id.clone(),
            plan_id: plan.plan_id.clone(),
            state: state_value,
            snapshot,
            total_objects: plan_record.total_objects,
            total_bytes: plan_record.total_bytes,
            configuration_incomplete_count: plan_record.configuration_incomplete_count,
            supported_objects_not_migrated: plan_record.supported_objects_not_migrated,
            excluded_repository_count: plan_record.excluded_repository_count,
            unresolved_conflict_count: plan_record.unresolved_conflict_count,
        },
        &repositories,
    )
    .await?;
    runtime.promote_to_task(&plan_record.discovery_id, task_id.clone());
    crate::migration::executor::spawn_task(state, runtime, task_id);
    Ok((StatusCode::ACCEPTED, Json(task_info(task)?)))
}

#[derive(Serialize)]
struct TaskSnapshotRef<'a> {
    discovery: &'a DiscoverySnapshot,
    plan: &'a MigrationPlan,
    credentials_required: bool,
}

#[utoipa::path(get, path = "/api/v1/migration/tasks", tag = "migration", params(("page" = Option<u32>, Query), ("page_size" = Option<u32>, Query)), responses((status = 200, body = TaskListResponse), (status = 403, body = crate::error::ErrorBody)))]
pub async fn list_tasks(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<TaskListResponse>, ApiError> {
    let Query(query) = query.map_err(invalid_query)?;
    let (page, page_size) = query.resolve();
    let (tasks, total) = migration::list_migration_tasks(&state.pool, page, page_size).await?;
    Ok(Json(TaskListResponse {
        items: tasks.into_iter().map(task_info).collect::<Result<_, _>>()?,
        page,
        page_size,
        total,
    }))
}

#[utoipa::path(get, path = "/api/v1/migration/tasks/{id}", tag = "migration", params(("id" = String, Path)), responses((status = 200, body = TaskInfo), (status = 404, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)))]
pub async fn task_detail(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TaskInfo>, ApiError> {
    Ok(Json(task_info(load_task(&state, &id).await?)?))
}

#[utoipa::path(post, path = "/api/v1/migration/tasks/{id}/pause", tag = "migration", params(("id" = String, Path)), responses((status = 202, body = TaskInfo), (status = 409, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)))]
pub async fn pause_task(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let task = apply_event(&state, &id, TaskEvent::PauseRequested).await?;
    Ok((StatusCode::ACCEPTED, Json(task_info(task)?)))
}

#[utoipa::path(post, path = "/api/v1/migration/tasks/{id}/resume", tag = "migration", params(("id" = String, Path)), responses((status = 202, body = TaskInfo), (status = 409, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)))]
pub async fn resume_task(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Extension(runtime): Extension<Arc<MigrationRuntime>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let task = apply_event(&state, &id, TaskEvent::Resume).await?;
    crate::migration::executor::spawn_task(state, runtime, id);
    Ok((StatusCode::ACCEPTED, Json(task_info(task)?)))
}

#[utoipa::path(post, path = "/api/v1/migration/tasks/{id}/cancel", tag = "migration", params(("id" = String, Path)), responses((status = 202, body = TaskInfo), (status = 409, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)))]
pub async fn cancel_task(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Extension(runtime): Extension<Arc<MigrationRuntime>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let task = apply_event(&state, &id, TaskEvent::CancelRequested).await?;
    if task.state.status == TaskStatus::Cancelled {
        runtime.remove_task(&id);
    } else if task.state.status == TaskStatus::Cancelling {
        crate::migration::executor::spawn_task(state, runtime, id);
    }
    Ok((StatusCode::ACCEPTED, Json(task_info(task)?)))
}

#[utoipa::path(post, path = "/api/v1/migration/tasks/{id}/retry", tag = "migration", params(("id" = String, Path)), request_body = Option<RetryTaskRequest>, responses((status = 202, body = TaskInfo), (status = 400, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)))]
pub async fn retry_task(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Extension(runtime): Extension<Arc<MigrationRuntime>>,
    Path(id): Path<String>,
    body: OptionalJson<RetryTaskRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let credentials = retry_credentials(body)?;
    let current = load_task(&state, &id).await?;
    if current.state.source != SourceKind::Online && credentials.is_some() {
        return Err(invalid_request("离线迁移任务不接受 Nexus 临时凭据"));
    }
    let task = apply_event(
        &state,
        &id,
        TaskEvent::Retry {
            credentials_available: task_credentials_available(
                &current,
                &runtime,
                credentials.is_some(),
            )?,
        },
    )
    .await?;
    if let Some(credentials) = credentials {
        runtime.store_task(id.clone(), credentials);
    }
    if task.state.status == TaskStatus::Running {
        crate::migration::executor::spawn_task(state, runtime, id);
    }
    Ok((StatusCode::ACCEPTED, Json(task_info(task)?)))
}

#[utoipa::path(post, path = "/api/v1/migration/tasks/{id}/finalize", tag = "migration", params(("id" = String, Path)), request_body = Option<RetryTaskRequest>, responses((status = 202, body = TaskInfo), (status = 400, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)))]
pub async fn finalize_task(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Extension(runtime): Extension<Arc<MigrationRuntime>>,
    Path(id): Path<String>,
    body: OptionalJson<RetryTaskRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let credentials = retry_credentials(body)?;
    let current = load_task(&state, &id).await?;
    let task = apply_event(
        &state,
        &id,
        TaskEvent::Finalize {
            credentials_available: task_credentials_available(
                &current,
                &runtime,
                credentials.is_some(),
            )?,
        },
    )
    .await?;
    if let Some(credentials) = credentials {
        runtime.store_task(id.clone(), credentials);
    }
    crate::migration::executor::spawn_task(state, runtime, id);
    Ok((StatusCode::ACCEPTED, Json(task_info(task)?)))
}

#[utoipa::path(get, path = "/api/v1/migration/tasks/{id}/report", tag = "migration", params(("id" = String, Path)), responses((status = 200, body = MigrationReport), (status = 404, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)))]
pub async fn task_report(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MigrationReport>, ApiError> {
    let task = load_task(&state, &id).await?;
    if let Some(report) = task.report.clone()
        && let Ok(report) = serde_json::from_value::<MigrationReport>(report)
    {
        return Ok(Json(report));
    }
    let snapshot: TaskSnapshot = serde_json::from_value(task.snapshot.clone())
        .map_err(|_| ApiError::internal("迁移任务快照无效"))?;
    let repositories = migration::load_migration_task_repositories(&state.pool, &id)
        .await?
        .into_iter()
        .map(|item| {
            Ok(RepoReport {
                target: item
                    .target_repository
                    .clone()
                    .unwrap_or_else(|| item.source_repository.clone()),
                source: item.source_repository,
                total: i64_value(item.total_objects)?,
                done: i64_value(item.done_objects)?,
                migrated: i64_value(item.migrated_objects)?,
                skipped: i64_value(item.skipped_objects)?,
                failed: i64_value(item.failed_objects)?,
                total_bytes: i64_value(item.total_bytes)?,
                done_bytes: i64_value(item.done_bytes)?,
                duration_ms: i64_value(item.duration_ms)?,
                credential_status: item.credential_status,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    let failures = migration::load_migration_failures(&state.pool, &id)
        .await?
        .into_iter()
        .map(|item| FailureSample {
            repo: item.repository.unwrap_or_default(),
            object_key: item.object_key.unwrap_or_default(),
            category: item.category,
            message: item.message,
        })
        .collect();
    let validation_counts = migration::validate_migration_object_indexes(&state.pool, &id).await?;
    let npm_validation =
        crate::persistence::load_npm_migration_validation_report(&state.pool, &id).await?;
    let decisions = snapshot
        .plan
        .mappings
        .iter()
        .flat_map(|mapping| mapping.conflict_decisions.clone())
        .collect();
    let drift = migration::load_migration_drift_counts(&state.pool, &id).await?;
    Ok(Json(MigrationReport {
        task_id: id,
        status: task.state.status,
        configuration_incomplete_count: i64_value(task.configuration_incomplete_count)?,
        supported_objects_not_migrated: i64_value(task.supported_objects_not_migrated)?,
        validation: migration_validation(validation_counts, npm_validation)?,
        online_drift: OnlineDriftSummary {
            added: i64_value(drift.added)?,
            modified: i64_value(drift.modified)?,
            deleted: i64_value(drift.deleted)?,
            unresolved_conflict_count: i64_value(task.unresolved_conflict_count)?,
            conflict_decisions: decisions,
        },
        repositories,
        failures,
        failures_omitted: i64_value(task.failures_omitted)?,
        unsupported: snapshot.plan.unsupported,
        excluded: snapshot
            .plan
            .mappings
            .into_iter()
            .filter(|mapping| mapping.excluded)
            .map(|mapping| mapping.source)
            .collect(),
    }))
}

async fn discover_snapshot(
    state: &AppState,
    source: &MigrationSource,
    credentials: Option<&TemporaryCredentials>,
) -> Result<DiscoverySnapshot, ApiError> {
    match source.kind {
        SourceKind::Online => {
            reject_path(source)?;
            let url = required_value(source.url.as_deref(), "在线来源必须提供 Nexus 地址")?;
            let client = migration_client()?;
            crate::migration::online::discover_online(&client, url, credentials)
                .await
                .map_err(|_| invalid_request("在线 Nexus 发现失败"))
        }
        SourceKind::NativeDir => {
            let path = resolve_offline_path(state, source)?;
            let configs = if let Some(url) = source.url.as_deref() {
                let client = migration_client()?;
                crate::migration::online::discover_online(&client, url, credentials)
                    .await
                    .map_err(|_| invalid_request("拉取 Nexus 仓库配置失败"))?
                    .repositories
                    .into_iter()
                    .map(source_config)
                    .collect()
            } else {
                if credentials.is_some() {
                    return Err(invalid_request("未提供 Nexus 地址时不得提交临时凭据"));
                }
                Vec::new()
            };
            tokio::task::spawn_blocking(move || {
                crate::migration::native::discover_native_directory(&path, &configs)
            })
            .await
            .map_err(|_| ApiError::internal("原生目录发现任务异常终止"))?
            .map_err(|_| invalid_request("Nexus 原生数据目录无效"))
        }
        SourceKind::Package => {
            reject_url_and_credentials(source, credentials)?;
            let path = resolve_offline_path(state, source)?;
            tokio::task::spawn_blocking(move || crate::migration::package::discover_package(&path))
                .await
                .map_err(|_| ApiError::internal("离线包发现任务异常终止"))?
                .map_err(|_| invalid_request("迁移离线包无效"))
        }
    }
}

fn resolve_offline_path(
    state: &AppState,
    source: &MigrationSource,
) -> Result<std::path::PathBuf, ApiError> {
    let relative = required_value(source.path.as_deref(), "离线来源必须提供服务器相对路径")?;
    crate::migration::path::resolve_source_directory(
        state.config.migration_workdir.as_deref(),
        relative,
    )
    .map_err(|_| invalid_request("迁移来源路径无效或不在允许工作根内"))
}

fn source_config(repository: DiscoveredRepository) -> RepositorySourceConfig {
    RepositorySourceConfig {
        name: repository.name,
        format: repository.format,
        repo_type: repository.repo_type,
        upstream_url: repository.upstream_url,
        upstream_credential_ref: repository.upstream_credential_ref,
        members: repository.members,
    }
}

async fn enumerate_plan_objects(
    snapshot: &DiscoverySnapshot,
    runtime: &MigrationRuntime,
    discovery_id: &str,
) -> Result<PlanInventory, ApiError> {
    match snapshot.source_kind {
        SourceKind::NativeDir => enumerate_native_plan_objects(snapshot).await,
        SourceKind::Package => enumerate_package_plan_objects(snapshot).await,
        SourceKind::Online => enumerate_online_plan_objects(snapshot, runtime, discovery_id).await,
    }
}

async fn enumerate_native_plan_objects(
    snapshot: &DiscoverySnapshot,
) -> Result<PlanInventory, ApiError> {
    let path = std::path::PathBuf::from(&snapshot.source_locator);
    let configs: Vec<_> = snapshot
        .repositories
        .iter()
        .cloned()
        .map(source_config)
        .collect();
    let expected = snapshot.snapshot_id.clone();
    let (current, assets) = tokio::task::spawn_blocking(move || {
        let current = crate::migration::native::discover_native_directory(&path, &configs)?;
        let assets =
            crate::migration::native::enumerate_native_assets_with_configs(&path, &configs)?;
        Ok::<_, crate::migration::native::NativeDiscoveryError>((current, assets))
    })
    .await
    .map_err(|_| ApiError::internal("原生目录计划重验任务异常终止"))?
    .map_err(|_| invalid_request("Nexus 原生数据目录计划重验失败"))?;
    verify_plan_snapshot(&expected, &current.snapshot_id)?;
    Ok(PlanInventory {
        objects: assets
            .into_iter()
            .map(|asset| PlanObject {
                source_repository: asset.repository,
                source_object_key: asset.object_key,
                sha256: asset.sha256,
                size: asset.size,
            })
            .collect(),
    })
}

async fn enumerate_package_plan_objects(
    snapshot: &DiscoverySnapshot,
) -> Result<PlanInventory, ApiError> {
    let path = std::path::PathBuf::from(&snapshot.source_locator);
    let expected = snapshot.snapshot_id.clone();
    let (current, assets) = tokio::task::spawn_blocking(move || {
        let current = crate::migration::package::discover_package(&path)?;
        let assets = crate::migration::package::enumerate_package_assets(&path)?;
        Ok::<_, crate::migration::package::PackageDiscoveryError>((current, assets))
    })
    .await
    .map_err(|_| ApiError::internal("离线包计划重验任务异常终止"))?
    .map_err(|_| invalid_request("迁移离线包计划重验失败"))?;
    verify_plan_snapshot(&expected, &current.snapshot_id)?;
    Ok(PlanInventory {
        objects: assets
            .into_iter()
            .map(|asset| PlanObject {
                source_repository: asset.repository,
                source_object_key: asset.object_key,
                sha256: asset.sha256,
                size: asset.size,
            })
            .collect(),
    })
}

async fn enumerate_online_plan_objects(
    snapshot: &DiscoverySnapshot,
    runtime: &MigrationRuntime,
    discovery_id: &str,
) -> Result<PlanInventory, ApiError> {
    let credentials = runtime.discovery_credentials(discovery_id);
    let client = migration_client()?;
    let current = crate::migration::online::discover_online_with_assets(
        &client,
        &snapshot.source_locator,
        credentials.as_deref(),
    )
    .await
    .map_err(|_| invalid_request("在线 Nexus 计划重验失败"))?;
    verify_plan_snapshot(&snapshot.snapshot_id, &current.snapshot.snapshot_id)?;
    Ok(PlanInventory {
        objects: current
            .assets
            .into_iter()
            .map(|asset| PlanObject {
                source_repository: asset.repository,
                source_object_key: asset.path,
                sha256: asset.sha256,
                size: asset.size,
            })
            .collect(),
    })
}

fn verify_plan_snapshot(expected: &str, current: &str) -> Result<(), ApiError> {
    if expected == current {
        Ok(())
    } else {
        Err(invalid_request("迁移来源已变化，请重新执行发现"))
    }
}

fn validate_mapping_coverage(
    discovered_repository_count: usize,
    mapped_repository_count: usize,
) -> Result<(), ApiError> {
    if discovered_repository_count != mapped_repository_count {
        return Err(invalid_request("每个已发现仓库都必须显式映射或标记为排除"));
    }
    Ok(())
}

fn initial_configuration_incomplete_count(snapshot: &DiscoverySnapshot) -> u64 {
    u64::from(snapshot.degraded_bare_blob)
}

fn plan_repository_statuses(
    snapshot: &DiscoverySnapshot,
    mappings: &[RepoMapping],
) -> Result<HashMap<String, bool>, ApiError> {
    let repositories: HashMap<_, _> = snapshot
        .repositories
        .iter()
        .map(|repository| (repository.name.as_str(), repository))
        .collect();
    let mapping_index: HashMap<_, _> = mappings
        .iter()
        .map(|mapping| (mapping.source.as_str(), mapping))
        .collect();
    if mapping_index.len() != mappings.len() {
        return Err(invalid_request("同一源仓库只能出现一次映射"));
    }
    let mut statuses = HashMap::new();
    for name in repositories.keys() {
        let mut visiting = HashSet::new();
        let complete = repository_config_complete(
            name,
            &repositories,
            &mapping_index,
            &mut statuses,
            &mut visiting,
        );
        statuses.insert((*name).to_string(), complete);
    }
    Ok(statuses)
}

fn repository_config_complete(
    name: &str,
    repositories: &HashMap<&str, &DiscoveredRepository>,
    mappings: &HashMap<&str, &RepoMapping>,
    statuses: &mut HashMap<String, bool>,
    visiting: &mut HashSet<String>,
) -> bool {
    if let Some(complete) = statuses.get(name) {
        return *complete;
    }
    let (Some(repository), Some(mapping)) = (repositories.get(name), mappings.get(name)) else {
        return false;
    };
    if mapping.excluded || !visiting.insert(name.to_string()) {
        return false;
    }
    let complete = local_config_complete(repository, mapping)
        && group_members_complete(
            repository,
            mapping,
            repositories,
            mappings,
            statuses,
            visiting,
        );
    visiting.remove(name);
    statuses.insert(name.to_string(), complete);
    complete
}

fn local_config_complete(repository: &DiscoveredRepository, mapping: &RepoMapping) -> bool {
    match repository.repo_type {
        RepoType::Hosted => true,
        RepoType::Proxy => {
            mapping
                .manual_upstream_url
                .as_deref()
                .or(repository.upstream_url.as_deref())
                .is_some_and(|value| !value.is_empty())
                && mapping
                    .upstream_credential_ref
                    .as_deref()
                    .or(repository.upstream_credential_ref.as_deref())
                    .map(credential_available)
                    .unwrap_or(true)
        }
        RepoType::Group => mapping
            .manual_members
            .as_ref()
            .or(repository.members.as_ref())
            .is_some_and(|members| !members.is_empty()),
    }
}

fn group_members_complete(
    repository: &DiscoveredRepository,
    mapping: &RepoMapping,
    repositories: &HashMap<&str, &DiscoveredRepository>,
    mappings: &HashMap<&str, &RepoMapping>,
    statuses: &mut HashMap<String, bool>,
    visiting: &mut HashSet<String>,
) -> bool {
    if repository.repo_type != RepoType::Group {
        return true;
    }
    mapping
        .manual_members
        .as_ref()
        .or(repository.members.as_ref())
        .is_some_and(|members| {
            members.iter().all(|member| {
                repository_config_complete(member, repositories, mappings, statuses, visiting)
            })
        })
}

fn calculate_executable_scope(
    snapshot: &DiscoverySnapshot,
    mappings: &[RepoMapping],
    statuses: &HashMap<String, bool>,
) -> Result<ExecutableScope, ApiError> {
    let repositories: HashMap<_, _> = snapshot
        .repositories
        .iter()
        .map(|repository| (repository.name.as_str(), repository))
        .collect();
    let mut scope = ExecutableScope {
        configuration_incomplete_count: initial_configuration_incomplete_count(snapshot),
        ..ExecutableScope::default()
    };
    for mapping in mappings {
        let repository = repositories
            .get(mapping.source.as_str())
            .ok_or_else(|| invalid_request("迁移映射引用了不存在的源仓库"))?;
        if mapping.excluded {
            add_not_migrated(&mut scope, repository.object_count)?;
        } else if statuses.get(&mapping.source) != Some(&true) {
            scope.configuration_incomplete_count = scope
                .configuration_incomplete_count
                .checked_add(1)
                .ok_or_else(|| ApiError::internal("迁移配置未完成计数溢出"))?;
            add_not_migrated(&mut scope, repository.object_count)?;
        } else {
            scope.object_count = scope
                .object_count
                .checked_add(repository.object_count)
                .ok_or_else(|| ApiError::internal("迁移对象计数溢出"))?;
            scope.total_bytes = scope
                .total_bytes
                .checked_add(repository.total_bytes)
                .ok_or_else(|| ApiError::internal("迁移字节计数溢出"))?;
        }
    }
    Ok(scope)
}

fn add_not_migrated(scope: &mut ExecutableScope, count: u64) -> Result<(), ApiError> {
    scope.supported_objects_not_migrated = scope
        .supported_objects_not_migrated
        .checked_add(count)
        .ok_or_else(|| ApiError::internal("迁移未执行对象计数溢出"))?;
    Ok(())
}

async fn build_plan(
    state: &AppState,
    snapshot: &DiscoverySnapshot,
    objects: &[PlanObject],
    mappings: Vec<RepoMapping>,
) -> Result<MigrationPlan, ApiError> {
    validate_plan_mappings(snapshot, &mappings)?;
    validate_plan_object_totals(snapshot, objects)?;
    let statuses = plan_repository_statuses(snapshot, &mappings)?;
    let decisions: Vec<_> = mappings
        .iter()
        .flat_map(|mapping| mapping.conflict_decisions.clone())
        .collect();
    validate_unique_source_decisions(&decisions).map_err(policy_bad_request)?;
    let (digests, existing) =
        load_plan_target_state(state, objects, &mappings, &statuses, &decisions).await?;
    validate_decisions(
        snapshot, objects, &mappings, &statuses, &decisions, &digests, &existing,
    )?;

    let mut conflicts = plan_configuration_conflicts(state, snapshot, &mappings, &statuses);
    conflicts.extend(unresolved_object_conflicts(
        objects, &mappings, &statuses, &decisions, &digests,
    ));
    let mut scope = calculate_executable_scope(snapshot, &mappings, &statuses)?;
    add_abandoned_objects(&mut scope, &decisions)?;
    create_plan_result(state, snapshot, mappings, conflicts, scope)
}

fn validate_plan_mappings(
    snapshot: &DiscoverySnapshot,
    mappings: &[RepoMapping],
) -> Result<(), ApiError> {
    if mappings.is_empty() {
        return Err(invalid_request("迁移映射不能为空"));
    }
    let repositories: HashMap<_, _> = snapshot
        .repositories
        .iter()
        .map(|repository| (repository.name.as_str(), repository))
        .collect();
    let mut sources = HashSet::new();
    let mut targets = HashSet::new();
    for mapping in mappings {
        let repository = repositories
            .get(mapping.source.as_str())
            .ok_or_else(|| invalid_request("迁移映射引用了不存在的源仓库"))?;
        if !sources.insert(mapping.source.as_str()) {
            return Err(invalid_request("同一源仓库只能出现一次映射"));
        }
        validate_repository_name(resolved_target(mapping))?;
        if !mapping.excluded && !targets.insert(resolved_target(mapping)) {
            return Err(invalid_request("多个源仓库不得映射到同一目标仓库"));
        }
        validate_mapping_config(repository, mapping, &repositories)?;
        if mapping
            .conflict_decisions
            .iter()
            .any(|decision| decision.source_repository != mapping.source)
        {
            return Err(invalid_request("冲突决策必须放在对应源仓库映射中"));
        }
    }
    validate_mapping_coverage(repositories.len(), sources.len())
}

fn validate_plan_object_totals(
    snapshot: &DiscoverySnapshot,
    objects: &[PlanObject],
) -> Result<(), ApiError> {
    let mut totals = HashMap::<&str, (u64, u64)>::new();
    let mut identities = HashSet::new();
    for object in objects {
        if !identities.insert((
            object.source_repository.as_str(),
            object.source_object_key.as_str(),
        )) {
            return Err(invalid_request("计划重枚举发现重复源对象"));
        }
        let total = totals.entry(object.source_repository.as_str()).or_default();
        total.0 = total
            .0
            .checked_add(1)
            .ok_or_else(|| ApiError::internal("迁移对象计数溢出"))?;
        total.1 = total
            .1
            .checked_add(object.size)
            .ok_or_else(|| ApiError::internal("迁移字节计数溢出"))?;
    }
    for repository in &snapshot.repositories {
        if repository.repo_type == RepoType::Group {
            continue;
        }
        let actual = totals
            .get(repository.name.as_str())
            .copied()
            .unwrap_or_default();
        if actual != (repository.object_count, repository.total_bytes) {
            return Err(invalid_request("计划重枚举对象统计与发现快照不一致"));
        }
    }
    Ok(())
}

async fn load_plan_target_state(
    state: &AppState,
    objects: &[PlanObject],
    mappings: &[RepoMapping],
    statuses: &HashMap<String, bool>,
    decisions: &[ObjectConflictDecision],
) -> Result<(BTreeMap<(String, String), String>, HashSet<String>), ApiError> {
    let mapping_index: HashMap<_, _> = mappings
        .iter()
        .map(|mapping| (mapping.source.as_str(), mapping))
        .collect();
    let mut targets: Vec<_> = objects
        .iter()
        .filter(|object| statuses.get(&object.source_repository) == Some(&true))
        .filter_map(|object| {
            mapping_index
                .get(object.source_repository.as_str())
                .map(|mapping| {
                    (
                        resolved_target(mapping).to_string(),
                        object.source_object_key.clone(),
                    )
                })
        })
        .collect();
    targets.extend(decisions.iter().map(|decision| {
        (
            decision.original_target_repository.clone(),
            decision.original_target_path.clone(),
        )
    }));
    targets.extend(decisions.iter().filter_map(|decision| {
        Some((
            decision.target_repository.clone()?,
            decision.target_path.clone()?,
        ))
    }));
    let digests = migration::target_artifact_digests(&state.pool, &targets).await?;
    let names: Vec<_> = decisions
        .iter()
        .filter_map(|decision| decision.target_repository.clone())
        .collect();
    let existing = migration::existing_repository_names(&state.pool, &names).await?;
    Ok((digests, existing))
}

fn plan_configuration_conflicts(
    state: &AppState,
    snapshot: &DiscoverySnapshot,
    mappings: &[RepoMapping],
    statuses: &HashMap<String, bool>,
) -> Vec<PlanConflict> {
    let repositories: HashMap<_, _> = snapshot
        .repositories
        .iter()
        .map(|repository| (repository.name.as_str(), repository))
        .collect();
    mappings
        .iter()
        .filter(|mapping| statuses.get(&mapping.source) == Some(&true))
        .filter_map(|mapping| {
            npm_proxy_public_base_url_conflict(
                state.config.public_base_url.as_deref(),
                repositories[mapping.source.as_str()],
                mapping,
            )
        })
        .collect()
}

fn unresolved_object_conflicts(
    objects: &[PlanObject],
    mappings: &[RepoMapping],
    statuses: &HashMap<String, bool>,
    decisions: &[ObjectConflictDecision],
    digests: &BTreeMap<(String, String), String>,
) -> Vec<PlanConflict> {
    let mapping_index: HashMap<_, _> = mappings
        .iter()
        .map(|mapping| (mapping.source.as_str(), mapping))
        .collect();
    let decided: HashSet<_> = decisions
        .iter()
        .map(|decision| {
            (
                decision.source_repository.as_str(),
                decision.source_object_key.as_str(),
            )
        })
        .collect();
    objects
        .iter()
        .filter(|object| statuses.get(&object.source_repository) == Some(&true))
        .filter(|object| {
            !decided.contains(&(
                object.source_repository.as_str(),
                object.source_object_key.as_str(),
            ))
        })
        .filter_map(|object| {
            let mapping = mapping_index.get(object.source_repository.as_str())?;
            let target = resolved_target(mapping);
            let digest = digests
                .get(&(target.to_string(), object.source_object_key.clone()))
                .map(String::as_str);
            object_conflict_candidate(object, target, digest)
        })
        .collect()
}

fn object_conflict_candidate(
    object: &PlanObject,
    target_repository: &str,
    target_sha256: Option<&str>,
) -> Option<PlanConflict> {
    let target_sha256 = target_sha256.filter(|digest| *digest != object.sha256.as_str())?;
    let decision = ObjectConflictDecision {
        source_repository: object.source_repository.clone(),
        source_object_key: object.source_object_key.clone(),
        source_sha256: object.sha256.clone(),
        original_target_repository: target_repository.to_string(),
        original_target_path: object.source_object_key.clone(),
        observed_target_sha256: target_sha256.to_string(),
        action: ConflictAction::Abandon,
        target_repository: None,
        target_path: None,
    };
    Some(PlanConflict {
        target: target_repository.to_string(),
        path: object.source_object_key.clone(),
        category: "object_conflict".to_string(),
        decision: Some(decision),
        detail: "目标路径已存在不同摘要对象，请选择重命名、映射空路径或放弃".to_string(),
    })
}

fn add_abandoned_objects(
    scope: &mut ExecutableScope,
    decisions: &[ObjectConflictDecision],
) -> Result<(), ApiError> {
    let abandoned = u64::try_from(
        decisions
            .iter()
            .filter(|decision| decision.action == ConflictAction::Abandon)
            .count(),
    )
    .map_err(|_| ApiError::internal("迁移放弃计数溢出"))?;
    add_not_migrated(scope, abandoned)
}

fn create_plan_result(
    state: &AppState,
    snapshot: &DiscoverySnapshot,
    mappings: Vec<RepoMapping>,
    conflicts: Vec<PlanConflict>,
    scope: ExecutableScope,
) -> Result<MigrationPlan, ApiError> {
    let estimated = scope
        .total_bytes
        .saturating_mul(DISK_ESTIMATE_NUMERATOR)
        .div_ceil(DISK_ESTIMATE_DENOMINATOR)
        .saturating_add(state.config.min_free_bytes);
    let available = fs4::available_space(&state.config.data_dir)
        .map_err(|_| ApiError::internal("读取迁移目标磁盘空间失败"))?;
    Ok(MigrationPlan {
        plan_id: crate::auth::random_hex(16),
        blocked: !conflicts.is_empty(),
        unresolved_conflict_count: i64_value(
            u64::try_from(conflicts.len()).map_err(|_| ApiError::internal("迁移冲突计数溢出"))?,
        )?,
        conflicts,
        mappings,
        configuration_incomplete_count: i64_value(scope.configuration_incomplete_count)?,
        supported_objects_not_migrated: i64_value(scope.supported_objects_not_migrated)?,
        object_count: i64_value(scope.object_count)?,
        total_bytes: i64_value(scope.total_bytes)?,
        estimated_disk_bytes: i64_value(estimated)?,
        disk_sufficient: available >= estimated,
        unsupported: snapshot.unsupported.iter().map(unsupported_item).collect(),
    })
}

fn npm_proxy_public_base_url_conflict(
    public_base_url: Option<&str>,
    repository: &DiscoveredRepository,
    mapping: &RepoMapping,
) -> Option<PlanConflict> {
    let required = !mapping.excluded
        && repository.format == Format::Npm
        && repository.repo_type == RepoType::Proxy
        && repository.object_count > 0;
    if !required || public_base_url.is_some_and(is_valid_public_base_url) {
        return None;
    }
    Some(PlanConflict {
        target: resolved_target(mapping).to_string(),
        path: String::new(),
        category: "公开基础地址配置无效".to_string(),
        decision: None,
        detail: "迁移有对象的 npm proxy 仓库需要有效的 public_base_url".to_string(),
    })
}

pub(crate) fn is_valid_public_base_url(value: &str) -> bool {
    if value.is_empty() || value != value.trim() || value.chars().any(char::is_control) {
        return false;
    }
    let authority = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"));
    if authority.is_none_or(|value| value.is_empty() || value.starts_with('/')) {
        return false;
    }
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.has_host()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && !url.cannot_be_a_base()
}

fn validate_decisions(
    snapshot: &DiscoverySnapshot,
    objects: &[PlanObject],
    mappings: &[RepoMapping],
    statuses: &HashMap<String, bool>,
    decisions: &[ObjectConflictDecision],
    digests: &BTreeMap<(String, String), String>,
    existing: &HashSet<String>,
) -> Result<(), ApiError> {
    let repositories: HashMap<_, _> = snapshot
        .repositories
        .iter()
        .map(|repository| (repository.name.as_str(), repository))
        .collect();
    let mapping_index: HashMap<_, _> = mappings
        .iter()
        .map(|mapping| (mapping.source.as_str(), mapping))
        .collect();
    let source_objects: HashMap<_, _> = objects
        .iter()
        .map(|object| {
            (
                (
                    object.source_repository.as_str(),
                    object.source_object_key.as_str(),
                ),
                object,
            )
        })
        .collect();
    for decision in decisions {
        let mapping = mapping_index
            .get(decision.source_repository.as_str())
            .ok_or_else(|| invalid_request("冲突决策引用了未映射的源仓库"))?;
        let repository = repositories
            .get(decision.source_repository.as_str())
            .ok_or_else(|| invalid_request("冲突决策引用了不存在的源仓库"))?;
        let object = source_objects
            .get(&(
                decision.source_repository.as_str(),
                decision.source_object_key.as_str(),
            ))
            .ok_or_else(|| invalid_request("冲突决策引用了不存在的源对象"))?;
        validate_decision_identity(decision, mapping, object, statuses)?;
        validate_decision_target_syntax(decision)?;
        validate_protocol_decision(repository.format, decision)?;
        let original_key = (
            decision.original_target_repository.clone(),
            decision.original_target_path.clone(),
        );
        let original_digest = digests
            .get(&original_key)
            .ok_or_else(|| invalid_request("冲突决策引用的原目标当前不存在"))?;
        if original_digest == &object.sha256 {
            return Err(invalid_request("同摘要目标对象不构成真实冲突"));
        }
        let target_digest = decision_target_digest(decision, digests);
        validate_conflict_decision(
            decision,
            ConflictObservation {
                current_source_sha256: &object.sha256,
                current_original_target_sha256: Some(original_digest),
                target_repository_exists: decision
                    .target_repository
                    .as_deref()
                    .is_some_and(|repository| existing.contains(repository)),
                current_target_sha256: target_digest,
            },
        )
        .map_err(policy_bad_request)?;
    }
    Ok(())
}

fn validate_decision_identity(
    decision: &ObjectConflictDecision,
    mapping: &RepoMapping,
    object: &PlanObject,
    statuses: &HashMap<String, bool>,
) -> Result<(), ApiError> {
    if statuses.get(&decision.source_repository) != Some(&true) {
        return Err(invalid_request(
            "配置不完整或已排除仓库不得提交对象冲突决策",
        ));
    }
    if decision.source_sha256 != object.sha256 {
        return Err(invalid_request("冲突决策的源对象摘要与计划重枚举不一致"));
    }
    if decision.original_target_repository != original_target(mapping) {
        return Err(invalid_request("冲突决策的原目标仓库与映射不一致"));
    }
    if decision.original_target_path != object.source_object_key {
        return Err(invalid_request("冲突决策的原目标路径与源对象键不一致"));
    }
    Ok(())
}

fn validate_decision_target_syntax(decision: &ObjectConflictDecision) -> Result<(), ApiError> {
    if let Some(repository) = decision.target_repository.as_deref() {
        validate_repository_name(repository)?;
    }
    if let Some(path) = decision.target_path.as_deref() {
        crate::domain::artifact::ArtifactPath::parse(path)
            .map_err(|_| invalid_request("冲突决策的最终目标路径无效"))?;
    }
    Ok(())
}

fn validate_protocol_decision(
    format: Format,
    decision: &ObjectConflictDecision,
) -> Result<(), ApiError> {
    if format == Format::Raw || decision.action == ConflictAction::Abandon {
        return Ok(());
    }
    Err(invalid_request(
        "npm 与 Maven 对象冲突不得跨仓库重定向或改写协议路径",
    ))
}

fn decision_target_digest<'a>(
    decision: &ObjectConflictDecision,
    digests: &'a BTreeMap<(String, String), String>,
) -> Option<&'a str> {
    let (Some(repository), Some(path)) = (&decision.target_repository, &decision.target_path)
    else {
        return None;
    };
    digests
        .get(&(repository.clone(), path.clone()))
        .map(String::as_str)
}

fn validate_mapping_config(
    repository: &DiscoveredRepository,
    mapping: &RepoMapping,
    repositories: &HashMap<&str, &DiscoveredRepository>,
) -> Result<(), ApiError> {
    if let Some(reference) = mapping.upstream_credential_ref.as_deref() {
        validate_credential_ref(reference)?;
    }
    if let Some(url) = mapping.manual_upstream_url.as_deref() {
        validate_upstream_url(url)?;
    }
    if let Some(members) = mapping.manual_members.as_deref() {
        let unique: HashSet<_> = members.iter().map(String::as_str).collect();
        if members.is_empty() || unique.len() != members.len() {
            return Err(invalid_request("group 手动成员不能为空或重复"));
        }
        if members
            .iter()
            .any(|member| !repositories.contains_key(member.as_str()))
        {
            return Err(invalid_request("group 手动成员必须来自本次发现快照"));
        }
        if repository.repo_type == RepoType::Group
            && repository
                .members
                .as_deref()
                .is_some_and(|authoritative| authoritative != members)
        {
            return Err(invalid_request("存在权威 group 成员时不得手动改写成员"));
        }
    }
    match repository.repo_type {
        RepoType::Hosted
            if mapping.manual_upstream_url.is_some()
                || mapping.manual_members.is_some()
                || mapping.upstream_credential_ref.is_some() =>
        {
            Err(invalid_request("hosted 仓库不得携带 proxy 或 group 配置"))
        }
        RepoType::Proxy if mapping.manual_members.is_some() => {
            Err(invalid_request("proxy 仓库不得携带 group 成员"))
        }
        RepoType::Group
            if mapping.manual_upstream_url.is_some()
                || mapping.upstream_credential_ref.is_some() =>
        {
            Err(invalid_request("group 仓库不得携带 proxy 配置"))
        }
        _ => Ok(()),
    }
}

fn task_repositories(
    discovery: &DiscoverySnapshot,
    plan: &MigrationPlan,
) -> Result<Vec<MigrationRepositoryInput>, ApiError> {
    let repositories: HashMap<_, _> = discovery
        .repositories
        .iter()
        .map(|repository| (repository.name.as_str(), repository))
        .collect();
    let statuses = plan_repository_statuses(discovery, &plan.mappings)?;
    plan.mappings
        .iter()
        .map(|mapping| {
            let repository = repositories
                .get(mapping.source.as_str())
                .ok_or_else(|| ApiError::internal("迁移计划引用的源仓库不存在"))?;
            let config_complete = statuses.get(&mapping.source) == Some(&true);
            Ok(MigrationRepositoryInput {
                source_repository: mapping.source.clone(),
                target_repository: (!mapping.excluded)
                    .then(|| resolved_target(mapping).to_string()),
                excluded: mapping.excluded,
                credential_status: repository_credential_status(repository, mapping),
                config_complete,
                total_objects: if config_complete {
                    repository.object_count
                } else {
                    0
                },
                total_bytes: if config_complete {
                    repository.total_bytes
                } else {
                    0
                },
            })
        })
        .collect()
}

fn repository_credential_status(
    repository: &DiscoveredRepository,
    mapping: &RepoMapping,
) -> String {
    if repository.repo_type != RepoType::Proxy {
        return "不适用".to_string();
    }
    if mapping
        .upstream_credential_ref
        .as_ref()
        .or(repository.upstream_credential_ref.as_ref())
        .is_some()
    {
        "已映射"
    } else {
        "未映射"
    }
    .to_string()
}

async fn load_task(state: &AppState, id: &str) -> Result<MigrationTaskRecord, ApiError> {
    migration::load_migration_task(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("MIGRATION_TASK_NOT_FOUND", "迁移任务不存在"))
}

fn task_credentials_available(
    task: &MigrationTaskRecord,
    runtime: &MigrationRuntime,
    submitted: bool,
) -> Result<bool, ApiError> {
    if task.state.source != SourceKind::Online {
        return Ok(true);
    }
    let required = task
        .snapshot
        .get("credentials_required")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Ok(!required || submitted || runtime.has_task(&task.id))
}

async fn apply_event(
    state: &AppState,
    id: &str,
    event: TaskEvent,
) -> Result<MigrationTaskRecord, ApiError> {
    for _ in 0..4 {
        let current = load_task(state, id).await?;
        let next = transition(current.state, event).map_err(state_error)?;
        if migration::compare_and_set_migration_task_state(&state.pool, id, current.state, next)
            .await?
        {
            return load_task(state, id).await;
        }
    }
    Err(ApiError::conflict(
        "MIGRATION_STATE_CHANGED",
        "迁移任务状态已并发变化，请刷新后重试",
    ))
}

fn task_info(task: MigrationTaskRecord) -> Result<TaskInfo, ApiError> {
    Ok(TaskInfo {
        task_id: task.id,
        status: task.state.status,
        phase: task.state.phase,
        plan_id: task.plan_id,
        done_objects: i64_value(task.done_objects)?,
        total_objects: i64_value(task.total_objects)?,
        done_bytes: i64_value(task.done_bytes)?,
        total_bytes: i64_value(task.total_bytes)?,
        created_at: task.created_at,
        updated_at: task.updated_at,
    })
}

fn discovery_result(id: String, snapshot: &DiscoverySnapshot) -> Result<DiscoveryResult, ApiError> {
    Ok(DiscoveryResult {
        discovery_id: id,
        snapshot_time: snapshot.snapshot_time.clone(),
        repositories: snapshot
            .repositories
            .iter()
            .map(|repository| {
                Ok(DiscoveredRepo {
                    name: repository.name.clone(),
                    format: repository.format,
                    repo_type: repository.repo_type,
                    upstream_url: repository.upstream_url.clone(),
                    has_credential_ref: repository.upstream_credential_ref.is_some(),
                    members: repository.members.clone(),
                    object_count: i64_value(repository.object_count)?,
                    total_bytes: i64_value(repository.total_bytes)?,
                    config_complete: repository.config_complete,
                })
            })
            .collect::<Result<_, ApiError>>()?,
        unsupported: snapshot.unsupported.iter().map(unsupported_item).collect(),
        degraded_bare_blob: snapshot.degraded_bare_blob,
    })
}

fn unsupported_item(issue: &DiscoveryIssue) -> UnsupportedItem {
    UnsupportedItem {
        name: issue.name.clone(),
        category: issue.category.clone(),
        reason: issue.reason.clone(),
    }
}

fn original_target(mapping: &RepoMapping) -> &str {
    mapping.target.as_deref().unwrap_or(&mapping.source)
}

fn resolved_target(mapping: &RepoMapping) -> &str {
    mapping
        .conflict_decisions
        .iter()
        .find(|decision| decision.action == ConflictAction::Rename)
        .and_then(|decision| decision.target_repository.as_deref())
        .unwrap_or_else(|| original_target(mapping))
}

fn validate_repository_name(name: &str) -> Result<(), ApiError> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    valid
        .then_some(())
        .ok_or_else(|| invalid_request("目标仓库名必须为 1 至 64 位小写字母、数字或连字符"))
}

fn validate_upstream_url(value: &str) -> Result<(), ApiError> {
    let valid = value == value.trim()
        && !value.chars().any(char::is_control)
        && !value.contains('#')
        && (value.starts_with("http://") || value.starts_with("https://"))
        && !value.split("//").nth(1).unwrap_or_default().contains('@');
    valid
        .then_some(())
        .ok_or_else(|| invalid_request("proxy 上游地址格式无效"))
}

fn validate_credential_ref(reference: &str) -> Result<(), ApiError> {
    let mut bytes = reference.bytes();
    let valid = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    valid
        .then_some(())
        .ok_or_else(|| invalid_request("上游凭据引用名不是合法环境变量名"))
}

fn credential_available(reference: &str) -> bool {
    std::env::var(reference).is_ok_and(|value| {
        !value.chars().any(char::is_control)
            && value
                .strip_prefix("Basic ")
                .or_else(|| value.strip_prefix("Bearer "))
                .is_some_and(|credential| {
                    !credential.is_empty() && !credential.chars().any(char::is_whitespace)
                })
    })
}

fn migration_validation(
    counts: migration::MigrationValidationCounts,
    npm: Option<crate::persistence::NpmMigrationValidationReport>,
) -> Result<MigrationValidation, ApiError> {
    let npm = npm.unwrap_or_default();
    Ok(MigrationValidation {
        proxy_positive_cache: npm_validation_counts(npm.proxy_positive_cache)?,
        blob: ValidationCounts {
            checked: i64_value(counts.checked)?,
            passed: i64_value(counts.passed)?,
            failed: i64_value(counts.failed)?,
        },
        protocol_metadata: npm_validation_counts(npm.protocol_metadata)?,
    })
}

fn npm_validation_counts(
    counts: crate::persistence::NpmMigrationValidationCounts,
) -> Result<ValidationCounts, ApiError> {
    Ok(ValidationCounts {
        checked: i64_value(counts.checked)?,
        passed: i64_value(counts.passed)?,
        failed: i64_value(counts.failed)?,
    })
}

fn migration_client() -> Result<reqwest::Client, ApiError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| ApiError::internal("创建 Nexus 迁移客户端失败"))
}

fn reject_path(source: &MigrationSource) -> Result<(), ApiError> {
    if source.path.is_some() {
        Err(invalid_request("在线来源不得提供服务器路径"))
    } else {
        Ok(())
    }
}

fn reject_url_and_credentials(
    source: &MigrationSource,
    credentials: Option<&TemporaryCredentials>,
) -> Result<(), ApiError> {
    if source.url.is_some() || credentials.is_some() {
        Err(invalid_request("离线包来源不得提供 Nexus 地址或临时凭据"))
    } else {
        Ok(())
    }
}

fn required_value<'a>(value: Option<&'a str>, message: &'static str) -> Result<&'a str, ApiError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid_request(message))
}

fn retry_credentials(
    body: OptionalJson<RetryTaskRequest>,
) -> Result<Option<TemporaryCredentials>, ApiError> {
    let mut request = body
        .map_err(invalid_json)?
        .map(|Json(value)| value)
        .unwrap_or(RetryTaskRequest {
            username: None,
            password: None,
        });
    request
        .take_temporary_credentials()
        .map_err(policy_bad_request)
}

fn invalid_json(_: JsonRejection) -> ApiError {
    invalid_request("迁移请求字段或 JSON 格式无效")
}

fn invalid_query(_: QueryRejection) -> ApiError {
    invalid_request("迁移查询参数无效")
}

fn invalid_request(message: impl Into<String>) -> ApiError {
    ApiError::bad_request("MIGRATION_INVALID_REQUEST", message)
}

fn policy_bad_request(error: MigrationPolicyError) -> ApiError {
    invalid_request(error.to_string())
}

fn state_error(error: TaskStateError) -> ApiError {
    ApiError::conflict(error.code(), error.to_string())
}

fn i64_value(value: u64) -> Result<i64, ApiError> {
    i64::try_from(value).map_err(|_| ApiError::internal("迁移计数超过接口上限"))
}

fn to_u64(value: i64) -> Result<u64, ApiError> {
    u64::try_from(value).map_err(|_| ApiError::internal("迁移计数无效"))
}

#[cfg(test)]
mod tests {
    use super::{
        ConflictAction, DiscoveredRepository, Format, ObjectConflictDecision, PlanObject,
        RepoMapping, RepoType, RetryTaskRequest, TaskPhase, TaskStatus, calculate_executable_scope,
        initial_configuration_incomplete_count, is_valid_public_base_url, migration_validation,
        npm_proxy_public_base_url_conflict, object_conflict_candidate, plan_repository_statuses,
        retry_credentials, validate_decisions, validate_mapping_config, validate_mapping_coverage,
        validate_protocol_decision,
    };
    use crate::domain::migration::{DiscoverySnapshot, SourceKind};
    use crate::persistence::migration::MigrationValidationCounts;
    use crate::persistence::{NpmMigrationValidationCounts, NpmMigrationValidationReport};
    use axum::Json;

    #[test]
    fn 任务状态保持十态且序列化名称稳定() -> Result<(), serde_json::Error> {
        let statuses = [
            TaskStatus::Queued,
            TaskStatus::Running,
            TaskStatus::Pausing,
            TaskStatus::Paused,
            TaskStatus::Cancelling,
            TaskStatus::Cancelled,
            TaskStatus::Succeeded,
            TaskStatus::Partial,
            TaskStatus::Failed,
            TaskStatus::Interrupted,
        ];
        assert_eq!(statuses.len(), 10);
        assert_eq!(
            serde_json::to_value(statuses)?,
            serde_json::json!([
                "queued",
                "running",
                "pausing",
                "paused",
                "cancelling",
                "cancelled",
                "succeeded",
                "partial",
                "failed",
                "interrupted"
            ])
        );
        Ok(())
    }

    #[test]
    fn 阶段与冲突动作序列化名称稳定() -> Result<(), serde_json::Error> {
        assert_eq!(
            serde_json::to_value([
                TaskPhase::InitialTransfer,
                TaskPhase::AwaitingCutover,
                TaskPhase::Finalizing,
                TaskPhase::Completed
            ])?,
            serde_json::json!([
                "initial-transfer",
                "awaiting-cutover",
                "finalizing",
                "completed"
            ])
        );
        assert_eq!(
            serde_json::to_value([
                ConflictAction::Rename,
                ConflictAction::MapEmpty,
                ConflictAction::Abandon
            ])?,
            serde_json::json!(["rename", "map-empty", "abandon"])
        );
        Ok(())
    }

    #[test]
    fn 迁移计划要求显式覆盖全部发现仓库() {
        assert!(validate_mapping_coverage(2, 2).is_ok());
        assert!(validate_mapping_coverage(2, 1).is_err());
    }

    fn npm_proxy(object_count: u64) -> DiscoveredRepository {
        DiscoveredRepository {
            name: "npm-proxy".to_string(),
            format: Format::Npm,
            repo_type: RepoType::Proxy,
            upstream_url: Some("https://registry.npmjs.org".to_string()),
            upstream_credential_ref: None,
            members: None,
            object_count,
            total_bytes: object_count,
            config_complete: true,
        }
    }

    fn mapping(excluded: bool) -> RepoMapping {
        RepoMapping {
            source: "npm-proxy".to_string(),
            target: None,
            excluded,
            upstream_credential_ref: None,
            manual_upstream_url: None,
            manual_members: None,
            conflict_decisions: Vec::new(),
        }
    }

    #[test]
    fn npm_proxy有对象时缺失或空白公开地址阻断计划() {
        let repository = npm_proxy(1);
        let mapping = mapping(false);

        assert!(npm_proxy_public_base_url_conflict(None, &repository, &mapping).is_some());
        assert!(npm_proxy_public_base_url_conflict(Some("   "), &repository, &mapping).is_some());
    }

    #[test]
    fn npm_proxy有对象时非法公开地址阻断计划() {
        let repository = npm_proxy(1);
        let mapping = mapping(false);

        for value in [
            "ftp://packages.example/base",
            "https:///base",
            "https://user@packages.example/base",
            "https://packages.example/base#fragment",
        ] {
            assert!(!is_valid_public_base_url(value));
            assert!(
                npm_proxy_public_base_url_conflict(Some(value), &repository, &mapping).is_some()
            );
        }
    }

    #[test]
    fn npm_proxy有效基础路径公开地址不阻断计划() {
        let repository = npm_proxy(1);
        let mapping = mapping(false);
        let value = "https://packages.example/jianartifact/base";

        assert!(is_valid_public_base_url(value));
        assert!(npm_proxy_public_base_url_conflict(Some(value), &repository, &mapping).is_none());
    }

    #[test]
    fn npm_proxy无对象时不要求公开地址() {
        assert!(npm_proxy_public_base_url_conflict(None, &npm_proxy(0), &mapping(false)).is_none());
    }

    #[test]
    fn 原生降级发现计入配置未完成证据() {
        let snapshot = DiscoverySnapshot {
            source_kind: SourceKind::NativeDir,
            source_instance_id: "source".to_string(),
            snapshot_id: "snapshot".to_string(),
            snapshot_time: "0".to_string(),
            source_locator: "source".to_string(),
            repositories: Vec::new(),
            unsupported: Vec::new(),
            degraded_bare_blob: true,
        };

        assert_eq!(initial_configuration_incomplete_count(&snapshot), 1);
    }

    #[test]
    fn 手动成员不得绕过权威group成员排除() {
        let repository = DiscoveredRepository {
            name: "raw-group".to_string(),
            format: Format::Raw,
            repo_type: RepoType::Group,
            upstream_url: None,
            upstream_credential_ref: None,
            members: Some(vec!["raw-a".to_string(), "raw-b".to_string()]),
            object_count: 0,
            total_bytes: 0,
            config_complete: true,
        };
        let raw_a = DiscoveredRepository {
            name: "raw-a".to_string(),
            format: Format::Raw,
            repo_type: RepoType::Hosted,
            upstream_url: None,
            upstream_credential_ref: None,
            members: None,
            object_count: 0,
            total_bytes: 0,
            config_complete: true,
        };
        let raw_b = DiscoveredRepository {
            name: "raw-b".to_string(),
            ..raw_a.clone()
        };
        let repositories = std::collections::HashMap::from([
            (repository.name.as_str(), &repository),
            (raw_a.name.as_str(), &raw_a),
            (raw_b.name.as_str(), &raw_b),
        ]);
        let mapping = RepoMapping {
            source: repository.name.clone(),
            target: None,
            excluded: false,
            upstream_credential_ref: None,
            manual_upstream_url: None,
            manual_members: Some(vec!["raw-a".to_string()]),
            conflict_decisions: Vec::new(),
        };

        assert!(validate_mapping_config(&repository, &mapping, &repositories).is_err());
    }

    #[test]
    fn 来源凭据引用缺失时proxy配置不完整并退出执行范围() {
        let repository = DiscoveredRepository {
            name: "raw-proxy".to_string(),
            format: Format::Raw,
            repo_type: RepoType::Proxy,
            upstream_url: Some("https://packages.example".to_string()),
            upstream_credential_ref: Some(
                "JIANARTIFACT_TEST_MISSING_UPSTREAM_CREDENTIAL_7D28B6C1".to_string(),
            ),
            members: None,
            object_count: 3,
            total_bytes: 30,
            config_complete: true,
        };
        let snapshot = DiscoverySnapshot {
            source_kind: SourceKind::Package,
            source_instance_id: "source".to_string(),
            snapshot_id: "snapshot".to_string(),
            snapshot_time: "0".to_string(),
            source_locator: "package".to_string(),
            repositories: vec![repository.clone()],
            unsupported: Vec::new(),
            degraded_bare_blob: false,
        };
        let mappings = vec![mapping_for(&repository.name, false)];

        let statuses = plan_repository_statuses(&snapshot, &mappings);
        assert!(statuses.is_ok());
        let Ok(statuses) = statuses else {
            return;
        };
        assert_eq!(statuses.get(&repository.name), Some(&false));

        let scope = calculate_executable_scope(&snapshot, &mappings, &statuses);
        assert!(scope.is_ok());
        let Ok(scope) = scope else {
            return;
        };
        assert_eq!(scope.configuration_incomplete_count, 1);
        assert_eq!(scope.supported_objects_not_migrated, 3);
        assert_eq!((scope.object_count, scope.total_bytes), (0, 0));
    }

    #[test]
    fn 配置不完整传播到group并从可执行范围剔除() {
        let hosted = DiscoveredRepository {
            name: "raw-hosted".to_string(),
            format: Format::Raw,
            repo_type: RepoType::Hosted,
            upstream_url: None,
            upstream_credential_ref: None,
            members: None,
            object_count: 2,
            total_bytes: 20,
            config_complete: true,
        };
        let proxy = DiscoveredRepository {
            name: "raw-proxy".to_string(),
            format: Format::Raw,
            repo_type: RepoType::Proxy,
            upstream_url: None,
            upstream_credential_ref: None,
            members: None,
            object_count: 3,
            total_bytes: 30,
            config_complete: false,
        };
        let group = DiscoveredRepository {
            name: "raw-group".to_string(),
            format: Format::Raw,
            repo_type: RepoType::Group,
            upstream_url: None,
            upstream_credential_ref: None,
            members: Some(vec![hosted.name.clone(), proxy.name.clone()]),
            object_count: 4,
            total_bytes: 40,
            config_complete: true,
        };
        let snapshot = DiscoverySnapshot {
            source_kind: SourceKind::Package,
            source_instance_id: "source".to_string(),
            snapshot_id: "snapshot".to_string(),
            snapshot_time: "0".to_string(),
            source_locator: "package".to_string(),
            repositories: vec![hosted.clone(), proxy.clone(), group.clone()],
            unsupported: Vec::new(),
            degraded_bare_blob: false,
        };
        let mappings = vec![
            mapping_for(&hosted.name, false),
            mapping_for(&proxy.name, false),
            mapping_for(&group.name, false),
        ];

        let statuses = plan_repository_statuses(&snapshot, &mappings);
        assert!(statuses.is_ok());
        let Ok(statuses) = statuses else {
            return;
        };
        assert_eq!(statuses.get(&hosted.name), Some(&true));
        assert_eq!(statuses.get(&proxy.name), Some(&false));
        assert_eq!(statuses.get(&group.name), Some(&false));

        let scope = calculate_executable_scope(&snapshot, &mappings, &statuses);
        assert!(scope.is_ok());
        let Ok(scope) = scope else {
            return;
        };
        assert_eq!(scope.object_count, 2);
        assert_eq!(scope.total_bytes, 20);
        assert_eq!(scope.configuration_incomplete_count, 2);
        assert_eq!(scope.supported_objects_not_migrated, 7);
    }

    #[test]
    fn group引用被排除成员时仅标记配置不完整() {
        let member = DiscoveredRepository {
            name: "raw-member".to_string(),
            format: Format::Raw,
            repo_type: RepoType::Hosted,
            upstream_url: None,
            upstream_credential_ref: None,
            members: None,
            object_count: 1,
            total_bytes: 10,
            config_complete: true,
        };
        let group = DiscoveredRepository {
            name: "raw-group".to_string(),
            format: Format::Raw,
            repo_type: RepoType::Group,
            upstream_url: None,
            upstream_credential_ref: None,
            members: Some(vec![member.name.clone()]),
            object_count: 0,
            total_bytes: 0,
            config_complete: true,
        };
        let snapshot = DiscoverySnapshot {
            source_kind: SourceKind::Package,
            source_instance_id: "source".to_string(),
            snapshot_id: "snapshot".to_string(),
            snapshot_time: "0".to_string(),
            source_locator: "package".to_string(),
            repositories: vec![member.clone(), group.clone()],
            unsupported: Vec::new(),
            degraded_bare_blob: false,
        };
        let mappings = vec![
            mapping_for(&member.name, true),
            mapping_for(&group.name, false),
        ];

        let statuses = plan_repository_statuses(&snapshot, &mappings);
        assert!(matches!(statuses, Ok(values) if values.get(&group.name) == Some(&false)));
    }

    #[test]
    fn 默认目标缺失或同摘要不冲突_异摘要生成兼容候选() {
        let object = PlanObject {
            source_repository: "raw-source".to_string(),
            source_object_key: "dir/a.bin".to_string(),
            sha256: "a".repeat(64),
            size: 10,
        };
        assert!(object_conflict_candidate(&object, "raw-target", None).is_none());
        assert!(object_conflict_candidate(&object, "raw-target", Some(&object.sha256)).is_none());

        let conflict = object_conflict_candidate(&object, "raw-target", Some(&"b".repeat(64)));
        assert!(conflict.is_some());
        let Some(conflict) = conflict else {
            return;
        };
        assert_eq!(conflict.category, "object_conflict");
        assert_eq!(conflict.target, "raw-target");
        assert_eq!(conflict.path, object.source_object_key);
        assert!(matches!(
            conflict.decision,
            Some(ObjectConflictDecision {
                action: ConflictAction::Abandon,
                target_repository: None,
                target_path: None,
                ..
            })
        ));
    }

    #[test]
    fn 冲突决策必须匹配真实源对象与当前原目标摘要() {
        let repository = DiscoveredRepository {
            name: "raw-source".to_string(),
            format: Format::Raw,
            repo_type: RepoType::Hosted,
            upstream_url: None,
            upstream_credential_ref: None,
            members: None,
            object_count: 1,
            total_bytes: 10,
            config_complete: true,
        };
        let snapshot = DiscoverySnapshot {
            source_kind: SourceKind::Package,
            source_instance_id: "source".to_string(),
            snapshot_id: "snapshot".to_string(),
            snapshot_time: "0".to_string(),
            source_locator: "package".to_string(),
            repositories: vec![repository.clone()],
            unsupported: Vec::new(),
            degraded_bare_blob: false,
        };
        let mapping = mapping_for(&repository.name, false);
        let object = PlanObject {
            source_repository: repository.name.clone(),
            source_object_key: "dir/a.bin".to_string(),
            sha256: "a".repeat(64),
            size: 10,
        };
        let conflict = object_conflict_candidate(&object, &repository.name, Some(&"b".repeat(64)));
        let Some(conflict) = conflict else {
            return;
        };
        let Some(decision) = conflict.decision else {
            return;
        };
        let statuses = std::collections::HashMap::from([(repository.name.clone(), true)]);
        let digests = std::collections::BTreeMap::from([(
            (repository.name.clone(), object.source_object_key.clone()),
            "b".repeat(64),
        )]);

        assert!(
            validate_decisions(
                &snapshot,
                std::slice::from_ref(&object),
                std::slice::from_ref(&mapping),
                &statuses,
                std::slice::from_ref(&decision),
                &digests,
                &std::collections::HashSet::new(),
            )
            .is_ok()
        );

        let mut stale = decision;
        stale.source_sha256 = "c".repeat(64);
        assert!(
            validate_decisions(
                &snapshot,
                &[object],
                &[mapping],
                &statuses,
                &[stale],
                &digests,
                &std::collections::HashSet::new(),
            )
            .is_err()
        );
    }

    #[test]
    fn raw允许三种动作_npm与maven拒绝协议破坏重定向() {
        for action in [
            ConflictAction::Rename,
            ConflictAction::MapEmpty,
            ConflictAction::Abandon,
        ] {
            assert!(validate_protocol_decision(Format::Raw, &decision(action)).is_ok());
        }
        for format in [Format::Npm, Format::Maven] {
            assert!(validate_protocol_decision(format, &decision(ConflictAction::Rename)).is_err());
            assert!(
                validate_protocol_decision(format, &decision(ConflictAction::MapEmpty)).is_err()
            );
            assert!(validate_protocol_decision(format, &decision(ConflictAction::Abandon)).is_ok());
        }
    }

    fn mapping_for(source: &str, excluded: bool) -> RepoMapping {
        RepoMapping {
            source: source.to_string(),
            target: None,
            excluded,
            upstream_credential_ref: None,
            manual_upstream_url: None,
            manual_members: None,
            conflict_decisions: Vec::new(),
        }
    }

    fn decision(action: ConflictAction) -> ObjectConflictDecision {
        let redirected = action != ConflictAction::Abandon;
        ObjectConflictDecision {
            source_repository: "source".to_string(),
            source_object_key: "dir/a.bin".to_string(),
            source_sha256: "a".repeat(64),
            original_target_repository: "target".to_string(),
            original_target_path: "dir/a.bin".to_string(),
            observed_target_sha256: "b".repeat(64),
            action,
            target_repository: redirected.then(|| "redirected".to_string()),
            target_path: redirected.then(|| "dir/other.bin".to_string()),
        }
    }

    #[test]
    fn 重试与收口请求合并可选临时凭据() {
        assert!(matches!(retry_credentials(Ok(None)), Ok(None)));
        let credentials = retry_credentials(Ok(Some(Json(RetryTaskRequest {
            username: Some("admin".to_string()),
            password: Some("secret".to_string()),
        }))));
        assert!(matches!(credentials, Ok(Some(_))));
    }

    #[test]
    fn 报告包含持久化的_npm_校验计数() {
        let validation = migration_validation(
            MigrationValidationCounts {
                checked: 5,
                passed: 4,
                failed: 1,
            },
            Some(NpmMigrationValidationReport {
                proxy_positive_cache: NpmMigrationValidationCounts {
                    checked: 2,
                    passed: 1,
                    failed: 1,
                },
                protocol_metadata: NpmMigrationValidationCounts {
                    checked: 3,
                    passed: 2,
                    failed: 1,
                },
            }),
        );
        assert!(validation.is_ok());
        if let Ok(validation) = validation {
            assert_eq!(validation.blob.checked, 5);
            assert_eq!(validation.proxy_positive_cache.failed, 1);
            assert_eq!(validation.protocol_metadata.passed, 2);
        }
    }

    #[test]
    fn 重试请求允许省略临时凭据() -> Result<(), serde_json::Error> {
        let mut request: RetryTaskRequest = serde_json::from_str("{}")?;
        assert!(matches!(request.take_temporary_credentials(), Ok(None)));
        Ok(())
    }
}
