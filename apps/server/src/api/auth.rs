//! 认证接口（M1-FR-002/003/005/045）：登录、刷新、退出、改本人口令、当前身份。
//! 入口层只做 HTTP 适配：取 State 与提取器，调领域服务 / 仓储 / 认证原语，不写裸 SQL。

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::extractor::{LogoutClaims, RequireAuth, SessionClaims};
use crate::api::users::UserInfo;
use crate::auth::jwt;
use crate::auth::password::verify_password_or_dummy_async;
use crate::auth::revocation::{session_family_revoke_hash, session_revoke_hash};
use crate::error::ApiError;
use crate::persistence;
use crate::state::AppState;

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

/// 登录失败统一错误（不区分用户不存在 / 口令错误 / 账户禁用，防账户枚举）。
fn invalid_credentials() -> ApiError {
    ApiError::unauthorized("AUTH_INVALID_CREDENTIALS", "用户名或口令无效")
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
pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<SessionToken>, ApiError> {
    let record =
        persistence::load_password_and_version_by_username(&state.pool, &req.username).await?;
    let password_hash = record.as_ref().map(|(_, phc, _, _, _)| phc.as_str());
    let password_valid = verify_password_or_dummy_async(&req.password, password_hash).await?;
    let Some((id, _phc, enabled, _role, token_version)) = record else {
        return Err(invalid_credentials());
    };
    if !password_valid || !enabled {
        return Err(invalid_credentials());
    }
    let (token, expires_in) = jwt::sign(&state.auth, id, token_version)?;
    Ok(Json(SessionToken { token, expires_in }))
}

/// 刷新：原子撤销仍有效的当前会话，并沿用声明版本换发新令牌。
#[utoipa::path(
    post,
    path = "/api/v1/auth/refresh",
    tag = "auth",
    responses(
        (status = 200, description = "刷新成功", body = SessionToken),
        (status = 401, description = "会话无效或已撤销", body = crate::error::ErrorBody),
    ),
)]
pub async fn refresh(
    SessionClaims(claims): SessionClaims,
    State(state): State<AppState>,
) -> Result<Json<SessionToken>, ApiError> {
    let user_id: i64 = claims
        .sub
        .parse()
        .map_err(|_| ApiError::unauthorized("AUTH_INVALID_SESSION", "会话无效"))?;
    // 单条写语句同时确认账户启用、版本未变且旧 jti 未撤销，只有首个并发请求能取得刷新资格
    let revoked = persistence::revoke_session_for_refresh(
        &state.pool,
        user_id,
        claims.ver,
        &session_revoke_hash(&claims.jti),
        &session_family_revoke_hash(&claims.sid),
        claims.exp as i64,
    )
    .await?;
    if !revoked {
        return Err(ApiError::unauthorized(
            "AUTH_INVALID_SESSION",
            "会话无效或已失效",
        ));
    }
    // 必须沿用已原子校验的声明版本，禁止把竞态中的旧会话升级到新版本
    let (token, expires_in) = jwt::sign_with_family(&state.auth, user_id, claims.ver, &claims.sid)?;
    Ok(Json(SessionToken { token, expires_in }))
}

/// 退出：吊销当前 sid 会话家族。
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "auth",
    responses(
        (status = 204, description = "已退出"),
        (status = 401, description = "会话无效", body = crate::error::ErrorBody),
    ),
)]
pub async fn logout(
    LogoutClaims(claims): LogoutClaims,
    State(state): State<AppState>,
) -> Result<StatusCode, ApiError> {
    let revoke_until = claims.exp.saturating_add(state.auth.session_ttl_secs()) as i64;
    persistence::insert_revoked(
        &state.pool,
        &session_family_revoke_hash(&claims.sid),
        revoke_until,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 修改本人口令请求体（M1-FR-045）。新旧口令均不落日志与响应。
#[derive(Debug, Deserialize, ToSchema)]
pub struct ChangePasswordRequest {
    /// 当前口令（校验通过才允许修改）。
    pub old_password: String,
    /// 新口令。
    pub new_password: String,
}

/// 修改本人口令：旧口令验证通过后生效，旧口令与既有会话立即失效。
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
pub async fn change_password(
    RequireAuth(identity): RequireAuth,
    State(state): State<AppState>,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<StatusCode, ApiError> {
    crate::domain::change_own_password(
        &state.pool,
        identity.user_id,
        &req.old_password,
        &req.new_password,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 当前认证用户信息。
#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
    tag = "auth",
    responses(
        (status = 200, description = "当前认证用户信息", body = UserInfo),
        (status = 401, description = "未认证或会话无效", body = crate::error::ErrorBody),
    ),
)]
pub async fn me(
    RequireAuth(identity): RequireAuth,
    State(state): State<AppState>,
) -> Result<Json<UserInfo>, ApiError> {
    let user = persistence::find_user_by_id(&state.pool, identity.user_id)
        .await?
        .ok_or_else(|| ApiError::unauthorized("AUTH_INVALID_SESSION", "会话无效"))?;
    Ok(Json(UserInfo::from(user)))
}
