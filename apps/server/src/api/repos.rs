//! 仓库核心管理 API：元数据生命周期、可见性、ACL、制品浏览与路径搜索。
//! 使用片段仍保持契约占位，格式协议由 protocol 分域实现。

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;

use crate::api::extractor::{
    OptionalAuth, RequireAdmin, subject_from_identity, subject_from_optional,
};
use crate::api::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, PageQuery};
use crate::domain::repository::{AclRecord, Repository};
use crate::error::ApiError;
use crate::persistence::{ArtifactDirectoryEntry, ArtifactRecord};
use crate::repository::{self, CreateRepository, DeleteMode as ServiceDeleteMode, RepositoryPatch};
use crate::state::AppState;

/// 仓库领域枚举下移至 domain，此处重新导出以保持既有 API 引用路径。
pub use crate::domain::repository::{
    Format, OverwritePolicy, Permission, RepoType, VersionPolicy, Visibility,
};

/// 反序列化 PATCH 的可空字段：缺失为 None，显式 null 为 Some(None)。
fn deserialize_optional_nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// 反序列化可选但不可为 null 的字段：缺失为 None，显式 null 直接报错。
fn deserialize_optional_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// 仓库信息。
#[derive(Debug, Serialize, ToSchema)]
pub struct RepoInfo {
    /// 仓库名（全局唯一）。
    pub name: String,
    /// 制品格式。
    pub format: Format,
    /// 仓库类型。
    pub repo_type: RepoType,
    /// 可见性。
    pub visibility: Visibility,
    /// 是否已停用。
    pub disabled: bool,
    /// 覆盖策略（hosted 有效）。
    pub overwrite_policy: Option<OverwritePolicy>,
    /// Maven 版本策略（Maven hosted 有效）。
    pub version_policy: Option<VersionPolicy>,
    /// proxy 上游地址。
    pub upstream_url: Option<String>,
    /// proxy 上游凭据引用名。
    pub upstream_credential_ref: Option<String>,
    /// group 有序成员。
    pub members: Option<Vec<String>>,
    /// 当前制品索引数。
    pub object_count: i64,
    /// 当前制品总字节数。
    pub total_bytes: i64,
    /// 创建时间。
    pub created_at: String,
}

impl From<Repository> for RepoInfo {
    fn from(repository: Repository) -> Self {
        let members = (repository.repo_type == RepoType::Group).then_some(repository.members);
        Self {
            name: repository.name,
            format: repository.format,
            repo_type: repository.repo_type,
            visibility: repository.visibility,
            disabled: repository.disabled,
            overwrite_policy: repository.overwrite_policy,
            version_policy: repository.version_policy,
            upstream_url: repository.upstream_url,
            upstream_credential_ref: repository.upstream_credential_ref,
            members,
            object_count: repository.object_count,
            total_bytes: repository.total_bytes,
            created_at: repository.created_at,
        }
    }
}

/// 创建仓库请求体。
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateRepoRequest {
    /// 仓库名：小写字母/数字/连字符，1–64 字符。
    pub name: String,
    /// 制品格式（创建后不可变）。
    pub format: Format,
    /// 仓库类型（创建后不可变）。
    pub repo_type: RepoType,
    /// 可见性。
    pub visibility: Visibility,
    /// hosted 覆盖策略；缺失时默认 allow。
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    #[schema(nullable = false)]
    pub overwrite_policy: Option<OverwritePolicy>,
    /// Maven hosted 版本策略；缺失时默认 mixed。
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    #[schema(nullable = false)]
    pub version_policy: Option<VersionPolicy>,
    /// proxy 上游地址。
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    #[schema(nullable = false)]
    pub upstream_url: Option<String>,
    /// proxy 上游凭据引用名。
    pub upstream_credential_ref: Option<String>,
    /// group 有序成员。
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    #[schema(nullable = false)]
    pub members: Option<Vec<String>>,
}

impl From<CreateRepoRequest> for CreateRepository {
    fn from(request: CreateRepoRequest) -> Self {
        Self {
            name: request.name,
            format: request.format,
            repo_type: request.repo_type,
            visibility: request.visibility,
            overwrite_policy: request.overwrite_policy,
            version_policy: request.version_policy,
            upstream_url: request.upstream_url,
            upstream_credential_ref: request.upstream_credential_ref,
            members: request.members,
        }
    }
}

/// 编辑仓库请求体：字段可选，只更新给出的项。
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateRepoRequest {
    /// 可见性。
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    #[schema(nullable = false)]
    pub visibility: Option<Visibility>,
    /// 停用/启用状态。
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    #[schema(nullable = false)]
    pub disabled: Option<bool>,
    /// hosted 覆盖策略。
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    #[schema(nullable = false)]
    pub overwrite_policy: Option<OverwritePolicy>,
    /// Maven hosted 版本策略。
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    #[schema(nullable = false)]
    pub version_policy: Option<VersionPolicy>,
    /// proxy 上游地址。
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    #[schema(nullable = false)]
    pub upstream_url: Option<String>,
    /// proxy 上游凭据引用名：缺失不更新，字符串替换，显式 null 清空。
    #[serde(default, deserialize_with = "deserialize_optional_nullable")]
    #[schema(nullable = true)]
    pub upstream_credential_ref: Option<Option<String>>,
    /// group 成员整体替换并保持顺序。
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    #[schema(nullable = false)]
    pub members: Option<Vec<String>>,
}

impl From<UpdateRepoRequest> for RepositoryPatch {
    fn from(request: UpdateRepoRequest) -> Self {
        Self {
            visibility: request.visibility,
            disabled: request.disabled,
            overwrite_policy: request.overwrite_policy,
            version_policy: request.version_policy,
            upstream_url: request.upstream_url,
            upstream_credential_ref: request.upstream_credential_ref,
            members: request.members,
        }
    }
}

/// 仓库列表分页响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct RepoListResponse {
    /// 当前页仓库。
    pub items: Vec<RepoInfo>,
    /// 当前页码。
    pub page: u32,
    /// 每页条数。
    pub page_size: u32,
    /// 权限过滤后的总条数。
    pub total: i64,
}

/// 删除模式。
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DeleteMode {
    /// 删除配置与制品索引，保留无引用 blob。
    Config,
    /// 删除配置与制品索引，并回收无其他引用的 blob。
    Data,
}

#[derive(Debug, Deserialize)]
pub struct DeleteQuery {
    mode: DeleteMode,
}

/// 删除结果。
#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteRepoResponse {
    /// config 模式删除后失去全部引用、保留待 GC 的 blob 数。
    pub orphaned_blobs: i64,
    /// data 模式已物理回收的 blob 数。
    pub reclaimed_blobs: i64,
}

/// ACL 条目。
#[derive(Debug, Serialize, ToSchema)]
pub struct AclEntry {
    /// 用户标识。
    pub user_id: i64,
    /// 精确用户名。
    pub username: String,
    /// 授予权限。
    pub permission: Permission,
}

impl From<AclRecord> for AclEntry {
    fn from(record: AclRecord) -> Self {
        Self {
            user_id: record.user_id,
            username: record.username,
            permission: record.permission,
        }
    }
}

/// 设置 ACL 请求体。
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetAclRequest {
    /// 授予权限。
    pub permission: Permission,
}

/// 浏览条目（目录或文件）。
#[derive(Debug, Serialize, ToSchema)]
pub struct FileEntry {
    /// 条目名。
    pub name: String,
    /// 仓库根相对完整路径。
    pub path: String,
    /// 是否目录。
    pub is_dir: bool,
    /// 文件大小。
    pub size: Option<i64>,
    /// sha256 校验和。
    pub sha256: Option<String>,
    /// sha1 校验和。
    pub sha1: Option<String>,
    /// md5 校验和。
    pub md5: Option<String>,
    /// sha512 校验和。
    pub sha512: Option<String>,
    /// 内容类型。
    pub content_type: Option<String>,
    /// 更新时间。
    pub updated_at: Option<String>,
}

/// 浏览/搜索分页响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct FileListResponse {
    /// 当前页条目。
    pub items: Vec<FileEntry>,
    /// 当前页码。
    pub page: u32,
    /// 每页条数。
    pub page_size: u32,
    /// 总条数。
    pub total: i64,
}

#[derive(Debug, Deserialize)]
pub struct FilesQuery {
    path: Option<String>,
    page: Option<u32>,
    page_size: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    q: String,
    page: Option<u32>,
    page_size: Option<u32>,
}

fn resolve_artifact_page(page: Option<u32>, page_size: Option<u32>) -> (u32, u32) {
    (
        page.unwrap_or(1).max(1),
        page_size
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE),
    )
}

/// 使用方式片段。
#[derive(Debug, Serialize, ToSchema)]
pub struct UsageSnippet {
    /// 片段标题。
    pub title: String,
    /// 可复制内容。
    pub content: String,
}

fn invalid_json(_: JsonRejection) -> ApiError {
    ApiError::bad_request("REPO_INVALID_CONFIG", "仓库请求字段或 JSON 格式无效")
}

fn invalid_query(_: QueryRejection) -> ApiError {
    ApiError::bad_request("REPO_INVALID_QUERY", "仓库查询参数无效")
}

fn directory_file_entry(entry: ArtifactDirectoryEntry) -> Result<FileEntry, ApiError> {
    let content_type = (!entry.is_directory).then(|| artifact_content_type(&entry.path));
    Ok(FileEntry {
        name: entry.name,
        path: entry.path,
        is_dir: entry.is_directory,
        size: entry.size.map(artifact_size).transpose()?,
        sha256: entry.sha256,
        sha1: entry.sha1,
        md5: entry.md5,
        sha512: entry.sha512,
        content_type,
        updated_at: Some(entry.updated_at),
    })
}

fn search_file_entry(record: ArtifactRecord) -> Result<FileEntry, ApiError> {
    let name = record
        .path
        .rsplit('/')
        .next()
        .unwrap_or(record.path.as_str())
        .to_string();
    Ok(FileEntry {
        name,
        content_type: Some(artifact_content_type(&record.path)),
        path: record.path,
        is_dir: false,
        size: Some(artifact_size(record.size)?),
        sha256: Some(record.sha256),
        sha1: Some(record.sha1),
        md5: Some(record.md5),
        sha512: Some(record.sha512),
        updated_at: Some(record.updated_at),
    })
}

fn artifact_size(size: u64) -> Result<i64, ApiError> {
    i64::try_from(size).map_err(|_| ApiError::internal("制品大小超出接口范围"))
}

fn artifact_content_type(path: &str) -> String {
    mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string()
}

/// 仓库列表（按权限过滤，分页）。
#[utoipa::path(
    get,
    operation_id = "repos_list",
    path = "/api/v1/repos",
    tag = "repos",
    params(
        ("page" = Option<u32>, Query, description = "页码，从 1 起"),
        ("page_size" = Option<u32>, Query, description = "每页条数，最大 200"),
        ("q" = Option<String>, Query, description = "按仓库名子串筛选（可选）"),
    ),
    responses((status = 200, description = "仓库分页", body = RepoListResponse)),
)]
pub async fn list(
    auth: OptionalAuth,
    State(state): State<AppState>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<RepoListResponse>, ApiError> {
    let Query(query) = query.map_err(invalid_query)?;
    let (page, page_size) = query.resolve();
    let (repositories, total) = repository::list_repositories(
        &state.pool,
        subject_from_optional(auth),
        page,
        page_size,
        query.q.as_deref(),
    )
    .await?;
    Ok(Json(RepoListResponse {
        items: repositories.into_iter().map(RepoInfo::from).collect(),
        page,
        page_size,
        total,
    }))
}

/// 创建仓库（仅全局 Admin）。
#[utoipa::path(
    post,
    operation_id = "repos_create",
    path = "/api/v1/repos",
    tag = "repos",
    request_body = CreateRepoRequest,
    responses(
        (status = 201, description = "创建成功", body = RepoInfo),
        (status = 400, description = "类型字段或配置组合非法", body = crate::error::ErrorBody),
        (status = 403, description = "无管理员权限", body = crate::error::ErrorBody),
        (status = 409, description = "仓库名已存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn create(
    RequireAdmin(identity): RequireAdmin,
    State(state): State<AppState>,
    body: Result<Json<CreateRepoRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(request) = body.map_err(invalid_json)?;
    let repository =
        repository::create_repository(&state.pool, subject_from_identity(identity), request.into())
            .await?;
    Ok((StatusCode::CREATED, Json(RepoInfo::from(repository))))
}

/// 仓库详情。
#[utoipa::path(
    get,
    path = "/api/v1/repos/{name}",
    tag = "repos",
    params(("name" = String, Path, description = "仓库名")),
    responses(
        (status = 200, description = "仓库详情", body = RepoInfo),
        (status = 404, description = "仓库不存在或无权访问", body = crate::error::ErrorBody),
    ),
)]
pub async fn detail(
    auth: OptionalAuth,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<RepoInfo>, ApiError> {
    let repository =
        repository::get_repository(&state.pool, subject_from_optional(auth), &name).await?;
    Ok(Json(RepoInfo::from(repository)))
}

/// 编辑仓库（仓库 admin）。
#[utoipa::path(
    patch,
    operation_id = "repos_update",
    path = "/api/v1/repos/{name}",
    tag = "repos",
    params(("name" = String, Path, description = "仓库名")),
    request_body = UpdateRepoRequest,
    responses(
        (status = 200, description = "更新成功", body = RepoInfo),
        (status = 400, description = "类型字段或配置组合非法", body = crate::error::ErrorBody),
        (status = 403, description = "已有读取权限但无仓库管理权限", body = crate::error::ErrorBody),
        (status = 404, description = "仓库不存在或无权访问", body = crate::error::ErrorBody),
    ),
)]
pub async fn update(
    auth: OptionalAuth,
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: Result<Json<UpdateRepoRequest>, JsonRejection>,
) -> Result<Json<RepoInfo>, ApiError> {
    let Json(request) = body.map_err(invalid_json)?;
    let repository = repository::update_repository(
        &state.pool,
        &state.artifacts,
        subject_from_optional(auth),
        &name,
        request.into(),
    )
    .await?;
    Ok(Json(RepoInfo::from(repository)))
}

/// 删除仓库（仓库 admin，需确认头）。
#[utoipa::path(
    delete,
    path = "/api/v1/repos/{name}",
    tag = "repos",
    params(
        ("name" = String, Path, description = "仓库名"),
        ("mode" = DeleteMode, Query, description = "删除模式"),
        ("X-Confirm-Repository" = String, Header, description = "必须与仓库名一致"),
    ),
    responses(
        (status = 200, description = "删除完成", body = DeleteRepoResponse),
        (status = 400, description = "确认头或模式无效", body = crate::error::ErrorBody),
        (status = 403, description = "已有读取权限但无仓库管理权限", body = crate::error::ErrorBody),
        (status = 404, description = "仓库不存在或无权访问", body = crate::error::ErrorBody),
        (status = 409, description = "仓库仍被 group 引用", body = crate::error::ErrorBody),
        (status = 500, description = "索引删除或 blob 回收失败", body = crate::error::ErrorBody),
    ),
)]
pub async fn delete(
    auth: OptionalAuth,
    State(state): State<AppState>,
    Path(name): Path<String>,
    query: Result<Query<DeleteQuery>, QueryRejection>,
    headers: HeaderMap,
) -> Result<Json<DeleteRepoResponse>, ApiError> {
    let Query(query) = query.map_err(invalid_query)?;
    let mode = match query.mode {
        DeleteMode::Config => ServiceDeleteMode::Config,
        DeleteMode::Data => ServiceDeleteMode::Data,
    };
    let confirmation = headers
        .get("x-confirm-repository")
        .and_then(|value| value.to_str().ok());
    let result = repository::delete_repository(
        &state.pool,
        &state.artifacts,
        subject_from_optional(auth),
        &name,
        mode,
        confirmation,
    )
    .await?;
    Ok(Json(DeleteRepoResponse {
        orphaned_blobs: result.orphaned_blobs,
        reclaimed_blobs: result.reclaimed_blobs,
    }))
}

/// ACL 条目列表（仓库 admin）。
#[utoipa::path(
    get,
    path = "/api/v1/repos/{name}/acl",
    tag = "repos",
    params(("name" = String, Path, description = "仓库名")),
    responses(
        (status = 200, description = "ACL 条目", body = [AclEntry]),
        (status = 403, description = "已有读取权限但无仓库管理权限", body = crate::error::ErrorBody),
        (status = 404, description = "仓库不存在或无权访问", body = crate::error::ErrorBody),
    ),
)]
pub async fn acl_list(
    auth: OptionalAuth,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Vec<AclEntry>>, ApiError> {
    let entries = repository::list_acl(&state.pool, subject_from_optional(auth), &name).await?;
    Ok(Json(entries.into_iter().map(AclEntry::from).collect()))
}

/// 按精确用户名设置 ACL（仓库 admin）。
#[utoipa::path(
    put,
    path = "/api/v1/repos/{name}/acl/{username}",
    tag = "repos",
    params(
        ("name" = String, Path, description = "仓库名"),
        ("username" = String, Path, description = "精确用户名"),
    ),
    request_body = SetAclRequest,
    responses(
        (status = 200, description = "已设置", body = AclEntry),
        (status = 403, description = "已有读取权限但无仓库管理权限", body = crate::error::ErrorBody),
        (status = 404, description = "仓库或目标用户不存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn acl_set(
    auth: OptionalAuth,
    State(state): State<AppState>,
    Path((name, username)): Path<(String, String)>,
    body: Result<Json<SetAclRequest>, JsonRejection>,
) -> Result<Json<AclEntry>, ApiError> {
    let Json(request) = body.map_err(invalid_json)?;
    let entry = repository::set_acl(
        &state.pool,
        subject_from_optional(auth),
        &name,
        &username,
        request.permission,
    )
    .await?;
    Ok(Json(AclEntry::from(entry)))
}

/// 按精确用户名移除 ACL（仓库 admin）。
#[utoipa::path(
    delete,
    path = "/api/v1/repos/{name}/acl/{username}",
    tag = "repos",
    params(
        ("name" = String, Path, description = "仓库名"),
        ("username" = String, Path, description = "精确用户名"),
    ),
    responses(
        (status = 204, description = "已移除"),
        (status = 403, description = "已有读取权限但无仓库管理权限", body = crate::error::ErrorBody),
        (status = 404, description = "仓库、用户或 ACL 条目不存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn acl_remove(
    auth: OptionalAuth,
    State(state): State<AppState>,
    Path((name, username)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    repository::remove_acl(&state.pool, subject_from_optional(auth), &name, &username).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 仓库内目录浏览。
#[utoipa::path(
    get,
    path = "/api/v1/repos/{name}/files",
    tag = "repos",
    params(
        ("name" = String, Path, description = "仓库名"),
        ("path" = Option<String>, Query, description = "目录路径"),
        ("page" = Option<u32>, Query, description = "页码"),
        ("page_size" = Option<u32>, Query, description = "每页条数"),
    ),
    responses(
        (status = 200, description = "目录条目分页", body = FileListResponse),
        (status = 400, description = "路径或分页参数无效", body = crate::error::ErrorBody),
        (status = 404, description = "仓库、目录不存在或无权读取", body = crate::error::ErrorBody),
    ),
)]
pub async fn files(
    auth: OptionalAuth,
    State(state): State<AppState>,
    Path(name): Path<String>,
    query: Result<Query<FilesQuery>, QueryRejection>,
) -> Result<Json<FileListResponse>, ApiError> {
    let Query(query) = query.map_err(invalid_query)?;
    let (page, page_size) = resolve_artifact_page(query.page, query.page_size);
    let (entries, total) = repository::browse_artifacts(
        &state.pool,
        &state.artifacts,
        subject_from_optional(auth),
        &name,
        query.path.as_deref().unwrap_or_default(),
        page,
        page_size,
    )
    .await?;
    let items = entries
        .into_iter()
        .map(directory_file_entry)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(FileListResponse {
        items,
        page,
        page_size,
        total,
    }))
}

/// 仓库内路径搜索。
#[utoipa::path(
    get,
    path = "/api/v1/repos/{name}/search",
    tag = "repos",
    params(
        ("name" = String, Path, description = "仓库名"),
        ("q" = String, Query, description = "路径子串"),
        ("page" = Option<u32>, Query, description = "页码"),
        ("page_size" = Option<u32>, Query, description = "每页条数"),
    ),
    responses(
        (status = 200, description = "匹配条目分页", body = FileListResponse),
        (status = 400, description = "搜索或分页参数无效", body = crate::error::ErrorBody),
        (status = 404, description = "仓库不存在或无权读取", body = crate::error::ErrorBody),
    ),
)]
pub async fn search(
    auth: OptionalAuth,
    State(state): State<AppState>,
    Path(name): Path<String>,
    query: Result<Query<SearchQuery>, QueryRejection>,
) -> Result<Json<FileListResponse>, ApiError> {
    let Query(query) = query.map_err(invalid_query)?;
    let (page, page_size) = resolve_artifact_page(query.page, query.page_size);
    let (records, total) = repository::search_artifacts(
        &state.pool,
        &state.artifacts,
        subject_from_optional(auth),
        &name,
        &query.q,
        page,
        page_size,
    )
    .await?;
    let items = records
        .into_iter()
        .map(search_file_entry)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(FileListResponse {
        items,
        page,
        page_size,
        total,
    }))
}

/// 使用方式片段（本批次不实现）。
#[utoipa::path(
    get,
    path = "/api/v1/repos/{name}/usage",
    tag = "repos",
    params(("name" = String, Path, description = "仓库名")),
    responses(
        (status = 200, description = "按格式生成的客户端配置片段", body = [UsageSnippet]),
        (status = 404, description = "仓库不存在", body = crate::error::ErrorBody),
        (status = 501, description = "使用片段尚未实现", body = crate::error::ErrorBody),
    ),
)]
pub async fn usage() -> ApiError {
    ApiError::not_implemented("使用片段接口不在本批次范围")
}
