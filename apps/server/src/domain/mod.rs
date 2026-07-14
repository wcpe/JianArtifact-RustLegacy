//! 领域服务层：角色类型与用户服务（自举、增改查、口令修改与最后 Admin 保护）。
//! 依赖方向：domain（领域）→ auth（认证原语）/ persistence（基础设施），不反向依赖 api 入口层。

pub mod artifact;
pub mod repository;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::ToSchema;

use crate::auth::password::{hash_password_async, verify_password_async};
use crate::error::{ApiError, StartupError};
use crate::persistence::{self, User};

/// 全局角色。落库为 users.role（CHECK 约束取值 'admin'/'user'），序列化为小写。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// 管理员：全局管理权限。
    Admin,
    /// 普通用户：按仓库 ACL 访问。
    User,
}

impl Role {
    /// 数据库存储值（users.role CHECK 约束取值 'admin'/'user'）。
    pub fn as_db_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
        }
    }

    /// 从数据库存储值解析角色；非法值返回 None，由调用方转为内部错误。
    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "admin" => Some(Role::Admin),
            "user" => Some(Role::User),
            _ => None,
        }
    }
}

/// 创建用户：重名先判 409；口令 Argon2 哈希后入库，默认启用。
pub async fn create_user(
    pool: &SqlitePool,
    username: &str,
    password: &str,
    role: Role,
) -> Result<User, ApiError> {
    let hash = hash_password_async(password).await?;
    persistence::insert_user(pool, username, &hash, role, true).await
}

/// 更新用户启停 / 角色 / 口令。字段可选，只更新给出的项。
/// 保护：将禁用或降级最后一名启用管理员时拒绝；重置口令会原子递增会话版本（撤销既有会话）。
pub async fn update_user(
    pool: &SqlitePool,
    target_id: i64,
    enabled: Option<bool>,
    role: Option<Role>,
    password: Option<String>,
) -> Result<User, ApiError> {
    if persistence::find_user_by_id(pool, target_id)
        .await?
        .is_none()
    {
        return Err(ApiError::not_found("AUTH_USER_NOT_FOUND", "用户不存在"));
    }

    // Argon2 计算在任何写事务之外完成
    let password_hash = match password {
        Some(plain) => Some(hash_password_async(&plain).await?),
        None => None,
    };
    let updated =
        persistence::update_user_preserving_admin(pool, target_id, enabled, role, password_hash)
            .await?;
    if !updated {
        return Err(ApiError::forbidden(
            "AUTH_LAST_ADMIN",
            "不能禁用或降级最后一名启用的管理员",
        ));
    }

    persistence::find_user_by_id(pool, target_id)
        .await?
        .ok_or_else(|| ApiError::internal("更新后未能读回用户"))
}

/// 修改本人口令：校验旧口令，通过后写入新口令并原子递增会话版本（旧口令与既有会话立即失效）。
pub async fn change_own_password(
    pool: &SqlitePool,
    user_id: i64,
    old_password: &str,
    new_password: &str,
) -> Result<(), ApiError> {
    // 缺用户按凭据无效处理，不泄露账户存在性
    let (current_hash, current_version) =
        persistence::load_password_and_version_by_id(pool, user_id)
            .await?
            .ok_or_else(|| ApiError::unauthorized("AUTH_INVALID_CREDENTIALS", "旧口令错误"))?;
    if !verify_password_async(old_password, &current_hash).await? {
        return Err(ApiError::unauthorized(
            "AUTH_INVALID_CREDENTIALS",
            "旧口令错误",
        ));
    }
    let new_hash = hash_password_async(new_password).await?;
    let updated = persistence::compare_and_update_password(
        pool,
        user_id,
        &current_hash,
        current_version,
        &new_hash,
    )
    .await?;
    if !updated {
        return Err(ApiError::unauthorized(
            "AUTH_CREDENTIALS_CHANGED",
            "口令已被其他请求修改，请重新登录后重试",
        ));
    }
    Ok(())
}

/// 分页列出用户（薄封装仓储分页）。
pub async fn list_users(
    pool: &SqlitePool,
    page: u32,
    page_size: u32,
    q: Option<&str>,
) -> Result<(Vec<User>, i64), ApiError> {
    persistence::list_users(pool, page, page_size, q).await
}

/// 自举核心：仅当用户表为空且凭据齐备时创建首个 admin；表非空则跳过并告警。
/// 任何分支都不记录凭据值。
pub(crate) async fn bootstrap_admin_with(
    pool: &SqlitePool,
    username: Option<String>,
    password: Option<String>,
) -> Result<(), StartupError> {
    let count = persistence::count_users(pool)
        .await
        .map_err(|_| StartupError::AuthConfig("管理员自举读取用户数失败".to_string()))?;
    if count > 0 {
        tracing::warn!("检测到已有用户，跳过管理员自举");
        return Ok(());
    }
    match (username, password) {
        (Some(username), Some(password)) => {
            let hash = hash_password_async(&password)
                .await
                .map_err(|_| StartupError::AuthConfig("管理员自举口令哈希失败".to_string()))?;
            let inserted = persistence::insert_admin_if_users_empty(pool, &username, &hash)
                .await
                .map_err(|_| StartupError::AuthConfig("管理员自举写入用户失败".to_string()))?;
            if inserted {
                tracing::info!("已完成管理员自举");
            } else {
                tracing::warn!("检测到已有用户，跳过管理员自举");
            }
            Ok(())
        }
        _ => {
            tracing::info!("数据库无用户且未提供自举变量，服务照常运行但暂无账户");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::Arc;

    use axum::http::StatusCode;
    use tokio::sync::Barrier;

    use super::*;
    use crate::auth::password::verify_password;
    use crate::persistence::{
        concurrent_test_pool, count_enabled_admins, count_users, find_user_by_id,
        find_user_by_username, test_pool,
    };

    #[tokio::test]
    async fn 创建重名返回409() {
        let pool = test_pool().await;
        create_user(&pool, "dup", "pw", Role::User).await.unwrap();
        let err = create_user(&pool, "dup", "pw2", Role::Admin)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "AUTH_USERNAME_TAKEN");
        assert_eq!(err.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn 更新不存在用户返回404() {
        let pool = test_pool().await;
        let err = update_user(&pool, 123, Some(false), None, None)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "AUTH_USER_NOT_FOUND");
        assert_eq!(err.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn 最后一名启用管理员禁用与降级均被拒() {
        let pool = test_pool().await;
        let admin = create_user(&pool, "root", "pw", Role::Admin).await.unwrap();
        // 唯一启用 admin：禁用被拒
        let err1 = update_user(&pool, admin.id, Some(false), None, None)
            .await
            .unwrap_err();
        assert_eq!(err1.code(), "AUTH_LAST_ADMIN");
        assert_eq!(err1.status(), StatusCode::FORBIDDEN);
        // 降级被拒
        let err2 = update_user(&pool, admin.id, None, Some(Role::User), None)
            .await
            .unwrap_err();
        assert_eq!(err2.code(), "AUTH_LAST_ADMIN");
    }

    #[tokio::test]
    async fn 存在第二名管理员后可禁用第一名() {
        let pool = test_pool().await;
        let admin1 = create_user(&pool, "root1", "pw", Role::Admin)
            .await
            .unwrap();
        create_user(&pool, "root2", "pw", Role::Admin)
            .await
            .unwrap();
        let updated = update_user(&pool, admin1.id, Some(false), None, None)
            .await
            .unwrap();
        assert!(!updated.enabled);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn 并发创建同名用户仅一项成功且另一项返回409() {
        let (pool, path) = concurrent_test_pool().await;
        let barrier = Arc::new(Barrier::new(2));
        let first_pool = pool.clone();
        let second_pool = pool.clone();
        let first_barrier = Arc::clone(&barrier);
        let second_barrier = Arc::clone(&barrier);

        let first = tokio::spawn(async move {
            first_barrier.wait().await;
            create_user(&first_pool, "parallel-user", "pw-1", Role::User).await
        });
        let second = tokio::spawn(async move {
            second_barrier.wait().await;
            create_user(&second_pool, "parallel-user", "pw-2", Role::User).await
        });
        let (first_result, second_result) = tokio::join!(first, second);
        let results = [first_result.unwrap(), second_result.unwrap()];

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let error = results
            .into_iter()
            .find_map(Result::err)
            .expect("并发创建应有一项因重名被拒");
        assert_eq!(error.code(), "AUTH_USERNAME_TAKEN");
        assert_eq!(error.status(), StatusCode::CONFLICT);

        pool.close().await;
        tokio::fs::remove_file(path).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn 两名启用管理员并发退出角色仅一项成功() {
        let (pool, path) = concurrent_test_pool().await;
        let admin1 = create_user(&pool, "parallel-admin-1", "pw", Role::Admin)
            .await
            .unwrap();
        let admin2 = create_user(&pool, "parallel-admin-2", "pw", Role::Admin)
            .await
            .unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let pool1 = pool.clone();
        let pool2 = pool.clone();
        let barrier1 = Arc::clone(&barrier);
        let barrier2 = Arc::clone(&barrier);

        let disable = tokio::spawn(async move {
            barrier1.wait().await;
            update_user(&pool1, admin1.id, Some(false), None, None).await
        });
        let demote = tokio::spawn(async move {
            barrier2.wait().await;
            update_user(&pool2, admin2.id, None, Some(Role::User), None).await
        });
        let (disable_result, demote_result) = tokio::join!(disable, demote);

        let results = [disable_result.unwrap(), demote_result.unwrap()];
        assert_eq!(
            results.iter().filter(|result| result.is_ok()).count(),
            1,
            "并发更新应仅一项成功"
        );
        let error = results
            .into_iter()
            .find_map(Result::err)
            .expect("并发更新应有一项被拒");
        assert_eq!(error.code(), "AUTH_LAST_ADMIN");
        assert_eq!(count_enabled_admins(&pool).await.unwrap(), 1);

        pool.close().await;
        tokio::fs::remove_file(path).await.unwrap();
    }

    #[tokio::test]
    async fn 仅改口令的更新不触发最后管理员保护并递增版本() {
        let pool = test_pool().await;
        let admin = create_user(&pool, "solo", "old", Role::Admin)
            .await
            .unwrap();
        let updated = update_user(&pool, admin.id, None, None, Some("new".to_string()))
            .await
            .unwrap();
        assert_eq!(updated.token_version, 1, "重置口令应递增会话版本");
        let hash = persistence::load_password_by_id(&pool, admin.id)
            .await
            .unwrap()
            .unwrap();
        assert!(verify_password("new", &hash), "新口令应可校验");
    }

    #[tokio::test]
    async fn 改本人口令校验旧口令且递增版本() {
        let pool = test_pool().await;
        let user = create_user(&pool, "self", "old", Role::User).await.unwrap();
        // 旧口令错误
        let err = change_own_password(&pool, user.id, "wrong", "new")
            .await
            .unwrap_err();
        assert_eq!(err.code(), "AUTH_INVALID_CREDENTIALS");
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
        // 正确旧口令 → 成功且版本递增
        change_own_password(&pool, user.id, "old", "new")
            .await
            .unwrap();
        let after = find_user_by_id(&pool, user.id).await.unwrap().unwrap();
        assert_eq!(after.token_version, 1);
        let hash = persistence::load_password_by_id(&pool, user.id)
            .await
            .unwrap()
            .unwrap();
        assert!(verify_password("new", &hash));
        assert!(!verify_password("old", &hash), "旧口令应失效");
    }

    #[tokio::test]
    async fn 自举空库有凭据创建可登录管理员() {
        let pool = test_pool().await;
        bootstrap_admin_with(&pool, Some("admin".to_string()), Some("secret".to_string()))
            .await
            .unwrap();
        assert_eq!(count_users(&pool).await.unwrap(), 1);
        let u = find_user_by_username(&pool, "admin")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(u.role, Role::Admin));
        // 可登录校验：读回凭据并验签口令
        let (_, hash, enabled, role, _) =
            persistence::load_password_and_version_by_username(&pool, "admin")
                .await
                .unwrap()
                .unwrap();
        assert!(enabled);
        assert!(matches!(role, Role::Admin));
        assert!(verify_password("secret", &hash));
    }

    #[tokio::test]
    async fn 自举表非空跳过不新增() {
        let pool = test_pool().await;
        create_user(&pool, "existing", "pw", Role::User)
            .await
            .unwrap();
        bootstrap_admin_with(&pool, Some("admin".to_string()), Some("secret".to_string()))
            .await
            .unwrap();
        assert_eq!(count_users(&pool).await.unwrap(), 1, "表非空不应新增");
        assert!(
            find_user_by_username(&pool, "admin")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn 并发自举空库最多创建一个管理员() {
        let (pool, path) = concurrent_test_pool().await;
        let barrier = Arc::new(Barrier::new(2));
        let pool1 = pool.clone();
        let pool2 = pool.clone();
        let barrier1 = Arc::clone(&barrier);
        let barrier2 = Arc::clone(&barrier);

        let first = tokio::spawn(async move {
            barrier1.wait().await;
            bootstrap_admin_with(
                &pool1,
                Some("bootstrap-admin-1".to_string()),
                Some("secret-1".to_string()),
            )
            .await
        });
        let second = tokio::spawn(async move {
            barrier2.wait().await;
            bootstrap_admin_with(
                &pool2,
                Some("bootstrap-admin-2".to_string()),
                Some("secret-2".to_string()),
            )
            .await
        });
        let (first_result, second_result) = tokio::join!(first, second);

        first_result.unwrap().unwrap();
        second_result.unwrap().unwrap();
        assert_eq!(count_users(&pool).await.unwrap(), 1);
        assert_eq!(count_enabled_admins(&pool).await.unwrap(), 1);

        pool.close().await;
        tokio::fs::remove_file(path).await.unwrap();
    }

    #[tokio::test]
    async fn 自举空库无凭据保持零用户() {
        let pool = test_pool().await;
        bootstrap_admin_with(&pool, None, None).await.unwrap();
        assert_eq!(count_users(&pool).await.unwrap(), 0);
        // 仅缺其一同样不创建
        bootstrap_admin_with(&pool, Some("admin".to_string()), None)
            .await
            .unwrap();
        assert_eq!(count_users(&pool).await.unwrap(), 0);
    }
}
