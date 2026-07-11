//! Nexus 迁移域契约骨架（M1-FR-022~024、028、031、034、038~040）。
//! 阶段 1 只定契约，handler 返回未实现；执行器与状态机随阶段 3 落地（见 docs/specs/m1-nexus-migration.md）。

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::ApiError;

use super::repos::{Format, RepoType};

/// 迁移来源类别（ADR-0015：默认离线直读原生数据目录）。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    /// 离线直读 Nexus 原生数据目录（首选）。
    NativeDir,
    /// 在线 REST（备选）。
    Online,
    /// JianArtifact 自有离线迁移包（可选归档）。
    Package,
}

/// 仓库配置的来源（native-dir 源使用）。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ConfigSource {
    /// Nexus 可达时经 REST 一次性拉取。
    Rest,
    /// 管理员在向导手动补（proxy 上游、group 成员）。
    Manual,
}

/// 迁移源配置（发现请求体）。凭据一律为运行时引用名，绝不接受明文。
#[derive(Debug, Deserialize, ToSchema)]
pub struct MigrationSource {
    /// 来源类别。
    pub kind: SourceKind,
    /// 服务器上的路径（native-dir：Nexus 数据目录；package：包路径）；须落在 migration-workdir 白名单根内。
    pub path: Option<String>,
    /// Nexus 实例地址（online 或 native-dir + rest 配置源）。
    pub url: Option<String>,
    /// Nexus 凭据引用名（运行时从环境取值，不落库不进日志）。
    pub credential_ref: Option<String>,
    /// 仓库配置来源（native-dir 必填）。
    pub config_source: Option<ConfigSource>,
}

/// 发现到的源仓库。
#[derive(Debug, Serialize, ToSchema)]
pub struct DiscoveredRepo {
    /// 源仓库名。
    pub name: String,
    /// 格式（矩阵外格式在 unsupported 中列出，不出现在此）。
    pub format: Format,
    /// 仓库类型。
    pub repo_type: RepoType,
    /// proxy 上游地址（脱敏，不含凭据）。
    pub upstream_url: Option<String>,
    /// 是否已有可用的上游凭据引用（凭据缺失的 proxy 为「配置未完成」）。
    pub has_credential_ref: bool,
    /// group 成员（有序）。
    pub members: Option<Vec<String>>,
    /// 可迁对象数（估算）。
    pub object_count: i64,
    /// 可迁字节数（估算）。
    pub total_bytes: i64,
    /// 配置是否完整（false = 待补充，不可全量成功）。
    pub config_complete: bool,
}

/// 发现结果。
#[derive(Debug, Serialize, ToSchema)]
pub struct DiscoveryResult {
    /// 本次发现的标识（后续计划引用）。
    pub discovery_id: String,
    /// 源快照时间。
    pub snapshot_time: String,
    /// 可迁仓库清单。
    pub repositories: Vec<DiscoveredRepo>,
    /// 不支持项清单（矩阵外格式/系统数据，禁止静默丢弃）。
    pub unsupported: Vec<UnsupportedItem>,
    /// 是否为裸 blob 降级模式（true 时任务总状态不得全量成功）。
    pub degraded_bare_blob: bool,
}

/// 不支持项。
#[derive(Debug, Serialize, ToSchema)]
pub struct UnsupportedItem {
    /// 名称或坐标。
    pub name: String,
    /// 稳定类别（如「非受支持格式」「系统配置」）。
    pub category: String,
    /// 中文说明。
    pub reason: String,
}

/// 单仓库迁移映射（计划请求）。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RepoMapping {
    /// 源仓库名。
    pub source: String,
    /// 目标仓库名（缺省同名）。
    pub target: Option<String>,
    /// 是否排除（显式排除保留在报告中，任务不得全量成功）。
    pub excluded: bool,
    /// proxy 上游凭据引用名（manual 配置源或补凭据时提供）。
    pub upstream_credential_ref: Option<String>,
    /// 手动补的 proxy 上游地址（config_source=manual 时）。
    pub manual_upstream_url: Option<String>,
    /// 手动补的 group 成员（config_source=manual 时，有序）。
    pub manual_members: Option<Vec<String>>,
}

/// 计划请求体。
#[derive(Debug, Deserialize, ToSchema)]
pub struct PlanRequest {
    /// 引用的发现结果。
    pub discovery_id: String,
    /// 各仓库映射与决策。
    pub mappings: Vec<RepoMapping>,
}

/// 冲突条目（默认禁止覆盖；同校验跳过，异校验阻断待显式决策）。
#[derive(Debug, Serialize, ToSchema)]
pub struct PlanConflict {
    /// 目标仓库名。
    pub target: String,
    /// 冲突路径。
    pub path: String,
    /// 稳定类别（如「同路径同校验」「同路径不同校验」「目标仓库已存在」）。
    pub category: String,
    /// 中文说明与可选决策。
    pub detail: String,
}

/// 迁移计划预览。
#[derive(Debug, Serialize, ToSchema)]
pub struct MigrationPlan {
    /// 计划标识（创建任务时引用）。
    pub plan_id: String,
    /// 纳入执行的仓库映射。
    pub mappings: Vec<RepoMapping>,
    /// 冲突清单（存在未决阻断性冲突时不可执行）。
    pub conflicts: Vec<PlanConflict>,
    /// 是否存在阻断执行的未决冲突。
    pub blocked: bool,
    /// 对象总数。
    pub object_count: i64,
    /// 字节总数。
    pub total_bytes: i64,
    /// 目标空间估算（字节，含安全系数与保留量，NFR-12）。
    pub estimated_disk_bytes: i64,
    /// 磁盘空间是否满足估算。
    pub disk_sufficient: bool,
    /// 不支持项清单。
    pub unsupported: Vec<UnsupportedItem>,
}

/// 任务状态机十态（M1-FR-028）。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// 已入队。
    Queued,
    /// 运行中。
    Running,
    /// 暂停请求已受理，等待对象原子边界。
    Pausing,
    /// 已暂停。
    Paused,
    /// 取消请求已受理，等待对象原子边界。
    Cancelling,
    /// 已取消。
    Cancelled,
    /// 全量成功（零排除、零未迁移受支持对象、零未解决配置项）。
    Succeeded,
    /// 部分完成（逐项计数见报告）。
    Partial,
    /// 失败。
    Failed,
    /// 服务重启时在途，已中断（可续传）。
    Interrupted,
}

/// 任务信息与节流进度。
#[derive(Debug, Serialize, ToSchema)]
pub struct TaskInfo {
    /// 任务标识。
    pub task_id: String,
    /// 当前状态。
    pub status: TaskStatus,
    /// 引用的计划标识。
    pub plan_id: String,
    /// 已完成对象数。
    pub done_objects: i64,
    /// 对象总数。
    pub total_objects: i64,
    /// 已完成字节。
    pub done_bytes: i64,
    /// 字节总数。
    pub total_bytes: i64,
    /// 创建时间。
    pub created_at: String,
    /// 最近更新时间。
    pub updated_at: String,
}

/// 任务列表分页响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct TaskListResponse {
    /// 当前页任务。
    pub items: Vec<TaskInfo>,
    /// 当前页码（从 1 起）。
    pub page: u32,
    /// 每页条数。
    pub page_size: u32,
    /// 总条数。
    pub total: i64,
}

/// 单仓库迁移报告。
#[derive(Debug, Serialize, ToSchema)]
pub struct RepoReport {
    /// 源仓库名。
    pub source: String,
    /// 目标仓库名。
    pub target: String,
    /// 对象总数。
    pub total: i64,
    /// 已完成数。
    pub done: i64,
    /// 实际迁移数。
    pub migrated: i64,
    /// 已存在跳过数（幂等命中）。
    pub skipped: i64,
    /// 失败数。
    pub failed: i64,
    /// 字节总数。
    pub total_bytes: i64,
    /// 已完成字节。
    pub done_bytes: i64,
    /// 耗时（毫秒）。
    pub duration_ms: i64,
    /// 凭据引用状态（如「已映射」「未映射」）。
    pub credential_status: String,
}

/// 失败明细（有界样本）。
#[derive(Debug, Serialize, ToSchema)]
pub struct FailureSample {
    /// 仓库名。
    pub repo: String,
    /// 对象键。
    pub object_key: String,
    /// 稳定错误类别。
    pub category: String,
    /// 中文说明（脱敏）。
    pub message: String,
}

/// 迁移任务报告。
#[derive(Debug, Serialize, ToSchema)]
pub struct MigrationReport {
    /// 任务标识。
    pub task_id: String,
    /// 最终状态。
    pub status: TaskStatus,
    /// 分仓库报告。
    pub repositories: Vec<RepoReport>,
    /// 失败样本（有界）。
    pub failures: Vec<FailureSample>,
    /// 因样本上限被省略的失败数。
    pub failures_omitted: i64,
    /// 不支持/未迁移项。
    pub unsupported: Vec<UnsupportedItem>,
    /// 显式排除的仓库（保留在报告中，不伪装成从未发现）。
    pub excluded: Vec<String>,
}

/// 发现：按源配置枚举可迁仓库（Admin）。
#[utoipa::path(
    post,
    path = "/api/v1/migration/discover",
    tag = "migration",
    request_body = MigrationSource,
    responses(
        (status = 200, description = "发现结果", body = DiscoveryResult),
        (status = 400, description = "源配置无效（路径越界/矩阵外来源等）", body = crate::error::ErrorBody),
        (status = 403, description = "无管理员权限", body = crate::error::ErrorBody),
    ),
)]
pub async fn discover() -> ApiError {
    ApiError::not_implemented("迁移发现接口待阶段 3 实现")
}

/// 计划：按映射与决策生成可确认的迁移计划预览（Admin）。
#[utoipa::path(
    post,
    path = "/api/v1/migration/plan",
    tag = "migration",
    request_body = PlanRequest,
    responses(
        (status = 200, description = "计划预览", body = MigrationPlan),
        (status = 400, description = "映射无效或引用的发现结果不存在", body = crate::error::ErrorBody),
        (status = 403, description = "无管理员权限", body = crate::error::ErrorBody),
    ),
)]
pub async fn plan() -> ApiError {
    ApiError::not_implemented("迁移计划接口待阶段 3 实现")
}

/// 创建任务：确认计划并异步执行，立即返回任务标识（Admin）。
#[utoipa::path(
    post,
    path = "/api/v1/migration/tasks",
    tag = "migration",
    params(("plan_id" = String, Query, description = "已确认的计划标识")),
    responses(
        (status = 202, description = "任务已受理", body = TaskInfo),
        (status = 400, description = "计划不存在或存在未决阻断冲突", body = crate::error::ErrorBody),
        (status = 403, description = "无管理员权限", body = crate::error::ErrorBody),
    ),
)]
pub async fn create_task() -> ApiError {
    ApiError::not_implemented("迁移任务创建接口待阶段 3 实现")
}

/// 任务列表（分页，Admin）。
#[utoipa::path(
    get,
    path = "/api/v1/migration/tasks",
    tag = "migration",
    params(
        ("page" = Option<u32>, Query, description = "页码，从 1 起"),
        ("page_size" = Option<u32>, Query, description = "每页条数，最大 200"),
    ),
    responses(
        (status = 200, description = "任务分页", body = TaskListResponse),
        (status = 403, description = "无管理员权限", body = crate::error::ErrorBody),
    ),
)]
pub async fn list_tasks() -> ApiError {
    ApiError::not_implemented("迁移任务列表接口待阶段 3 实现")
}

/// 任务详情与进度（Admin）。
#[utoipa::path(
    get,
    path = "/api/v1/migration/tasks/{id}",
    tag = "migration",
    params(("id" = String, Path, description = "任务标识")),
    responses(
        (status = 200, description = "任务详情", body = TaskInfo),
        (status = 404, description = "任务不存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn task_detail() -> ApiError {
    ApiError::not_implemented("迁移任务详情接口待阶段 3 实现")
}

/// 暂停任务（在单对象原子提交边界生效，Admin）。
#[utoipa::path(
    post,
    path = "/api/v1/migration/tasks/{id}/pause",
    tag = "migration",
    params(("id" = String, Path, description = "任务标识")),
    responses(
        (status = 202, description = "暂停请求已受理", body = TaskInfo),
        (status = 409, description = "当前状态不允许暂停", body = crate::error::ErrorBody),
    ),
)]
pub async fn pause_task() -> ApiError {
    ApiError::not_implemented("迁移暂停接口待阶段 3 实现")
}

/// 继续任务（Admin）。
#[utoipa::path(
    post,
    path = "/api/v1/migration/tasks/{id}/resume",
    tag = "migration",
    params(("id" = String, Path, description = "任务标识")),
    responses(
        (status = 202, description = "继续请求已受理", body = TaskInfo),
        (status = 409, description = "当前状态不允许继续", body = crate::error::ErrorBody),
    ),
)]
pub async fn resume_task() -> ApiError {
    ApiError::not_implemented("迁移继续接口待阶段 3 实现")
}

/// 取消任务（清理未提交临时文件，不删已成功且被引用的数据，Admin）。
#[utoipa::path(
    post,
    path = "/api/v1/migration/tasks/{id}/cancel",
    tag = "migration",
    params(("id" = String, Path, description = "任务标识")),
    responses(
        (status = 202, description = "取消请求已受理", body = TaskInfo),
        (status = 409, description = "当前状态不允许取消", body = crate::error::ErrorBody),
    ),
)]
pub async fn cancel_task() -> ApiError {
    ApiError::not_implemented("迁移取消接口待阶段 3 实现")
}

/// 重试/续传（只处理失败、中断或变化对象，幂等，Admin）。
#[utoipa::path(
    post,
    path = "/api/v1/migration/tasks/{id}/retry",
    tag = "migration",
    params(("id" = String, Path, description = "任务标识")),
    responses(
        (status = 202, description = "重试请求已受理", body = TaskInfo),
        (status = 409, description = "当前状态不允许重试", body = crate::error::ErrorBody),
    ),
)]
pub async fn retry_task() -> ApiError {
    ApiError::not_implemented("迁移重试接口待阶段 3 实现")
}

/// 任务报告（Admin）。
#[utoipa::path(
    get,
    path = "/api/v1/migration/tasks/{id}/report",
    tag = "migration",
    params(("id" = String, Path, description = "任务标识")),
    responses(
        (status = 200, description = "迁移报告", body = MigrationReport),
        (status = 404, description = "任务不存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn task_report() -> ApiError {
    ApiError::not_implemented("迁移报告接口待阶段 3 实现")
}
