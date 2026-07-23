//! 鉴权提取器（入口层 HTTP 适配）：解析 `Authorization` 头 → 组装已认证身份。
//! 只调认证原语（auth::jwt/token/password）与仓储（persistence），不写裸 SQL、不含业务规则（架构不变量：api 层不直连库）。
//! 携带凭据但无效一律 401 不降级；每请求都查库校验启用 / 撤销 / 版本，这是即时失效的根本。
//! 唯一缓存的是 Basic 口令的 Argon2 校验结果（进程内 60 秒），不缓存身份。

use std::collections::HashMap;
use std::time::Instant;

use axum::extract::{FromRequestParts, Path};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;

use crate::auth::jwt::{self, Claims};
use crate::auth::password::{verify_password_async, verify_password_or_dummy_async};
use crate::auth::revocation::{session_family_revoke_hash, session_revoke_hash};
use crate::auth::token::hash_token;
use crate::auth::{API_TOKEN_PREFIX, AuthIdentity, PASSWORD_CACHE_TTL_SECS, domain_hash};
use crate::domain::Role;
use crate::domain::encoding::strict_base64_decode;
use crate::domain::repository::Subject;
use crate::error::ApiError;
use crate::persistence;
use crate::state::AppState;

/// Basic 口令校验缓存的哈希域前缀：缓存键绑定「口令 + 当前 PHC」，
/// 改口令后 PHC 变化即令旧缓存键不可达，旧口令必重新走 Argon2 而失败（即时失效）。
const PASSWORD_CACHE_DOMAIN: &str = "basic-password-cache:";

/// 未携带（或无法解析）身份凭据。
fn err_unauthenticated() -> ApiError {
    ApiError::unauthorized("AUTH_UNAUTHENTICATED", "缺少身份凭据")
}

/// 携带了凭据但校验不通过（口令 / Token / 会话无效、被撤销、版本失配、账户禁用等一律归此，防泄露具体原因）。
fn err_invalid() -> ApiError {
    ApiError::unauthorized("AUTH_INVALID_SESSION", "身份凭据无效或已失效")
}

/// 任一有效凭据（Bearer JWT / Bearer API Token / Basic 口令或 Token）均可通过，产出已认证身份。
pub struct RequireAuth(pub AuthIdentity);

/// 可选身份：无 Authorization 头时返回匿名；一旦携带头，任何无效、吊销或禁用凭据都返回 401。
pub struct OptionalAuth(pub Option<AuthIdentity>);

/// 将可选认证结果转换为仓库授权主体。
pub fn subject_from_optional(auth: OptionalAuth) -> Subject {
    match auth.0 {
        Some(identity) => subject_from_identity(identity),
        None => Subject::Anonymous,
    }
}

/// 将已认证身份转换为仓库授权主体。
pub fn subject_from_identity(identity: AuthIdentity) -> Subject {
    Subject::Authenticated {
        user_id: identity.user_id,
        role: identity.role,
        enabled: true,
    }
}

/// Raw 协议路径：仓库名与保留尾斜杠状态。
pub struct ArtifactRoute {
    pub repository: String,
    pub path: String,
    pub trailing_slash: bool,
}

impl FromRequestParts<AppState> for ArtifactRoute {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Path(values) = Path::<HashMap<String, String>>::from_request_parts(parts, _state)
            .await
            .map_err(|_| ApiError::bad_request("ARTIFACT_INVALID_PATH", "制品路径无效"))?;
        let repository = values
            .get("name")
            .cloned()
            .ok_or_else(|| ApiError::bad_request("ARTIFACT_INVALID_PATH", "制品路径无效"))?;
        let path = values.get("path").cloned().unwrap_or_default();
        let trailing_slash = parts.uri.path().ends_with('/');
        Ok(Self {
            repository,
            path,
            trailing_slash,
        })
    }
}

/// 在 `RequireAuth` 基础上要求管理员角色，否则 403。
pub struct RequireAdmin(pub AuthIdentity);

/// 仅接受严格有效的 Bearer JWT 会话令牌，产出刷新所需声明；Basic 与 API Token 一律拒绝。
pub struct SessionClaims(pub Claims);

/// 仅接受可退出的 Bearer JWT 会话令牌；允许当前 jti 已被同家族刷新消费，但家族撤销仍拒绝。
pub struct LogoutClaims(pub Claims);

impl FromRequestParts<AppState> for RequireAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(RequireAuth(resolve_identity(parts, state).await?))
    }
}

impl FromRequestParts<AppState> for OptionalAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if !parts.headers.contains_key(AUTHORIZATION) {
            return Ok(OptionalAuth(None));
        }
        resolve_identity(parts, state)
            .await
            .map(|identity| OptionalAuth(Some(identity)))
    }
}

impl FromRequestParts<AppState> for RequireAdmin {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let identity = resolve_identity(parts, state).await?;
        // 前端隐藏入口只是体验优化，真正的授权判定以此后端结果为准
        if !matches!(identity.role, Role::Admin) {
            return Err(ApiError::forbidden("AUTH_FORBIDDEN", "需要管理员权限"));
        }
        Ok(RequireAdmin(identity))
    }
}

impl FromRequestParts<AppState> for SessionClaims {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = session_token(parts)?;
        let (claims, _identity) = validate_jwt(state, token).await?;
        Ok(SessionClaims(claims))
    }
}

impl FromRequestParts<AppState> for LogoutClaims {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = session_token(parts)?;
        let (claims, _identity) = validate_logout_jwt(state, token).await?;
        Ok(LogoutClaims(claims))
    }
}

/// 提取 Bearer JWT 会话令牌；API Token 与 Basic 不是会话，统一拒绝。
fn session_token(parts: &Parts) -> Result<&str, ApiError> {
    let header = authorization_header(parts).ok_or_else(err_unauthenticated)?;
    let (scheme, value) = split_scheme(header).ok_or_else(err_unauthenticated)?;
    if !scheme.eq_ignore_ascii_case("bearer") || value.starts_with(API_TOKEN_PREFIX) {
        return Err(err_invalid());
    }
    Ok(value)
}

/// 读取 `Authorization` 头文本；缺失或含非 ASCII 视为未携带凭据。
fn authorization_header(parts: &Parts) -> Option<&str> {
    parts.headers.get(AUTHORIZATION)?.to_str().ok()
}

/// 拆分 "Scheme value" 为 (scheme, value)；value 去空白后为空则视为无效。
fn split_scheme(header: &str) -> Option<(&str, &str)> {
    let (scheme, value) = header.trim().split_once(' ')?;
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some((scheme, value))
    }
}

/// 解析任一受支持凭据为身份：无头 401 未认证；有头无效 401，不降级为匿名。
async fn resolve_identity(parts: &Parts, state: &AppState) -> Result<AuthIdentity, ApiError> {
    let header = authorization_header(parts).ok_or_else(err_unauthenticated)?;
    let (scheme, value) = split_scheme(header).ok_or_else(err_unauthenticated)?;
    if scheme.eq_ignore_ascii_case("bearer") {
        resolve_bearer(state, value).await
    } else if scheme.eq_ignore_ascii_case("basic") {
        resolve_basic(state, value).await
    } else {
        Err(err_unauthenticated())
    }
}

/// Bearer：`jat_` 前缀走 API Token，否则按 JWT 会话令牌校验。
async fn resolve_bearer(state: &AppState, value: &str) -> Result<AuthIdentity, ApiError> {
    if value.starts_with(API_TOKEN_PREFIX) {
        resolve_api_token(state, value).await
    } else {
        let (_claims, identity) = validate_jwt(state, value).await?;
        Ok(identity)
    }
}

/// 普通会话严格校验单令牌与会话家族撤销状态。
async fn validate_jwt(state: &AppState, token: &str) -> Result<(Claims, AuthIdentity), ApiError> {
    validate_jwt_revocation(state, token, false).await
}

/// 退出允许旧 jti 已被刷新消费，但仍严格拒绝已撤销的会话家族。
async fn validate_logout_jwt(
    state: &AppState,
    token: &str,
) -> Result<(Claims, AuthIdentity), ApiError> {
    validate_jwt_revocation(state, token, true).await
}

/// 校验 JWT 验签、过期、用户状态、会话版本与撤销状态，返回声明与身份。
async fn validate_jwt_revocation(
    state: &AppState,
    token: &str,
    allow_revoked_jti: bool,
) -> Result<(Claims, AuthIdentity), ApiError> {
    let claims = jwt::verify(&state.auth, token)?;
    let user_id: i64 = claims.sub.parse().map_err(|_| err_invalid())?;
    let user = persistence::find_user_by_id(&state.pool, user_id)
        .await?
        .ok_or_else(err_invalid)?;
    let jti_revoked =
        persistence::is_revoked(&state.pool, &session_revoke_hash(&claims.jti)).await?;
    let family_revoked =
        persistence::is_revoked(&state.pool, &session_family_revoke_hash(&claims.sid)).await?;
    if family_revoked || (jti_revoked && !allow_revoked_jti) {
        return Err(err_invalid());
    }
    if user.token_version != claims.ver || !user.enabled {
        return Err(err_invalid());
    }
    let identity = AuthIdentity {
        user_id: user.id,
        username: user.username,
        role: user.role,
        token_version: user.token_version,
    };
    Ok((claims, identity))
}

/// API Token：明文哈希 → 查归属用户 → 校验启用。未命中或禁用一律 401。
async fn resolve_api_token(state: &AppState, plain: &str) -> Result<AuthIdentity, ApiError> {
    let hash = hash_token(plain);
    let owner = persistence::find_token_owner_by_hash(&state.pool, &hash)
        .await?
        .ok_or_else(err_invalid)?;
    identity_of_enabled_user(state, owner).await
}

/// 按 id 查用户并校验启用，组装身份（API Token 与 Basic Token 路径共用）。
async fn identity_of_enabled_user(
    state: &AppState,
    user_id: i64,
) -> Result<AuthIdentity, ApiError> {
    let user = persistence::find_user_by_id(&state.pool, user_id)
        .await?
        .ok_or_else(err_invalid)?;
    if !user.enabled {
        return Err(err_invalid());
    }
    Ok(AuthIdentity {
        user_id: user.id,
        username: user.username,
        role: user.role,
        token_version: user.token_version,
    })
}

/// Basic：解码 `user:pass`；pass 先按 API Token 匹配，未命中再按账户口令（带 60 秒 Argon2 缓存）。
async fn resolve_basic(state: &AppState, value: &str) -> Result<AuthIdentity, ApiError> {
    let decoded = strict_base64_decode(value).ok_or_else(err_unauthenticated)?;
    let text = String::from_utf8(decoded).map_err(|_| err_unauthenticated())?;
    let (username, password) = text.split_once(':').ok_or_else(err_unauthenticated)?;
    // 先按 API Token 校验口令位：命中即用（供 CLI 以「任意用户名 + Token」方式鉴权）
    let token_hash = hash_token(password);
    if let Some(owner) = persistence::find_token_owner_by_hash(&state.pool, &token_hash).await? {
        return identity_of_enabled_user(state, owner).await;
    }
    resolve_basic_password(state, username, password).await
}

/// Basic 口令路径：每请求查库拿当前哈希 / 启用 / 版本，仅缓存 Argon2 校验结果以省重复开销。
async fn resolve_basic_password(
    state: &AppState,
    username: &str,
    password: &str,
) -> Result<AuthIdentity, ApiError> {
    // 每请求查库：账户禁用、口令改动都在此即时反映（不缓存身份、不缓存启用态）
    let record = persistence::load_password_and_version_by_username(&state.pool, username).await?;
    let Some((id, phc, enabled, role, token_version)) = record else {
        verify_password_or_dummy_async(password, None).await?;
        return Err(err_invalid());
    };
    // 缓存键绑定「口令 + 当前 PHC + 会话版本」：改口令或禁用推进版本后旧缓存键不可达
    let cache_key = domain_hash(
        PASSWORD_CACHE_DOMAIN,
        &format!("{password}\u{0}{phc}\u{0}{token_version}"),
    );
    if !cache_hit(state, &cache_key, id) {
        // Argon2 校验在锁外执行（高开销，禁止持锁计算）
        if !verify_password_async(password, &phc).await? {
            return Err(err_invalid());
        }
        cache_store(state, cache_key, id);
    }
    if !enabled {
        return Err(err_invalid());
    }
    Ok(AuthIdentity {
        user_id: id,
        username: username.to_string(),
        role,
        token_version,
    })
}

/// 查 Basic 口令缓存：键命中、用户一致且未过期，则可跳过 Argon2。锁即取即放，绝不跨 await。
fn cache_hit(state: &AppState, key: &str, user_id: i64) -> bool {
    // 锁中毒时保守地放弃缓存、改走完整校验，绝不 panic
    let Ok(cache) = state.auth.password_cache().lock() else {
        return false;
    };
    match cache.get(key) {
        Some(entry) => entry.1 == user_id && entry.0.elapsed().as_secs() < PASSWORD_CACHE_TTL_SECS,
        None => false,
    }
}

/// 写 Basic 口令缓存：记录校验时刻与用户 id，并顺带清理过期项以防无界增长。锁即取即放。
fn cache_store(state: &AppState, key: String, user_id: i64) {
    let Ok(mut cache) = state.auth.password_cache().lock() else {
        return;
    };
    cache.retain(|_, entry| entry.0.elapsed().as_secs() < PASSWORD_CACHE_TTL_SECS);
    cache.insert(key, (Instant::now(), user_id));
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn 拆分鉴权头() {
        assert_eq!(split_scheme("Bearer abc.def"), Some(("Bearer", "abc.def")));
        assert_eq!(split_scheme("  Basic  dXNlcg  "), Some(("Basic", "dXNlcg")));
        assert_eq!(split_scheme("Bearer "), None);
        assert_eq!(split_scheme("NoValue"), None);
    }
}
