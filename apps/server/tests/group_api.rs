//! 三格式 group 协议真实 HTTP 集成测试。
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
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
use sha1::Sha1;
use sha2::{Digest, Sha512};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

const JWT_SECRET: &str = "group-api-test-jwt-secret";

struct TestContext {
    app: Router,
    pool: SqlitePool,
    _dir: TempDir,
}

struct TestResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

async fn setup() -> TestContext {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let database_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = persistence::init(&database_url, &db_path).await.unwrap();
    create_user(&pool, "admin", "admin-pw", Role::Admin)
        .await
        .unwrap();
    create_user(&pool, "reader", "reader-pw", Role::User)
        .await
        .unwrap();
    let config = Config {
        bind: "127.0.0.1:0".to_string(),
        database_path: db_path,
        data_dir: dir.path().join("blobs"),
        migration_workdir: Some(dir.path().join("migration")),
        migration_object_concurrency: 4,
        log_level: "warn".to_string(),
        jwt_secret: JWT_SECRET.to_string(),
        session_ttl_hours: 8,
        public_base_url: None,
        maven_metadata_ttl_secs: 1800,
        npm_packument_ttl_secs: 1800,
        npm_publish_max_bytes: 1024 * 1024,
        max_artifact_bytes: 1024 * 1024,
        min_free_bytes: 0,
        min_free_percent: 0,
    };
    let blob_store = Arc::new(BlobStore::new(
        config.data_dir.clone(),
        config.max_artifact_bytes,
        0,
        0,
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

async fn login(app: &Router, username: &str, password: &str) -> String {
    let body = json!({"username": username, "password": password});
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&body).unwrap();
    format!("Bearer {}", value["token"].as_str().unwrap())
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    auth: Option<&str>,
    body: impl Into<Body>,
) -> TestResponse {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "packages.test");
    if let Some(auth) = auth {
        request = request.header(header::AUTHORIZATION, auth);
    }
    let response = app
        .clone()
        .oneshot(request.body(body.into()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    TestResponse {
        status,
        headers,
        body,
    }
}

async fn repository(pool: &SqlitePool, name: &str, format: &str, repo_type: &str) -> i64 {
    let overwrite = (repo_type == "hosted").then_some("allow");
    let version_policy = (format == "maven" && repo_type == "hosted").then_some("mixed");
    sqlx::query_as::<_, (i64,)>(
        "INSERT INTO repositories (name, format, repo_type, visibility, overwrite_policy, version_policy) VALUES (?, ?, ?, 'private', ?, ?) RETURNING id",
    )
    .bind(name)
    .bind(format)
    .bind(repo_type)
    .bind(overwrite)
    .bind(version_policy)
    .fetch_one(pool)
    .await
    .unwrap()
    .0
}

async fn grant(pool: &SqlitePool, repo_id: i64, username: &str) {
    sqlx::query(
        "INSERT INTO repository_acl (repo_id, user_id, permission) SELECT ?, id, 'read' FROM users WHERE username = ?",
    )
    .bind(repo_id)
    .bind(username)
    .execute(pool)
    .await
    .unwrap();
}

async fn add_members(pool: &SqlitePool, group_id: i64, members: &[i64]) {
    for (position, member_id) in members.iter().enumerate() {
        sqlx::query(
            "INSERT INTO repository_group_members (group_repo_id, member_repo_id, position) VALUES (?, ?, ?)",
        )
        .bind(group_id)
        .bind(member_id)
        .bind(position as i64)
        .execute(pool)
        .await
        .unwrap();
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or_default();
        let third = chunk.get(2).copied().unwrap_or_default();
        output.push(ALPHABET[(first >> 2) as usize] as char);
        output.push(ALPHABET[(((first & 3) << 4) | (second >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[(((second & 15) << 2) | (third >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(third & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

fn npm_publish(name: &str, version: &str, tarball: &[u8], tag: &str) -> Vec<u8> {
    let filename = format!("{}-{version}.tgz", name.rsplit('/').next().unwrap());
    serde_json::to_vec(&json!({
        "name": name,
        "dist-tags": {tag: version},
        "versions": {
            version: {
                "name": name,
                "version": version,
                "dist": {
                    "tarball": format!("http://source.invalid/{filename}"),
                    "integrity": format!("sha512-{}", base64(&Sha512::digest(tarball))),
                    "shasum": format!("{:x}", Sha1::digest(tarball))
                }
            }
        },
        "_attachments": {
            filename: {
                "data": base64(tarball),
                "length": tarball.len()
            }
        }
    }))
    .unwrap()
}

#[tokio::test]
async fn raw_group按可读成员顺序命中且空组与写入契约正确() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    let reader = login(&context.app, "reader", "reader-pw").await;
    let first = repository(&context.pool, "raw-first", "raw", "hosted").await;
    let hidden = repository(&context.pool, "raw-hidden", "raw", "hosted").await;
    let second = repository(&context.pool, "raw-second", "raw", "hosted").await;
    let group = repository(&context.pool, "raw-all", "raw", "group").await;
    let empty = repository(&context.pool, "raw-empty", "raw", "group").await;
    add_members(&context.pool, group, &[first, hidden, second]).await;
    add_members(&context.pool, empty, &[hidden]).await;
    for id in [first, second, group, empty] {
        grant(&context.pool, id, "reader").await;
    }
    for (repo, path, body) in [
        ("raw-first", "same.bin", "first"),
        ("raw-hidden", "secret.bin", "secret"),
        ("raw-second", "same.bin", "second"),
        ("raw-second", "later.bin", "later"),
    ] {
        assert_eq!(
            send(
                &context.app,
                "PUT",
                &format!("/repository/{repo}/{path}"),
                Some(&admin),
                body,
            )
            .await
            .status,
            StatusCode::CREATED
        );
    }

    let first_match = send(
        &context.app,
        "GET",
        "/repository/raw-all/same.bin",
        Some(&reader),
        Body::empty(),
    )
    .await;
    assert_eq!(first_match.status, StatusCode::OK);
    assert_eq!(first_match.body, b"first");
    let later = send(
        &context.app,
        "HEAD",
        "/repository/raw-all/later.bin",
        Some(&reader),
        Body::empty(),
    )
    .await;
    assert_eq!(later.status, StatusCode::OK);
    assert!(later.body.is_empty());
    assert_eq!(later.headers[header::CONTENT_LENGTH], "5");
    assert_eq!(
        send(
            &context.app,
            "GET",
            "/repository/raw-all/secret.bin",
            Some(&reader),
            Body::empty(),
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &context.app,
            "GET",
            "/repository/raw-empty/secret.bin",
            Some(&reader),
            Body::empty(),
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    for method in ["PUT", "DELETE"] {
        assert_eq!(
            send(
                &context.app,
                method,
                "/repository/raw-all/new.bin",
                Some(&reader),
                Body::empty(),
            )
            .await
            .status,
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
}

#[tokio::test]
async fn maven_group聚合可读hosted元数据并保持构件first_match() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    let reader = login(&context.app, "reader", "reader-pw").await;
    let first = repository(&context.pool, "maven-first", "maven", "hosted").await;
    let hidden = repository(&context.pool, "maven-hidden", "maven", "hosted").await;
    let second = repository(&context.pool, "maven-second", "maven", "hosted").await;
    let group = repository(&context.pool, "maven-all", "maven", "group").await;
    let empty = repository(&context.pool, "maven-empty", "maven", "group").await;
    add_members(&context.pool, group, &[first, hidden, second]).await;
    add_members(&context.pool, empty, &[hidden]).await;
    for id in [first, second, group, empty] {
        grant(&context.pool, id, "reader").await;
    }
    for (repo, version, body) in [
        ("maven-first", "1.0", "first-1.0"),
        ("maven-hidden", "9.9", "hidden-9.9"),
        ("maven-second", "1.0", "second-1.0"),
        ("maven-second", "2.0", "second-2.0"),
    ] {
        let path = format!("/repository/{repo}/com/example/demo/{version}/demo-{version}.jar");
        assert_eq!(
            send(&context.app, "PUT", &path, Some(&admin), body)
                .await
                .status,
            StatusCode::CREATED
        );
    }

    let artifact = send(
        &context.app,
        "GET",
        "/repository/maven-all/com/example/demo/1.0/demo-1.0.jar",
        Some(&reader),
        Body::empty(),
    )
    .await;
    assert_eq!(artifact.status, StatusCode::OK);
    assert_eq!(artifact.body, b"first-1.0");
    let metadata = send(
        &context.app,
        "GET",
        "/repository/maven-all/com/example/demo/maven-metadata.xml",
        Some(&reader),
        Body::empty(),
    )
    .await;
    let xml = String::from_utf8(metadata.body).unwrap();
    assert_eq!(metadata.status, StatusCode::OK);
    assert!(xml.contains("<version>1.0</version>"));
    assert!(xml.contains("<version>2.0</version>"));
    assert!(!xml.contains("9.9"));
    let empty_metadata = send(
        &context.app,
        "GET",
        "/repository/maven-empty/com/example/demo/maven-metadata.xml",
        Some(&reader),
        Body::empty(),
    )
    .await;
    assert_eq!(empty_metadata.status, StatusCode::OK);
    let empty_xml = String::from_utf8(empty_metadata.body).unwrap();
    assert!(empty_xml.contains("<groupId>com.example</groupId>"));
    assert!(empty_xml.contains("<versions"));
    assert!(!empty_xml.contains("9.9"));
}

#[tokio::test]
async fn npm_group前序优先合并重写地址并按版本提供者路由tarball() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    let reader = login(&context.app, "reader", "reader-pw").await;
    let first = repository(&context.pool, "npm-first", "npm", "hosted").await;
    let hidden = repository(&context.pool, "npm-hidden", "npm", "hosted").await;
    let second = repository(&context.pool, "npm-second", "npm", "hosted").await;
    let group = repository(&context.pool, "npm-all", "npm", "group").await;
    let empty = repository(&context.pool, "npm-empty", "npm", "group").await;
    add_members(&context.pool, group, &[first, hidden, second]).await;
    add_members(&context.pool, empty, &[hidden]).await;
    for id in [first, second, group, empty] {
        grant(&context.pool, id, "reader").await;
    }
    for (repo, version, tarball, tag) in [
        ("npm-first", "1.0.0", b"first".as_slice(), "latest"),
        ("npm-hidden", "9.9.9", b"hidden".as_slice(), "hidden"),
        ("npm-second", "1.0.0", b"second".as_slice(), "latest"),
        ("npm-second", "2.0.0", b"later".as_slice(), "next"),
    ] {
        assert_eq!(
            send(
                &context.app,
                "PUT",
                &format!("/repository/{repo}/demo"),
                Some(&admin),
                npm_publish("demo", version, tarball, tag),
            )
            .await
            .status,
            StatusCode::CREATED
        );
    }

    let packument = send(
        &context.app,
        "GET",
        "/repository/npm-all/demo",
        Some(&reader),
        Body::empty(),
    )
    .await;
    assert_eq!(packument.status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&packument.body).unwrap();
    assert_eq!(value["dist-tags"]["latest"], "1.0.0");
    assert_eq!(value["dist-tags"]["next"], "2.0.0");
    assert!(value["versions"].get("9.9.9").is_none());
    let first_url = value["versions"]["1.0.0"]["dist"]["tarball"]
        .as_str()
        .unwrap();
    assert!(first_url.starts_with("http://packages.test/repository/npm-all/"));
    assert!(!first_url.contains("npm-first"));
    let first_tarball = send(
        &context.app,
        "GET",
        first_url.strip_prefix("http://packages.test").unwrap(),
        Some(&reader),
        Body::empty(),
    )
    .await;
    assert_eq!(first_tarball.status, StatusCode::OK);
    assert_eq!(first_tarball.body, b"first");
    let head = send(
        &context.app,
        "HEAD",
        "/repository/npm-all/demo",
        Some(&reader),
        Body::empty(),
    )
    .await;
    assert_eq!(head.status, StatusCode::OK);
    assert!(head.body.is_empty());
    assert_eq!(
        head.headers[header::CONTENT_LENGTH],
        packument.body.len().to_string()
    );
    let empty_packument = send(
        &context.app,
        "GET",
        "/repository/npm-empty/demo",
        Some(&reader),
        Body::empty(),
    )
    .await;
    assert_eq!(empty_packument.status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&empty_packument.body).unwrap(),
        json!({"name": "demo", "versions": {}, "dist-tags": {}, "time": {}})
    );
    for method in ["PUT", "DELETE"] {
        assert_eq!(
            send(
                &context.app,
                method,
                "/repository/npm-all/demo",
                Some(&reader),
                Body::empty(),
            )
            .await
            .status,
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
}
