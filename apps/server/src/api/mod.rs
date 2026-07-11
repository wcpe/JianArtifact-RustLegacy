//! 管理 API 路由装配与 OpenAPI 文档（ADR-0012：utoipa 生成的 OpenAPI 是契约唯一真源）。

pub mod auth;
pub mod tokens;
pub mod users;

use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use utoipa::OpenApi;

use crate::health;
use crate::state::AppState;

/// 管理 API 的 OpenAPI 文档定义：新增端点必须同步登记到此处。
#[derive(OpenApi)]
#[openapi(
    info(
        title = "JianArtifact 管理 API",
        description = "自托管制品仓库的管理 API 契约。由 utoipa 从代码生成，是契约唯一真源（ADR-0012）。",
    ),
    paths(
        health::healthz,
        health::readyz,
        auth::login,
        auth::refresh,
        auth::logout,
        tokens::create,
        tokens::list,
        tokens::revoke,
        users::list,
        users::create,
        users::update,
    ),
    tags(
        (name = "health", description = "健康检查与就绪自检"),
        (name = "auth", description = "登录、会话刷新与退出"),
        (name = "tokens", description = "API Token 管理"),
        (name = "users", description = "用户管理（Admin）"),
    ),
)]
pub struct ApiDoc;

/// 返回 OpenAPI 契约 JSON（供前端类型生成与 devmock 契约比对消费）。
async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

/// 装配全部路由与中间件。
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/api/v1/openapi.json", get(openapi_json))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/refresh", post(auth::refresh))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/tokens", post(tokens::create).get(tokens::list))
        .route("/api/v1/tokens/{id}", delete(tokens::revoke))
        .route("/api/v1/users", get(users::list).post(users::create))
        .route("/api/v1/users/{id}", patch(users::update))
        // 请求标识：入站缺失则生成 UUID，并在响应头回传，便于日志关联
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state)
}
