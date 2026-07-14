//! 元数据库连接与迁移框架（SQLite 唯一真源，ADR-0002）。迁移脚本只向前追加。
//! 兼含认证域仓储：用户、API Token、会话撤销的增改查，均只做数据存取、不含业务规则。

use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::time::Duration;

use sqlx::SqlitePool;
#[cfg(test)]
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;

use crate::domain::Role;
use crate::error::{ApiError, StartupError};

/// 编译期内嵌的迁移集合（`apps/server/migrations`）。
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// 建立连接池并执行全部未应用的迁移。
pub async fn init(database_url: &str, database_path: &Path) -> Result<SqlitePool, StartupError> {
    // 确保库文件所在目录存在（sqlite rwc 模式只创建文件不创建目录）
    if let Some(parent) = database_path.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect(database_url)
        .await?;
    MIGRATOR.run(&pool).await?;
    tracing::info!("元数据库已就绪并完成迁移");
    Ok(pool)
}

/// 用户读取列（统一列表，避免多处重复书写并保证 FromRow 列名一致）。
const USER_COLUMNS: &str = "id, username, role, enabled, created_at, token_version";

/// 用户实体（仓储读出，领域与接口层消费；不含口令哈希）。
#[derive(Debug)]
pub struct User {
    /// 用户标识。
    pub id: i64,
    /// 用户名。
    pub username: String,
    /// 全局角色。
    pub role: Role,
    /// 是否启用。
    pub enabled: bool,
    /// 创建时间（SQLite datetime 文本）。
    pub created_at: String,
    /// 会话版本号。
    pub token_version: i64,
}

/// 数据库原始用户行：role 为文本，转换为领域 Role 后对外暴露 User。
#[derive(sqlx::FromRow)]
struct UserRow {
    id: i64,
    username: String,
    role: String,
    enabled: bool,
    created_at: String,
    token_version: i64,
}

impl UserRow {
    /// 将原始行转换为领域 User；角色文本非法时归为内部错误。
    fn into_user(self) -> Result<User, ApiError> {
        let role =
            Role::from_db_str(&self.role).ok_or_else(|| ApiError::internal("用户角色值非法"))?;
        Ok(User {
            id: self.id,
            username: self.username,
            role,
            enabled: self.enabled,
            created_at: self.created_at,
            token_version: self.token_version,
        })
    }
}

/// API Token 元数据行（列表返回，永不含明文或哈希）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TokenRow {
    /// Token 标识。
    pub id: i64,
    /// Token 名称。
    pub name: String,
    /// 创建时间。
    pub created_at: String,
}

/// 分页边界：page 从 1、page_size 下限 1 上限 200（NFR-16），返回 (limit, offset)。
fn page_bounds(page: u32, page_size: u32) -> (i64, i64) {
    let page = page.max(1);
    let size = page_size.clamp(1, 200);
    let offset = page.saturating_sub(1).saturating_mul(size);
    (i64::from(size), i64::from(offset))
}

/// 构造 LIKE 子串匹配模式，转义 `\` `%` `_` 通配符；q 为空时以 `%` 匹配全部。
fn like_pattern(q: Option<&str>) -> String {
    match q {
        Some(s) if !s.is_empty() => {
            let escaped = s
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            format!("%{escaped}%")
        }
        _ => "%".to_string(),
    }
}

// ==================== 用户仓储 ====================

/// 按用户名查用户；不存在返回 None。
pub async fn find_user_by_username(
    pool: &SqlitePool,
    username: &str,
) -> Result<Option<User>, ApiError> {
    let row = sqlx::query_as::<_, UserRow>(&format!(
        "SELECT {USER_COLUMNS} FROM users WHERE username = ?"
    ))
    .bind(username)
    .fetch_optional(pool)
    .await
    .map_err(|_| ApiError::internal("查询用户失败"))?;
    row.map(UserRow::into_user).transpose()
}

/// 按 id 查用户；不存在返回 None。
pub async fn find_user_by_id(pool: &SqlitePool, id: i64) -> Result<Option<User>, ApiError> {
    let row =
        sqlx::query_as::<_, UserRow>(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"))
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(|_| ApiError::internal("查询用户失败"))?;
    row.map(UserRow::into_user).transpose()
}

/// 插入用户并回读完整行；用户名唯一约束冲突映射为稳定的 409 业务错误。
pub async fn insert_user(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
    role: Role,
    enabled: bool,
) -> Result<User, ApiError> {
    let row = sqlx::query_as::<_, UserRow>(&format!(
        "INSERT INTO users (username, password_hash, role, enabled) VALUES (?, ?, ?, ?) RETURNING {USER_COLUMNS}"
    ))
    .bind(username)
    .bind(password_hash)
    .bind(role.as_db_str())
    .bind(enabled)
    .fetch_one(pool)
    .await
    .map_err(|error| match error {
        sqlx::Error::Database(database_error) if database_error.is_unique_violation() => {
            ApiError::conflict("AUTH_USERNAME_TAKEN", "用户名已存在")
        }
        _ => ApiError::internal("创建用户失败"),
    })?;
    row.into_user()
}

/// 按给出的字段部分更新用户；改口令或禁用时原子递增 token_version，禁用同时永久吊销 API Token。
/// 全部为 None 时不写库。
pub async fn update_user_fields(
    pool: &SqlitePool,
    id: i64,
    enabled: Option<bool>,
    role: Option<Role>,
    password_hash: Option<String>,
) -> Result<(), ApiError> {
    if enabled.is_none() && role.is_none() && password_hash.is_none() {
        return Ok(());
    }
    let role_str = role.map(|r| r.as_db_str().to_string());
    let reset_sessions = password_hash.is_some() || enabled == Some(false);
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ApiError::internal("开启用户更新事务失败"))?;
    sqlx::query(
        "UPDATE users SET \
         enabled = COALESCE(?, enabled), \
         role = COALESCE(?, role), \
         password_hash = COALESCE(?, password_hash), \
         token_version = token_version + (CASE WHEN ? THEN 1 ELSE 0 END) \
         WHERE id = ?",
    )
    .bind(enabled)
    .bind(role_str)
    .bind(password_hash.as_deref())
    .bind(reset_sessions)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApiError::internal("更新用户失败"))?;
    if enabled == Some(false) {
        sqlx::query("DELETE FROM api_tokens WHERE user_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal("吊销用户 Token 失败"))?;
    }
    tx.commit()
        .await
        .map_err(|_| ApiError::internal("提交用户更新失败"))?;
    Ok(())
}

/// 条件更新用户，并由同一事务保护最后一名启用管理员；改口令或禁用会推进会话版本，禁用永久吊销 API Token。
pub async fn update_user_preserving_admin(
    pool: &SqlitePool,
    id: i64,
    enabled: Option<bool>,
    role: Option<Role>,
    password_hash: Option<String>,
) -> Result<bool, ApiError> {
    if enabled.is_none() && role.is_none() && password_hash.is_none() {
        return Ok(true);
    }
    let role_str = role.map(|value| value.as_db_str().to_string());
    let reset_sessions = password_hash.is_some() || enabled == Some(false);
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ApiError::internal("开启用户更新事务失败"))?;
    let result = sqlx::query(
        "UPDATE users SET \
         enabled = COALESCE(?, enabled), \
         role = COALESCE(?, role), \
         password_hash = COALESCE(?, password_hash), \
         token_version = token_version + (CASE WHEN ? THEN 1 ELSE 0 END) \
         WHERE id = ? AND (\
           role <> 'admin' OR enabled <> 1 \
           OR (COALESCE(?, role) = 'admin' AND COALESCE(?, enabled) = 1) \
           OR EXISTS (\
             SELECT 1 FROM users AS other \
             WHERE other.id <> users.id AND other.role = 'admin' AND other.enabled = 1\
           )\
         )",
    )
    .bind(enabled)
    .bind(role_str.as_deref())
    .bind(password_hash.as_deref())
    .bind(reset_sessions)
    .bind(id)
    .bind(role_str.as_deref())
    .bind(enabled)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApiError::internal("更新用户失败"))?;
    let updated = result.rows_affected() == 1;
    if updated && enabled == Some(false) {
        sqlx::query("DELETE FROM api_tokens WHERE user_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal("吊销用户 Token 失败"))?;
    }
    tx.commit()
        .await
        .map_err(|_| ApiError::internal("提交用户更新失败"))?;
    Ok(updated)
}

/// 仅当用户表仍为空时插入首名管理员；插入成功返回 true。
pub async fn insert_admin_if_users_empty(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
) -> Result<bool, ApiError> {
    let result = sqlx::query(
        "INSERT INTO users (username, password_hash, role, enabled) \
         SELECT ?, ?, 'admin', 1 WHERE NOT EXISTS (SELECT 1 FROM users)",
    )
    .bind(username)
    .bind(password_hash)
    .execute(pool)
    .await
    .map_err(|_| ApiError::internal("创建管理员失败"))?;
    Ok(result.rows_affected() == 1)
}

/// 分页列出用户，可选按用户名子串（大小写不敏感）筛选，稳定按 id 排序。返回 (当前页, 筛选后总数)。
pub async fn list_users(
    pool: &SqlitePool,
    page: u32,
    page_size: u32,
    q: Option<&str>,
) -> Result<(Vec<User>, i64), ApiError> {
    let (limit, offset) = page_bounds(page, page_size);
    let pattern = like_pattern(q);
    let total: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM users WHERE username LIKE ? ESCAPE '\\'")
            .bind(&pattern)
            .fetch_one(pool)
            .await
            .map_err(|_| ApiError::internal("统计用户失败"))?;
    let rows = sqlx::query_as::<_, UserRow>(&format!(
        "SELECT {USER_COLUMNS} FROM users WHERE username LIKE ? ESCAPE '\\' ORDER BY id LIMIT ? OFFSET ?"
    ))
    .bind(&pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(|_| ApiError::internal("查询用户列表失败"))?;
    let users = rows
        .into_iter()
        .map(UserRow::into_user)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((users, total.0))
}

/// 用户总数。
pub async fn count_users(pool: &SqlitePool) -> Result<i64, ApiError> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await
        .map_err(|_| ApiError::internal("统计用户失败"))?;
    Ok(row.0)
}

/// 当前启用的管理员数量（最后一名启用 Admin 保护据此判定）。
pub async fn count_enabled_admins(pool: &SqlitePool) -> Result<i64, ApiError> {
    let row: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM users WHERE role = 'admin' AND enabled = 1")
            .fetch_one(pool)
            .await
            .map_err(|_| ApiError::internal("统计管理员失败"))?;
    Ok(row.0)
}

/// 递增用户会话版本号（使其所有既有会话失配失效）。
pub async fn increment_token_version(pool: &SqlitePool, id: i64) -> Result<(), ApiError> {
    sqlx::query("UPDATE users SET token_version = token_version + 1 WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|_| ApiError::internal("更新会话版本失败"))?;
    Ok(())
}

/// 登录用：按用户名读出 (id, 口令哈希, 是否启用, 角色, 会话版本)；不存在返回 None。
pub async fn load_password_and_version_by_username(
    pool: &SqlitePool,
    username: &str,
) -> Result<Option<(i64, String, bool, Role, i64)>, ApiError> {
    let row: Option<(i64, String, bool, String, i64)> = sqlx::query_as(
        "SELECT id, password_hash, enabled, role, token_version FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
    .map_err(|_| ApiError::internal("查询登录凭据失败"))?;
    match row {
        Some((id, hash, enabled, role_str, ver)) => {
            let role =
                Role::from_db_str(&role_str).ok_or_else(|| ApiError::internal("用户角色值非法"))?;
            Ok(Some((id, hash, enabled, role, ver)))
        }
        None => Ok(None),
    }
}

/// 改口令用：按 id 读出当前口令哈希与会话版本；不存在返回 None。
pub async fn load_password_and_version_by_id(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<(String, i64)>, ApiError> {
    sqlx::query_as("SELECT password_hash, token_version FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|_| ApiError::internal("查询口令凭据失败"))
}

/// 仅当旧口令哈希与会话版本均未变化时更新口令并推进版本，防止覆盖并发管理员重置。
pub async fn compare_and_update_password(
    pool: &SqlitePool,
    id: i64,
    expected_hash: &str,
    expected_version: i64,
    new_hash: &str,
) -> Result<bool, ApiError> {
    let result = sqlx::query(
        "UPDATE users SET password_hash = ?, token_version = token_version + 1 \
         WHERE id = ? AND enabled = 1 AND password_hash = ? AND token_version = ?",
    )
    .bind(new_hash)
    .bind(id)
    .bind(expected_hash)
    .bind(expected_version)
    .execute(pool)
    .await
    .map_err(|_| ApiError::internal("更新口令失败"))?;
    Ok(result.rows_affected() == 1)
}

/// 测试与只读校验用：按 id 读出口令哈希；不存在返回 None。
pub async fn load_password_by_id(pool: &SqlitePool, id: i64) -> Result<Option<String>, ApiError> {
    Ok(load_password_and_version_by_id(pool, id)
        .await?
        .map(|(hash, _)| hash))
}

// ==================== API Token 仓储 ====================

/// 插入 Token 并回读 (id, created_at)。持久化只存哈希，不存明文。
pub async fn insert_token(
    pool: &SqlitePool,
    user_id: i64,
    name: &str,
    token_hash: &str,
) -> Result<(i64, String), ApiError> {
    let row: (i64, String) = sqlx::query_as(
        "INSERT INTO api_tokens (user_id, name, token_hash) VALUES (?, ?, ?) RETURNING id, created_at",
    )
    .bind(user_id)
    .bind(name)
    .bind(token_hash)
    .fetch_one(pool)
    .await
    .map_err(|_| ApiError::internal("创建 Token 失败"))?;
    Ok(row)
}

/// 按 token_hash 查归属用户 id（Bearer/Basic 中 Token 校验用）；未命中返回 None。
pub async fn find_token_owner_by_hash(
    pool: &SqlitePool,
    token_hash: &str,
) -> Result<Option<i64>, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT user_id FROM api_tokens WHERE token_hash = ?")
        .bind(token_hash)
        .fetch_optional(pool)
        .await
        .map_err(|_| ApiError::internal("查询 Token 失败"))?;
    Ok(row.map(|r| r.0))
}

/// 分页列出某用户的 Token 元数据，可选按名称子串筛选，稳定按 id 排序。返回 (当前页, 总数)。
pub async fn list_tokens_by_user(
    pool: &SqlitePool,
    user_id: i64,
    page: u32,
    page_size: u32,
    q: Option<&str>,
) -> Result<(Vec<TokenRow>, i64), ApiError> {
    let (limit, offset) = page_bounds(page, page_size);
    let pattern = like_pattern(q);
    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM api_tokens WHERE user_id = ? AND name LIKE ? ESCAPE '\\'",
    )
    .bind(user_id)
    .bind(&pattern)
    .fetch_one(pool)
    .await
    .map_err(|_| ApiError::internal("统计 Token 失败"))?;
    let rows = sqlx::query_as::<_, TokenRow>(
        "SELECT id, name, created_at FROM api_tokens WHERE user_id = ? AND name LIKE ? ESCAPE '\\' ORDER BY id LIMIT ? OFFSET ?",
    )
    .bind(user_id)
    .bind(&pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(|_| ApiError::internal("查询 Token 列表失败"))?;
    Ok((rows, total.0))
}

/// 删除本人 Token（id 与 user_id 双重限定，防越权删除他人 Token）；命中返回 true。
pub async fn delete_token(pool: &SqlitePool, id: i64, user_id: i64) -> Result<bool, ApiError> {
    let result = sqlx::query("DELETE FROM api_tokens WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|_| ApiError::internal("删除 Token 失败"))?;
    Ok(result.rows_affected() > 0)
}

// ==================== 会话撤销仓储 ====================

/// 写入会话撤销记录（幂等：已存在则忽略）。expires_at 传 Unix 秒，落库为可比较的 datetime 文本。
pub async fn insert_revoked(
    pool: &SqlitePool,
    jti_hash: &str,
    expires_at_unix: i64,
) -> Result<(), ApiError> {
    sqlx::query("INSERT OR IGNORE INTO revoked_sessions (jti_hash, expires_at) VALUES (?, datetime(?, 'unixepoch'))")
        .bind(jti_hash)
        .bind(expires_at_unix)
        .execute(pool)
        .await
        .map_err(|_| ApiError::internal("写入会话撤销记录失败"))?;
    Ok(())
}

/// 刷新会话时条件撤销旧 jti：仅当用户仍启用、会话版本仍等于声明版本且 jti 尚未撤销时插入。
/// 条件检查与插入由 SQLite 单条语句原子完成；返回 true 表示本次请求唯一取得刷新资格。
pub async fn revoke_session_for_refresh(
    pool: &SqlitePool,
    user_id: i64,
    token_version: i64,
    jti_hash: &str,
    family_hash: &str,
    expires_at_unix: i64,
) -> Result<bool, ApiError> {
    let result = sqlx::query(
        "INSERT OR IGNORE INTO revoked_sessions (jti_hash, expires_at) \
         SELECT ?, datetime(?, 'unixepoch') FROM users \
         WHERE id = ? AND enabled = 1 AND token_version = ? \
         AND NOT EXISTS (SELECT 1 FROM revoked_sessions WHERE jti_hash IN (?, ?))",
    )
    .bind(jti_hash)
    .bind(expires_at_unix)
    .bind(user_id)
    .bind(token_version)
    .bind(jti_hash)
    .bind(family_hash)
    .execute(pool)
    .await
    .map_err(|_| ApiError::internal("刷新会话撤销失败"))?;
    Ok(result.rows_affected() == 1)
}

/// 查询会话是否已被撤销。
pub async fn is_revoked(pool: &SqlitePool, jti_hash: &str) -> Result<bool, ApiError> {
    let row: (bool,) =
        sqlx::query_as("SELECT EXISTS(SELECT 1 FROM revoked_sessions WHERE jti_hash = ?)")
            .bind(jti_hash)
            .fetch_one(pool)
            .await
            .map_err(|_| ApiError::internal("查询会话撤销状态失败"))?;
    Ok(row.0)
}

/// 清理已过期的撤销记录（now 传 Unix 秒）；返回删除行数。
pub async fn cleanup_expired_revoked(pool: &SqlitePool, now_unix: i64) -> Result<u64, ApiError> {
    let result =
        sqlx::query("DELETE FROM revoked_sessions WHERE expires_at < datetime(?, 'unixepoch')")
            .bind(now_unix)
            .execute(pool)
            .await
            .map_err(|_| ApiError::internal("清理过期撤销记录失败"))?;
    Ok(result.rows_affected())
}

/// 测试辅助：内存 SQLite（单连接，确保迁移与查询同库）+ 迁移，供普通单测复用。
#[cfg(test)]
pub(crate) async fn test_pool() -> SqlitePool {
    #![allow(clippy::expect_used)]
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("创建内存测试库失败");
    MIGRATOR.run(&pool).await.expect("测试库迁移失败");
    pool
}

/// 测试辅助：多连接文件 SQLite + 迁移，供并发不变量单测复用。
#[cfg(test)]
pub(crate) async fn concurrent_test_pool() -> (SqlitePool, PathBuf) {
    #![allow(clippy::expect_used)]
    static NEXT_DATABASE_ID: AtomicU64 = AtomicU64::new(0);

    let database_id = NEXT_DATABASE_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "jianartifact-{}-{database_id}.sqlite",
        std::process::id()
    ));
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .expect("创建并发测试库失败");
    MIGRATOR.run(&pool).await.expect("并发测试库迁移失败");
    (pool, path)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::auth::password::hash_password;
    use crate::auth::token::generate_token;

    #[tokio::test]
    async fn 用户增改查往返一致() {
        let pool = test_pool().await;
        let hash = hash_password("pw").unwrap();
        let user = insert_user(&pool, "alice", &hash, Role::User, true)
            .await
            .unwrap();
        assert_eq!(user.username, "alice");
        assert!(matches!(user.role, Role::User));
        assert!(user.enabled);
        assert_eq!(user.token_version, 0);

        let by_id = find_user_by_id(&pool, user.id).await.unwrap().unwrap();
        assert_eq!(by_id.username, "alice");
        let by_name = find_user_by_username(&pool, "alice")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(by_name.id, user.id);
        assert!(find_user_by_id(&pool, 9999).await.unwrap().is_none());

        // 部分更新：启停与角色
        update_user_fields(&pool, user.id, Some(false), Some(Role::Admin), None)
            .await
            .unwrap();
        let updated = find_user_by_id(&pool, user.id).await.unwrap().unwrap();
        assert!(!updated.enabled);
        assert!(matches!(updated.role, Role::Admin));
        assert_eq!(updated.token_version, 1, "禁用应推进会话版本");
    }

    #[tokio::test]
    async fn 改口令原子递增会话版本() {
        let pool = test_pool().await;
        let user = insert_user(
            &pool,
            "bob",
            &hash_password("old").unwrap(),
            Role::User,
            true,
        )
        .await
        .unwrap();
        let new_hash = hash_password("new").unwrap();
        update_user_fields(&pool, user.id, None, None, Some(new_hash))
            .await
            .unwrap();
        let updated = find_user_by_id(&pool, user.id).await.unwrap().unwrap();
        assert_eq!(updated.token_version, 1, "写口令应同步递增版本");
    }

    #[tokio::test]
    async fn 口令条件更新拒绝覆盖并发管理员重置() {
        let pool = test_pool().await;
        let original_hash = hash_password("old").unwrap();
        let user = insert_user(&pool, "cas-user", &original_hash, Role::User, true)
            .await
            .unwrap();
        let observed = load_password_and_version_by_id(&pool, user.id)
            .await
            .unwrap()
            .unwrap();

        let admin_hash = hash_password("admin-reset").unwrap();
        update_user_fields(&pool, user.id, None, None, Some(admin_hash.clone()))
            .await
            .unwrap();
        let stale_write = compare_and_update_password(
            &pool,
            user.id,
            &observed.0,
            observed.1,
            &hash_password("self-change").unwrap(),
        )
        .await
        .unwrap();

        assert!(!stale_write, "旧快照不得覆盖并发管理员重置");
        assert_eq!(
            load_password_by_id(&pool, user.id).await.unwrap(),
            Some(admin_hash)
        );
    }

    #[tokio::test]
    async fn 分页与用户名子串筛选() {
        let pool = test_pool().await;
        for name in ["alpha", "alberta", "beta", "gamma"] {
            insert_user(&pool, name, &hash_password("pw").unwrap(), Role::User, true)
                .await
                .unwrap();
        }
        // 首页两条
        let (page1, total) = list_users(&pool, 1, 2, None).await.unwrap();
        assert_eq!(total, 4);
        assert_eq!(page1.len(), 2);
        // 次页无重叠
        let (page2, _) = list_users(&pool, 2, 2, None).await.unwrap();
        assert_ne!(page1[0].id, page2[0].id);
        // 子串筛选（大小写不敏感）
        let (filtered, ftotal) = list_users(&pool, 1, 50, Some("AL")).await.unwrap();
        assert_eq!(ftotal, 2, "alpha/alberta 命中");
        assert_eq!(filtered.len(), 2);
    }

    #[tokio::test]
    async fn like通配符按字面匹配不放大() {
        let pool = test_pool().await;
        insert_user(
            &pool,
            "a_b",
            &hash_password("pw").unwrap(),
            Role::User,
            true,
        )
        .await
        .unwrap();
        insert_user(
            &pool,
            "axb",
            &hash_password("pw").unwrap(),
            Role::User,
            true,
        )
        .await
        .unwrap();
        // `_` 应作字面下划线匹配，不得匹配到 axb
        let (rows, total) = list_users(&pool, 1, 50, Some("a_b")).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].username, "a_b");
    }

    #[tokio::test]
    async fn 统计启用管理员与递增版本() {
        let pool = test_pool().await;
        insert_user(
            &pool,
            "admin1",
            &hash_password("pw").unwrap(),
            Role::Admin,
            true,
        )
        .await
        .unwrap();
        let admin2 = insert_user(
            &pool,
            "admin2",
            &hash_password("pw").unwrap(),
            Role::Admin,
            true,
        )
        .await
        .unwrap();
        insert_user(
            &pool,
            "admin3",
            &hash_password("pw").unwrap(),
            Role::Admin,
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            count_enabled_admins(&pool).await.unwrap(),
            2,
            "仅启用的 admin 计入"
        );
        assert_eq!(count_users(&pool).await.unwrap(), 3);

        increment_token_version(&pool, admin2.id).await.unwrap();
        let after = find_user_by_id(&pool, admin2.id).await.unwrap().unwrap();
        assert_eq!(after.token_version, 1);
    }

    #[tokio::test]
    async fn token增查删且本人隔离() {
        let pool = test_pool().await;
        let owner = insert_user(
            &pool,
            "owner",
            &hash_password("pw").unwrap(),
            Role::User,
            true,
        )
        .await
        .unwrap();
        let other = insert_user(
            &pool,
            "other",
            &hash_password("pw").unwrap(),
            Role::User,
            true,
        )
        .await
        .unwrap();
        let (_, hash) = generate_token();
        let (tid, _created) = insert_token(&pool, owner.id, "ci", &hash).await.unwrap();

        assert_eq!(
            find_token_owner_by_hash(&pool, &hash).await.unwrap(),
            Some(owner.id)
        );
        let (rows, total) = list_tokens_by_user(&pool, owner.id, 1, 50, None)
            .await
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].name, "ci");
        // 他人列表为空
        let (_, other_total) = list_tokens_by_user(&pool, other.id, 1, 50, None)
            .await
            .unwrap();
        assert_eq!(other_total, 0);
        // 他人不得删除本 Token
        assert!(!delete_token(&pool, tid, other.id).await.unwrap());
        // 本人删除成功
        assert!(delete_token(&pool, tid, owner.id).await.unwrap());
        assert_eq!(find_token_owner_by_hash(&pool, &hash).await.unwrap(), None);
    }

    #[tokio::test]
    async fn 撤销写入查询与清理() {
        let pool = test_pool().await;
        let far_future = 4_102_444_800; // 2100-01-01
        insert_revoked(&pool, "hash-a", far_future).await.unwrap();
        assert!(is_revoked(&pool, "hash-a").await.unwrap());
        assert!(!is_revoked(&pool, "hash-b").await.unwrap());
        // 幂等写入不报错
        insert_revoked(&pool, "hash-a", far_future).await.unwrap();

        // 过期记录：expires_at 在过去
        insert_revoked(&pool, "hash-expired", 1).await.unwrap();
        let removed = cleanup_expired_revoked(&pool, 1_000).await.unwrap();
        assert_eq!(removed, 1, "仅过期记录被清理");
        assert!(!is_revoked(&pool, "hash-expired").await.unwrap());
        assert!(is_revoked(&pool, "hash-a").await.unwrap(), "未过期记录保留");
    }

    #[tokio::test]
    async fn 刷新撤销原子校验状态版本与唯一性() {
        let pool = test_pool().await;
        let user = insert_user(
            &pool,
            "refresh-user",
            &hash_password("old").unwrap(),
            Role::User,
            true,
        )
        .await
        .unwrap();
        let expires_at = 4_102_444_800;

        assert!(
            revoke_session_for_refresh(
                &pool,
                user.id,
                0,
                "hash-first",
                "family-active",
                expires_at,
            )
            .await
            .unwrap(),
            "首次有效刷新应取得资格"
        );
        assert!(
            !revoke_session_for_refresh(
                &pool,
                user.id,
                0,
                "hash-first",
                "family-active",
                expires_at,
            )
            .await
            .unwrap(),
            "同一 jti 不得重复取得刷新资格"
        );
        insert_revoked(&pool, "family-revoked", expires_at)
            .await
            .unwrap();
        assert!(
            !revoke_session_for_refresh(
                &pool,
                user.id,
                0,
                "hash-family-child",
                "family-revoked",
                expires_at,
            )
            .await
            .unwrap(),
            "已撤销会话家族不得继续刷新"
        );

        update_user_fields(
            &pool,
            user.id,
            None,
            None,
            Some(hash_password("new").unwrap()),
        )
        .await
        .unwrap();
        assert!(
            !revoke_session_for_refresh(
                &pool,
                user.id,
                0,
                "hash-stale",
                "family-active",
                expires_at,
            )
            .await
            .unwrap(),
            "改口令后的旧声明版本不得刷新"
        );
        assert!(!is_revoked(&pool, "hash-stale").await.unwrap());

        update_user_fields(&pool, user.id, Some(false), None, None)
            .await
            .unwrap();
        assert!(
            !revoke_session_for_refresh(
                &pool,
                user.id,
                2,
                "hash-disabled",
                "family-active",
                expires_at,
            )
            .await
            .unwrap(),
            "禁用用户不得刷新"
        );
        assert!(!is_revoked(&pool, "hash-disabled").await.unwrap());
    }

    #[tokio::test]
    async fn 零泄露_口令仅存phc_token仅存哈希() {
        let pool = test_pool().await;
        let user = insert_user(
            &pool,
            "secretive",
            &hash_password("pw").unwrap(),
            Role::User,
            true,
        )
        .await
        .unwrap();
        let stored: (String,) = sqlx::query_as("SELECT password_hash FROM users WHERE id = ?")
            .bind(user.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            stored.0.starts_with("$argon2id$"),
            "口令列应为 Argon2 PHC 串"
        );

        let (plain, hash) = generate_token();
        insert_token(&pool, user.id, "t", &hash).await.unwrap();
        let stored_hash: (String,) =
            sqlx::query_as("SELECT token_hash FROM api_tokens WHERE user_id = ?")
                .bind(user.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            stored_hash.0.len(),
            64,
            "Token 列应为 64 位 sha256 十六进制"
        );
        assert_ne!(stored_hash.0, plain, "Token 列不得为明文");
        assert!(!stored_hash.0.starts_with("jat_"), "Token 列不得含明文前缀");
    }
}
