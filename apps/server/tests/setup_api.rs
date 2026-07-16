//! 首启初始化接口与令牌文件集成测试。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use jianartifact_server::config::Config;
use jianartifact_server::domain::{Role, create_user};
use jianartifact_server::persistence;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::util::ServiceExt;

const TOKEN_FILE: &str = "initial-admin.token";

fn config(dir: &TempDir) -> Config {
    Config {
        bind: "127.0.0.1:0".to_string(),
        database_path: dir.path().join("metadata/test.db"),
        data_dir: dir.path().join("blobs"),
        log_level: "warn".to_string(),
        jwt_secret: "setup-test-jwt-secret".to_string(),
        session_ttl_hours: 8,
        public_base_url: None,
        maven_metadata_ttl_secs: 1800,
        npm_packument_ttl_secs: 1800,
        npm_publish_max_bytes: 134_217_728,
        max_artifact_bytes: 5 * 1024 * 1024 * 1024,
        min_free_bytes: 0,
        min_free_percent: 0,
    }
}

fn token_path(config: &Config) -> PathBuf {
    config
        .database_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(TOKEN_FILE)
}

async fn empty_app() -> (Router, TempDir, Config, String) {
    let dir = tempfile::tempdir().expect("创建临时目录");
    let config = config(&dir);
    let app = jianartifact_server::build_app(config.clone())
        .await
        .expect("构建空库应用");
    let token = std::fs::read_to_string(token_path(&config)).expect("读取首启令牌");
    (app, dir, config, token)
}

async fn request(app: &Router, method: &str, body: Option<Value>) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(match method {
        "GET" => "/api/v1/setup/status",
        _ => "/api/v1/setup",
    });
    let body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&value).unwrap())
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn raw_setup_request(
    app: &Router,
    content_type: Option<&str>,
    body: &'static str,
) -> axum::response::Response {
    let mut builder = Request::post("/api/v1/setup");
    if let Some(value) = content_type {
        builder = builder.header(header::CONTENT_TYPE, value);
    }
    app.clone()
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn 数据库路径含url保留字符时仍按文件路径启动() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(&dir);
    config.database_path = dir.path().join("metadata?/实例#一.db");

    let _app = jianartifact_server::build_app(config.clone())
        .await
        .unwrap();

    assert!(config.database_path.exists());
    assert!(token_path(&config).exists());
}

#[tokio::test]
async fn 空库创建并复用六十四位令牌且状态禁止缓存() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(&dir);
    let path = token_path(&config);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let reused = "a".repeat(64);
    std::fs::write(&path, &reused).unwrap();

    let app = jianartifact_server::build_app(config.clone())
        .await
        .unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), reused);
    let response = request(&app, "GET", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(json_body(response).await, json!({ "setup_required": true }));
}

#[cfg(unix)]
#[tokio::test]
async fn unix复用令牌时把零六四四权限修正为零六零零() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let config = config(&dir);
    let path = token_path(&config);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "a".repeat(64)).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let _app = jianartifact_server::build_app(config).await.unwrap();

    let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[tokio::test]
async fn 非法既有令牌拒绝启动且错误不泄露路径或内容() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(&dir);
    let path = token_path(&config);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let invalid = "INVALID-TOKEN-CONTENT";
    std::fs::write(&path, invalid).unwrap();

    let result = jianartifact_server::build_app(config.clone()).await;
    assert!(result.is_err(), "非法令牌必须拒绝启动");
    let text = match result {
        Err(error) => error.to_string(),
        Ok(_) => String::new(),
    };

    assert!(text.contains("首启初始化令牌文件无效"));
    assert!(!text.contains(invalid));
    assert!(!text.contains(&path.display().to_string()));
}

#[cfg(unix)]
#[tokio::test]
async fn linux新建令牌权限为零六零零() {
    use std::os::unix::fs::PermissionsExt;

    let (_app, _dir, config, token) = empty_app().await;
    assert_eq!(token.len(), 64);
    assert!(
        token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    let mode = std::fs::metadata(token_path(&config))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[cfg(unix)]
#[tokio::test]
async fn 运行期令牌权限变为零六四四时状态为false且提交不可用() {
    use std::os::unix::fs::PermissionsExt;

    let (app, _dir, config, token) = empty_app().await;
    let path = token_path(&config);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let status = request(&app, "GET", None).await;
    assert_eq!(status.status(), StatusCode::OK);
    assert_eq!(status.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(json_body(status).await, json!({ "setup_required": false }));
    let unavailable = request(
        &app,
        "POST",
        Some(json!({
            "token": token,
            "username": "admin",
            "password": "密".repeat(12)
        })),
    )
    .await;
    assert_eq!(unavailable.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(unavailable).await["code"], "SETUP_UNAVAILABLE");
}

#[tokio::test]
async fn 运行期令牌缺失或格式无效时状态为false且提交不可用() {
    let (app, _dir, config, _token) = empty_app().await;
    let path = token_path(&config);
    std::fs::remove_file(&path).unwrap();

    let missing = request(&app, "GET", None).await;
    assert_eq!(missing.status(), StatusCode::OK);
    assert_eq!(missing.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(json_body(missing).await, json!({ "setup_required": false }));

    std::fs::write(&path, "invalid-token").unwrap();
    let invalid = request(&app, "GET", None).await;
    assert_eq!(json_body(invalid).await, json!({ "setup_required": false }));
    let unavailable = request(
        &app,
        "POST",
        Some(json!({
            "token": "invalid-token",
            "username": "admin",
            "password": "密".repeat(12)
        })),
    )
    .await;
    assert_eq!(unavailable.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(unavailable).await["code"], "SETUP_UNAVAILABLE");
}

#[tokio::test]
async fn 非法json空请求与错误内容类型统一返回结构化四百() {
    let (app, _dir, _config, _token) = empty_app().await;
    for response in [
        raw_setup_request(&app, Some("application/json"), "{").await,
        raw_setup_request(&app, Some("application/json"), "").await,
        raw_setup_request(&app, Some("text/plain"), "{}").await,
        raw_setup_request(&app, None, "{}").await,
    ] {
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = json_body(response).await;
        assert_eq!(body["code"], "SETUP_REQUEST_INVALID");
        assert_eq!(body["message"], "初始化请求格式无效");
    }
}

#[tokio::test]
async fn 用户名与十二个unicode字符边界按契约校验() {
    let (app, _dir, config, token) = empty_app().await;
    let blank = request(
        &app,
        "POST",
        Some(json!({ "token": token, "username": "  ", "password": "密".repeat(12) })),
    )
    .await;
    assert_eq!(blank.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(blank).await["code"], "SETUP_USERNAME_REQUIRED");

    let short = request(
        &app,
        "POST",
        Some(json!({ "token": token, "username": " admin ", "password": "密".repeat(11) })),
    )
    .await;
    assert_eq!(short.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(short).await["code"], "SETUP_PASSWORD_TOO_SHORT");

    let success = request(
        &app,
        "POST",
        Some(json!({ "token": token, "username": " admin ", "password": "密".repeat(12) })),
    )
    .await;
    assert_eq!(success.status(), StatusCode::NO_CONTENT);

    let status = request(&app, "GET", None).await;
    assert_eq!(json_body(status).await, json!({ "setup_required": false }));
    let pool = persistence::init(&config.database_url(), &config.database_path)
        .await
        .unwrap();
    let username: (String,) = sqlx::query_as("SELECT username FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(username.0, "admin");
}

#[tokio::test]
async fn 成功后删除令牌且明文不进入数据库() {
    let (app, _dir, config, token) = empty_app().await;
    let response = request(
        &app,
        "POST",
        Some(json!({
            "token": token,
            "username": "first-admin",
            "password": "密".repeat(12)
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(!token_path(&config).exists());

    let pool = persistence::init(&config.database_url(), &config.database_path)
        .await
        .unwrap();
    let hits: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM users WHERE instr(username, ?1) > 0 OR instr(password_hash, ?1) > 0 OR instr(role, ?1) > 0 OR instr(created_at, ?1) > 0",
    )
    .bind(&token)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(hits.0, 0);
}

#[tokio::test]
async fn 无效已使用已有用户统一返回setup不可用() {
    let (app, _dir, _config, token) = empty_app().await;
    let wrong = request(
        &app,
        "POST",
        Some(json!({
            "token": "0".repeat(64),
            "username": "admin",
            "password": "密".repeat(12)
        })),
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(wrong).await["code"], "SETUP_UNAVAILABLE");

    let success = request(
        &app,
        "POST",
        Some(json!({
            "token": token,
            "username": "admin",
            "password": "密".repeat(12)
        })),
    )
    .await;
    assert_eq!(success.status(), StatusCode::NO_CONTENT);

    let reused = request(
        &app,
        "POST",
        Some(json!({
            "token": token,
            "username": "other",
            "password": "密".repeat(12)
        })),
    )
    .await;
    assert_eq!(reused.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(reused).await["code"], "SETUP_UNAVAILABLE");
}

#[tokio::test]
async fn setup真实数据库写入错误返回五百internal() {
    let (app, _dir, config, token) = empty_app().await;
    let pool = persistence::init(&config.database_url(), &config.database_path)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_setup_insert BEFORE INSERT ON users BEGIN SELECT RAISE(FAIL, 'blocked'); END",
    )
    .execute(&pool)
    .await
    .unwrap();

    let response = request(
        &app,
        "POST",
        Some(json!({
            "token": token.clone(),
            "username": "admin",
            "password": "密".repeat(12)
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = json_body(response).await;
    assert_eq!(body["code"], "INTERNAL");
    assert!(!body.to_string().contains(&token));
    assert!(token_path(&config).exists(), "写入失败不得消费令牌");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn 十六个并发初始化请求仅一个成功() {
    let (app, _dir, config, token) = empty_app().await;
    let mut tasks = Vec::new();
    for index in 0..16 {
        let app = app.clone();
        let token = token.clone();
        tasks.push(tokio::spawn(async move {
            request(
                &app,
                "POST",
                Some(json!({
                    "token": token,
                    "username": format!("admin-{index}"),
                    "password": format!("并发安全口令十二字符{index:02}")
                })),
            )
            .await
            .status()
        }));
    }
    let mut statuses = Vec::new();
    for task in tasks {
        statuses.push(task.await.unwrap());
    }
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::NO_CONTENT)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::NOT_FOUND)
            .count(),
        15
    );

    let pool = persistence::init(&config.database_url(), &config.database_path)
        .await
        .unwrap();
    assert_eq!(persistence::count_users(&pool).await.unwrap(), 1);
}

#[tokio::test]
async fn 已有用户启动时清理陈旧令牌并永久关闭setup() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(&dir);
    let pool = persistence::init(&config.database_url(), &config.database_path)
        .await
        .unwrap();
    create_user(&pool, "existing", "existing-password", Role::Admin)
        .await
        .unwrap();
    pool.close().await;
    let path = token_path(&config);
    std::fs::write(&path, "b".repeat(64)).unwrap();

    let app = jianartifact_server::build_app(config.clone())
        .await
        .unwrap();

    assert!(!path.exists());
    let status = request(&app, "GET", None).await;
    assert_eq!(json_body(status).await, json!({ "setup_required": false }));
    let unavailable = request(
        &app,
        "POST",
        Some(json!({
            "token": "b".repeat(64),
            "username": "other",
            "password": "密".repeat(12)
        })),
    )
    .await;
    assert_eq!(unavailable.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(unavailable).await["code"], "SETUP_UNAVAILABLE");
}

#[tokio::test]
async fn openapi登记两个setup端点() {
    let (app, _dir, _config, _token) = empty_app().await;
    let response = app
        .oneshot(
            Request::get("/api/v1/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let doc = json_body(response).await;
    assert!(doc["paths"]["/api/v1/setup/status"]["get"].is_object());
    assert!(doc["paths"]["/api/v1/setup"]["post"].is_object());
}
