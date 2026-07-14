//! API Token 接口（M1-FR-004）：签发、列表、吊销。
//! 明文只在签发响应出现一次，持久化只存哈希；入口层只做 HTTP 适配，调仓储与认证原语。

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::PageQuery;
use crate::api::extractor::RequireAuth;
use crate::auth::token::generate_token;
use crate::error::ApiError;
use crate::persistence::{self, TokenRow};
use crate::state::AppState;

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

impl From<TokenRow> for TokenInfo {
    /// 由仓储 Token 行映射为对外元数据（永不含明文或哈希）。
    fn from(row: TokenRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            created_at: row.created_at,
        }
    }
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
    operation_id = "tokens_create",
    path = "/api/v1/tokens",
    tag = "tokens",
    request_body = CreateTokenRequest,
    responses(
        (status = 201, description = "签发成功，明文仅此一次", body = CreatedToken),
        (status = 401, description = "未认证", body = crate::error::ErrorBody),
    ),
)]
pub async fn create(
    RequireAuth(identity): RequireAuth,
    State(state): State<AppState>,
    Json(req): Json<CreateTokenRequest>,
) -> Result<(StatusCode, Json<CreatedToken>), ApiError> {
    // 明文只此一次返回；持久化只存域分隔哈希
    let (plain, hash) = generate_token();
    let (id, created_at) =
        persistence::insert_token(&state.pool, identity.user_id, &req.name, &hash).await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedToken {
            id,
            name: req.name,
            token: plain,
            created_at,
        }),
    ))
}

/// 列出本人 Token 元数据（分页）。
#[utoipa::path(
    get,
    operation_id = "tokens_list",
    path = "/api/v1/tokens",
    tag = "tokens",
    params(
        ("page" = Option<u32>, Query, description = "页码，从 1 起"),
        ("page_size" = Option<u32>, Query, description = "每页条数，最大 200"),
        ("q" = Option<String>, Query, description = "按 Token 名子串筛选（可选）"),
    ),
    responses(
        (status = 200, description = "Token 元数据分页", body = TokenListResponse),
        (status = 401, description = "未认证", body = crate::error::ErrorBody),
    ),
)]
pub async fn list(
    RequireAuth(identity): RequireAuth,
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Result<Json<TokenListResponse>, ApiError> {
    let (page, page_size) = query.resolve();
    let (rows, total) = persistence::list_tokens_by_user(
        &state.pool,
        identity.user_id,
        page,
        page_size,
        query.q.as_deref(),
    )
    .await?;
    let items = rows.into_iter().map(TokenInfo::from).collect();
    Ok(Json(TokenListResponse {
        items,
        page,
        page_size,
        total,
    }))
}

/// 吊销 Token（仅限本人）。
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
pub async fn revoke(
    RequireAuth(identity): RequireAuth,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    // id 与 user_id 双重限定：未命中即视为不存在，不泄露他人 Token 的存在性
    let deleted = persistence::delete_token(&state.pool, id, identity.user_id).await?;
    if !deleted {
        return Err(ApiError::not_found("AUTH_TOKEN_NOT_FOUND", "Token 不存在"));
    }
    Ok(StatusCode::NO_CONTENT)
}
