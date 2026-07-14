//! 格式协议统一入口契约：具体 Raw、Maven、npm 与 group 语义在阶段 3 分域实现。

use crate::error::ApiError;

/// 读取仓库路径内容。
#[utoipa::path(
    get,
    operation_id = "repository_get",
    path = "/repository/{name}/{path}",
    tag = "formats",
    params(
        ("name" = String, Path, description = "仓库名"),
        ("path" = String, Path, description = "仓库根相对完整路径"),
    ),
    responses(
        (status = 501, description = "格式协议读取待对应域实现", body = crate::error::ErrorBody),
    ),
)]
pub async fn get() -> ApiError {
    ApiError::not_implemented("格式协议读取接口待阶段 3 实现")
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
        (status = 501, description = "格式协议探测待对应域实现", body = crate::error::ErrorBody),
    ),
)]
pub async fn head() -> ApiError {
    ApiError::not_implemented("格式协议探测接口待阶段 3 实现")
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
    responses(
        (status = 501, description = "格式协议写入待对应域实现", body = crate::error::ErrorBody),
    ),
)]
pub async fn put() -> ApiError {
    ApiError::not_implemented("格式协议写入接口待阶段 3 实现")
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
        (status = 501, description = "格式协议删除待对应域实现", body = crate::error::ErrorBody),
    ),
)]
pub async fn delete() -> ApiError {
    ApiError::not_implemented("格式协议删除接口待阶段 3 实现")
}
