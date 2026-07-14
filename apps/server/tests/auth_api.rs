//! 认证域 handler 集成测试。
//! 起真实应用（真实 SQLite + 迁移 + 种子用户）发真实 HTTP，覆盖认证闭环、即时失效、
//! 管理员保护与敏感凭据零泄露。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use jianartifact_server::api::router;
use jianartifact_server::auth::AuthRuntime;
use jianartifact_server::auth::jwt;
use jianartifact_server::auth::revocation::session_family_revoke_hash;
use jianartifact_server::auth::token::hash_token;
use jianartifact_server::config::Config;
use jianartifact_server::domain::{Role, create_user};
use jianartifact_server::persistence;
use jianartifact_server::state::AppState;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::util::ServiceExt;

const JWT_SECRET: &str = "test-jwt-secret-please-change";
const SESSION_TTL_HOURS: u64 = 8;
const SESSION_TTL_SECS: u64 = SESSION_TTL_HOURS * 60 * 60;

/// 构建带种子用户的完整应用：一名管理员 admin、一名普通用户 alice。
/// 手工装配 AppState 以便种子直接写入应用共享的连接池（不走 env 自举，避免测试间串扰）。
async fn setup_details() -> (Router, TempDir, SqlitePool, Arc<AuthRuntime>, i64, i64) {
    let dir = tempfile::tempdir().expect("创建临时目录");
    let db_path = dir.path().join("test.db");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = persistence::init(&url, &db_path)
        .await
        .expect("初始化元数据库");
    let admin = create_user(&pool, "admin", "admin-pw", Role::Admin)
        .await
        .expect("创建管理员种子");
    let alice = create_user(&pool, "alice", "alice-pw", Role::User)
        .await
        .expect("创建普通用户种子");
    let config = Config {
        bind: "127.0.0.1:0".to_string(),
        database_path: db_path,
        data_dir: dir.path().join("blobs"),
        log_level: "warn".to_string(),
        jwt_secret: JWT_SECRET.to_string(),
        session_ttl_hours: SESSION_TTL_HOURS,
    };
    let auth = Arc::new(AuthRuntime::new(
        &config.jwt_secret,
        config.session_ttl_hours,
    ));
    let state = AppState {
        pool: pool.clone(),
        config: Arc::new(config),
        auth: auth.clone(),
    };
    (router(state), dir, pool, auth, admin.id, alice.id)
}

async fn setup() -> (Router, TempDir) {
    let (app, dir, _, _, _, _) = setup_details().await;
    (app, dir)
}

/// 发送一次请求，返回（状态码，响应体字节）。auth 为完整 Authorization 头值。
async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    auth: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(value) = auth {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    let request = match body {
        Some(json) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&json).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(request).await.unwrap();
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, bytes)
}

/// 解析响应体为 JSON。
fn as_json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

/// 登录并返回可直接用于 Authorization 头的 `Bearer <jwt>` 串。
async fn login(app: &Router, username: &str, password: &str) -> String {
    let (status, body) = send(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "username": username, "password": password })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "登录应成功");
    let token = as_json(&body)["token"].as_str().unwrap().to_string();
    format!("Bearer {token}")
}

/// 生成 Basic 头值（标准 base64 编码 `user:pass`），仅测试用。
fn basic(user: &str, pass: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let raw = format!("{user}:{pass}");
    let mut out = String::new();
    for chunk in raw.as_bytes().chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(ALPHABET[(b0 >> 2) as usize] as char);
        out.push(ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(b2 & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    format!("Basic {out}")
}

/// 签发 API Token，返回（标识、明文）。
async fn create_token(app: &Router, auth: &str, name: &str) -> (i64, String) {
    let (status, body) = send(
        app,
        "POST",
        "/api/v1/tokens",
        Some(auth),
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "API Token 签发应成功");
    let value = as_json(&body);
    (
        value["id"].as_i64().expect("签发响应应含 id"),
        value["token"]
            .as_str()
            .expect("签发响应应含明文")
            .to_string(),
    )
}

/// 断言携带的凭据被认证层拒绝，且返回 AUTH_ 错误码。
async fn assert_auth_401(app: &Router, uri: &str, auth: &str) {
    let (status, body) = send(app, "GET", uri, Some(auth), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "凭据应返回 401");
    let code = as_json(&body)["code"]
        .as_str()
        .expect("认证错误响应应含 code")
        .to_string();
    assert!(code.starts_with("AUTH_"), "应返回 AUTH_ 错误码：{code}");
}

/// 统计三张认证表所有文本列中包含指定明文的记录数。
async fn database_text_hits(pool: &SqlitePool, needle: &str) -> i64 {
    let users: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM users WHERE instr(username, ?1) > 0 OR instr(password_hash, ?1) > 0 OR instr(role, ?1) > 0 OR instr(created_at, ?1) > 0",
    )
    .bind(needle)
    .fetch_one(pool)
    .await
    .unwrap();
    let tokens: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM api_tokens WHERE instr(name, ?1) > 0 OR instr(token_hash, ?1) > 0 OR instr(created_at, ?1) > 0",
    )
    .bind(needle)
    .fetch_one(pool)
    .await
    .unwrap();
    let revoked: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM revoked_sessions WHERE instr(jti_hash, ?1) > 0 OR instr(expires_at, ?1) > 0",
    )
    .bind(needle)
    .fetch_one(pool)
    .await
    .unwrap();
    users.0 + tokens.0 + revoked.0
}

#[tokio::test]
async fn 登录成功返回会话令牌() {
    let (app, _dir) = setup().await;
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "username": "admin", "password": "admin-pw" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v = as_json(&body);
    assert!(!v["token"].as_str().unwrap().is_empty(), "应返回非空令牌");
    assert!(v["expires_in"].as_i64().unwrap() > 0, "应返回正有效期");
}

#[tokio::test]
async fn 口令错误与用户不存在均返回统一凭据无效码() {
    let (app, _dir) = setup().await;
    // 口令错误
    let (s1, b1) = send(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "username": "admin", "password": "wrong" })),
    )
    .await;
    assert_eq!(s1, StatusCode::UNAUTHORIZED);
    assert_eq!(as_json(&b1)["code"], "AUTH_INVALID_CREDENTIALS");
    // 用户不存在：状态码与错误码与口令错误完全一致，不泄露存在性
    let (s2, b2) = send(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "username": "ghost", "password": "whatever" })),
    )
    .await;
    assert_eq!(s2, StatusCode::UNAUTHORIZED);
    assert_eq!(as_json(&b2)["code"], "AUTH_INVALID_CREDENTIALS");
}

#[tokio::test]
async fn 无凭据访问受保护端点返回未认证() {
    let (app, _dir) = setup().await;
    let (status, body) = send(&app, "GET", "/api/v1/auth/me", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(as_json(&body)["code"], "AUTH_UNAUTHENTICATED");
}

#[tokio::test]
async fn 当前身份接口返回认证身份() {
    let (app, _dir) = setup().await;
    let auth = login(&app, "admin", "admin-pw").await;
    let (status, body) = send(&app, "GET", "/api/v1/auth/me", Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let v = as_json(&body);
    assert_eq!(v["username"], "admin");
    assert_eq!(v["role"], "admin");
    assert_eq!(v["enabled"], true);
}

#[tokio::test]
async fn 接口令牌可经承载与基本鉴权() {
    let (app, _dir) = setup().await;
    let auth = login(&app, "alice", "alice-pw").await;
    // 签发 Token（明文仅此一次）
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/tokens",
        Some(&auth),
        Some(json!({ "name": "ci" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = as_json(&body)["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("jat_"), "Token 明文应带 jat_ 前缀");
    // Bearer + API Token
    let (s2, b2) = send(
        &app,
        "GET",
        "/api/v1/auth/me",
        Some(&format!("Bearer {token}")),
        None,
    )
    .await;
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(as_json(&b2)["username"], "alice");
    // Basic（任意用户名 + Token 作为口令位）
    let (s3, b3) = send(
        &app,
        "GET",
        "/api/v1/auth/me",
        Some(&basic("ignored", &token)),
        None,
    )
    .await;
    assert_eq!(s3, StatusCode::OK);
    assert_eq!(as_json(&b3)["username"], "alice");
}

#[tokio::test]
async fn 基本口令鉴权闭环() {
    let (app, _dir) = setup().await;
    // 正确口令
    let (s1, b1) = send(
        &app,
        "GET",
        "/api/v1/auth/me",
        Some(&basic("alice", "alice-pw")),
        None,
    )
    .await;
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(as_json(&b1)["username"], "alice");
    // 错误口令一律 401
    let (s2, _) = send(
        &app,
        "GET",
        "/api/v1/auth/me",
        Some(&basic("alice", "wrong")),
        None,
    )
    .await;
    assert_eq!(s2, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn 退出后原会话令牌立即失效() {
    let (app, _dir) = setup().await;
    let auth = login(&app, "admin", "admin-pw").await;
    // 退出前可用
    let (s0, _) = send(&app, "GET", "/api/v1/auth/me", Some(&auth), None).await;
    assert_eq!(s0, StatusCode::OK);
    // 退出
    let (s1, _) = send(&app, "POST", "/api/v1/auth/logout", Some(&auth), None).await;
    assert_eq!(s1, StatusCode::NO_CONTENT);
    // 原令牌失效
    let (s2, b2) = send(&app, "GET", "/api/v1/auth/me", Some(&auth), None).await;
    assert_eq!(s2, StatusCode::UNAUTHORIZED);
    assert_eq!(as_json(&b2)["code"], "AUTH_INVALID_SESSION");
}

#[tokio::test]
async fn 刷新沿用会话家族且换发后旧令牌失效() {
    let (app, _dir, _pool, runtime, _, _) = setup_details().await;
    let auth = login(&app, "admin", "admin-pw").await;
    let old_claims = jwt::verify(&runtime, auth.strip_prefix("Bearer ").unwrap()).unwrap();
    let (status, body) = send(&app, "POST", "/api/v1/auth/refresh", Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let new_auth = format!("Bearer {}", as_json(&body)["token"].as_str().unwrap());
    let new_claims = jwt::verify(&runtime, new_auth.strip_prefix("Bearer ").unwrap()).unwrap();
    assert_eq!(new_claims.sid, old_claims.sid, "刷新必须沿用原会话家族");
    assert_ne!(new_claims.jti, old_claims.jti, "刷新必须换发令牌标识");
    // 新令牌可用
    let (s2, _) = send(&app, "GET", "/api/v1/auth/me", Some(&new_auth), None).await;
    assert_eq!(s2, StatusCode::OK);
    // 旧令牌已被撤销
    let (s3, _) = send(&app, "GET", "/api/v1/auth/me", Some(&auth), None).await;
    assert_eq!(s3, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn 同一旧令牌并发刷新仅一个成功() {
    let (app, _dir) = setup().await;
    let old_auth = login(&app, "admin", "admin-pw").await;

    let first = send(&app, "POST", "/api/v1/auth/refresh", Some(&old_auth), None);
    let second = send(&app, "POST", "/api/v1/auth/refresh", Some(&old_auth), None);
    let (first_result, second_result) = tokio::join!(first, second);
    let results = [first_result, second_result];

    assert_eq!(
        results
            .iter()
            .filter(|(status, _)| *status == StatusCode::OK)
            .count(),
        1,
        "同一旧令牌只能刷新成功一次"
    );
    assert_eq!(
        results
            .iter()
            .filter(|(status, _)| *status == StatusCode::UNAUTHORIZED)
            .count(),
        1,
        "另一个并发刷新必须返回 401"
    );

    let success_body = results
        .iter()
        .find_map(|(status, body)| (*status == StatusCode::OK).then_some(body))
        .expect("应有一个刷新成功响应");
    let new_auth = format!(
        "Bearer {}",
        as_json(success_body)["token"]
            .as_str()
            .expect("刷新成功应返回令牌")
    );
    let (new_status, _) = send(&app, "GET", "/api/v1/auth/me", Some(&new_auth), None).await;
    assert_eq!(new_status, StatusCode::OK, "新令牌应可用");
    let (old_status, _) = send(&app, "GET", "/api/v1/auth/me", Some(&old_auth), None).await;
    assert_eq!(old_status, StatusCode::UNAUTHORIZED, "旧令牌应失效");
}

#[tokio::test]
async fn 退出与刷新竞态不遗留有效新会话() {
    let (app, _dir) = setup().await;
    let old_auth = login(&app, "alice", "alice-pw").await;

    let refresh = send(&app, "POST", "/api/v1/auth/refresh", Some(&old_auth), None);
    let logout = send(&app, "POST", "/api/v1/auth/logout", Some(&old_auth), None);
    let (refresh_result, logout_result) = tokio::join!(refresh, logout);

    assert_eq!(logout_result.0, StatusCode::NO_CONTENT);
    assert!(matches!(
        refresh_result.0,
        StatusCode::OK | StatusCode::UNAUTHORIZED
    ));
    if refresh_result.0 == StatusCode::OK {
        let token = as_json(&refresh_result.1)["token"]
            .as_str()
            .expect("刷新成功应返回令牌")
            .to_owned();
        let new_auth = format!("Bearer {token}");
        assert_auth_401(&app, "/api/v1/auth/me", &new_auth).await;
    }
    assert_auth_401(&app, "/api/v1/auth/me", &old_auth).await;
}

#[tokio::test]
async fn 改口令与刷新竞态不产生有效新会话() {
    let (app, _dir) = setup().await;
    let old_auth = login(&app, "alice", "alice-pw").await;

    let refresh = send(&app, "POST", "/api/v1/auth/refresh", Some(&old_auth), None);
    let password_auth = basic("alice", "alice-pw");
    let change_password = send(
        &app,
        "POST",
        "/api/v1/auth/password",
        Some(&password_auth),
        Some(json!({
            "old_password": "alice-pw",
            "new_password": "alice-new-pw"
        })),
    );
    let (refresh_result, password_result) = tokio::join!(refresh, change_password);

    assert_eq!(password_result.0, StatusCode::NO_CONTENT);
    assert!(
        matches!(refresh_result.0, StatusCode::OK | StatusCode::UNAUTHORIZED),
        "刷新只允许成功或因竞态返回 401"
    );
    if refresh_result.0 == StatusCode::OK {
        let token = as_json(&refresh_result.1)["token"]
            .as_str()
            .expect("刷新成功应返回令牌")
            .to_string();
        let new_auth = format!("Bearer {token}");
        let (status, _) = send(&app, "GET", "/api/v1/auth/me", Some(&new_auth), None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "竞态中签发的旧版本令牌在改口令完成后必须失效"
        );
    }
}

#[tokio::test]
async fn 禁用与刷新竞态不产生有效新会话() {
    let (app, _dir) = setup().await;
    let admin_auth = login(&app, "admin", "admin-pw").await;
    let alice_auth = login(&app, "alice", "alice-pw").await;
    let (me_status, me_body) = send(&app, "GET", "/api/v1/auth/me", Some(&alice_auth), None).await;
    assert_eq!(me_status, StatusCode::OK);
    let alice_id = as_json(&me_body)["id"].as_i64().expect("用户信息应含 id");
    let disable_uri = format!("/api/v1/users/{alice_id}");

    let refresh = send(
        &app,
        "POST",
        "/api/v1/auth/refresh",
        Some(&alice_auth),
        None,
    );
    let disable = send(
        &app,
        "PATCH",
        &disable_uri,
        Some(&admin_auth),
        Some(json!({ "enabled": false })),
    );
    let (refresh_result, disable_result) = tokio::join!(refresh, disable);

    assert_eq!(disable_result.0, StatusCode::OK);
    assert!(
        matches!(refresh_result.0, StatusCode::OK | StatusCode::UNAUTHORIZED),
        "刷新只允许成功或因竞态返回 401"
    );
    if refresh_result.0 == StatusCode::OK {
        let token = as_json(&refresh_result.1)["token"]
            .as_str()
            .expect("刷新成功应返回令牌")
            .to_string();
        let new_auth = format!("Bearer {token}");
        let (status, _) = send(&app, "GET", "/api/v1/auth/me", Some(&new_auth), None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "竞态中签发的令牌在禁用完成后必须失效"
        );
    }
}

#[tokio::test]
async fn 会话端点拒绝基本鉴权与接口令牌() {
    let (app, _dir) = setup().await;

    let session_auth = login(&app, "alice", "alice-pw").await;
    let (create_status, create_body) = send(
        &app,
        "POST",
        "/api/v1/tokens",
        Some(&session_auth),
        Some(json!({ "name": "refresh-rejected" })),
    )
    .await;
    assert_eq!(create_status, StatusCode::CREATED);
    let token = as_json(&create_body)["token"]
        .as_str()
        .expect("签发应返回 API Token")
        .to_string();
    let credentials = [
        basic("alice", "alice-pw"),
        format!("Bearer {token}"),
        basic("ignored", &token),
    ];
    for credential in credentials {
        for uri in ["/api/v1/auth/refresh", "/api/v1/auth/logout"] {
            let (status, _) = send(&app, "POST", uri, Some(&credential), None).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "会话端点必须拒绝非 JWT 凭据"
            );
        }
    }
}

#[tokio::test]
async fn 普通用户访问用户管理被拒管理员可访问() {
    let (app, _dir) = setup().await;
    // 普通用户 403
    let alice = login(&app, "alice", "alice-pw").await;
    let (s1, b1) = send(&app, "GET", "/api/v1/users", Some(&alice), None).await;
    assert_eq!(s1, StatusCode::FORBIDDEN);
    assert_eq!(as_json(&b1)["code"], "AUTH_FORBIDDEN");
    // 管理员 200 且至少含两名种子用户
    let admin = login(&app, "admin", "admin-pw").await;
    let (s2, b2) = send(&app, "GET", "/api/v1/users", Some(&admin), None).await;
    assert_eq!(s2, StatusCode::OK);
    assert!(as_json(&b2)["total"].as_i64().unwrap() >= 2);
}

#[tokio::test]
async fn 管理接口拒绝各类无效与已吊销凭据() {
    let (app, _dir, _pool, _runtime, admin_id, _) = setup_details().await;
    let admin = login(&app, "admin", "admin-pw").await;
    let revoked_session = login(&app, "admin", "admin-pw").await;
    let (token_id, revoked_token) = create_token(&app, &admin, "待吊销").await;
    let (expired, _) = jwt::sign(&AuthRuntime::new(JWT_SECRET, 0), admin_id, 0).unwrap();

    let logout = send(
        &app,
        "POST",
        "/api/v1/auth/logout",
        Some(&revoked_session),
        None,
    )
    .await;
    assert_eq!(logout.0, StatusCode::NO_CONTENT);
    let revoke_uri = format!("/api/v1/tokens/{token_id}");
    let revoked = send(&app, "DELETE", &revoke_uri, Some(&admin), None).await;
    assert_eq!(revoked.0, StatusCode::NO_CONTENT);

    let credentials = [
        "Bearer broken.jwt.signature".to_string(),
        "Bearer jat_not-a-real-token".to_string(),
        format!("Bearer {expired}"),
        revoked_session,
        format!("Bearer {revoked_token}"),
    ];
    for credential in credentials {
        assert_auth_401(&app, "/api/v1/users", &credential).await;
    }
}

#[tokio::test]
async fn 禁用用户后既有会话与两种令牌立即失效() {
    let (app, _dir, _pool, _runtime, _, alice_id) = setup_details().await;
    let admin = login(&app, "admin", "admin-pw").await;
    let alice_session = login(&app, "alice", "alice-pw").await;
    let (_, token) = create_token(&app, &alice_session, "禁用校验").await;
    let bearer_token = format!("Bearer {token}");
    let basic_token = basic("ignored", &token);

    for credential in [&alice_session, &bearer_token, &basic_token] {
        let (status, _) = send(&app, "GET", "/api/v1/auth/me", Some(credential), None).await;
        assert_eq!(status, StatusCode::OK, "禁用前凭据应有效");
    }
    let uri = format!("/api/v1/users/{alice_id}");
    let disabled = send(
        &app,
        "PATCH",
        &uri,
        Some(&admin),
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(disabled.0, StatusCode::OK);

    for credential in [alice_session, bearer_token, basic_token] {
        assert_auth_401(&app, "/api/v1/users", &credential).await;
    }
}

#[tokio::test]
async fn 禁用再启用后旧会话与接口令牌永久失效但口令可重新校验() {
    let (app, _dir, _pool, _runtime, _, alice_id) = setup_details().await;
    let admin = login(&app, "admin", "admin-pw").await;
    let old_session = login(&app, "alice", "alice-pw").await;
    let (_, token) = create_token(&app, &old_session, "跨禁用周期").await;
    let bearer_token = format!("Bearer {token}");
    let basic_token = basic("ignored", &token);
    let basic_password = basic("alice", "alice-pw");

    let warmed = send(&app, "GET", "/api/v1/auth/me", Some(&basic_password), None).await;
    assert_eq!(warmed.0, StatusCode::OK, "禁用前应建立 Basic 正向缓存");

    let uri = format!("/api/v1/users/{alice_id}");
    let disabled = send(
        &app,
        "PATCH",
        &uri,
        Some(&admin),
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(disabled.0, StatusCode::OK);
    let enabled = send(
        &app,
        "PATCH",
        &uri,
        Some(&admin),
        Some(json!({ "enabled": true })),
    )
    .await;
    assert_eq!(enabled.0, StatusCode::OK);

    for credential in [old_session, bearer_token, basic_token] {
        assert_auth_401(&app, "/api/v1/auth/me", &credential).await;
    }
    let password_after_enable =
        send(&app, "GET", "/api/v1/auth/me", Some(&basic_password), None).await;
    assert_eq!(
        password_after_enable.0,
        StatusCode::OK,
        "重新启用不修改口令，但必须使用新代际重新校验"
    );
}

#[tokio::test]
async fn 修改本人密码使全部旧会话与旧口令失效() {
    let (app, _dir) = setup().await;
    let first = login(&app, "alice", "alice-pw").await;
    let second = login(&app, "alice", "alice-pw").await;
    let changed = send(
        &app,
        "POST",
        "/api/v1/auth/password",
        Some(&first),
        Some(json!({
            "old_password": "alice-pw",
            "new_password": "alice-self-new-pw"
        })),
    )
    .await;
    assert_eq!(changed.0, StatusCode::NO_CONTENT);
    assert_auth_401(&app, "/api/v1/auth/me", &first).await;
    assert_auth_401(&app, "/api/v1/auth/me", &second).await;

    let old_login = send(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "username": "alice", "password": "alice-pw" })),
    )
    .await;
    assert_eq!(old_login.0, StatusCode::UNAUTHORIZED);
    assert_eq!(as_json(&old_login.1)["code"], "AUTH_INVALID_CREDENTIALS");
    let new_session = login(&app, "alice", "alice-self-new-pw").await;
    let usable = send(&app, "GET", "/api/v1/auth/me", Some(&new_session), None).await;
    assert_eq!(usable.0, StatusCode::OK);
}

#[tokio::test]
async fn 管理员重置密码使全部旧会话与旧口令失效() {
    let (app, _dir, _pool, _runtime, _, alice_id) = setup_details().await;
    let admin = login(&app, "admin", "admin-pw").await;
    let first = login(&app, "alice", "alice-pw").await;
    let second = login(&app, "alice", "alice-pw").await;
    let uri = format!("/api/v1/users/{alice_id}");
    let reset = send(
        &app,
        "PATCH",
        &uri,
        Some(&admin),
        Some(json!({ "password": "alice-admin-new-pw" })),
    )
    .await;
    assert_eq!(reset.0, StatusCode::OK);
    assert_auth_401(&app, "/api/v1/auth/me", &first).await;
    assert_auth_401(&app, "/api/v1/auth/me", &second).await;

    let old_login = send(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "username": "alice", "password": "alice-pw" })),
    )
    .await;
    assert_eq!(old_login.0, StatusCode::UNAUTHORIZED);
    assert_eq!(as_json(&old_login.1)["code"], "AUTH_INVALID_CREDENTIALS");
    let new_session = login(&app, "alice", "alice-admin-new-pw").await;
    let usable = send(&app, "GET", "/api/v1/auth/me", Some(&new_session), None).await;
    assert_eq!(usable.0, StatusCode::OK);
}

#[tokio::test]
async fn 最后一名管理员不能经接口禁用或降级() {
    let (app, _dir, _pool, _runtime, admin_id, _) = setup_details().await;
    let admin = login(&app, "admin", "admin-pw").await;
    let uri = format!("/api/v1/users/{admin_id}");
    for body in [json!({ "enabled": false }), json!({ "role": "user" })] {
        let (status, response) = send(&app, "PATCH", &uri, Some(&admin), Some(body)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(as_json(&response)["code"], "AUTH_LAST_ADMIN");
    }
}

#[tokio::test]
async fn 敏感凭据只以规定哈希落库且列表零泄露() {
    let (app, _dir, pool, runtime, _, alice_id) = setup_details().await;
    let session = login(&app, "alice", "alice-pw").await;
    let raw_jwt = session.strip_prefix("Bearer ").unwrap();
    let claims = jwt::verify(&runtime, raw_jwt).unwrap();
    let (_, token) = create_token(&app, &session, "零泄露").await;

    let (list_status, list_body) = send(&app, "GET", "/api/v1/tokens", Some(&session), None).await;
    assert_eq!(list_status, StatusCode::OK);
    let list_text = String::from_utf8(list_body).unwrap();
    assert!(!list_text.contains(&token), "列表不得返回 Token 明文");
    assert!(!list_text.contains("token_hash"), "列表不得返回 Token 哈希");
    assert!(!list_text.contains("\"token\""), "列表不得含 token 字段");
    assert!(!list_text.contains("\"hash\""), "列表不得含 hash 字段");

    let password_hash: (String,) = sqlx::query_as("SELECT password_hash FROM users WHERE id = ?")
        .bind(alice_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(password_hash.0.starts_with("$argon2id$"));
    for parameter in ["m=19456", "t=2", "p=1"] {
        assert!(password_hash.0.contains(parameter), "缺少参数 {parameter}");
    }
    let token_hash: (String,) =
        sqlx::query_as("SELECT token_hash FROM api_tokens WHERE user_id = ?")
            .bind(alice_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(token_hash.0, hash_token(&token));
    assert!(!token_hash.0.contains(&token));

    let logout = send(&app, "POST", "/api/v1/auth/logout", Some(&session), None).await;
    assert_eq!(logout.0, StatusCode::NO_CONTENT);
    let family_revoke: (String, i64) = sqlx::query_as(
        "SELECT jti_hash, CAST(strftime('%s', expires_at) AS INTEGER) FROM revoked_sessions",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(family_revoke.0, session_family_revoke_hash(&claims.sid));
    assert!(!family_revoke.0.contains(&claims.sid));
    assert!(
        family_revoke.1 >= claims.exp.saturating_add(SESSION_TTL_SECS) as i64,
        "家族撤销记录必须覆盖该令牌可能派生的最后一枚子令牌"
    );
    for secret in [
        "alice-pw",
        token.as_str(),
        JWT_SECRET,
        claims.jti.as_str(),
        claims.sid.as_str(),
    ] {
        assert_eq!(database_text_hits(&pool, secret).await, 0, "明文不得落库");
    }
}

#[tokio::test]
async fn 严格非法基本鉴权头经接口统一返回未认证() {
    let (app, _dir) = setup().await;
    for credential in [
        "Basic ****",
        "Basic A",
        "Basic =AAA",
        "Basic AB==",
        "Basic Zm9v",
        "Basic /w==",
    ] {
        assert_auth_401(&app, "/api/v1/users", credential).await;
    }
}
