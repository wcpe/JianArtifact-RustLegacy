//! 应用共享状态：连接池与运行配置，注入各 handler。

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::config::Config;

/// axum 各 handler 共享的应用状态（克隆为廉价的引用计数）。
#[derive(Clone)]
pub struct AppState {
    /// 元数据库连接池。
    pub pool: SqlitePool,
    /// 运行配置（只读共享）。
    pub config: Arc<Config>,
    /// 认证运行时（JWT 密钥、会话时长、Basic 口令缓存）。
    pub auth: Arc<crate::auth::AuthRuntime>,
}
