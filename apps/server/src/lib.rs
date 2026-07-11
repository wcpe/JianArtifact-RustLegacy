//! JianArtifact 后端库：模块声明与应用装配。
//! 分层依赖单向：api（入口层）→ 领域服务层 → persistence/storage（基础设施层），config 只读引用。

pub mod api;
pub mod config;
pub mod error;
pub mod health;
pub mod logging;
pub mod persistence;
pub mod state;

use axum::Router;

use crate::config::Config;
use crate::error::StartupError;
use crate::state::AppState;

/// 按配置初始化基础设施并装配应用路由（测试与 main 共用的组装入口）。
pub async fn build_app(config: Config) -> Result<Router, StartupError> {
    let pool = persistence::init(&config.database_url(), &config.database_path).await?;
    let state = AppState {
        pool,
        config: std::sync::Arc::new(config),
    };
    Ok(api::router(state))
}
