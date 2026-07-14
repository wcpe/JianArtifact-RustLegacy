//! JWT HS256 会话令牌签发与验签。claims 内嵌用户 id、会话家族、jti、令牌版本与有效期。
//! 版本号（ver）与用户当前 token_version 比对由上层完成，此处只负责签名与过期校验。

use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

use crate::auth::AuthRuntime;
use crate::error::ApiError;

/// 会话令牌声明。
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// 用户标识（用户 id 的字符串形式）。
    pub sub: String,
    /// 会话家族标识；刷新沿用，退出撤销整个家族，避免刷新/退出竞态遗留新令牌。
    pub sid: String,
    /// 令牌标识（每次签发随机生成，刷新时旧 jti 单次消费）。
    pub jti: String,
    /// 令牌版本（签发时的用户 token_version，改口令后失配即失效）。
    pub ver: i64,
    /// 签发时刻（Unix 秒）。
    pub iat: u64,
    /// 过期时刻（Unix 秒）。
    pub exp: u64,
}

/// 当前 Unix 时间戳（秒）。
fn now_unix() -> Result<u64, ApiError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| ApiError::internal("系统时间早于 Unix 纪元"))
}

/// 为指定用户创建新会话家族并签发令牌，返回（令牌串，有效期秒数）。
pub fn sign(
    runtime: &AuthRuntime,
    user_id: i64,
    token_version: i64,
) -> Result<(String, i64), ApiError> {
    sign_with_family(
        runtime,
        user_id,
        token_version,
        &crate::auth::random_hex(16),
    )
}

/// 在既有会话家族内签发新令牌；刷新必须沿用原 sid。
pub fn sign_with_family(
    runtime: &AuthRuntime,
    user_id: i64,
    token_version: i64,
    sid: &str,
) -> Result<(String, i64), ApiError> {
    let iat = now_unix()?;
    let ttl = runtime.session_ttl_secs();
    let claims = Claims {
        sub: user_id.to_string(),
        sid: sid.to_string(),
        jti: crate::auth::random_hex(16),
        ver: token_version,
        iat,
        exp: iat.saturating_add(ttl),
    };
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        runtime.encoding_key(),
    )
    .map_err(|_| ApiError::internal("会话令牌签发失败"))?;
    Ok((token, ttl as i64))
}

/// 验签并校验过期，返回声明；验签失败或已过期一律 401（不泄露具体原因）。
pub fn verify(runtime: &AuthRuntime, token: &str) -> Result<Claims, ApiError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.leeway = 0;
    let data = decode::<Claims>(token, runtime.decoding_key(), &validation)
        .map_err(|_| ApiError::unauthorized("AUTH_INVALID_SESSION", "会话无效或已过期"))?;
    if data.claims.exp <= now_unix()? {
        return Err(ApiError::unauthorized(
            "AUTH_INVALID_SESSION",
            "会话无效或已过期",
        ));
    }
    Ok(data.claims)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use jsonwebtoken::EncodingKey;

    use super::*;

    #[test]
    fn 签发与验签往返一致() {
        let runtime = AuthRuntime::new("单元测试密钥-非生产", 8);
        let (token, expires_in) = sign(&runtime, 42, 7).unwrap();
        assert_eq!(expires_in, 8 * 3600, "有效期应等于配置 TTL");
        let claims = verify(&runtime, &token).unwrap();
        assert_eq!(claims.sub, "42");
        assert!(!claims.sid.is_empty(), "会话家族标识不得为空");
        assert_eq!(claims.ver, 7, "令牌版本应回传一致");
        assert!(claims.exp > claims.iat, "过期时刻应晚于签发时刻");
    }

    #[test]
    fn 篡改令牌被拒绝() {
        let runtime = AuthRuntime::new("单元测试密钥-非生产", 8);
        let (token, _) = sign(&runtime, 1, 0).unwrap();
        let segments: Vec<&str> = token.split('.').collect();
        assert_eq!(segments.len(), 3, "JWT 应有三段");
        // 替换签名段为无效值
        let tampered = format!("{}.{}.{}", segments[0], segments[1], "Zm9yZ2Vk");
        assert!(verify(&runtime, &tampered).is_err(), "签名不匹配应被拒绝");
    }

    #[test]
    fn 异密钥验签被拒绝() {
        let signer = AuthRuntime::new("密钥甲", 8);
        let verifier = AuthRuntime::new("密钥乙", 8);
        let (token, _) = sign(&signer, 1, 0).unwrap();
        assert!(verify(&verifier, &token).is_err(), "不同密钥不得通过验签");
    }

    fn 构造指定过期时刻的令牌(secret: &str, exp: u64) -> String {
        let claims = Claims {
            sub: "1".to_string(),
            sid: "family-test".to_string(),
            jti: "test".to_string(),
            ver: 0,
            iat: 0,
            exp,
        };
        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap()
    }

    #[test]
    fn 默认宽限期内已过期令牌立即被拒绝() {
        let secret = "单元测试密钥-非生产";
        let runtime = AuthRuntime::new(secret, 8);
        let token =
            构造指定过期时刻的令牌(secret, now_unix().unwrap().saturating_sub(1));
        let err = verify(&runtime, &token).unwrap_err();
        assert_eq!(err.status(), axum::http::StatusCode::UNAUTHORIZED);
        assert_eq!(err.code(), "AUTH_INVALID_SESSION", "过期应归为会话无效");
    }

    #[test]
    fn 到达过期时刻即被拒绝() {
        let secret = "单元测试密钥-非生产";
        let runtime = AuthRuntime::new(secret, 8);
        let token = 构造指定过期时刻的令牌(secret, now_unix().unwrap());
        let err = verify(&runtime, &token).unwrap_err();
        assert_eq!(err.status(), axum::http::StatusCode::UNAUTHORIZED);
        assert_eq!(err.code(), "AUTH_INVALID_SESSION", "到点过期应归为会话无效");
    }
}
