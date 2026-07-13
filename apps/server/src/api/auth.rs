//! 认证契约骨架（M1-FR-002/003/005）：登录、刷新、退出。
//! 阶段 1 只定契约，handler 返回未实现；业务逻辑随阶段 3 落地（见 docs/specs/m1-auth.md）。

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::ApiError;

/// 登录请求体。
#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    /// 用户名。
    pub username: String,
    /// 明文口令（仅传输，不落库不进日志）。
    pub password: String,
}

/// 登录/刷新成功返回的会话令牌。
#[derive(Debug, Serialize, ToSchema)]
pub struct SessionToken {
    /// JWT 会话令牌，客户端置于 `Authorization: Bearer`。
    pub token: String,
    /// 令牌有效期（秒）。
    pub expires_in: i64,
}

/// 登录：校验口令并签发会话令牌。
#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "登录成功", body = SessionToken),
        (status = 401, description = "用户名或口令无效", body = crate::error::ErrorBody),
    ),
)]
pub async fn login() -> ApiError {
    ApiError::not_implemented("登录接口待阶段 3 实现")
}

/// 刷新：换发新会话令牌，旧令牌立即失效。
#[utoipa::path(
    post,
    path = "/api/v1/auth/refresh",
    tag = "auth",
    responses(
        (status = 200, description = "刷新成功", body = SessionToken),
        (status = 401, description = "会话无效或已撤销", body = crate::error::ErrorBody),
    ),
)]
pub async fn refresh() -> ApiError {
    ApiError::not_implemented("会话刷新接口待阶段 3 实现")
}

/// 退出：吊销当前会话。
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "auth",
    responses(
        (status = 204, description = "已退出"),
        (status = 401, description = "会话无效", body = crate::error::ErrorBody),
    ),
)]
pub async fn logout() -> ApiError {
    ApiError::not_implemented("退出接口待阶段 3 实现")
}

/// 修改本人口令请求体（M1-FR-045）。新旧口令均不落日志与响应。
#[derive(Debug, Deserialize, ToSchema)]
pub struct ChangePasswordRequest {
    /// 当前口令（校验通过才允许修改）。
    pub old_password: String,
    /// 新口令。
    pub new_password: String,
}

/// 修改本人口令：旧口令验证通过后生效，旧口令立即失效。
#[utoipa::path(
    post,
    path = "/api/v1/auth/password",
    tag = "auth",
    request_body = ChangePasswordRequest,
    responses(
        (status = 204, description = "修改成功，旧口令立即失效"),
        (status = 401, description = "未认证或旧口令错误", body = crate::error::ErrorBody),
    ),
)]
pub async fn change_password() -> ApiError {
    ApiError::not_implemented("口令修改接口待阶段 3 实现")
}
