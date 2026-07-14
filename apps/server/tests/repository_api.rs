//! 仓库核心真实 HTTP 与迁移集成测试。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode, header};
use http_body_util::BodyExt;
use jianartifact_server::api::router;
use jianartifact_server::auth::AuthRuntime;
use jianartifact_server::config::Config;
use jianartifact_server::domain::{Role, create_user};
use jianartifact_server::persistence;
use jianartifact_server::state::AppState;
use jianartifact_server::storage::BlobStore;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::util::ServiceExt;

const JWT_SECRET: &str = "repository-test-jwt-secret";

struct TestContext {
    app: Router,
    _dir: TempDir,
    pool: SqlitePool,
}

struct TestResponse {
    status: StatusCode,
    request_id: Option<String>,
    body: Vec<u8>,
}

async fn setup() -> TestContext {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = persistence::init(&url, &db_path).await.unwrap();
    create_user(&pool, "admin", "admin-pw", Role::Admin)
        .await
        .unwrap();
    create_user(&pool, "alice", "alice-pw", Role::User)
        .await
        .unwrap();
    create_user(&pool, "bob", "bob-pw", Role::User)
        .await
        .unwrap();
    let config = Config {
        bind: "127.0.0.1:0".to_string(),
        database_path: db_path,
        data_dir: dir.path().join("blobs"),
        log_level: "warn".to_string(),
        jwt_secret: JWT_SECRET.to_string(),
        session_ttl_hours: 8,
        max_artifact_bytes: 5 * 1024 * 1024 * 1024,
        min_free_bytes: 5 * 1024 * 1024 * 1024,
        min_free_percent: 5,
    };
    let blob_store = Arc::new(BlobStore::new(
        config.data_dir.clone(),
        config.max_artifact_bytes,
        config.min_free_bytes,
        config.min_free_percent,
    ));
    let artifacts = Arc::new(jianartifact_server::artifact::ArtifactService::new(
        pool.clone(),
        blob_store.as_ref().clone(),
    ));
    let state = AppState {
        pool: pool.clone(),
        auth: Arc::new(AuthRuntime::new(
            &config.jwt_secret,
            config.session_ttl_hours,
        )),
        config: Arc::new(config),
        blob_store,
        artifacts,
    };
    TestContext {
        app: router(state),
        _dir: dir,
        pool,
    }
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    auth: Option<&str>,
    request_id: Option<&str>,
    body: Option<Value>,
) -> TestResponse {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(value) = auth {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    if let Some(value) = request_id {
        builder = builder.header("x-request-id", value);
    }
    let request = match body {
        Some(value) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&value).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    TestResponse {
        status,
        request_id,
        body,
    }
}

fn body_json(response: &TestResponse) -> Value {
    serde_json::from_slice(&response.body).unwrap()
}

fn repository_item(response: &TestResponse, name: &str) -> Value {
    body_json(response)["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == name)
        .unwrap()
        .clone()
}

async fn login(app: &Router, username: &str, password: &str) -> String {
    let response = send(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({ "username": username, "password": password })),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    format!("Bearer {}", body_json(&response)["token"].as_str().unwrap())
}

async fn create_repo(app: &Router, auth: &str, body: Value) -> TestResponse {
    send(app, "POST", "/api/v1/repos", Some(auth), None, Some(body)).await
}

fn hosted(name: &str, format: &str, visibility: &str) -> Value {
    json!({
        "name": name,
        "format": format,
        "repo_type": "hosted",
        "visibility": visibility
    })
}

#[tokio::test]
async fn 迁移建立三表并启用外键与约束() {
    let context = setup().await;
    for table in ["repositories", "repository_acl", "repository_group_members"] {
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?")
                .bind(table)
                .fetch_one(&context.pool)
                .await
                .unwrap();
        assert_eq!(count.0, 1, "缺少表 {table}");
    }
    let foreign_keys: (i64,) = sqlx::query_as("PRAGMA foreign_keys")
        .fetch_one(&context.pool)
        .await
        .unwrap();
    assert_eq!(foreign_keys.0, 1);

    for name in ["UPPER", "with_underscore", "", "a/b"] {
        let result = sqlx::query(
            "INSERT INTO repositories (name, format, repo_type, visibility, overwrite_policy) VALUES (?, 'raw', 'hosted', 'private', 'allow')",
        )
        .bind(name)
        .execute(&context.pool)
        .await;
        assert!(result.is_err(), "非法仓库名应被约束拒绝：{name:?}");
    }
    let invalid_enum = sqlx::query(
        "INSERT INTO repositories (name, format, repo_type, visibility, overwrite_policy) VALUES ('bad-enum', 'docker', 'hosted', 'private', 'allow')",
    )
    .execute(&context.pool)
    .await;
    assert!(invalid_enum.is_err());
}

#[tokio::test]
async fn group关系保持顺序唯一且成员删除受限() {
    let context = setup().await;
    for name in ["member-a", "member-b"] {
        sqlx::query(
            "INSERT INTO repositories (name, format, repo_type, visibility, overwrite_policy) VALUES (?, 'raw', 'hosted', 'private', 'allow')",
        )
        .bind(name)
        .execute(&context.pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO repositories (name, format, repo_type, visibility) VALUES ('group-a', 'raw', 'group', 'private')",
    )
    .execute(&context.pool)
    .await
    .unwrap();
    let ids: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM repositories ORDER BY name")
        .fetch_all(&context.pool)
        .await
        .unwrap();
    let id = |name: &str| ids.iter().find(|row| row.1 == name).unwrap().0;
    sqlx::query(
        "INSERT INTO repository_group_members (group_repo_id, member_repo_id, position) VALUES (?, ?, 0), (?, ?, 1)",
    )
    .bind(id("group-a"))
    .bind(id("member-b"))
    .bind(id("group-a"))
    .bind(id("member-a"))
    .execute(&context.pool)
    .await
    .unwrap();
    let ordered: Vec<(String,)> = sqlx::query_as(
        "SELECT repositories.name FROM repository_group_members JOIN repositories ON repositories.id = member_repo_id WHERE group_repo_id = ? ORDER BY position",
    )
    .bind(id("group-a"))
    .fetch_all(&context.pool)
    .await
    .unwrap();
    assert_eq!(
        ordered,
        vec![("member-b".to_string(),), ("member-a".to_string(),)]
    );

    let duplicate = sqlx::query(
        "INSERT INTO repository_group_members (group_repo_id, member_repo_id, position) VALUES (?, ?, 2)",
    )
    .bind(id("group-a"))
    .bind(id("member-a"))
    .execute(&context.pool)
    .await;
    assert!(duplicate.is_err());
    let restricted = sqlx::query("DELETE FROM repositories WHERE id = ?")
        .bind(id("member-a"))
        .execute(&context.pool)
        .await;
    assert!(restricted.is_err());
}

#[tokio::test]
async fn 数据库拒绝非法group关系并禁止更新格式与类型() {
    let context = setup().await;
    for (name, format, repo_type, overwrite_policy) in [
        ("raw-hosted-a", "raw", "hosted", Some("allow")),
        ("raw-hosted-b", "raw", "hosted", Some("allow")),
        ("npm-hosted", "npm", "hosted", Some("allow")),
        ("raw-group-a", "raw", "group", None),
        ("raw-group-b", "raw", "group", None),
    ] {
        sqlx::query(
            "INSERT INTO repositories (name, format, repo_type, visibility, overwrite_policy) VALUES (?, ?, ?, 'private', ?)",
        )
        .bind(name)
        .bind(format)
        .bind(repo_type)
        .bind(overwrite_policy)
        .execute(&context.pool)
        .await
        .unwrap();
    }
    let ids: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM repositories")
        .fetch_all(&context.pool)
        .await
        .unwrap();
    let id = |name: &str| ids.iter().find(|row| row.1 == name).unwrap().0;

    for (group_repo_id, member_repo_id) in [
        (id("raw-hosted-a"), id("raw-hosted-b")),
        (id("raw-group-a"), id("raw-group-b")),
        (id("raw-group-a"), id("npm-hosted")),
    ] {
        let result = sqlx::query(
            "INSERT INTO repository_group_members (group_repo_id, member_repo_id, position) VALUES (?, ?, 0)",
        )
        .bind(group_repo_id)
        .bind(member_repo_id)
        .execute(&context.pool)
        .await;
        assert!(result.is_err());
    }

    sqlx::query(
        "INSERT INTO repository_group_members (group_repo_id, member_repo_id, position) VALUES (?, ?, 0)",
    )
    .bind(id("raw-group-a"))
    .bind(id("raw-hosted-a"))
    .execute(&context.pool)
    .await
    .unwrap();
    let invalid_update = sqlx::query(
        "UPDATE repository_group_members SET member_repo_id = ? WHERE group_repo_id = ?",
    )
    .bind(id("raw-group-b"))
    .bind(id("raw-group-a"))
    .execute(&context.pool)
    .await;
    assert!(invalid_update.is_err());

    for statement in [
        "UPDATE repositories SET format = 'npm' WHERE name = 'raw-hosted-a'",
        "UPDATE repositories SET repo_type = 'group' WHERE name = 'raw-hosted-a'",
    ] {
        let result = sqlx::query(statement).execute(&context.pool).await;
        assert!(result.is_err());
    }
}

#[tokio::test]
async fn 可选鉴权允许匿名但无效或禁用凭据返回四百零一() {
    let context = setup().await;
    let anonymous = send(&context.app, "GET", "/api/v1/repos", None, None, None).await;
    assert_eq!(anonymous.status, StatusCode::OK);
    let invalid = send(
        &context.app,
        "GET",
        "/api/v1/repos",
        Some("Bearer broken.jwt.signature"),
        None,
        None,
    )
    .await;
    assert_eq!(invalid.status, StatusCode::UNAUTHORIZED);

    let bob = login(&context.app, "bob", "bob-pw").await;
    let bob_id: (i64,) = sqlx::query_as("SELECT id FROM users WHERE username = 'bob'")
        .fetch_one(&context.pool)
        .await
        .unwrap();
    persistence::update_user_fields(&context.pool, bob_id.0, Some(false), None, None)
        .await
        .unwrap();
    let disabled = send(&context.app, "GET", "/api/v1/repos", Some(&bob), None, None).await;
    assert_eq!(disabled.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn 创建仅管理员且类型默认值与非法组合稳定() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    let alice = login(&context.app, "alice", "alice-pw").await;

    let unauthenticated = create_repo(&context.app, "", hosted("none", "raw", "private")).await;
    assert_eq!(unauthenticated.status, StatusCode::UNAUTHORIZED);
    let forbidden = create_repo(
        &context.app,
        &alice,
        hosted("user-created", "raw", "private"),
    )
    .await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);

    let raw = create_repo(&context.app, &admin, hosted("raw-hosted", "raw", "private")).await;
    assert_eq!(raw.status, StatusCode::CREATED);
    assert_eq!(body_json(&raw)["overwrite_policy"], "allow");
    assert!(body_json(&raw)["version_policy"].is_null());

    let maven = create_repo(
        &context.app,
        &admin,
        hosted("maven-hosted", "maven", "private"),
    )
    .await;
    assert_eq!(maven.status, StatusCode::CREATED);
    assert_eq!(body_json(&maven)["version_policy"], "mixed");

    for body in [
        json!({
            "name": "proxy-empty",
            "format": "raw",
            "repo_type": "proxy",
            "visibility": "private",
            "upstream_url": ""
        }),
        json!({
            "name": "hosted-upstream",
            "format": "raw",
            "repo_type": "hosted",
            "visibility": "private",
            "upstream_url": "https://example.invalid"
        }),
        json!({
            "name": "empty-group",
            "format": "raw",
            "repo_type": "group",
            "visibility": "private",
            "members": []
        }),
    ] {
        let response = create_repo(&context.app, &admin, body).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        assert_eq!(body_json(&response)["code"], "REPO_INVALID_CONFIG");
    }
}

#[tokio::test]
async fn proxy上游地址创建与patch执行最小严格校验() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    let invalid_urls = [
        "https://example.invalid ",
        "ftp://example.invalid",
        "http:///repository",
        "https://exa mple.invalid/repository",
        "https://example.invalid/\u{0001}",
    ];
    for (index, upstream_url) in invalid_urls.iter().enumerate() {
        let response = create_repo(
            &context.app,
            &admin,
            json!({
                "name": format!("invalid-proxy-{index}"),
                "format": "raw",
                "repo_type": "proxy",
                "visibility": "private",
                "upstream_url": upstream_url
            }),
        )
        .await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        assert_eq!(body_json(&response)["code"], "REPO_INVALID_CONFIG");
    }

    let proxy = create_repo(
        &context.app,
        &admin,
        json!({
            "name": "patch-proxy",
            "format": "raw",
            "repo_type": "proxy",
            "visibility": "private",
            "upstream_url": "https://example.invalid/repository"
        }),
    )
    .await;
    assert_eq!(proxy.status, StatusCode::CREATED);
    for upstream_url in invalid_urls {
        let response = send(
            &context.app,
            "PATCH",
            "/api/v1/repos/patch-proxy",
            Some(&admin),
            None,
            Some(json!({ "upstream_url": upstream_url })),
        )
        .await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        assert_eq!(body_json(&response)["code"], "REPO_INVALID_CONFIG");
    }

    let local = create_repo(
        &context.app,
        &admin,
        json!({
            "name": "local-proxy",
            "format": "raw",
            "repo_type": "proxy",
            "visibility": "private",
            "upstream_url": "http://127.0.0.1:8080/repository"
        }),
    )
    .await;
    assert_eq!(local.status, StatusCode::CREATED);
    let patched = send(
        &context.app,
        "PATCH",
        "/api/v1/repos/patch-proxy",
        Some(&admin),
        None,
        Some(json!({ "upstream_url": "http://localhost:8080/repository" })),
    )
    .await;
    assert_eq!(patched.status, StatusCode::OK);
}

#[tokio::test]
async fn 仓库查询解析错误返回统一json() {
    let context = setup().await;
    for (method, uri) in [
        ("GET", "/api/v1/repos?page=not-a-number"),
        ("DELETE", "/api/v1/repos/example?mode=invalid"),
    ] {
        let response = send(&context.app, method, uri, None, None, None).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        assert_eq!(body_json(&response)["code"], "REPO_INVALID_QUERY");
        assert_eq!(body_json(&response)["message"], "仓库查询参数无效");
    }
}

#[tokio::test]
async fn 列表在sql中过滤计数_字面查询并批量返回group成员() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    let alice = login(&context.app, "alice", "alice-pw").await;
    for body in [
        hosted("public-one", "raw", "public"),
        hosted("private-one", "raw", "private"),
        hosted("literalxname", "raw", "public"),
    ] {
        assert_eq!(
            create_repo(&context.app, &admin, body).await.status,
            StatusCode::CREATED
        );
    }
    let group = json!({
        "name": "raw-group",
        "format": "raw",
        "repo_type": "group",
        "visibility": "public",
        "members": ["public-one", "private-one"]
    });
    let created_group = create_repo(&context.app, &admin, group).await;
    assert_eq!(created_group.status, StatusCode::CREATED);
    assert_eq!(
        body_json(&created_group)["members"],
        json!(["public-one", "private-one"])
    );
    let group_admin = send(
        &context.app,
        "PUT",
        "/api/v1/repos/raw-group/acl/alice",
        Some(&admin),
        None,
        Some(json!({ "permission": "admin" })),
    )
    .await;
    assert_eq!(group_admin.status, StatusCode::OK);

    let before = send(
        &context.app,
        "GET",
        "/api/v1/repos?page=1&page_size=50",
        Some(&alice),
        None,
        None,
    )
    .await;
    assert_eq!(before.status, StatusCode::OK);
    assert_eq!(body_json(&before)["total"], 3);
    assert!(
        body_json(&before)["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["name"] != "private-one")
    );
    let group = repository_item(&before, "raw-group");
    assert_eq!(group["members"], json!(["public-one"]));

    let anonymous_list = send(
        &context.app,
        "GET",
        "/api/v1/repos?page=1&page_size=50",
        None,
        None,
        None,
    )
    .await;
    let anonymous_group = repository_item(&anonymous_list, "raw-group");
    assert_eq!(anonymous_group["members"], json!(["public-one"]));

    let admin_list = send(
        &context.app,
        "GET",
        "/api/v1/repos?page=1&page_size=50",
        Some(&admin),
        None,
        None,
    )
    .await;
    let admin_group = repository_item(&admin_list, "raw-group");
    assert_eq!(admin_group["members"], json!(["public-one", "private-one"]));

    let literal = send(
        &context.app,
        "GET",
        "/api/v1/repos?q=public_one",
        None,
        None,
        None,
    )
    .await;
    assert_eq!(body_json(&literal)["total"], 0);
}

#[tokio::test]
async fn group详情成员按读取权限过滤且patch不可探测私有成员() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    let alice = login(&context.app, "alice", "alice-pw").await;
    let bob = login(&context.app, "bob", "bob-pw").await;
    for body in [
        hosted("public-member", "raw", "public"),
        hosted("private-member", "raw", "private"),
    ] {
        assert_eq!(
            create_repo(&context.app, &admin, body).await.status,
            StatusCode::CREATED
        );
    }
    let group = json!({
        "name": "filtered-group",
        "format": "raw",
        "repo_type": "group",
        "visibility": "public",
        "members": ["public-member", "private-member"]
    });
    assert_eq!(
        create_repo(&context.app, &admin, group).await.status,
        StatusCode::CREATED
    );
    let grant = send(
        &context.app,
        "PUT",
        "/api/v1/repos/filtered-group/acl/alice",
        Some(&admin),
        None,
        Some(json!({ "permission": "admin" })),
    )
    .await;
    assert_eq!(grant.status, StatusCode::OK);
    let member_read = send(
        &context.app,
        "PUT",
        "/api/v1/repos/private-member/acl/bob",
        Some(&admin),
        None,
        Some(json!({ "permission": "read" })),
    )
    .await;
    assert_eq!(member_read.status, StatusCode::OK);

    for auth in [None, Some(alice.as_str())] {
        let detail = send(
            &context.app,
            "GET",
            "/api/v1/repos/filtered-group",
            auth,
            None,
            None,
        )
        .await;
        assert_eq!(detail.status, StatusCode::OK);
        assert_eq!(body_json(&detail)["members"], json!(["public-member"]));
    }
    let bob_detail = send(
        &context.app,
        "GET",
        "/api/v1/repos/filtered-group",
        Some(&bob),
        None,
        None,
    )
    .await;
    assert_eq!(
        body_json(&bob_detail)["members"],
        json!(["public-member", "private-member"])
    );
    let admin_detail = send(
        &context.app,
        "GET",
        "/api/v1/repos/filtered-group",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(
        body_json(&admin_detail)["members"],
        json!(["public-member", "private-member"])
    );

    let hidden = send(
        &context.app,
        "PATCH",
        "/api/v1/repos/filtered-group",
        Some(&alice),
        Some("fixed-request-id"),
        Some(json!({ "members": ["private-member"] })),
    )
    .await;
    let missing = send(
        &context.app,
        "PATCH",
        "/api/v1/repos/filtered-group",
        Some(&alice),
        Some("fixed-request-id"),
        Some(json!({ "members": ["not-existing"] })),
    )
    .await;
    assert_eq!(hidden.status, StatusCode::BAD_REQUEST);
    assert_eq!(hidden.status, missing.status);
    assert_eq!(hidden.body, missing.body);
    assert_eq!(body_json(&hidden)["code"], "REPO_INVALID_CONFIG");
    assert_eq!(body_json(&hidden)["message"], "group 成员不存在或无权访问");

    let readable = send(
        &context.app,
        "PATCH",
        "/api/v1/repos/filtered-group",
        Some(&alice),
        None,
        Some(json!({ "members": ["public-member"] })),
    )
    .await;
    assert_eq!(readable.status, StatusCode::OK);
    assert_eq!(body_json(&readable)["members"], json!(["public-member"]));
}

#[tokio::test]
async fn 私有无读取权与不存在响应逐字节一致_读取后动作不足返回四百零三() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    let alice = login(&context.app, "alice", "alice-pw").await;
    let bob = login(&context.app, "bob", "bob-pw").await;
    assert_eq!(
        create_repo(
            &context.app,
            &admin,
            hosted("private-secret", "raw", "private"),
        )
        .await
        .status,
        StatusCode::CREATED
    );

    let hidden = send(
        &context.app,
        "GET",
        "/api/v1/repos/private-secret",
        Some(&bob),
        Some("fixed-request-id"),
        None,
    )
    .await;
    let missing = send(
        &context.app,
        "GET",
        "/api/v1/repos/not-existing",
        Some(&bob),
        Some("fixed-request-id"),
        None,
    )
    .await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    assert_eq!(hidden.status, missing.status);
    assert_eq!(hidden.request_id, missing.request_id);
    assert_eq!(hidden.body, missing.body);
    assert_eq!(body_json(&hidden)["code"], "REPO_NOT_FOUND");
    assert_eq!(body_json(&hidden)["message"], "仓库不存在或无权访问");

    let acl = send(
        &context.app,
        "PUT",
        "/api/v1/repos/private-secret/acl/alice",
        Some(&admin),
        None,
        Some(json!({ "permission": "read" })),
    )
    .await;
    assert_eq!(acl.status, StatusCode::OK);
    let insufficient = send(
        &context.app,
        "PATCH",
        "/api/v1/repos/private-secret",
        Some(&alice),
        None,
        Some(json!({ "disabled": true })),
    )
    .await;
    assert_eq!(insufficient.status, StatusCode::FORBIDDEN);
    assert_eq!(body_json(&insufficient)["code"], "REPO_FORBIDDEN");
}

#[tokio::test]
async fn 删除先授权再校验确认头且私有无权与不存在逐字节一致() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    let bob = login(&context.app, "bob", "bob-pw").await;
    assert_eq!(
        create_repo(
            &context.app,
            &admin,
            hosted("delete-hidden", "raw", "private"),
        )
        .await
        .status,
        StatusCode::CREATED
    );

    let hidden = send(
        &context.app,
        "DELETE",
        "/api/v1/repos/delete-hidden?mode=config",
        Some(&bob),
        Some("fixed-request-id"),
        None,
    )
    .await;
    let missing = send(
        &context.app,
        "DELETE",
        "/api/v1/repos/not-existing?mode=config",
        Some(&bob),
        Some("fixed-request-id"),
        None,
    )
    .await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    assert_eq!(hidden.status, missing.status);
    assert_eq!(hidden.request_id, missing.request_id);
    assert_eq!(hidden.body, missing.body);
    assert_eq!(body_json(&hidden)["code"], "REPO_NOT_FOUND");

    let authorized = send(
        &context.app,
        "DELETE",
        "/api/v1/repos/delete-hidden?mode=config",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(authorized.status, StatusCode::BAD_REQUEST);
    assert_eq!(body_json(&authorized)["code"], "REPO_DELETE_CONFIRMATION");

    let invalid_utf8 = Request::delete("/api/v1/repos/delete-hidden?mode=config")
        .header(header::AUTHORIZATION, &admin)
        .header(
            "x-confirm-repository",
            HeaderValue::from_bytes(&[0xff]).unwrap(),
        )
        .body(Body::empty())
        .unwrap();
    let response = context.app.clone().oneshot(invalid_utf8).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let error: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["code"], "REPO_DELETE_CONFIRMATION");
}

#[tokio::test]
async fn patch只修改给定字段并可显式清空凭据_group失败替换保持原子() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    for body in [
        hosted("member-a", "raw", "private"),
        hosted("member-b", "raw", "private"),
        hosted("npm-member", "npm", "private"),
    ] {
        assert_eq!(
            create_repo(&context.app, &admin, body).await.status,
            StatusCode::CREATED
        );
    }
    let proxy = json!({
        "name": "proxy-a",
        "format": "raw",
        "repo_type": "proxy",
        "visibility": "private",
        "upstream_url": "https://example.invalid/repository",
        "upstream_credential_ref": "UPSTREAM_AUTH"
    });
    assert_eq!(
        create_repo(&context.app, &admin, proxy).await.status,
        StatusCode::CREATED
    );
    let cleared = send(
        &context.app,
        "PATCH",
        "/api/v1/repos/proxy-a",
        Some(&admin),
        None,
        Some(json!({ "upstream_credential_ref": null })),
    )
    .await;
    assert_eq!(cleared.status, StatusCode::OK);
    assert!(body_json(&cleared)["upstream_credential_ref"].is_null());
    assert_eq!(
        body_json(&cleared)["upstream_url"],
        "https://example.invalid/repository"
    );

    let group = json!({
        "name": "group-a",
        "format": "raw",
        "repo_type": "group",
        "visibility": "private",
        "members": ["member-b", "member-a"]
    });
    assert_eq!(
        create_repo(&context.app, &admin, group).await.status,
        StatusCode::CREATED
    );
    let invalid = send(
        &context.app,
        "PATCH",
        "/api/v1/repos/group-a",
        Some(&admin),
        None,
        Some(json!({ "members": ["member-a", "npm-member"] })),
    )
    .await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    let detail = send(
        &context.app,
        "GET",
        "/api/v1/repos/group-a",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(
        body_json(&detail)["members"],
        json!(["member-b", "member-a"])
    );
}

#[tokio::test]
async fn acl精确用户名_upsert_remove与目标不存在() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    assert_eq!(
        create_repo(&context.app, &admin, hosted("acl-repo", "raw", "private"),)
            .await
            .status,
        StatusCode::CREATED
    );
    let missing = send(
        &context.app,
        "PUT",
        "/api/v1/repos/acl-repo/acl/Alice",
        Some(&admin),
        None,
        Some(json!({ "permission": "read" })),
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);

    for permission in ["read", "admin"] {
        let response = send(
            &context.app,
            "PUT",
            "/api/v1/repos/acl-repo/acl/alice",
            Some(&admin),
            None,
            Some(json!({ "permission": permission })),
        )
        .await;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(body_json(&response)["permission"], permission);
    }
    let list = send(
        &context.app,
        "GET",
        "/api/v1/repos/acl-repo/acl",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(body_json(&list).as_array().unwrap().len(), 1);
    assert_eq!(body_json(&list)[0]["permission"], "admin");

    let removed = send(
        &context.app,
        "DELETE",
        "/api/v1/repos/acl-repo/acl/alice",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);
    let absent = send(
        &context.app,
        "DELETE",
        "/api/v1/repos/acl-repo/acl/alice",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(absent.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn 停用状态管理面仍可见_删除引用保护与双模式边界() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    for body in [
        hosted("delete-member", "raw", "private"),
        hosted("delete-standalone", "raw", "private"),
    ] {
        assert_eq!(
            create_repo(&context.app, &admin, body).await.status,
            StatusCode::CREATED
        );
    }
    assert_eq!(
        create_repo(
            &context.app,
            &admin,
            json!({
                "name": "delete-group",
                "format": "raw",
                "repo_type": "group",
                "visibility": "private",
                "members": ["delete-member"]
            }),
        )
        .await
        .status,
        StatusCode::CREATED
    );
    let disabled = send(
        &context.app,
        "PATCH",
        "/api/v1/repos/delete-member",
        Some(&admin),
        None,
        Some(json!({ "disabled": true })),
    )
    .await;
    assert_eq!(disabled.status, StatusCode::OK);
    let visible = send(
        &context.app,
        "GET",
        "/api/v1/repos/delete-member",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(visible.status, StatusCode::OK);
    assert_eq!(body_json(&visible)["disabled"], true);

    let in_use = send(
        &context.app,
        "DELETE",
        "/api/v1/repos/delete-member?mode=config",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(
        in_use.status,
        StatusCode::BAD_REQUEST,
        "完成仓库管理员授权后应校验确认头"
    );
    let request = Request::delete("/api/v1/repos/delete-member?mode=config")
        .header(header::AUTHORIZATION, &admin)
        .header("x-confirm-repository", "delete-member")
        .body(Body::empty())
        .unwrap();
    let response = context.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let data_request = Request::delete("/api/v1/repos/delete-standalone?mode=data")
        .header(header::AUTHORIZATION, &admin)
        .header("x-confirm-repository", "delete-standalone")
        .body(Body::empty())
        .unwrap();
    let data_response = context.app.clone().oneshot(data_request).await.unwrap();
    assert_eq!(data_response.status(), StatusCode::OK);
    let bytes = data_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let result: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(result["orphaned_blobs"], 0);
    assert_eq!(result["reclaimed_blobs"], 0);
    let remains = send(
        &context.app,
        "GET",
        "/api/v1/repos/delete-standalone",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(remains.status, StatusCode::NOT_FOUND);
}
