//! JWT 当前会话撤销状态的唯一元数据入口。
//!
//! 数据库仅保存域分隔撤销键的 sha256 哈希与会话过期时间，不保存原 JWT / jti 明文。

use sha2::{Digest, Sha256};

use super::{MetaError, MetaStore};

const JTI_DOMAIN: &[u8] = b"jwt-session:jti:";
const LEGACY_TOKEN_DOMAIN: &[u8] = b"jwt-session:token:";

/// JWT 会话撤销键的安全哈希表示；构造后不再持有原 token / jti。
#[derive(Clone, PartialEq, Eq)]
pub struct SessionKeyHash([u8; 32]);

impl SessionKeyHash {
    /// 为新 JWT 的 jti 构造域分隔撤销键。
    pub fn from_jti(jti: &str) -> Self {
        Self::new(JTI_DOMAIN, jti)
    }

    /// 为旧 JWT 的完整 token 构造域分隔撤销键。
    pub fn from_legacy_token(token: &str) -> Self {
        Self::new(LEGACY_TOKEN_DOMAIN, token)
    }

    fn new(domain: &[u8], value: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(domain);
        hasher.update(value.as_bytes());
        Self(hasher.finalize().into())
    }

    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl MetaStore {
    /// 撤销一条 JWT 会话；重复撤销保持幂等，并顺带清理已过期记录。
    pub async fn revoke_jwt_session(
        &self,
        session_key: &SessionKeyHash,
        expires_at: u64,
    ) -> Result<(), MetaError> {
        let expires_at = i64::try_from(expires_at)
            .map_err(|_| MetaError::SessionExpirationOutOfRange(expires_at))?;
        self.cleanup_expired_jwt_sessions().await?;
        sqlx::query(
            "INSERT INTO revoked_jwt_sessions (session_key_hash, expires_at) VALUES (?, ?) \
             ON CONFLICT(session_key_hash) DO UPDATE SET expires_at = excluded.expires_at",
        )
        .bind(session_key.as_bytes())
        .bind(expires_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 查询 JWT 会话是否已撤销；过期记录不再视为有效撤销态。
    pub async fn is_jwt_session_revoked(
        &self,
        session_key: &SessionKeyHash,
    ) -> Result<bool, MetaError> {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM revoked_jwt_sessions \
             WHERE session_key_hash = ? AND expires_at > CAST(strftime('%s', 'now') AS INTEGER)",
        )
        .bind(session_key.as_bytes())
        .fetch_optional(self.pool())
        .await?;
        Ok(found.is_some())
    }

    /// 清理已过期的撤销记录；重复调用保持幂等。
    pub async fn cleanup_expired_jwt_sessions(&self) -> Result<u64, MetaError> {
        let affected = sqlx::query(
            "DELETE FROM revoked_jwt_sessions \
             WHERE expires_at <= CAST(strftime('%s', 'now') AS INTEGER)",
        )
        .execute(self.pool())
        .await?
        .rows_affected();
        Ok(affected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn 撤销查询与重复撤销均幂等() {
        let store = MetaStore::open_in_memory().await.unwrap();
        let expires_at = 当前时间() + 3600;
        let first = SessionKeyHash::from_jti("session-1");
        let second = SessionKeyHash::from_jti("session-2");

        assert!(!store.is_jwt_session_revoked(&first).await.unwrap());
        store.revoke_jwt_session(&first, expires_at).await.unwrap();
        store.revoke_jwt_session(&first, expires_at).await.unwrap();

        assert!(store.is_jwt_session_revoked(&first).await.unwrap());
        assert!(!store.is_jwt_session_revoked(&second).await.unwrap());
    }

    #[tokio::test]
    async fn 数据库只保存安全哈希不保存明文() {
        let store = MetaStore::open_in_memory().await.unwrap();
        let jti = "不得入库的会话标识明文";
        let key = SessionKeyHash::from_jti(jti);
        store
            .revoke_jwt_session(&key, 当前时间() + 3600)
            .await
            .unwrap();

        let stored: Vec<u8> =
            sqlx::query_scalar("SELECT session_key_hash FROM revoked_jwt_sessions")
                .fetch_one(store.pool())
                .await
                .unwrap();
        assert_eq!(stored, key.as_bytes());
        assert_ne!(stored, jti.as_bytes());
    }

    #[test]
    fn 新旧_jwt_撤销键使用不同域前缀() {
        let value = "相同输入";
        assert_ne!(
            SessionKeyHash::from_jti(value).as_bytes(),
            SessionKeyHash::from_legacy_token(value).as_bytes()
        );
    }

    #[tokio::test]
    async fn 过期时间超出_sqlite_范围返回明确错误() {
        let store = MetaStore::open_in_memory().await.unwrap();
        let key = SessionKeyHash::from_jti("overflow");
        let error = store.revoke_jwt_session(&key, u64::MAX).await.unwrap_err();
        assert!(matches!(
            error,
            MetaError::SessionExpirationOutOfRange(u64::MAX)
        ));
    }

    #[tokio::test]
    async fn 撤销时会清理过期记录() {
        let store = MetaStore::open_in_memory().await.unwrap();
        let expired = SessionKeyHash::from_jti("expired");
        sqlx::query(
            "INSERT INTO revoked_jwt_sessions (session_key_hash, expires_at) VALUES (?, ?)",
        )
        .bind(expired.as_bytes())
        .bind(0_i64)
        .execute(store.pool())
        .await
        .unwrap();

        let active = SessionKeyHash::from_jti("active");
        store
            .revoke_jwt_session(&active, 当前时间() + 3600)
            .await
            .unwrap();

        assert!(!store.is_jwt_session_revoked(&expired).await.unwrap());
        assert_eq!(store.cleanup_expired_jwt_sessions().await.unwrap(), 0);
    }

    fn 当前时间() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }
}
