//! JianArtifact 后端库：模块声明与应用装配。
//! 分层依赖单向：api（入口层）→ 领域服务层 → persistence/storage（基础设施层），config 只读引用。

pub mod api;
pub mod auth;
pub mod config;
pub mod domain;
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
    let username = std::env::var("JIANARTIFACT_ADMIN_USER")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    // 口令不做 trim（空白可能是口令的一部分），仅剔除完全为空
    let password = std::env::var("JIANARTIFACT_ADMIN_PASSWORD")
        .ok()
        .filter(|value| !value.is_empty());
    build_app_with_bootstrap(config, username, password).await
}

/// 使用显式管理员自举凭据装配应用，供 crate 内测试等调用。
pub(crate) async fn build_app_with_bootstrap(
    config: Config,
    username: Option<String>,
    password: Option<String>,
) -> Result<Router, StartupError> {
    // JWT 密钥属机密配置，缺失即拒绝启动（不落库、不进日志）
    if config.jwt_secret.trim().is_empty() {
        return Err(StartupError::AuthConfig(
            "缺少 JWT 会话密钥，请经 JIANARTIFACT_JWT_SECRET 环境变量提供".to_string(),
        ));
    }
    let pool = persistence::init(&config.database_url(), &config.database_path).await?;
    // 元数据库就绪后执行管理员自举（空库且提供自举变量时创建首个 admin）
    domain::bootstrap_admin_with(&pool, username, password).await?;
    let auth = std::sync::Arc::new(crate::auth::AuthRuntime::new(
        &config.jwt_secret,
        config.session_ttl_hours,
    ));
    let state = AppState {
        pool,
        config: std::sync::Arc::new(config),
        auth,
    };
    Ok(api::router(state))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::*;

    const JWT_SECRET: &str = "lib-test-jwt-secret";

    fn 测试配置(目录: &TempDir) -> Config {
        Config {
            bind: "127.0.0.1:0".to_string(),
            database_path: 目录.path().join("bootstrap.db"),
            data_dir: 目录.path().join("blobs"),
            log_level: "warn".to_string(),
            jwt_secret: JWT_SECRET.to_string(),
            session_ttl_hours: 8,
        }
    }

    async fn 发送请求(app: &Router, request: Request<Body>) -> (StatusCode, String) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    async fn 登录(app: &Router, username: &str, password: &str) -> (StatusCode, String) {
        let request = Request::post("/api/v1/auth/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({ "username": username, "password": password })).unwrap(),
            ))
            .unwrap();
        发送请求(app, request).await
    }

    #[tokio::test]
    async fn 空库有凭据创建管理员且可经路由登录() {
        let 目录 = tempfile::tempdir().unwrap();
        let 口令 = "bootstrap-secret-response-check";
        let app = build_app_with_bootstrap(
            测试配置(&目录),
            Some("bootstrap-admin".to_string()),
            Some(口令.to_string()),
        )
        .await
        .unwrap();

        let (status, login_body) = 登录(&app, "bootstrap-admin", 口令).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!login_body.contains(口令), "登录响应不得包含自举口令");
        let token = serde_json::from_str::<Value>(&login_body).unwrap()["token"]
            .as_str()
            .unwrap()
            .to_string();
        let request = Request::get("/api/v1/users")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let (status, users_body) = 发送请求(&app, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_str::<Value>(&users_body).unwrap()["items"][0]["role"],
            "admin"
        );
        assert!(!users_body.contains(口令), "用户响应不得包含自举口令");
    }

    #[tokio::test]
    async fn 表非空时新凭据不新增用户() {
        let 目录 = tempfile::tempdir().unwrap();
        let config = 测试配置(&目录);
        let first_app = build_app_with_bootstrap(
            config.clone(),
            Some("first-admin".to_string()),
            Some("first-password".to_string()),
        )
        .await
        .unwrap();
        let second_app = build_app_with_bootstrap(
            config.clone(),
            Some("second-admin".to_string()),
            Some("second-password".to_string()),
        )
        .await
        .unwrap();

        assert_eq!(
            登录(&first_app, "first-admin", "first-password").await.0,
            StatusCode::OK
        );
        assert_eq!(
            登录(&second_app, "second-admin", "second-password").await.0,
            StatusCode::UNAUTHORIZED
        );
        let pool = persistence::init(&config.database_url(), &config.database_path)
            .await
            .unwrap();
        assert_eq!(persistence::count_users(&pool).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn 空库无凭据可装配且用户接口返回未认证() {
        let 目录 = tempfile::tempdir().unwrap();
        let app = build_app_with_bootstrap(测试配置(&目录), None, None)
            .await
            .unwrap();
        let request = Request::get("/api/v1/users").body(Body::empty()).unwrap();

        let (status, _) = 发送请求(&app, request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
