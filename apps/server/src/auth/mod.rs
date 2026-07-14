//! 认证领域服务层：口令哈希、JWT 会话、API Token、会话撤销与鉴权提取器。
//! 领域服务层，被 api 入口层单向依赖，不反向依赖 api（架构不变量）。
//! 运行时句柄与钉死常量在此；口令/JWT/Token/撤销原语拆分到同名子模块。

pub mod jwt;
pub mod password;
pub mod revocation;
pub mod token;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use jsonwebtoken::{DecodingKey, EncodingKey};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::domain::Role;

/// 会话撤销哈希域前缀（sha256("session-revoke:" + jti)）。
pub const SESSION_REVOKE_DOMAIN: &str = "session-revoke:";
/// API Token 哈希域前缀（sha256("api-token:" + 明文)）。
pub const API_TOKEN_DOMAIN: &str = "api-token:";
/// API Token 明文前缀。
pub const API_TOKEN_PREFIX: &str = "jat_";

/// Argon2id 内存成本（KiB，OWASP 推荐档，实现期钉死不得改）。
pub const ARGON2_MEMORY_KIB: u32 = 19456;
/// Argon2id 迭代次数（钉死）。
pub const ARGON2_ITERATIONS: u32 = 2;
/// Argon2id 并行度（钉死）。
pub const ARGON2_PARALLELISM: u32 = 1;

/// Basic 口令校验结果进程内缓存有效期（秒）。
pub const PASSWORD_CACHE_TTL_SECS: u64 = 60;

/// 认证运行时句柄：JWT 密钥、会话时长与 Basic 口令校验缓存，随 AppState 共享。
pub struct AuthRuntime {
    /// JWT HS256 签名密钥。
    encoding_key: EncodingKey,
    /// JWT HS256 验签密钥。
    decoding_key: DecodingKey,
    /// 会话令牌有效期（秒）。
    session_ttl_secs: u64,
    /// Basic 口令校验缓存：键为域分隔哈希，值为（校验时刻、用户 id）；由鉴权提取器读写。
    password_cache: Mutex<HashMap<String, (Instant, i64)>>,
}

impl AuthRuntime {
    /// 从 JWT 密钥与会话时长（小时）构建认证运行时。
    pub fn new(jwt_secret: &str, session_ttl_hours: u64) -> Self {
        Self {
            encoding_key: EncodingKey::from_secret(jwt_secret.as_bytes()),
            decoding_key: DecodingKey::from_secret(jwt_secret.as_bytes()),
            session_ttl_secs: session_ttl_hours.saturating_mul(3600),
            password_cache: Mutex::new(HashMap::new()),
        }
    }

    /// JWT HS256 签名密钥（供 jwt 子模块签发）。
    pub(crate) fn encoding_key(&self) -> &EncodingKey {
        &self.encoding_key
    }

    /// JWT HS256 验签密钥（供 jwt 子模块验签）。
    pub(crate) fn decoding_key(&self) -> &DecodingKey {
        &self.decoding_key
    }

    /// 会话令牌有效期（秒）。
    pub(crate) fn session_ttl_secs(&self) -> u64 {
        self.session_ttl_secs
    }

    /// Basic 口令校验缓存（供 extractor 读写）。
    pub(crate) fn password_cache(&self) -> &Mutex<HashMap<String, (Instant, i64)>> {
        &self.password_cache
    }
}

/// 字节序列转小写十六进制串。
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// 域分隔哈希：sha256(域前缀 + 值) 的十六进制串，不同域前缀互不碰撞。
pub(crate) fn domain_hash(domain: &str, value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update(value.as_bytes());
    to_hex(&hasher.finalize())
}

/// 生成 n 字节安全随机数的十六进制串（会话 jti 与 API Token 明文复用）。
pub(crate) fn random_hex(n_bytes: usize) -> String {
    let mut buf = vec![0u8; n_bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    to_hex(&buf)
}

/// 已认证身份：由鉴权提取器产出、handler 消费。
pub struct AuthIdentity {
    /// 用户标识。
    pub user_id: i64,
    /// 用户名。
    pub username: String,
    /// 全局角色。
    pub role: Role,
    /// 会话版本号（与 JWT ver 比对，撤销后失配即失效）。
    pub token_version: i64,
}
