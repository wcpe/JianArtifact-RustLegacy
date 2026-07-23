//! 本地管理员重置口令 CLI 集成测试。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use jianartifact_server::auth::password::verify_password;
use jianartifact_server::auth::token::{generate_token, hash_token};
use jianartifact_server::config::Config;
use jianartifact_server::domain::{Role, create_user};
use jianartifact_server::persistence;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::util::ServiceExt;

const JWT_SECRET: &str = "admin-cli-test-jwt-secret";

struct TestContext {
    app: Router,
    _dir: TempDir,
    config: Config,
    pool: SqlitePool,
    user_id: i64,
    api_token: String,
}

fn config(dir: &TempDir) -> Config {
    Config {
        bind: "127.0.0.1:0".to_string(),
        database_path: dir.path().join("metadata/test.db"),
        data_dir: dir.path().join("blobs"),
        migration_workdir: Some(dir.path().join("migration")),
        migration_object_concurrency: 4,
        log_level: "warn".to_string(),
        jwt_secret: JWT_SECRET.to_string(),
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

async fn context() -> TestContext {
    let dir = tempfile::tempdir().unwrap();
    let config = config(&dir);
    let pool = persistence::init(&config.database_url(), &config.database_path)
        .await
        .unwrap();
    let user = create_user(&pool, "alice", "old-password", Role::Admin)
        .await
        .unwrap();
    let (api_token, token_hash) = generate_token();
    persistence::insert_token(&pool, user.id, "cli-invariant", &token_hash)
        .await
        .unwrap();
    let app = jianartifact_server::build_app(config.clone())
        .await
        .unwrap();
    TestContext {
        app,
        _dir: dir,
        config,
        pool,
        user_id: user.id,
        api_token,
    }
}

fn run_cli(database_path: &Path, args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jianartifact"))
        .args(args)
        .env("JIANARTIFACT_DATABASE_PATH", database_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动 CLI");
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input).expect("写入 CLI 标准输入");
    }
    child.wait_with_output().expect("等待 CLI 完成")
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    auth: Option<&str>,
    body: Option<Value>,
) -> StatusCode {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(value) = auth {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    let request_body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&value).unwrap())
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(builder.body(request_body).unwrap())
        .await
        .unwrap()
        .status()
}

async fn login(app: &Router, username: &str, password: &str) -> String {
    let request = Request::post("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({ "username": username, "password": password })).unwrap(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&body).unwrap();
    format!("Bearer {}", value["token"].as_str().unwrap())
}

fn basic(username: &str, password: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let raw = format!("{username}:{password}");
    let mut out = String::new();
    for chunk in raw.as_bytes().chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(ALPHABET[(b0 >> 2) as usize] as char);
        out.push(ALPHABET[(((b0 & 3) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(((b1 & 15) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(b2 & 63) as usize] as char
        } else {
            '='
        });
    }
    format!("Basic {out}")
}

#[tokio::test]
async fn 命令拒绝明文参数未知参数与额外参数且不泄露口令() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("missing.db");
    let secret = "绝不能出现在输出中的口令";
    for args in [
        vec!["admin", "reset-password", "alice", "--password", secret],
        vec!["admin", "reset-password", "alice", "--unknown"],
        vec!["admin", "reset-password", "alice", "extra"],
    ] {
        let output = run_cli(&db, &args, b"");
        assert!(!output.status.success());
        let text = output_text(&output);
        assert!(text.contains("命令参数"), "应明确报告参数错误：{text}");
        assert!(!text.contains(secret), "CLI 输出不得回显口令");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn 默认交互模式无tty时失败并提示password_stdin() {
    let context = context().await;
    let input = format!("{}\n", "密".repeat(12));
    let output = run_cli(
        &context.config.database_path,
        &["admin", "reset-password", "alice"],
        input.as_bytes(),
    );

    assert!(!output.status.success());
    let text = output_text(&output);
    assert!(
        text.contains("--password-stdin"),
        "应提示显式 stdin 模式：{text}"
    );
    let user = persistence::find_user_by_id(&context.pool, context.user_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.token_version, 0, "无 TTY 失败不得更新用户");
}

#[tokio::test]
async fn password_stdin只接受单个非空utf8行并按unicode字符计数() {
    let context = context().await;
    for input in [b"".as_slice(), b"line-one\nline-two\n".as_slice()] {
        let output = run_cli(
            &context.config.database_path,
            &["admin", "reset-password", "alice", "--password-stdin"],
            input,
        );
        assert!(!output.status.success());
    }
    let invalid_utf8 = run_cli(
        &context.config.database_path,
        &["admin", "reset-password", "alice", "--password-stdin"],
        &[0xff, b'\n'],
    );
    assert!(!invalid_utf8.status.success());

    let short = format!("{}\n", "密".repeat(11));
    let output = run_cli(
        &context.config.database_path,
        &["admin", "reset-password", "alice", "--password-stdin"],
        short.as_bytes(),
    );
    assert!(!output.status.success());

    let valid = format!("{}\r\n", "密".repeat(12));
    let output = run_cli(
        &context.config.database_path,
        &["admin", "reset-password", "--password-stdin", "alice"],
        valid.as_bytes(),
    );
    assert!(output.status.success(), "{}", output_text(&output));
}

#[tokio::test]
async fn cli先校验口令再打开既有数据库且未知用户失败() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.db");
    let short_input = format!("{}\n", "密".repeat(11));
    let short = run_cli(
        &missing,
        &["admin", "reset-password", "alice", "--password-stdin"],
        short_input.as_bytes(),
    );
    assert!(!short.status.success());
    assert!(output_text(&short).contains("至少需要 12 个字符"));
    assert!(!missing.exists(), "口令校验阶段不得创建数据库");

    let input = format!("{}\n", "密".repeat(12));
    let output = run_cli(
        &missing,
        &["admin", "reset-password", "alice", "--password-stdin"],
        input.as_bytes(),
    );
    assert!(!output.status.success());
    assert!(!missing.exists(), "CLI 不得创建缺失数据库");

    let context = context().await;
    let unknown = run_cli(
        &context.config.database_path,
        &["admin", "reset-password", "ghost", "--password-stdin"],
        input.as_bytes(),
    );
    assert!(!unknown.status.success());
}

#[tokio::test]
async fn 数据库路径包含空格和unicode时仍只打开既有文件() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(&dir);
    config.database_path = dir.path().join("含 空格目录").join("用户 数据库.db");
    std::fs::create_dir_all(config.database_path.parent().unwrap()).unwrap();
    let pool = persistence::init(&config.database_url(), &config.database_path)
        .await
        .unwrap();
    let user = create_user(&pool, "path-user", "old-password", Role::Admin)
        .await
        .unwrap();

    let new_password = "路径安全口令十二字符以上";
    let input = format!("{new_password}\n");
    let output = run_cli(
        &config.database_path,
        &["admin", "reset-password", "path-user", "--password-stdin"],
        input.as_bytes(),
    );

    assert!(output.status.success(), "{}", output_text(&output));
    let updated: (String,) = sqlx::query_as("SELECT password_hash FROM users WHERE id = ?")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(verify_password(new_password, &updated.0));
}

#[tokio::test]
async fn 重置仅更新口令与token_version并使旧jwt_basic失效但api_token不变() {
    let context = context().await;
    let old_jwt = login(&context.app, "alice", "old-password").await;
    let old_basic = basic("alice", "old-password");
    let api_bearer = format!("Bearer {}", context.api_token);
    assert_eq!(
        send(
            &context.app,
            "GET",
            "/api/v1/auth/me",
            Some(&old_basic),
            None
        )
        .await,
        StatusCode::OK,
        "重置前先建立 Basic 正向缓存"
    );
    assert_eq!(
        send(
            &context.app,
            "GET",
            "/api/v1/auth/me",
            Some(&api_bearer),
            None
        )
        .await,
        StatusCode::OK
    );

    let before: (String, bool, String, i64, String) = sqlx::query_as(
        "SELECT role, enabled, created_at, token_version, password_hash FROM users WHERE id = ?",
    )
    .bind(context.user_id)
    .fetch_one(&context.pool)
    .await
    .unwrap();
    let token_before: (String, String) =
        sqlx::query_as("SELECT name, token_hash FROM api_tokens WHERE user_id = ?")
            .bind(context.user_id)
            .fetch_one(&context.pool)
            .await
            .unwrap();

    let new_password = "密".repeat(12);
    let input = format!("{new_password}\n");
    let output = run_cli(
        &context.config.database_path,
        &["admin", "reset-password", "alice", "--password-stdin"],
        input.as_bytes(),
    );
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(String::from_utf8_lossy(&output.stdout).contains("口令重置成功"));
    assert!(!output_text(&output).contains(&new_password));

    let after: (String, bool, String, i64, String) = sqlx::query_as(
        "SELECT role, enabled, created_at, token_version, password_hash FROM users WHERE id = ?",
    )
    .bind(context.user_id)
    .fetch_one(&context.pool)
    .await
    .unwrap();
    let token_after: (String, String) =
        sqlx::query_as("SELECT name, token_hash FROM api_tokens WHERE user_id = ?")
            .bind(context.user_id)
            .fetch_one(&context.pool)
            .await
            .unwrap();

    assert_eq!(after.0, before.0, "角色不变");
    assert_eq!(after.1, before.1, "启用状态不变");
    assert_eq!(after.2, before.2, "创建时间不变");
    assert_eq!(after.3, before.3 + 1, "会话版本递增一次");
    assert!(verify_password(&new_password, &after.4));
    assert!(!verify_password("old-password", &after.4));
    assert_eq!(token_after, token_before, "API Token 元数据与哈希不变");
    assert_eq!(token_after.1, hash_token(&context.api_token));

    for credential in [&old_jwt, &old_basic] {
        assert_eq!(
            send(
                &context.app,
                "GET",
                "/api/v1/auth/me",
                Some(credential),
                None,
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        send(
            &context.app,
            "GET",
            "/api/v1/auth/me",
            Some(&api_bearer),
            None
        )
        .await,
        StatusCode::OK,
        "API Token 在重置后仍有效"
    );
    let new_basic = basic("alice", &new_password);
    assert_eq!(
        send(
            &context.app,
            "GET",
            "/api/v1/auth/me",
            Some(&new_basic),
            None
        )
        .await,
        StatusCode::OK
    );
}
