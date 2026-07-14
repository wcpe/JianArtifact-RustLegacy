//! 仓库域契约骨架（M1-FR-007~009、019~020）：仓库生命周期、ACL、浏览、搜索与使用片段。
//! 阶段 1 只定契约，handler 返回未实现；业务逻辑随阶段 3 落地（见 docs/specs/m1-repository-core.md）。

use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;

use crate::error::ApiError;

/// 制品格式。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// 任意路径直传。
    Raw,
    /// Maven2 布局。
    Maven,
    /// npm registry 协议。
    Npm,
}

/// 仓库类型。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RepoType {
    /// 托管仓库：接收上传。
    Hosted,
    /// 代理仓库：回源并缓存上游。
    Proxy,
    /// 只读聚合仓库：按序聚合成员。
    Group,
}

/// 仓库可见性。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    /// 匿名可读。
    Public,
    /// 仅授权用户可见，不泄露存在性。
    Private,
}

/// 同路径覆盖策略。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OverwritePolicy {
    /// 允许覆盖（旧内容失去引用即回收）。
    Allow,
    /// 拒绝覆盖（同路径再上传返回冲突）。
    Deny,
}

/// Maven 版本策略（仅 Maven hosted 有效）。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum VersionPolicy {
    /// 仅 release。
    Release,
    /// 仅 snapshot。
    Snapshot,
    /// 混合。
    Mixed,
}

/// 仓库 ACL 动作（向下包含：admin ⊃ write ⊃ read）。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    /// 下载、浏览、元数据。
    Read,
    /// 上传、覆盖、删除制品。
    Write,
    /// 仓库配置、可见性与 ACL 管理。
    Admin,
}

/// 反序列化 PATCH 的可空字段：缺失为 None，显式 null 为 Some(None)。
fn deserialize_optional_nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
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
    /// 是否已停用（停用后格式端点按不存在处理）。
    pub disabled: bool,
    /// 覆盖策略（hosted 有效）。
    pub overwrite_policy: Option<OverwritePolicy>,
    /// Maven 版本策略（Maven hosted 有效）。
    pub version_policy: Option<VersionPolicy>,
    /// proxy 上游地址（proxy 有效）。
    pub upstream_url: Option<String>,
    /// proxy 上游凭据引用名（只存引用，不含凭据值）。
    pub upstream_credential_ref: Option<String>,
    /// group 成员（有序，group 有效）。
    pub members: Option<Vec<String>>,
    /// 制品数（仓库内文件总数，列表与详情展示）。
    pub object_count: i64,
    /// 总大小（仓库内文件字节合计）。
    pub total_bytes: i64,
    /// 创建时间。
    pub created_at: String,
}

/// 创建仓库请求体。
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateRepoRequest {
    /// 仓库名：小写字母/数字/连字符，1–64 字符，创建后不可改。
    pub name: String,
    /// 制品格式（创建后不可变）。
    pub format: Format,
    /// 仓库类型（创建后不可变）。
    pub repo_type: RepoType,
    /// 可见性。
    pub visibility: Visibility,
    /// 覆盖策略（hosted，可选按格式默认；提供时不可为 null）。
    #[schema(nullable = false)]
    pub overwrite_policy: Option<OverwritePolicy>,
    /// Maven 版本策略（Maven hosted，默认 mixed；提供时不可为 null）。
    #[schema(nullable = false)]
    pub version_policy: Option<VersionPolicy>,
    /// proxy 上游地址（proxy 必填；提供时不可为 null）。
    #[schema(nullable = false)]
    pub upstream_url: Option<String>,
    /// proxy 上游凭据引用名（可选）。
    pub upstream_credential_ref: Option<String>,
    /// group 成员（group 必填，有序，须同格式 hosted/proxy；提供时不可为 null）。
    #[schema(nullable = false)]
    pub members: Option<Vec<String>>,
}

/// 编辑仓库请求体：字段可选，只更新给出的项；format 与 repo_type 不可变。
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateRepoRequest {
    /// 可见性；提供时不可为 null。
    #[schema(nullable = false)]
    pub visibility: Option<Visibility>,
    /// 停用/启用；提供时不可为 null。
    #[schema(nullable = false)]
    pub disabled: Option<bool>,
    /// hosted 覆盖策略；提供时不可为 null。
    #[schema(nullable = false)]
    pub overwrite_policy: Option<OverwritePolicy>,
    /// Maven hosted 版本策略；提供时不可为 null。
    #[schema(nullable = false)]
    pub version_policy: Option<VersionPolicy>,
    /// proxy 上游地址；提供时不可为 null。
    #[schema(nullable = false)]
    pub upstream_url: Option<String>,
    /// proxy 上游凭据引用名：缺失不更新，字符串替换，显式 null 清空。
    #[serde(default, deserialize_with = "deserialize_optional_nullable")]
    #[schema(nullable = true)]
    pub upstream_credential_ref: Option<Option<String>>,
    /// group 成员（整体替换且保持顺序；空数组清空，提供时不可为 null）。
    #[schema(nullable = false)]
    pub members: Option<Vec<String>>,
}

/// 仓库列表分页响应（只含调用方可读仓库，零存在性泄露）。
#[derive(Debug, Serialize, ToSchema)]
pub struct RepoListResponse {
    /// 当前页仓库。
    pub items: Vec<RepoInfo>,
    /// 当前页码（从 1 起）。
    pub page: u32,
    /// 每页条数。
    pub page_size: u32,
    /// 总条数。
    pub total: i64,
}

/// 删除模式（高风险动作，必须显式给出）。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DeleteMode {
    /// 删除配置、ACL 与制品索引，blob 本体保留（数量入响应，物理回收待后续 GC）。
    Config,
    /// 在 config 基础上物理删除无其他引用的 blob。
    Data,
}

/// 删除结果。
#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteRepoResponse {
    /// 因本次删除失去全部引用而遗留的 blob 数（mode=config 时非零有意义）。
    pub orphaned_blobs: i64,
    /// 被物理回收的 blob 数（mode=data 时有意义）。
    pub reclaimed_blobs: i64,
}

/// ACL 条目。
#[derive(Debug, Serialize, ToSchema)]
pub struct AclEntry {
    /// 用户标识。
    pub user_id: i64,
    /// 用户名。
    pub username: String,
    /// 授予的动作（向下包含）。
    pub permission: Permission,
}

/// 设置 ACL 请求体。
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetAclRequest {
    /// 授予的动作。
    pub permission: Permission,
}

/// 浏览条目（目录或文件）。
#[derive(Debug, Serialize, ToSchema)]
pub struct FileEntry {
    /// 条目名（不含父目录）。
    pub name: String,
    /// 仓库根相对完整路径；客户端不得自行拼接。
    pub path: String,
    /// 是否目录。
    pub is_dir: bool,
    /// 文件大小（字节，目录为空）。
    pub size: Option<i64>,
    /// sha256 校验和（文件）。
    pub sha256: Option<String>,
    /// sha1 校验和（文件，Maven 生态）。
    pub sha1: Option<String>,
    /// md5 校验和（文件，Maven 生态）。
    pub md5: Option<String>,
    /// sha512 校验和（文件，npm integrity）。
    pub sha512: Option<String>,
    /// 内容类型（文件）。
    pub content_type: Option<String>,
    /// 更新时间。
    pub updated_at: Option<String>,
}

/// 浏览/搜索分页响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct FileListResponse {
    /// 当前页条目。
    pub items: Vec<FileEntry>,
    /// 当前页码（从 1 起）。
    pub page: u32,
    /// 每页条数。
    pub page_size: u32,
    /// 总条数。
    pub total: i64,
}

/// 使用方式片段。
#[derive(Debug, Serialize, ToSchema)]
pub struct UsageSnippet {
    /// 片段标题（如「Maven settings.xml」）。
    pub title: String,
    /// 可复制的配置/命令文本，凭据一律为占位符。
    pub content: String,
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
    responses(
        (status = 200, description = "仓库分页（只含调用方可读仓库）", body = RepoListResponse),
    ),
)]
pub async fn list() -> ApiError {
    ApiError::not_implemented("仓库列表接口待阶段 3 实现")
}

/// 创建仓库（Admin）。
#[utoipa::path(
    post,
    operation_id = "repos_create",
    path = "/api/v1/repos",
    tag = "repos",
    request_body = CreateRepoRequest,
    responses(
        (status = 201, description = "创建成功", body = RepoInfo),
        (status = 403, description = "无管理员权限", body = crate::error::ErrorBody),
        (status = 409, description = "仓库名已存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn create() -> ApiError {
    ApiError::not_implemented("仓库创建接口待阶段 3 实现")
}

/// 仓库详情。
#[utoipa::path(
    get,
    path = "/api/v1/repos/{name}",
    tag = "repos",
    params(("name" = String, Path, description = "仓库名")),
    responses(
        (status = 200, description = "仓库详情", body = RepoInfo),
        (status = 404, description = "仓库不存在（含无权访问的 private）", body = crate::error::ErrorBody),
    ),
)]
pub async fn detail() -> ApiError {
    ApiError::not_implemented("仓库详情接口待阶段 3 实现")
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
        (status = 404, description = "仓库不存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn update() -> ApiError {
    ApiError::not_implemented("仓库编辑接口待阶段 3 实现")
}

/// 删除仓库（仓库 admin，高风险，需显式模式与确认头 X-Confirm-Repository=仓库名）。
#[utoipa::path(
    delete,
    path = "/api/v1/repos/{name}",
    tag = "repos",
    params(
        ("name" = String, Path, description = "仓库名"),
        ("mode" = DeleteMode, Query, description = "删除模式：config 仅删配置与索引 / data 连带物理回收无引用 blob"),
        ("X-Confirm-Repository" = String, Header, description = "防误删确认：必须与仓库名一致"),
    ),
    responses(
        (status = 200, description = "删除完成", body = DeleteRepoResponse),
        (status = 400, description = "缺少或不匹配的确认头/模式", body = crate::error::ErrorBody),
        (status = 404, description = "仓库不存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn delete() -> ApiError {
    ApiError::not_implemented("仓库删除接口待阶段 3 实现")
}

/// ACL 条目列表（仓库 admin）。
#[utoipa::path(
    get,
    path = "/api/v1/repos/{name}/acl",
    tag = "repos",
    params(("name" = String, Path, description = "仓库名")),
    responses(
        (status = 200, description = "ACL 条目", body = [AclEntry]),
        (status = 404, description = "仓库不存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn acl_list() -> ApiError {
    ApiError::not_implemented("ACL 列表接口待阶段 3 实现")
}

/// 按用户名设置某用户的 ACL（仓库 admin）。
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
        (status = 404, description = "仓库或用户不存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn acl_set() -> ApiError {
    ApiError::not_implemented("ACL 设置接口待阶段 3 实现")
}

/// 按用户名移除某用户的 ACL（仓库 admin）。
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
        (status = 404, description = "仓库或条目不存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn acl_remove() -> ApiError {
    ApiError::not_implemented("ACL 移除接口待阶段 3 实现")
}

/// 仓库内目录浏览（read 权限，分页）。
#[utoipa::path(
    get,
    path = "/api/v1/repos/{name}/files",
    tag = "repos",
    params(
        ("name" = String, Path, description = "仓库名"),
        ("path" = Option<String>, Query, description = "目录路径，缺省为根"),
        ("page" = Option<u32>, Query, description = "页码，从 1 起"),
        ("page_size" = Option<u32>, Query, description = "每页条数，最大 200"),
    ),
    responses(
        (status = 200, description = "目录条目分页", body = FileListResponse),
        (status = 404, description = "仓库或路径不存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn files() -> ApiError {
    ApiError::not_implemented("仓库浏览接口待阶段 3 实现")
}

/// 仓库内路径搜索（read 权限，子串匹配，分页）。
#[utoipa::path(
    get,
    path = "/api/v1/repos/{name}/search",
    tag = "repos",
    params(
        ("name" = String, Path, description = "仓库名"),
        ("q" = String, Query, description = "路径子串"),
        ("page" = Option<u32>, Query, description = "页码，从 1 起"),
        ("page_size" = Option<u32>, Query, description = "每页条数，最大 200"),
    ),
    responses(
        (status = 200, description = "匹配条目分页", body = FileListResponse),
        (status = 404, description = "仓库不存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn search() -> ApiError {
    ApiError::not_implemented("仓库搜索接口待阶段 3 实现")
}

/// 使用方式片段（read 权限）。
#[utoipa::path(
    get,
    path = "/api/v1/repos/{name}/usage",
    tag = "repos",
    params(("name" = String, Path, description = "仓库名")),
    responses(
        (status = 200, description = "按格式生成的客户端配置片段", body = [UsageSnippet]),
        (status = 404, description = "仓库不存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn usage() -> ApiError {
    ApiError::not_implemented("使用片段接口待阶段 3 实现")
}
