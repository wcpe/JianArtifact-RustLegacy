//! Maven 协议真实 HTTP 集成测试。
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::routing::get;
use http_body_util::BodyExt;
use jianartifact_server::api::router;
use jianartifact_server::artifact::ArtifactService;
use jianartifact_server::auth::AuthRuntime;
use jianartifact_server::config::Config;
use jianartifact_server::domain::{Role, create_user};
use jianartifact_server::persistence;
use jianartifact_server::state::AppState;
use jianartifact_server::storage::BlobStore;
use sha1::Sha1;
use sha2::Digest;
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

const JWT_SECRET: &str = "maven-api-test-jwt-secret";

struct TestContext {
    app: Router,
    pool: SqlitePool,
    _dir: TempDir,
}

struct TestResponse {
    status: StatusCode,
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
    let config = Config {
        bind: "127.0.0.1:0".to_string(),
        database_path: db_path,
        data_dir: dir.path().join("blobs"),
        log_level: "warn".to_string(),
        jwt_secret: JWT_SECRET.to_string(),
        session_ttl_hours: 8,
        public_base_url: None,
        maven_metadata_ttl_secs: 1800,
        npm_packument_ttl_secs: 1800,
        npm_publish_max_bytes: 134_217_728,
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

async fn login(app: &Router) -> String {
    let request = Request::post("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"username":"admin","password":"admin-pw"}"#))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    format!("Bearer {}", json["token"].as_str().unwrap())
}

async fn repository(pool: &SqlitePool, name: &str, policy: &str, overwrite: &str) -> i64 {
    sqlx::query_as::<_, (i64,)>(
        "INSERT INTO repositories (name, format, repo_type, visibility, overwrite_policy, version_policy) VALUES (?, 'maven', 'hosted', 'public', ?, ?) RETURNING id",
    )
    .bind(name)
    .bind(overwrite)
    .bind(policy)
    .fetch_one(pool)
    .await
    .unwrap()
    .0
}

async fn proxy_repository(pool: &SqlitePool, name: &str, upstream_url: &str) -> i64 {
    sqlx::query_as::<_, (i64,)>(
        "INSERT INTO repositories (name, format, repo_type, visibility, upstream_url) VALUES (?, 'maven', 'proxy', 'public', ?) RETURNING id",
    )
    .bind(name)
    .bind(upstream_url)
    .fetch_one(pool)
    .await
    .unwrap()
    .0
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    auth: Option<&str>,
    body: impl Into<Body>,
) -> TestResponse {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(auth) = auth {
        request = request.header(header::AUTHORIZATION, auth);
    }
    let response = app
        .clone()
        .oneshot(request.body(body.into()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    TestResponse { status, body }
}

fn sha1(bytes: &[u8]) -> String {
    format!("{:x}", Sha1::digest(bytes))
}

async fn start_counting_upstream() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let state_hits = Arc::clone(&hits);
    let app = Router::new()
        .route(
            "/{*path}",
            get(
                |axum::extract::State(hits): axum::extract::State<Arc<AtomicUsize>>| async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    StatusCode::NOT_FOUND
                },
            ),
        )
        .with_state(state_hits);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), hits, task)
}

#[tokio::test]
async fn hosted发布读取_sidecar核验_metadata权威生成与默认不可变() {
    let context = setup().await;
    let auth = login(&context.app).await;
    repository(&context.pool, "maven-hosted", "mixed", "deny").await;
    let base = "/repository/maven-hosted/com/example/demo/1.0/demo-1.0.jar";

    assert_eq!(
        send(&context.app, "PUT", base, Some(&auth), "jar-one")
            .await
            .status,
        StatusCode::CREATED
    );
    assert_eq!(
        send(&context.app, "GET", base, None, Body::empty())
            .await
            .body,
        b"jar-one"
    );
    let checksum = send(
        &context.app,
        "GET",
        &format!("{base}.sha1"),
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(checksum.status, StatusCode::OK);
    assert_eq!(checksum.body, sha1(b"jar-one").as_bytes());
    assert_eq!(
        send(
            &context.app,
            "PUT",
            &format!("{base}.sha1"),
            Some(&auth),
            sha1(b"jar-one"),
        )
        .await
        .status,
        StatusCode::OK
    );
    assert_eq!(
        send(
            &context.app,
            "PUT",
            &format!("{base}.sha1"),
            Some(&auth),
            "deadbeef",
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(&context.app, "PUT", base, Some(&auth), "jar-two")
            .await
            .status,
        StatusCode::CONFLICT
    );

    let metadata = send(
        &context.app,
        "GET",
        "/repository/maven-hosted/com/example/demo/maven-metadata.xml",
        None,
        Body::empty(),
    )
    .await;
    let xml = String::from_utf8(metadata.body).unwrap();
    assert_eq!(metadata.status, StatusCode::OK);
    assert!(xml.contains("<latest>1.0</latest>"));
    assert!(xml.contains("<release>1.0</release>"));
    assert!(xml.contains("<version>1.0</version>"));
}

#[tokio::test]
async fn hosted版本策略与snapshot版本metadata() {
    let context = setup().await;
    let auth = login(&context.app).await;
    repository(&context.pool, "release-only", "release", "deny").await;
    repository(&context.pool, "snapshot-only", "snapshot", "deny").await;
    let snapshot = "com/example/demo/1.1-SNAPSHOT/demo-1.1-20250101.010203-2.jar";

    assert_eq!(
        send(
            &context.app,
            "PUT",
            &format!("/repository/release-only/{snapshot}"),
            Some(&auth),
            "snapshot",
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(
            &context.app,
            "PUT",
            "/repository/snapshot-only/com/example/demo/1.0/demo-1.0.jar",
            Some(&auth),
            "release",
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(
            &context.app,
            "PUT",
            &format!("/repository/snapshot-only/{snapshot}"),
            Some(&auth),
            "snapshot",
        )
        .await
        .status,
        StatusCode::CREATED
    );
    let metadata = send(
        &context.app,
        "GET",
        "/repository/snapshot-only/com/example/demo/1.1-SNAPSHOT/maven-metadata.xml",
        None,
        Body::empty(),
    )
    .await;
    let xml = String::from_utf8(metadata.body).unwrap();
    assert!(xml.contains("<timestamp>20250101.010203</timestamp>"));
    assert!(xml.contains("<buildNumber>2</buildNumber>"));
    assert!(xml.contains("<extension>jar</extension>"));
    assert!(xml.contains("<value>1.1-20250101.010203-2</value>"));
}

#[tokio::test]
async fn proxy非法布局本地拒绝且不回源() {
    let context = setup().await;
    let (upstream_url, hits, task) = start_counting_upstream().await;
    proxy_repository(&context.pool, "maven-proxy", &upstream_url).await;

    for path in [
        "arbitrary/path.txt",
        "com/example/demo/1.0/other-1.0.jar",
        "com/example/demo/1.0/demo-1.0.jar.sha512",
        "com/example/demo/1.0/demo-1.0.jar.sha1.md5",
        "com/maven-metadata.xml",
    ] {
        let response = send(
            &context.app,
            "GET",
            &format!("/repository/maven-proxy/{path}"),
            None,
            Body::empty(),
        )
        .await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "应拒绝 {path}");
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    task.abort();
}
