//! 格式协议 HTTP 适配：仅负责提取请求并转交协议领域服务。

use axum::body::Body;
use axum::extract::{OriginalUri, State};
use axum::http::HeaderMap;
use axum::response::Response;

use crate::api::extractor::{ArtifactRoute, OptionalAuth, subject_from_optional};
use crate::error::ApiError;
use crate::protocol::raw;
use crate::state::AppState;

/// 读取仓库路径内容。
#[utoipa::path(
    get,
    operation_id = "repository_get",
    path = "/repository/{name}/{path}",
    tag = "formats",
    params(
        ("name" = String, Path, description = "仓库名"),
        ("path" = String, Path, description = "仓库根相对完整路径"),
        ("page" = Option<u32>, Query, description = "目录页码，从 1 起"),
        ("page_size" = Option<u32>, Query, description = "目录每页条数，最大 200"),
    ),
    responses(
        (status = 200, description = "文件流或目录分页", body = raw::DirectoryResponse),
        (status = 400, description = "路径或查询参数无效", body = crate::error::ErrorBody),
        (status = 401, description = "凭据无效", body = crate::error::ErrorBody),
        (status = 404, description = "仓库、制品不存在或无权读取", body = crate::error::ErrorBody),
        (status = 502, description = "代理上游失败", body = crate::error::ErrorBody),
        (status = 504, description = "代理上游超时", body = crate::error::ErrorBody),
    ),
)]
pub async fn get(
    auth: OptionalAuth,
    State(state): State<AppState>,
    route: ArtifactRoute,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    raw::get(&state, subject_from_optional(auth), route, &uri).await
}

/// 探测仓库路径内容元数据。
#[utoipa::path(
    head,
    operation_id = "repository_head",
    path = "/repository/{name}/{path}",
    tag = "formats",
    params(
        ("name" = String, Path, description = "仓库名"),
        ("path" = String, Path, description = "仓库根相对完整路径"),
    ),
    responses(
        (status = 200, description = "文件元数据，响应体为空"),
        (status = 400, description = "路径无效", body = crate::error::ErrorBody),
        (status = 401, description = "凭据无效", body = crate::error::ErrorBody),
        (status = 404, description = "仓库、制品不存在或无权读取", body = crate::error::ErrorBody),
        (status = 502, description = "代理上游失败", body = crate::error::ErrorBody),
        (status = 504, description = "代理上游超时", body = crate::error::ErrorBody),
    ),
)]
pub async fn head(
    auth: OptionalAuth,
    State(state): State<AppState>,
    route: ArtifactRoute,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    raw::head(&state, subject_from_optional(auth), route, &uri).await
}

/// 向仓库路径写入内容。
#[utoipa::path(
    put,
    operation_id = "repository_put",
    path = "/repository/{name}/{path}",
    tag = "formats",
    params(
        ("name" = String, Path, description = "仓库名"),
        ("path" = String, Path, description = "仓库根相对完整路径"),
    ),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = 200, description = "覆盖成功"),
        (status = 201, description = "新建成功"),
        (status = 400, description = "路径或请求头无效", body = crate::error::ErrorBody),
        (status = 401, description = "凭据无效", body = crate::error::ErrorBody),
        (status = 403, description = "已有读取权限但无写入权限", body = crate::error::ErrorBody),
        (status = 404, description = "仓库不存在或无权读取", body = crate::error::ErrorBody),
        (status = 405, description = "仓库类型只读", body = crate::error::ErrorBody),
        (status = 409, description = "覆盖策略拒绝", body = crate::error::ErrorBody),
        (status = 413, description = "制品超过大小上限", body = crate::error::ErrorBody),
    ),
)]
pub async fn put(
    auth: OptionalAuth,
    State(state): State<AppState>,
    route: ArtifactRoute,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    raw::put(
        &state,
        subject_from_optional(auth),
        route,
        &uri,
        &headers,
        body,
    )
    .await
}

/// 删除仓库路径内容。
#[utoipa::path(
    delete,
    operation_id = "repository_delete",
    path = "/repository/{name}/{path}",
    tag = "formats",
    params(
        ("name" = String, Path, description = "仓库名"),
        ("path" = String, Path, description = "仓库根相对完整路径"),
    ),
    responses(
        (status = 204, description = "删除成功"),
        (status = 400, description = "路径无效", body = crate::error::ErrorBody),
        (status = 401, description = "凭据无效", body = crate::error::ErrorBody),
        (status = 403, description = "已有读取权限但无写入权限", body = crate::error::ErrorBody),
        (status = 404, description = "仓库、制品不存在或无权读取", body = crate::error::ErrorBody),
        (status = 405, description = "仓库类型只读", body = crate::error::ErrorBody),
    ),
)]
pub async fn delete(
    auth: OptionalAuth,
    State(state): State<AppState>,
    route: ArtifactRoute,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    raw::delete(&state, subject_from_optional(auth), route, &uri).await
}

/// 读取仓库根目录。
#[utoipa::path(
    get,
    operation_id = "repository_root_get",
    path = "/repository/{name}/",
    tag = "formats",
    params(
        ("name" = String, Path, description = "仓库名"),
        ("page" = Option<u32>, Query, description = "目录页码，从 1 起"),
        ("page_size" = Option<u32>, Query, description = "目录每页条数，最大 200"),
    ),
    responses(
        (status = 200, description = "根目录分页", body = raw::DirectoryResponse),
        (status = 404, description = "仓库不存在或无权读取", body = crate::error::ErrorBody),
    ),
)]
pub async fn root_get(
    auth: OptionalAuth,
    State(state): State<AppState>,
    route: ArtifactRoute,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    raw::get(&state, subject_from_optional(auth), route, &uri).await
}

/// 仓库根目录不支持 HEAD。
#[utoipa::path(
    head,
    operation_id = "repository_root_head",
    path = "/repository/{name}/",
    tag = "formats",
    params(("name" = String, Path, description = "仓库名")),
    responses(
        (status = 400, description = "根目录不可探测", body = crate::error::ErrorBody),
        (status = 401, description = "凭据无效", body = crate::error::ErrorBody),
        (status = 404, description = "仓库不存在或无权读取", body = crate::error::ErrorBody),
    ),
)]
pub async fn root_head(
    auth: OptionalAuth,
    State(state): State<AppState>,
    route: ArtifactRoute,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    raw::head(&state, subject_from_optional(auth), route, &uri).await
}

/// 仓库根目录不支持写入。
#[utoipa::path(
    put,
    operation_id = "repository_root_put",
    path = "/repository/{name}/",
    tag = "formats",
    params(("name" = String, Path, description = "仓库名")),
    responses(
        (status = 400, description = "根目录不可写入", body = crate::error::ErrorBody),
        (status = 401, description = "凭据无效", body = crate::error::ErrorBody),
        (status = 403, description = "无写入权限", body = crate::error::ErrorBody),
        (status = 404, description = "仓库不存在或无权读取", body = crate::error::ErrorBody),
    ),
)]
pub async fn root_put(
    auth: OptionalAuth,
    State(state): State<AppState>,
    route: ArtifactRoute,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    raw::put(
        &state,
        subject_from_optional(auth),
        route,
        &uri,
        &headers,
        body,
    )
    .await
}

/// 仓库根目录不支持删除。
#[utoipa::path(
    delete,
    operation_id = "repository_root_delete",
    path = "/repository/{name}/",
    tag = "formats",
    params(("name" = String, Path, description = "仓库名")),
    responses(
        (status = 400, description = "根目录不可删除", body = crate::error::ErrorBody),
        (status = 401, description = "凭据无效", body = crate::error::ErrorBody),
        (status = 403, description = "无写入权限", body = crate::error::ErrorBody),
        (status = 404, description = "仓库不存在或无权读取", body = crate::error::ErrorBody),
    ),
)]
pub async fn root_delete(
    auth: OptionalAuth,
    State(state): State<AppState>,
    route: ArtifactRoute,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    raw::delete(&state, subject_from_optional(auth), route, &uri).await
}
