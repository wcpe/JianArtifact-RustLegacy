//! 鉴权提取器（入口层 HTTP 适配）：解析 `Authorization` 头 → 组装已认证身份。
//! 只调认证原语（auth::jwt/token/password）与仓储（persistence），不写裸 SQL、不含业务规则（架构不变量：api 层不直连库）。
//! 携带凭据但无效一律 401 不降级；每请求都查库校验启用 / 撤销 / 版本，这是即时失效的根本。
//! 唯一缓存的是 Basic 口令的 Argon2 校验结果（进程内 60 秒），不缓存身份。

use std::time::Instant;

use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;

use crate::auth::jwt::{self, Claims};
use crate::auth::password::{verify_password_async, verify_password_or_dummy_async};
use crate::auth::revocation::{session_family_revoke_hash, session_revoke_hash};
use crate::auth::token::hash_token;
use crate::auth::{API_TOKEN_PREFIX, AuthIdentity, PASSWORD_CACHE_TTL_SECS, domain_hash};
use crate::domain::Role;
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

/// 在 `RequireAuth` 基础上要求管理员角色，否则 403。
pub struct RequireAdmin(pub AuthIdentity);

/// 仅接受 Bearer JWT 会话令牌，产出会话声明（供刷新 / 退出撤销当前会话）；Basic 与 API Token 一律拒绝。
pub struct SessionClaims(pub Claims);

impl FromRequestParts<AppState> for RequireAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(RequireAuth(resolve_identity(parts, state).await?))
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
        let header = authorization_header(parts).ok_or_else(err_unauthenticated)?;
        let (scheme, value) = split_scheme(header).ok_or_else(err_unauthenticated)?;
        // 撤销 / 刷新只针对 Bearer JWT 会话：API Token（jat_ 前缀）与 Basic 不是会话，拒绝
        if !scheme.eq_ignore_ascii_case("bearer") || value.starts_with(API_TOKEN_PREFIX) {
            return Err(err_invalid());
        }
        let (claims, _identity) = validate_jwt(state, value).await?;
        Ok(SessionClaims(claims))
    }
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

/// 校验 JWT 会话：验签 / 过期 → 查库 → 撤销、版本失配、禁用均即时失效。返回声明与身份。
async fn validate_jwt(state: &AppState, token: &str) -> Result<(Claims, AuthIdentity), ApiError> {
    let claims = jwt::verify(&state.auth, token)?;
    let user_id: i64 = claims.sub.parse().map_err(|_| err_invalid())?;
    let user = persistence::find_user_by_id(&state.pool, user_id)
        .await?
        .ok_or_else(err_invalid)?;
    // 每请求查库校验：会话家族或单令牌被撤销、版本失配、账户禁用，任一即失效
    let jti_revoked =
        persistence::is_revoked(&state.pool, &session_revoke_hash(&claims.jti)).await?;
    let family_revoked =
        persistence::is_revoked(&state.pool, &session_family_revoke_hash(&claims.sid)).await?;
    if jti_revoked || family_revoked {
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
    let decoded = base64_decode(value).ok_or_else(err_unauthenticated)?;
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

/// 标准 Base64 解码（RFC 4648），接受规范填充和无填充形式，仅供 Basic 凭据解码。
/// 非法字符、非法填充、非法长度或非零未使用尾部位均返回 None（绝不 panic）。
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    let data_len = base64_data_len(bytes)?;
    let data = &bytes[..data_len];
    if !base64_tail_bits_are_zero(data) {
        return None;
    }

    let mut out = Vec::with_capacity(data_len.saturating_mul(3) / 4);
    for chunk in data.chunks(4) {
        let first = base64_sextet(*chunk.first()?)?;
        let second = base64_sextet(*chunk.get(1)?)?;
        out.push((first << 2) | (second >> 4));
        if let Some(&third) = chunk.get(2) {
            let third = base64_sextet(third)?;
            out.push((second << 4) | (third >> 2));
            if let Some(&fourth) = chunk.get(3) {
                out.push((third << 6) | base64_sextet(fourth)?);
            }
        }
    }
    Some(out)
}

/// 校验填充只能位于末尾，且填充数量与末组数据长度一致。
fn base64_data_len(input: &[u8]) -> Option<usize> {
    let padding = input.iter().rev().take_while(|&&byte| byte == b'=').count();
    if padding > 2 {
        return None;
    }
    let data_len = input.len().checked_sub(padding)?;
    if input[..data_len].contains(&b'=') {
        return None;
    }
    match padding {
        0 if data_len % 4 != 1 => Some(data_len),
        1 if input.len().is_multiple_of(4) && data_len % 4 == 3 => Some(data_len),
        2 if input.len().is_multiple_of(4) && data_len % 4 == 2 => Some(data_len),
        _ => None,
    }
}

/// RFC 4648 要求末组未用于输出字节的低位必须为零。
fn base64_tail_bits_are_zero(data: &[u8]) -> bool {
    let Some(&last) = data.last() else {
        return true;
    };
    let Some(last) = base64_sextet(last) else {
        return false;
    };
    match data.len() % 4 {
        2 => last & 0b0000_1111 == 0,
        3 => last & 0b0000_0011 == 0,
        _ => true,
    }
}

/// 将标准 Base64 字母表字符转换为六位值。
fn base64_sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn base64解码接受标准填充与无填充形式() {
        assert_eq!(base64_decode("dXNlcjpwYXNz").unwrap(), b"user:pass");
        assert_eq!(base64_decode("YWRtaW46cHc=").unwrap(), b"admin:pw");
        assert_eq!(base64_decode("Zg==").unwrap(), b"f");
        assert_eq!(base64_decode("Zg").unwrap(), b"f");
        assert_eq!(base64_decode("Zm8=").unwrap(), b"fo");
        assert_eq!(base64_decode("Zm8").unwrap(), b"fo");
        assert_eq!(base64_decode("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn base64解码拒绝填充位置与数量非法() {
        for invalid in ["=AAA", "A=AA", "AA=A", "AA==A", "AAA==", "AAAA=", "AAAA=="] {
            assert!(base64_decode(invalid).is_none(), "应拒绝 {invalid}");
        }
    }

    #[test]
    fn base64解码拒绝非法字符与长度() {
        for invalid in ["****", "A", "AAAA A", " Zg==", "Zg== "] {
            assert!(base64_decode(invalid).is_none(), "应拒绝 {invalid:?}");
        }
    }

    #[test]
    fn base64解码拒绝非零未使用尾部位() {
        for invalid in ["AB==", "AB", "AAB=", "AAB"] {
            assert!(base64_decode(invalid).is_none(), "应拒绝 {invalid}");
        }
    }

    #[test]
    fn 拆分鉴权头() {
        assert_eq!(split_scheme("Bearer abc.def"), Some(("Bearer", "abc.def")));
        assert_eq!(split_scheme("  Basic  dXNlcg  "), Some(("Basic", "dXNlcg")));
        assert_eq!(split_scheme("Bearer "), None);
        assert_eq!(split_scheme("NoValue"), None);
    }
}
