//! 首启初始化管理 API：公开状态查询与单次管理员创建。

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::password::{hash_password_async, password_is_long_enough};
use crate::error::ApiError;
use crate::persistence;
use crate::setup;
use crate::state::AppState;

/// 首启初始化状态。
#[derive(Debug, Serialize, ToSchema)]
pub struct SetupStatus {
    /// 用户库为空时为 true；一旦存在用户便永久为 false。
    pub setup_required: bool,
}

/// 首启管理员创建请求。
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetupRequest {
    /// 数据库旁令牌文件中的一次性令牌。
    pub token: String,
    /// 首名管理员用户名，服务端会去除首尾空白。
    pub username: String,
    /// 首名管理员口令，不少于 12 个 Unicode 字符。
    pub password: String,
}

fn unavailable() -> ApiError {
    ApiError::not_found("SETUP_UNAVAILABLE", "首启初始化不可用")
}

/// 查询是否仍需首启初始化；响应禁止缓存。
#[utoipa::path(
    get,
    path = "/api/v1/setup/status",
    tag = "setup",
    responses((status = 200, description = "首启初始化状态", body = SetupStatus)),
)]
pub async fn status(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let users_empty = persistence::count_users(&state.pool).await? == 0;
    let token_path = setup::token_path(&state.config.database_path);
    let setup_required = users_empty && setup::read_valid_token_file(&token_path).await.is_some();
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(SetupStatus { setup_required }),
    ))
}

/// 使用本地令牌创建首名管理员；并发请求最多一项成功。
#[utoipa::path(
    post,
    path = "/api/v1/setup",
    tag = "setup",
    request_body = SetupRequest,
    responses(
        (status = 204, description = "首名管理员创建成功"),
        (status = 400, description = "请求格式、用户名或口令不符合要求", body = crate::error::ErrorBody),
        (status = 404, description = "初始化不可用", body = crate::error::ErrorBody),
    ),
)]
pub async fn create(
    State(state): State<AppState>,
    request: Result<Json<SetupRequest>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(request) = request
        .map_err(|_| ApiError::bad_request("SETUP_REQUEST_INVALID", "初始化请求格式无效"))?;
    let username = validate_request(&request)?;
    ensure_available(&state).await?;
    verify_token(&state, &request.token).await?;
    let password_hash = hash_password_async(&request.password).await?;
    insert_first_admin(&state, username, &password_hash).await?;
    setup::cleanup_token_file(&setup::token_path(&state.config.database_path)).await;
    Ok(StatusCode::NO_CONTENT)
}

fn validate_request(request: &SetupRequest) -> Result<&str, ApiError> {
    let username = request.username.trim();
    if username.is_empty() {
        return Err(ApiError::bad_request(
            "SETUP_USERNAME_REQUIRED",
            "用户名不能为空",
        ));
    }
    if !password_is_long_enough(&request.password) {
        return Err(ApiError::bad_request(
            "SETUP_PASSWORD_TOO_SHORT",
            "口令至少需要 12 个字符",
        ));
    }
    Ok(username)
}

async fn ensure_available(state: &AppState) -> Result<(), ApiError> {
    if persistence::count_users(&state.pool).await? == 0 {
        Ok(())
    } else {
        Err(unavailable())
    }
}

async fn verify_token(state: &AppState, provided: &str) -> Result<(), ApiError> {
    let path = setup::token_path(&state.config.database_path);
    let expected = setup::read_valid_token_file(&path)
        .await
        .ok_or_else(unavailable)?;
    if setup::token_matches(&expected, provided) {
        Ok(())
    } else {
        Err(unavailable())
    }
}

async fn insert_first_admin(
    state: &AppState,
    username: &str,
    password_hash: &str,
) -> Result<(), ApiError> {
    match persistence::insert_admin_if_users_empty(&state.pool, username, password_hash).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(unavailable()),
        Err(error) => {
            tracing::error!(code = error.code(), "首启管理员写入失败");
            Err(error)
        }
    }
}
