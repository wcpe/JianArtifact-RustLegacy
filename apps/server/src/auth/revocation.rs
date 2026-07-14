//! 会话撤销哈希：撤销表只存域分隔哈希，不存原始 jti，避免泄露会话标识。

use crate::auth::SESSION_REVOKE_DOMAIN;

/// 计算单令牌 jti 的域分隔哈希。
pub fn session_revoke_hash(jti: &str) -> String {
    crate::auth::domain_hash(SESSION_REVOKE_DOMAIN, &format!("jti:{jti}"))
}

/// 计算会话家族 sid 的域分隔哈希；退出以此撤销刷新链上的全部令牌。
pub fn session_family_revoke_hash(sid: &str) -> String {
    crate::auth::domain_hash(SESSION_REVOKE_DOMAIN, &format!("sid:{sid}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use sha2::{Digest, Sha256};

    use super::*;

    #[test]
    fn 撤销哈希确定且为64位十六进制() {
        let a = session_revoke_hash("jti-abc");
        let b = session_revoke_hash("jti-abc");
        assert_eq!(a, b, "同一 jti 哈希应确定");
        assert_eq!(a.len(), 64, "sha256 十六进制应为 64 位");
    }

    #[test]
    fn 含域前缀而区别于裸哈希() {
        let with_domain = session_revoke_hash("jti-abc");
        // 裸 sha256("jti-abc")，不含域前缀
        let mut hasher = Sha256::new();
        hasher.update(b"jti-abc");
        let bare = crate::auth::to_hex(&hasher.finalize());
        assert_ne!(with_domain, bare, "域分隔哈希须区别于裸 sha256");
    }

    #[test]
    fn 不同jti哈希不同() {
        assert_ne!(session_revoke_hash("a"), session_revoke_hash("b"));
    }
}
