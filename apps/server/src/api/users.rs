//! 用户管理契约骨架（M1-FR-006，Admin 专属）：列表、新增、启停与角色调整。
//! 阶段 1 handler 返回未实现；最后一名启用 Admin 保护等规则随阶段 3 落地。

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::ApiError;

/// 全局角色。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// 管理员：全局管理权限。
    Admin,
    /// 普通用户：按仓库 ACL 访问。
    User,
}

/// 用户信息。
#[derive(Debug, Serialize, ToSchema)]
pub struct UserInfo {
    /// 用户标识。
    pub id: i64,
    /// 用户名。
    pub username: String,
    /// 全局角色。
    pub role: Role,
    /// 是否启用。
    pub enabled: bool,
    /// 创建时间。
    pub created_at: String,
}

/// 创建用户请求体。
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateUserRequest {
    /// 用户名。
    pub username: String,
    /// 初始明文口令（仅传输，落库为 Argon2 哈希）。
    pub password: String,
    /// 全局角色。
    pub role: Role,
}

/// 更新用户请求体：字段可选，只更新给出的项。
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateUserRequest {
    /// 启用状态。
    pub enabled: Option<bool>,
    /// 全局角色。
    pub role: Option<Role>,
    /// 重置口令为该明文（M1-FR-045，Admin 专属；不进日志与响应，提交后旧口令立即失效）。
    pub password: Option<String>,
}

/// 用户列表分页响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct UserListResponse {
    /// 当前页用户。
    pub items: Vec<UserInfo>,
    /// 当前页码（从 1 起）。
    pub page: u32,
    /// 每页条数。
    pub page_size: u32,
    /// 总条数。
    pub total: i64,
}

/// 列出用户（分页，Admin）。
#[utoipa::path(
    get,
    path = "/api/v1/users",
    tag = "users",
    params(
        ("page" = Option<u32>, Query, description = "页码，从 1 起"),
        ("page_size" = Option<u32>, Query, description = "每页条数，最大 200"),
        ("q" = Option<String>, Query, description = "按用户名子串筛选（可选）"),
    ),
    responses(
        (status = 200, description = "用户分页", body = UserListResponse),
        (status = 403, description = "无管理员权限", body = crate::error::ErrorBody),
    ),
)]
pub async fn list() -> ApiError {
    ApiError::not_implemented("用户列表接口待阶段 3 实现")
}

/// 新增用户（Admin）。
#[utoipa::path(
    post,
    path = "/api/v1/users",
    tag = "users",
    request_body = CreateUserRequest,
    responses(
        (status = 201, description = "创建成功", body = UserInfo),
        (status = 403, description = "无管理员权限", body = crate::error::ErrorBody),
        (status = 409, description = "用户名已存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn create() -> ApiError {
    ApiError::not_implemented("用户新增接口待阶段 3 实现")
}

/// 更新用户启停与角色（Admin）。
#[utoipa::path(
    patch,
    path = "/api/v1/users/{id}",
    tag = "users",
    params(("id" = i64, Path, description = "用户标识")),
    request_body = UpdateUserRequest,
    responses(
        (status = 200, description = "更新成功", body = UserInfo),
        (status = 403, description = "无管理员权限或违反最后 Admin 保护", body = crate::error::ErrorBody),
        (status = 404, description = "用户不存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn update() -> ApiError {
    ApiError::not_implemented("用户更新接口待阶段 3 实现")
}
