//! API Token 生成与哈希。明文仅在签发响应出现一次，持久化只存 sha256 域分隔哈希。

use crate::auth::{API_TOKEN_DOMAIN, API_TOKEN_PREFIX};

/// 生成新的 API Token，返回（明文，token_hash）。
/// 明文 = `jat_` 前缀 + 32 字节随机数的十六进制串；哈希 = sha256("api-token:" + 明文)。
pub fn generate_token() -> (String, String) {
    let plain = format!("{API_TOKEN_PREFIX}{}", crate::auth::random_hex(32));
    let hash = hash_token(&plain);
    (plain, hash)
}

/// 计算明文 Token 的域分隔哈希（供按明文查库校验）。
pub fn hash_token(plain: &str) -> String {
    crate::auth::domain_hash(API_TOKEN_DOMAIN, plain)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn 明文带前缀且哈希与重算一致() {
        let (plain, hash) = generate_token();
        assert!(plain.starts_with("jat_"), "明文应以 jat_ 前缀开头：{plain}");
        // jat_ 前缀（4）+ 32 字节随机数的十六进制（64）
        assert_eq!(plain.len(), 4 + 64, "明文长度应为前缀加 64 位十六进制");
        assert_eq!(hash, hash_token(&plain), "重算哈希应与生成时一致");
    }

    #[test]
    fn 哈希确定且区别于明文() {
        let (plain, hash) = generate_token();
        assert_ne!(plain, hash, "哈希不得等于明文");
        assert_eq!(hash.len(), 64, "sha256 十六进制应为 64 位");
        assert_eq!(hash_token(&plain), hash_token(&plain), "同一明文哈希应确定");
    }

    #[test]
    fn 两次生成明文互不相同() {
        let (a, _) = generate_token();
        let (b, _) = generate_token();
        assert_ne!(a, b, "随机源应使两次明文不同");
    }
}
