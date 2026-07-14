//! 用户管理接口（M1-FR-006，Admin 专属）：列表、新增、启停与角色调整、重置口令。
//! 入口层只做 HTTP 适配：RequireAdmin 提取器 + 请求体 / 查询 / 路径，调领域服务，不写裸 SQL。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::PageQuery;
use crate::api::extractor::RequireAdmin;
use crate::error::ApiError;
use crate::persistence::User;
use crate::state::AppState;

/// 全局角色（定义下移至领域层，此处再导出保持 OpenAPI schema 名为 "Role"）。
pub use crate::domain::Role;

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

impl From<User> for UserInfo {
    /// 由仓储用户实体映射为对外信息（不含口令哈希与会话版本）。
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            username: user.username,
            role: user.role,
            enabled: user.enabled,
            created_at: user.created_at,
        }
    }
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
    operation_id = "users_list",
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
pub async fn list(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Result<Json<UserListResponse>, ApiError> {
    let (page, page_size) = query.resolve();
    let (users, total) =
        crate::domain::list_users(&state.pool, page, page_size, query.q.as_deref()).await?;
    let items = users.into_iter().map(UserInfo::from).collect();
    Ok(Json(UserListResponse {
        items,
        page,
        page_size,
        total,
    }))
}

/// 新增用户（Admin）。
#[utoipa::path(
    post,
    operation_id = "users_create",
    path = "/api/v1/users",
    tag = "users",
    request_body = CreateUserRequest,
    responses(
        (status = 201, description = "创建成功", body = UserInfo),
        (status = 403, description = "无管理员权限", body = crate::error::ErrorBody),
        (status = 409, description = "用户名已存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn create(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Json(req): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<UserInfo>), ApiError> {
    let user =
        crate::domain::create_user(&state.pool, &req.username, &req.password, req.role).await?;
    Ok((StatusCode::CREATED, Json(UserInfo::from(user))))
}

/// 更新用户启停与角色（Admin）。
#[utoipa::path(
    patch,
    operation_id = "users_update",
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
pub async fn update(
    _admin: RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<Json<UserInfo>, ApiError> {
    let user =
        crate::domain::update_user(&state.pool, id, req.enabled, req.role, req.password).await?;
    Ok(Json(UserInfo::from(user)))
}
