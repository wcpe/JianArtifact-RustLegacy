//! 编译期内嵌前端静态资源与路由优先级集成测试。
#![cfg(feature = "embedded-web")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use jianartifact_server::build_app;
use jianartifact_server::config::Config;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

struct TestContext {
    app: Router,
    _dir: TempDir,
}

async fn setup() -> TestContext {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        bind: "127.0.0.1:0".to_string(),
        database_path: dir.path().join("static-web.db"),
        data_dir: dir.path().join("blobs"),
        log_level: "warn".to_string(),
        jwt_secret: "static-web-test-secret".to_string(),
        session_ttl_hours: 8,
        public_base_url: None,
        maven_metadata_ttl_secs: 1800,
        npm_packument_ttl_secs: 1800,
        npm_publish_max_bytes: 134_217_728,
        max_artifact_bytes: 1024 * 1024,
        min_free_bytes: 0,
        min_free_percent: 0,
    };
    TestContext {
        app: build_app(config).await.unwrap(),
        _dir: dir,
    }
}

async fn get(app: &Router, path: &str) -> (StatusCode, Option<String>, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, content_type, body.to_vec())
}

#[tokio::test]
async fn 首页与spa前端路由返回内嵌index() {
    let context = setup().await;
    let root = get(&context.app, "/").await;
    let route = get(&context.app, "/repos/example").await;

    assert_eq!(root.0, StatusCode::OK);
    assert_eq!(root.1.as_deref(), Some("text/html"));
    assert!(String::from_utf8_lossy(&root.2).contains("<div id=\"root\"></div>"));
    assert_eq!(route.0, StatusCode::OK);
    assert_eq!(route.1.as_deref(), Some("text/html"));
    assert_eq!(route.2, root.2);
}

#[tokio::test]
async fn 静态文件返回正确mime且无需运行vite() {
    let context = setup().await;
    let index = get(&context.app, "/").await;
    let html = String::from_utf8(index.2).unwrap();
    let asset_path = html
        .split('"')
        .find(|value| value.starts_with("/assets/") && value.ends_with(".js"))
        .unwrap();
    let asset = get(&context.app, asset_path).await;
    let icon = get(&context.app, "/favicon.svg").await;

    assert_eq!(asset.0, StatusCode::OK);
    assert_eq!(asset.1.as_deref(), Some("text/javascript"));
    assert!(!asset.2.is_empty());
    assert_eq!(icon.0, StatusCode::OK);
    assert_eq!(icon.1.as_deref(), Some("image/svg+xml"));
}

#[tokio::test]
async fn api与协议路由优先且未知api保持json四百零四() {
    let context = setup().await;
    let health = get(&context.app, "/healthz").await;
    let unknown_api = get(&context.app, "/api/v1/not-existing").await;
    let unknown_protocol = get(&context.app, "/repository/not-existing/file.bin").await;

    assert_eq!(health.0, StatusCode::OK);
    assert!(!String::from_utf8_lossy(&health.2).contains("<html"));
    assert_eq!(unknown_api.0, StatusCode::NOT_FOUND);
    assert_eq!(unknown_api.1.as_deref(), Some("application/json"));
    let api_error: Value = serde_json::from_slice(&unknown_api.2).unwrap();
    assert_eq!(api_error["code"], "NOT_FOUND");
    assert_eq!(unknown_protocol.0, StatusCode::NOT_FOUND);
    assert_eq!(unknown_protocol.1.as_deref(), Some("application/json"));
    assert!(!String::from_utf8_lossy(&unknown_protocol.2).contains("<html"));
}

#[tokio::test]
async fn 不存在的静态资源不回退spa_html() {
    let context = setup().await;
    let response = get(&context.app, "/assets/not-existing.js").await;

    assert_eq!(response.0, StatusCode::NOT_FOUND);
    assert_eq!(response.1.as_deref(), Some("application/json"));
    assert!(!String::from_utf8_lossy(&response.2).contains("<html"));
}
