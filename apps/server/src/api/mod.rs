//! 管理 API 路由装配与 OpenAPI 文档（ADR-0012：utoipa 生成的 OpenAPI 是契约唯一真源）。

pub mod auth;
pub mod extractor;
pub mod formats;
pub mod migration;
pub mod repos;
pub mod setup;
pub mod tokens;
pub mod users;

use axum::extract::Extension;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use utoipa::OpenApi;

use crate::health;
use crate::state::AppState;

/// 列表接口每页默认条数（未显式指定 page_size 时采用）。
pub(crate) const DEFAULT_PAGE_SIZE: u32 = 50;
/// 列表接口每页条数上限（NFR-16，超出按上限执行）。
pub(crate) const MAX_PAGE_SIZE: u32 = 200;

/// 列表分页查询参数（用户 / Token 列表共用）：page 默认 1、page_size 默认 50 上限 200。
#[derive(Debug, Deserialize)]
pub struct PageQuery {
    /// 页码，从 1 起。
    pub page: Option<u32>,
    /// 每页条数。
    pub page_size: Option<u32>,
    /// 名称子串筛选（可选）。
    pub q: Option<String>,
}

impl PageQuery {
    /// 应用默认值与上限，返回生效的 (page, page_size)。
    pub(crate) fn resolve(&self) -> (u32, u32) {
        let page = self.page.unwrap_or(1).max(1);
        let page_size = self
            .page_size
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        (page, page_size)
    }
}

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
        setup::status,
        setup::create,
        auth::login,
        auth::refresh,
        auth::logout,
        auth::change_password,
        auth::me,
        tokens::create,
        tokens::list,
        tokens::revoke,
        users::list,
        users::create,
        users::update,
        repos::list,
        repos::create,
        repos::detail,
        repos::update,
        repos::delete,
        repos::acl_list,
        repos::acl_set,
        repos::acl_remove,
        repos::files,
        repos::search,
        repos::usage,
        formats::get,
        formats::head,
        formats::put,
        formats::delete,
        formats::root_get,
        formats::root_head,
        formats::root_put,
        formats::root_delete,
        migration::discover,
        migration::plan,
        migration::create_task,
        migration::list_tasks,
        migration::task_detail,
        migration::pause_task,
        migration::resume_task,
        migration::cancel_task,
        migration::retry_task,
        migration::finalize_task,
        migration::task_report,
    ),
    // 仅被参数引用的 schema 不会被自动收集，须显式注册，否则契约文档出现悬空 $ref
    components(schemas(repos::DeleteMode)),
    tags(
        (name = "health", description = "健康检查与就绪自检"),
        (name = "setup", description = "首启初始化"),
        (name = "auth", description = "登录、会话刷新与退出"),
        (name = "tokens", description = "API Token 管理"),
        (name = "users", description = "用户管理（Admin）"),
        (name = "repos", description = "仓库生命周期、ACL、浏览与使用片段"),
        (name = "formats", description = "Raw、Maven、npm 与 group 共用的格式协议路径空间"),
        (name = "migration", description = "Nexus 迁移：发现、计划、任务与报告（Admin）"),
    ),
)]
pub struct ApiDoc;

/// 返回 OpenAPI 契约 JSON（供前端类型生成与 devmock 契约比对消费）。
async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

/// 装配全部路由与中间件。
pub fn router(state: AppState) -> Router {
    let migration_runtime = std::sync::Arc::new(crate::migration::MigrationRuntime::default());
    let router = Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/api/v1/openapi.json", get(openapi_json))
        .route("/api/v1/setup/status", get(setup::status))
        .route("/api/v1/setup", post(setup::create))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/refresh", post(auth::refresh))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/auth/password", post(auth::change_password))
        .route("/api/v1/auth/me", get(auth::me))
        .route("/api/v1/tokens", post(tokens::create).get(tokens::list))
        .route("/api/v1/tokens/{id}", delete(tokens::revoke))
        .route("/api/v1/users", get(users::list).post(users::create))
        .route("/api/v1/users/{id}", patch(users::update))
        .route("/api/v1/repos", get(repos::list).post(repos::create))
        .route(
            "/api/v1/repos/{name}",
            get(repos::detail)
                .patch(repos::update)
                .delete(repos::delete),
        )
        .route("/api/v1/repos/{name}/acl", get(repos::acl_list))
        .route(
            "/api/v1/repos/{name}/acl/{username}",
            axum::routing::put(repos::acl_set).delete(repos::acl_remove),
        )
        .route("/api/v1/repos/{name}/files", get(repos::files))
        .route("/api/v1/repos/{name}/search", get(repos::search))
        .route("/api/v1/repos/{name}/usage", get(repos::usage))
        .route(
            "/repository/{name}/",
            get(formats::root_get)
                .head(formats::root_head)
                .put(formats::root_put)
                .delete(formats::root_delete),
        )
        .route(
            "/repository/{name}/{*path}",
            get(formats::get)
                .head(formats::head)
                .put(formats::put)
                .delete(formats::delete),
        )
        .route("/api/v1/migration/discover", post(migration::discover))
        .route("/api/v1/migration/plan", post(migration::plan))
        .route(
            "/api/v1/migration/tasks",
            get(migration::list_tasks).post(migration::create_task),
        )
        .route("/api/v1/migration/tasks/{id}", get(migration::task_detail))
        .route(
            "/api/v1/migration/tasks/{id}/pause",
            post(migration::pause_task),
        )
        .route(
            "/api/v1/migration/tasks/{id}/resume",
            post(migration::resume_task),
        )
        .route(
            "/api/v1/migration/tasks/{id}/cancel",
            post(migration::cancel_task),
        )
        .route(
            "/api/v1/migration/tasks/{id}/retry",
            post(migration::retry_task),
        )
        .route(
            "/api/v1/migration/tasks/{id}/finalize",
            post(migration::finalize_task),
        )
        .route(
            "/api/v1/migration/tasks/{id}/report",
            get(migration::task_report),
        );
    #[cfg(feature = "embedded-web")]
    let router = router.fallback(crate::web::fallback);
    // 请求标识：入站缺失则生成 UUID，并在响应头回传，便于日志关联
    router
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(Extension(migration_runtime))
        .with_state(state)
}
