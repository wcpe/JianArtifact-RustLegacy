//! 阶段 1 契约骨架集成测试：健康/就绪为真实现，业务端点为 501 占位，OpenAPI 契约面完整。

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use jianartifact_server::config::Config;
use tower::util::ServiceExt;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// 在临时目录上构建一个完整应用（真实 SQLite 文件 + 迁移）。
async fn test_app() -> Result<(Router, tempfile::TempDir), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let config = Config {
        bind: "127.0.0.1:0".to_string(),
        database_path: dir.path().join("test.db"),
        data_dir: dir.path().join("blobs"),
        log_level: "warn".to_string(),
    };
    let app = jianartifact_server::build_app(config).await?;
    Ok((app, dir))
}

/// 向应用发送一次请求并返回响应。
async fn send(
    app: &Router,
    method: &str,
    uri: &str,
) -> Result<axum::response::Response, Box<dyn std::error::Error>> {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())?;
    Ok(app.clone().oneshot(request).await?)
}

#[tokio::test]
async fn 健康探针存活即二百() -> TestResult {
    let (app, _dir) = test_app().await?;
    let resp = send(&app, "GET", "/healthz").await?;
    assert_eq!(resp.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn 就绪自检依赖齐备时二百() -> TestResult {
    let (app, _dir) = test_app().await?;
    let resp = send(&app, "GET", "/readyz").await?;
    assert_eq!(resp.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn 就绪自检数据目录不可写时五百零三() -> TestResult {
    // 把 data_dir 指到一个「文件」之下，create_dir_all 必然失败
    let dir = tempfile::tempdir()?;
    let blocker = dir.path().join("occupied");
    std::fs::write(&blocker, b"x")?;
    let config = Config {
        bind: "127.0.0.1:0".to_string(),
        database_path: dir.path().join("test.db"),
        data_dir: blocker.join("blobs"),
        log_level: "warn".to_string(),
    };
    let app = jianartifact_server::build_app(config).await?;
    let resp = send(&app, "GET", "/readyz").await?;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    Ok(())
}

#[tokio::test]
async fn 业务契约端点返回五百零一占位() -> TestResult {
    let (app, _dir) = test_app().await?;
    // 端点 × 方法：阶段 1 全部为未实现占位
    let cases = [
        ("POST", "/api/v1/auth/login"),
        ("POST", "/api/v1/auth/refresh"),
        ("POST", "/api/v1/auth/logout"),
        ("POST", "/api/v1/tokens"),
        ("GET", "/api/v1/tokens"),
        ("DELETE", "/api/v1/tokens/1"),
        ("GET", "/api/v1/users"),
        ("POST", "/api/v1/users"),
        ("PATCH", "/api/v1/users/1"),
    ];
    for (method, uri) in cases {
        let resp = send(&app, method, uri).await?;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_IMPLEMENTED,
            "{method} {uri} 应返回 501"
        );
        let body = resp.into_body().collect().await?.to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body)?;
        assert_eq!(
            json["code"], "NOT_IMPLEMENTED",
            "{method} {uri} 错误码应为占位码"
        );
    }
    Ok(())
}

#[tokio::test]
async fn 契约文档包含全部端点且可获取() -> TestResult {
    let (app, _dir) = test_app().await?;
    let resp = send(&app, "GET", "/api/v1/openapi.json").await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await?.to_bytes();
    let doc: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(doc["info"]["title"], "JianArtifact 管理 API");
    let expected_paths = [
        "/healthz",
        "/readyz",
        "/api/v1/auth/login",
        "/api/v1/auth/refresh",
        "/api/v1/auth/logout",
        "/api/v1/tokens",
        "/api/v1/tokens/{id}",
        "/api/v1/users",
        "/api/v1/users/{id}",
    ];
    for path in expected_paths {
        assert!(doc["paths"].get(path).is_some(), "OpenAPI 缺少路径 {path}");
    }
    Ok(())
}

#[tokio::test]
async fn 响应头带请求标识() -> TestResult {
    let (app, _dir) = test_app().await?;
    let resp = send(&app, "GET", "/healthz").await?;
    assert!(
        resp.headers().contains_key("x-request-id"),
        "响应应携带 x-request-id"
    );
    Ok(())
}

#[tokio::test]
async fn 迁移建立认证三表() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("test.db");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = jianartifact_server::persistence::init(&url, &db_path).await?;
    for table in ["users", "api_tokens", "revoked_sessions"] {
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?")
                .bind(table)
                .fetch_one(&pool)
                .await?;
        assert_eq!(row.0, 1, "迁移后应存在表 {table}");
    }
    Ok(())
}
