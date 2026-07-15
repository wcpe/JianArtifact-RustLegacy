//! 仓库使用片段真实 HTTP 集成测试。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use jianartifact_server::api::router;
use jianartifact_server::artifact::ArtifactService;
use jianartifact_server::auth::AuthRuntime;
use jianartifact_server::config::Config;
use jianartifact_server::domain::{Role, create_user};
use jianartifact_server::persistence;
use jianartifact_server::state::AppState;
use jianartifact_server::storage::BlobStore;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

const JWT_SECRET: &str = "usage-test-jwt-secret";
const ADMIN_PASSWORD: &str = "usage-admin-sensitive";

struct TestContext {
    app: Router,
    pool: SqlitePool,
    _dir: TempDir,
}

async fn setup(public_base_url: Option<&str>) -> TestContext {
    let dir = tempfile::tempdir().unwrap();
    let database_path = dir.path().join("usage.db");
    let database_url = format!("sqlite://{}?mode=rwc", database_path.display());
    let pool = persistence::init(&database_url, &database_path)
        .await
        .unwrap();
    create_user(&pool, "admin", ADMIN_PASSWORD, Role::Admin)
        .await
        .unwrap();
    create_user(&pool, "reader", "reader-sensitive", Role::User)
        .await
        .unwrap();
    create_user(&pool, "outsider", "outsider-sensitive", Role::User)
        .await
        .unwrap();
    let config = Config {
        bind: "127.0.0.1:0".to_string(),
        database_path,
        data_dir: dir.path().join("blobs"),
        log_level: "warn".to_string(),
        jwt_secret: JWT_SECRET.to_string(),
        session_ttl_hours: 8,
        public_base_url: public_base_url.map(str::to_string),
        maven_metadata_ttl_secs: 1800,
        npm_packument_ttl_secs: 1800,
        npm_publish_max_bytes: 134_217_728,
        max_artifact_bytes: 5 * 1024 * 1024 * 1024,
        min_free_bytes: 0,
        min_free_percent: 0,
    };
    let blob_store = Arc::new(BlobStore::new(
        config.data_dir.clone(),
        config.max_artifact_bytes,
        config.min_free_bytes,
        config.min_free_percent,
    ));
    blob_store.initialize().await.unwrap();
    let artifacts = Arc::new(ArtifactService::new(
        pool.clone(),
        blob_store.as_ref().clone(),
    ));
    let state = AppState {
        pool: pool.clone(),
        config: Arc::new(config),
        auth: Arc::new(AuthRuntime::new(JWT_SECRET, 8)),
        blob_store,
        artifacts,
    };
    TestContext {
        app: router(state),
        pool,
        _dir: dir,
    }
}

async fn request(
    app: &Router,
    method: &str,
    uri: &str,
    auth: Option<&str>,
    host: Option<&str>,
    forwarded_proto: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(value) = auth {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    if let Some(value) = host {
        builder = builder.header(header::HOST, value);
    }
    if let Some(value) = forwarded_proto {
        builder = builder.header("x-forwarded-proto", value);
    }
    let body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&value).unwrap())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, bytes.to_vec())
}

async fn login(app: &Router, username: &str, password: &str) -> String {
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some("internal.invalid"),
        None,
        Some(json!({"username": username, "password": password})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body).unwrap();
    format!("Bearer {}", value["token"].as_str().unwrap())
}

async fn create_repo(app: &Router, admin: &str, name: &str, format: &str, visibility: &str) {
    let (status, _) = request(
        app,
        "POST",
        "/api/v1/repos",
        Some(admin),
        Some("internal.invalid"),
        None,
        Some(json!({
            "name": name,
            "format": format,
            "repo_type": "hosted",
            "visibility": visibility
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

async fn insert_repository(pool: &SqlitePool, name: &str, format: &str, repo_type: &str) {
    let overwrite_policy = (repo_type == "hosted").then_some("allow");
    let upstream_url = (repo_type == "proxy").then_some("https://upstream.example.test");
    sqlx::query(
        "INSERT INTO repositories (name, format, repo_type, visibility, overwrite_policy, upstream_url) VALUES (?, ?, ?, 'public', ?, ?)",
    )
    .bind(name)
    .bind(format)
    .bind(repo_type)
    .bind(overwrite_policy)
    .bind(upstream_url)
    .execute(pool)
    .await
    .unwrap();
}

fn snippets(body: &[u8]) -> Vec<Value> {
    serde_json::from_slice(body).unwrap()
}

fn snippet<'a>(items: &'a [Value], title: &str) -> &'a str {
    items.iter().find(|item| item["title"] == title).unwrap()["content"]
        .as_str()
        .unwrap()
}

#[tokio::test]
async fn raw生成上传下载且公开基址优先于请求头() {
    let context = setup(Some("https://packages.example.test/root/")).await;
    let admin = login(&context.app, "admin", ADMIN_PASSWORD).await;
    create_repo(&context.app, &admin, "raw-files", "raw", "public").await;

    let (status, body) = request(
        &context.app,
        "GET",
        "/api/v1/repos/raw-files/usage",
        None,
        Some("ignored.example.test:8443"),
        Some("http"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = snippets(&body);
    assert_eq!(items.len(), 2);
    for title in ["curl 上传", "curl 下载"] {
        let content = snippet(&items, title);
        assert!(content.contains("https://packages.example.test/root/repository/raw-files/"));
        assert!(content.contains("<用户名>:<令牌>"));
        assert!(!content.contains("ignored.example.test"));
        assert!(!content.contains(ADMIN_PASSWORD));
    }
}

#[tokio::test]
async fn maven覆盖四类配置并从请求协议与host生成地址() {
    let context = setup(None).await;
    let admin = login(&context.app, "admin", ADMIN_PASSWORD).await;
    create_repo(&context.app, &admin, "maven-releases", "maven", "public").await;

    let (status, body) = request(
        &context.app,
        "GET",
        "/api/v1/repos/maven-releases/usage",
        None,
        Some("repo.example.test:9443"),
        Some("https"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = snippets(&body);
    for title in [
        "pom.xml 仓库配置",
        "build.gradle（Groovy DSL）",
        "build.gradle.kts（Kotlin DSL）",
    ] {
        let content = snippet(&items, title);
        assert!(content.contains("https://repo.example.test:9443/repository/maven-releases"));
    }
    let credentials = snippet(&items, "settings.xml 凭据");
    assert!(credentials.contains("<用户名>"));
    assert!(credentials.contains("<令牌>"));
}

#[tokio::test]
async fn npm覆盖两类配置与三客户端安装发布() {
    let context = setup(None).await;
    let admin = login(&context.app, "admin", ADMIN_PASSWORD).await;
    create_repo(&context.app, &admin, "npm-hosted", "npm", "public").await;

    let (status, body) = request(
        &context.app,
        "GET",
        "/api/v1/repos/npm-hosted/usage",
        None,
        Some("npm.example.test"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = snippets(&body);
    for title in [
        ".npmrc 配置",
        ".yarnrc.yml 配置",
        "npm 安装与发布",
        "yarn 安装与发布",
        "pnpm 安装与发布",
    ] {
        let content = snippet(&items, title);
        assert!(content.contains("http://npm.example.test/repository/npm-hosted"));
    }
    assert!(snippet(&items, ".npmrc 配置").contains("<令牌>"));
    assert!(snippet(&items, ".yarnrc.yml 配置").contains("<令牌>"));
    assert!(snippet(&items, "npm 安装与发布").contains("npm publish"));
    assert!(snippet(&items, "yarn 安装与发布").contains("yarn npm publish"));
    assert!(snippet(&items, "pnpm 安装与发布").contains("pnpm publish"));
}

#[tokio::test]
async fn proxy与group仅生成只读或安装片段() {
    let context = setup(None).await;
    for (name, format, repo_type) in [
        ("raw-proxy", "raw", "proxy"),
        ("raw-group", "raw", "group"),
        ("maven-proxy", "maven", "proxy"),
        ("maven-group", "maven", "group"),
        ("npm-proxy", "npm", "proxy"),
        ("npm-group", "npm", "group"),
    ] {
        insert_repository(&context.pool, name, format, repo_type).await;
        let (status, body) = request(
            &context.app,
            "GET",
            &format!("/api/v1/repos/{name}/usage"),
            None,
            Some("repo.example.test"),
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let items = snippets(&body);
        let content = String::from_utf8(body).unwrap();
        match format {
            "raw" => {
                assert!(items.iter().any(|item| item["title"] == "curl 下载"));
                assert!(items.iter().all(|item| item["title"] != "curl 上传"));
                assert!(!content.contains(" -T "));
            }
            "maven" => {
                assert!(items.iter().any(|item| item["title"] == "pom.xml 仓库配置"));
                assert!(
                    items
                        .iter()
                        .any(|item| item["title"] == "settings.xml 凭据")
                );
            }
            "npm" => {
                for client in ["npm", "yarn", "pnpm"] {
                    assert!(
                        items
                            .iter()
                            .any(|item| item["title"] == format!("{client} 安装"))
                    );
                }
                assert!(!content.contains("publish"));
            }
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
async fn private先授权read并隐藏于无权用户() {
    let context = setup(None).await;
    let admin = login(&context.app, "admin", ADMIN_PASSWORD).await;
    let reader = login(&context.app, "reader", "reader-sensitive").await;
    let outsider = login(&context.app, "outsider", "outsider-sensitive").await;
    create_repo(&context.app, &admin, "private-usage", "raw", "private").await;
    let (grant_status, _) = request(
        &context.app,
        "PUT",
        "/api/v1/repos/private-usage/acl/reader",
        Some(&admin),
        Some("repo.example.test"),
        None,
        Some(json!({"permission": "read"})),
    )
    .await;
    assert_eq!(grant_status, StatusCode::OK);

    let readable = request(
        &context.app,
        "GET",
        "/api/v1/repos/private-usage/usage",
        Some(&reader),
        Some("repo.example.test"),
        None,
        None,
    )
    .await;
    assert_eq!(readable.0, StatusCode::OK);
    let hidden = request(
        &context.app,
        "GET",
        "/api/v1/repos/private-usage/usage",
        Some(&outsider),
        Some("repo.example.test"),
        None,
        None,
    )
    .await;
    let missing = request(
        &context.app,
        "GET",
        "/api/v1/repos/not-existing/usage",
        Some(&outsider),
        Some("repo.example.test"),
        None,
        None,
    )
    .await;
    assert_eq!(hidden.0, StatusCode::NOT_FOUND);
    assert_eq!(hidden, missing);
}

#[tokio::test]
async fn 停用仓库在管理面授权后仍可获取且不泄露敏感值() {
    let context = setup(None).await;
    let admin = login(&context.app, "admin", ADMIN_PASSWORD).await;
    create_repo(&context.app, &admin, "disabled-usage", "npm", "private").await;
    let (patch_status, _) = request(
        &context.app,
        "PATCH",
        "/api/v1/repos/disabled-usage",
        Some(&admin),
        Some("repo.example.test"),
        None,
        Some(json!({"disabled": true})),
    )
    .await;
    assert_eq!(patch_status, StatusCode::OK);

    let (status, body) = request(
        &context.app,
        "GET",
        "/api/v1/repos/disabled-usage/usage",
        Some(&admin),
        Some("repo.example.test"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body).unwrap();
    for secret in [
        ADMIN_PASSWORD,
        JWT_SECRET,
        "reader-sensitive",
        "outsider-sensitive",
    ] {
        assert!(!text.contains(secret));
    }
    assert!(text.contains("<令牌>"));
}
