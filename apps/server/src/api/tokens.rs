//! API Token 契约骨架（M1-FR-004）：签发、列表、吊销。
//! 明文只在签发响应出现一次，持久化只存哈希；阶段 1 handler 返回未实现。

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::ApiError;

/// 创建 Token 请求体。
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTokenRequest {
    /// Token 名称（便于用户区分用途）。
    pub name: String,
}

/// Token 元数据（列表返回，永不含明文或哈希）。
#[derive(Debug, Serialize, ToSchema)]
pub struct TokenInfo {
    /// Token 标识。
    pub id: i64,
    /// Token 名称。
    pub name: String,
    /// 创建时间。
    pub created_at: String,
}

/// 签发成功返回：明文仅此一次出现。
#[derive(Debug, Serialize, ToSchema)]
pub struct CreatedToken {
    /// Token 标识。
    pub id: i64,
    /// Token 名称。
    pub name: String,
    /// Token 明文，仅在本次响应出现一次，请立即保存。
    pub token: String,
    /// 创建时间。
    pub created_at: String,
}

/// Token 列表分页响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct TokenListResponse {
    /// 当前页 Token 元数据。
    pub items: Vec<TokenInfo>,
    /// 当前页码（从 1 起）。
    pub page: u32,
    /// 每页条数。
    pub page_size: u32,
    /// 总条数。
    pub total: i64,
}

/// 签发 Token。
#[utoipa::path(
    post,
    path = "/api/v1/tokens",
    tag = "tokens",
    request_body = CreateTokenRequest,
    responses(
        (status = 201, description = "签发成功，明文仅此一次", body = CreatedToken),
        (status = 401, description = "未认证", body = crate::error::ErrorBody),
    ),
)]
pub async fn create() -> ApiError {
    ApiError::not_implemented("Token 签发接口待阶段 3 实现")
}

/// 列出本人 Token 元数据（分页）。
#[utoipa::path(
    get,
    path = "/api/v1/tokens",
    tag = "tokens",
    params(
        ("page" = Option<u32>, Query, description = "页码，从 1 起"),
        ("page_size" = Option<u32>, Query, description = "每页条数，最大 200"),
    ),
    responses(
        (status = 200, description = "Token 元数据分页", body = TokenListResponse),
        (status = 401, description = "未认证", body = crate::error::ErrorBody),
    ),
)]
pub async fn list() -> ApiError {
    ApiError::not_implemented("Token 列表接口待阶段 3 实现")
}

/// 吊销 Token。
#[utoipa::path(
    delete,
    path = "/api/v1/tokens/{id}",
    tag = "tokens",
    params(("id" = i64, Path, description = "Token 标识")),
    responses(
        (status = 204, description = "已吊销"),
        (status = 401, description = "未认证", body = crate::error::ErrorBody),
        (status = 404, description = "Token 不存在", body = crate::error::ErrorBody),
    ),
)]
pub async fn revoke() -> ApiError {
    ApiError::not_implemented("Token 吊销接口待阶段 3 实现")
}
