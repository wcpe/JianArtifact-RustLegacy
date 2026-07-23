//! Raw 协议真实 HTTP 集成测试。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use axum::{Router, response::IntoResponse};
use http_body_util::BodyExt;
use jianartifact_server::api::router;
use jianartifact_server::artifact::ArtifactService;
use jianartifact_server::auth::AuthRuntime;
use jianartifact_server::config::Config;
use jianartifact_server::domain::{Role, create_user};
use jianartifact_server::persistence;
use jianartifact_server::state::AppState;
use jianartifact_server::storage::BlobStore;
use md5::Md5;
use serde_json::Value;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tower::ServiceExt;

const JWT_SECRET: &str = "raw-api-test-jwt-secret";

struct TestContext {
    app: Router,
    pool: SqlitePool,
    _dir: TempDir,
}

struct TestResponse {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

struct Upstream {
    base_url: String,
    hits: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
struct BlockingUpstreamState {
    hits: Arc<AtomicUsize>,
    started: Arc<Notify>,
    release: Arc<Notify>,
}

struct BlockingUpstream {
    upstream: Upstream,
    started: Arc<Notify>,
    release: Arc<Notify>,
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
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
    create_user(&pool, "outsider", "outsider-pw", Role::User)
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
        npm_publish_max_bytes: 134_217_728,
        max_artifact_bytes: 1024 * 1024,
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

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    auth: Option<&str>,
    body: impl Into<Body>,
) -> TestResponse {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(auth) = auth {
        builder = builder.header(header::AUTHORIZATION, auth);
    }
    let response = app
        .clone()
        .oneshot(builder.body(body.into()).unwrap())
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

async fn delete_repo(app: &Router, name: &str, mode: &str, auth: &str) -> TestResponse {
    let response = app
        .clone()
        .oneshot(
            Request::delete(format!("/api/v1/repos/{name}?mode={mode}"))
                .header(header::AUTHORIZATION, auth)
                .header("x-confirm-repository", name)
                .body(Body::empty())
                .unwrap(),
        )
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

async fn patch_upstream(app: &Router, name: &str, upstream_url: &str, auth: &str) -> TestResponse {
    let request = Request::patch(format!("/api/v1/repos/{name}"))
        .header(header::AUTHORIZATION, auth)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({ "upstream_url": upstream_url })).unwrap(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
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

async fn login(app: &Router, username: &str, password: &str) -> String {
    let body = serde_json::json!({ "username": username, "password": password });
    let request = Request::post("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    format!("Bearer {}", json["token"].as_str().unwrap())
}

async fn insert_repository(
    pool: &SqlitePool,
    name: &str,
    format: &str,
    repo_type: &str,
    visibility: &str,
    disabled: bool,
    configuration: (Option<&str>, Option<&str>),
) -> i64 {
    let (overwrite_policy, upstream_url) = configuration;
    let version_policy = (format == "maven" && repo_type == "hosted").then_some("mixed");
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO repositories (name, format, repo_type, visibility, disabled, overwrite_policy, version_policy, upstream_url) VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(name)
    .bind(format)
    .bind(repo_type)
    .bind(visibility)
    .bind(disabled)
    .bind(overwrite_policy)
    .bind(version_policy)
    .bind(upstream_url)
    .fetch_one(pool)
    .await
    .unwrap();
    row.0
}

async fn grant(pool: &SqlitePool, repo_id: i64, username: &str, permission: &str) {
    sqlx::query(
        "INSERT INTO repository_acl (repo_id, user_id, permission) SELECT ?, id, ? FROM users WHERE username = ?",
    )
    .bind(repo_id)
    .bind(permission)
    .bind(username)
    .execute(pool)
    .await
    .unwrap();
}

fn json_body(response: &TestResponse) -> Value {
    serde_json::from_slice(&response.body).unwrap()
}

fn digest_hex<D: Digest + Default>(bytes: &[u8]) -> String {
    let mut digest = D::default();
    digest.update(bytes);
    digest
        .finalize()
        .iter()
        .fold(String::new(), |mut text, byte| {
            text.push_str(&format!("{byte:02x}"));
            text
        })
}

async fn upstream_handler(
    State(hits): State<Arc<AtomicUsize>>,
    Path(path): Path<String>,
) -> Response {
    hits.fetch_add(1, Ordering::SeqCst);
    match path.as_str() {
        "base/ok.bin" => (StatusCode::OK, "proxy-content").into_response(),
        "base/head.bin" => (StatusCode::OK, "head-content").into_response(),
        "base/single.bin" => {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            (StatusCode::OK, "single-flight").into_response()
        }
        "base/race.bin" => (StatusCode::OK, "new-content").into_response(),
        "base/not-found.bin" => StatusCode::NOT_FOUND.into_response(),
        "base/redirect.bin" => (
            StatusCode::FOUND,
            [(header::LOCATION, "/base/ok.bin")],
            Body::empty(),
        )
            .into_response(),
        "base/client-error.bin" => StatusCode::BAD_REQUEST.into_response(),
        "base/server-error.bin" => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn start_upstream() -> Upstream {
    let hits = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/{*path}", get(upstream_handler))
        .with_state(Arc::clone(&hits));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Upstream {
        base_url: format!("http://{address}/base"),
        hits,
        task,
    }
}

async fn blocking_upstream_handler(
    State(state): State<BlockingUpstreamState>,
    Path(path): Path<String>,
) -> Response {
    state.hits.fetch_add(1, Ordering::SeqCst);
    state.started.notify_one();
    state.release.notified().await;
    match path.as_str() {
        "base/race.bin" => (StatusCode::OK, "old-content").into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn start_blocking_upstream() -> BlockingUpstream {
    let hits = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let app = Router::new()
        .route("/{*path}", get(blocking_upstream_handler))
        .with_state(BlockingUpstreamState {
            hits: Arc::clone(&hits),
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    BlockingUpstream {
        upstream: Upstream {
            base_url: format!("http://{address}/base"),
            hits,
            task,
        },
        started,
        release,
    }
}

async fn start_broken_upstream() -> Upstream {
    let hits = Arc::new(AtomicUsize::new(0));
    let server_hits = Arc::clone(&hits);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let server_hits = Arc::clone(&server_hits);
            tokio::spawn(async move {
                server_hits.fetch_add(1, Ordering::SeqCst);
                let mut request = vec![0_u8; 4096];
                let _ = stream.read(&mut request).await;
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nshort",
                    )
                    .await
                    .unwrap();
            });
        }
    });
    Upstream {
        base_url: format!("http://{address}/base"),
        hits,
        task,
    }
}

#[tokio::test]
async fn hosted支持流式crud_覆盖策略_响应头与目录分页() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    insert_repository(
        &context.pool,
        "raw-public",
        "raw",
        "hosted",
        "public",
        false,
        (Some("allow"), None),
    )
    .await;
    insert_repository(
        &context.pool,
        "raw-deny",
        "raw",
        "hosted",
        "private",
        false,
        (Some("deny"), None),
    )
    .await;

    let first = send(
        &context.app,
        "PUT",
        "/repository/raw-public/dir/file.txt",
        Some(&admin),
        "hello raw",
    )
    .await;
    assert_eq!(first.status, StatusCode::CREATED);

    let get = send(
        &context.app,
        "GET",
        "/repository/raw-public/dir/file.txt?download=1",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(get.status, StatusCode::OK);
    assert_eq!(get.body, b"hello raw");
    assert_eq!(get.headers[header::CONTENT_TYPE], "text/plain");
    assert_eq!(get.headers[header::CONTENT_LENGTH], "9");
    let sha256 = digest_hex::<Sha256>(b"hello raw");
    assert_eq!(get.headers[header::ETAG], format!("\"{sha256}\""));
    assert_eq!(get.headers["x-checksum-sha256"], sha256);
    assert_eq!(
        get.headers["x-checksum-sha1"],
        digest_hex::<Sha1>(b"hello raw")
    );
    assert_eq!(
        get.headers["x-checksum-sha512"],
        digest_hex::<Sha512>(b"hello raw")
    );
    assert_eq!(
        get.headers["x-checksum-md5"],
        digest_hex::<Md5>(b"hello raw")
    );

    let head = send(
        &context.app,
        "HEAD",
        "/repository/raw-public/dir/file.txt",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(head.status, StatusCode::OK);
    assert!(head.body.is_empty());
    assert_eq!(head.headers[header::CONTENT_LENGTH], "9");
    assert_eq!(head.headers[header::ETAG], get.headers[header::ETAG]);

    let overwrite = send(
        &context.app,
        "PUT",
        "/repository/raw-public/dir/file.txt",
        Some(&admin),
        "new",
    )
    .await;
    assert_eq!(overwrite.status, StatusCode::OK);

    assert_eq!(
        send(
            &context.app,
            "PUT",
            "/repository/raw-deny/file.bin",
            Some(&admin),
            "old",
        )
        .await
        .status,
        StatusCode::CREATED
    );
    assert_eq!(
        send(
            &context.app,
            "PUT",
            "/repository/raw-deny/file.bin",
            Some(&admin),
            "new",
        )
        .await
        .status,
        StatusCode::CONFLICT
    );

    let root = send(
        &context.app,
        "GET",
        "/repository/raw-public/?page=1&page_size=999",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(root.status, StatusCode::OK);
    assert_eq!(json_body(&root)["page"], 1);
    assert_eq!(json_body(&root)["page_size"], 200);
    assert_eq!(json_body(&root)["total"], 1);
    assert_eq!(json_body(&root)["items"][0]["name"], "dir");
    assert_eq!(json_body(&root)["items"][0]["path"], "dir");
    assert_eq!(json_body(&root)["items"][0]["is_directory"], true);
    assert!(json_body(&root)["items"][0]["size"].is_null());
    assert!(json_body(&root)["items"][0]["updated_at"].is_string());

    let directory = send(
        &context.app,
        "GET",
        "/repository/raw-public/dir/",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(directory.status, StatusCode::OK);
    assert_eq!(json_body(&directory)["items"][0]["path"], "dir/file.txt");
    assert_eq!(json_body(&directory)["items"][0]["is_directory"], false);
    assert_eq!(json_body(&directory)["items"][0]["size"], 3);

    let managed_files = send(
        &context.app,
        "GET",
        "/api/v1/repos/raw-public/files?path=dir",
        Some(&admin),
        Body::empty(),
    )
    .await;
    assert_eq!(managed_files.status, StatusCode::OK);
    assert_eq!(json_body(&managed_files)["total"], 1);
    assert_eq!(
        json_body(&managed_files)["items"][0]["path"],
        "dir/file.txt"
    );
    assert_eq!(
        json_body(&managed_files)["items"][0]["content_type"],
        "text/plain"
    );
    assert!(json_body(&managed_files)["items"][0]["sha256"].is_string());

    let managed_search = send(
        &context.app,
        "GET",
        "/api/v1/repos/raw-public/search?q=file",
        Some(&admin),
        Body::empty(),
    )
    .await;
    assert_eq!(managed_search.status, StatusCode::OK);
    assert_eq!(json_body(&managed_search)["total"], 1);
    assert_eq!(
        json_body(&managed_search)["items"][0]["path"],
        "dir/file.txt"
    );

    for (method, uri) in [
        ("PUT", "/repository/raw-public/"),
        ("DELETE", "/repository/raw-public/"),
        ("PUT", "/repository/raw-public/dir/"),
        ("DELETE", "/repository/raw-public/dir/"),
    ] {
        assert_eq!(
            send(&context.app, method, uri, Some(&admin), Body::empty())
                .await
                .status,
            StatusCode::BAD_REQUEST
        );
    }

    assert_eq!(
        send(
            &context.app,
            "DELETE",
            "/repository/raw-public/dir/file.txt",
            Some(&admin),
            Body::empty(),
        )
        .await
        .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(
            &context.app,
            "DELETE",
            "/repository/raw-public/dir/file.txt",
            Some(&admin),
            Body::empty(),
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn 仓库删除按跨仓引用区分保留与物理回收() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    for name in ["delete-shared-a", "delete-shared-b", "delete-config"] {
        insert_repository(
            &context.pool,
            name,
            "raw",
            "hosted",
            "private",
            false,
            (Some("allow"), None),
        )
        .await;
    }
    for name in ["delete-shared-a", "delete-shared-b"] {
        assert_eq!(
            send(
                &context.app,
                "PUT",
                &format!("/repository/{name}/shared.bin"),
                Some(&admin),
                "shared",
            )
            .await
            .status,
            StatusCode::CREATED
        );
    }
    assert_eq!(
        send(
            &context.app,
            "PUT",
            "/repository/delete-config/unique.bin",
            Some(&admin),
            "unique",
        )
        .await
        .status,
        StatusCode::CREATED
    );

    let first = delete_repo(&context.app, "delete-shared-a", "data", &admin).await;
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(json_body(&first)["reclaimed_blobs"], 0);
    assert_eq!(
        send(
            &context.app,
            "GET",
            "/repository/delete-shared-b/shared.bin",
            Some(&admin),
            Body::empty(),
        )
        .await
        .status,
        StatusCode::OK
    );

    let second = delete_repo(&context.app, "delete-shared-b", "data", &admin).await;
    assert_eq!(second.status, StatusCode::OK);
    assert_eq!(json_body(&second)["reclaimed_blobs"], 1);

    let config = delete_repo(&context.app, "delete-config", "config", &admin).await;
    assert_eq!(config.status, StatusCode::OK);
    assert_eq!(json_body(&config)["orphaned_blobs"], 1);
    assert_eq!(json_body(&config)["reclaimed_blobs"], 0);
}

#[tokio::test]
async fn 协议授权隐藏私有与停用仓库并拒绝无效路径和不支持写入() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    let reader = login(&context.app, "reader", "reader-pw").await;
    let outsider = login(&context.app, "outsider", "outsider-pw").await;
    let private_id = insert_repository(
        &context.pool,
        "raw-private",
        "raw",
        "hosted",
        "private",
        false,
        (Some("allow"), None),
    )
    .await;
    grant(&context.pool, private_id, "reader", "read").await;
    insert_repository(
        &context.pool,
        "raw-disabled",
        "raw",
        "hosted",
        "public",
        true,
        (Some("allow"), None),
    )
    .await;
    insert_repository(
        &context.pool,
        "maven-hosted",
        "maven",
        "hosted",
        "private",
        false,
        (Some("allow"), None),
    )
    .await;
    insert_repository(
        &context.pool,
        "raw-group",
        "raw",
        "group",
        "private",
        false,
        (None, None),
    )
    .await;
    insert_repository(
        &context.pool,
        "raw-proxy-write",
        "raw",
        "proxy",
        "private",
        false,
        (None, Some("http://127.0.0.1:9/base")),
    )
    .await;

    assert_eq!(
        send(
            &context.app,
            "PUT",
            "/repository/raw-private/file.bin",
            Some(&admin),
            "secret",
        )
        .await
        .status,
        StatusCode::CREATED
    );
    assert_eq!(
        send(
            &context.app,
            "GET",
            "/repository/raw-private/file.bin",
            Some(&reader),
            Body::empty(),
        )
        .await
        .status,
        StatusCode::OK
    );
    assert_eq!(
        send(
            &context.app,
            "PUT",
            "/repository/raw-private/file.bin",
            Some(&reader),
            "denied",
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );

    for auth in [None, Some(outsider.as_str())] {
        assert_eq!(
            send(
                &context.app,
                "GET",
                "/repository/raw-private/file.bin",
                auth,
                Body::empty(),
            )
            .await
            .status,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        send(
            &context.app,
            "GET",
            "/repository/raw-disabled/file.bin",
            Some(&admin),
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
            "/repository/raw-private/file.bin",
            Some("Bearer broken.jwt"),
            Body::empty(),
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &context.app,
            "GET",
            "/repository/maven-hosted/file.bin",
            Some(&admin),
            Body::empty(),
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    for repo in ["raw-group", "raw-proxy-write"] {
        assert_eq!(
            send(
                &context.app,
                "PUT",
                &format!("/repository/{repo}/file.bin"),
                Some(&admin),
                "body",
            )
            .await
            .status,
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
    for uri in [
        "/repository/raw-proxy-write/",
        "/repository/raw-proxy-write/dir/",
    ] {
        assert_eq!(
            send(&context.app, "PUT", uri, Some(&admin), "body")
                .await
                .status,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            send(&context.app, "DELETE", uri, Some(&admin), Body::empty(),)
                .await
                .status,
            StatusCode::BAD_REQUEST
        );
    }

    for path in [
        "dir//file",
        "dir/../file",
        "dir%2Ffile",
        "%252e%252e/file",
        &"a".repeat(4097),
    ] {
        let response = send(
            &context.app,
            "GET",
            &format!("/repository/raw-private/{path}"),
            Some(&admin),
            Body::empty(),
        )
        .await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "路径={path:?}");
    }
}

#[tokio::test]
async fn proxy配置更新后旧回源拒绝提交并按新快照透明重试() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    let old_upstream = start_blocking_upstream().await;
    let new_upstream = start_upstream().await;
    insert_repository(
        &context.pool,
        "proxy-race",
        "raw",
        "proxy",
        "public",
        false,
        (None, Some(&old_upstream.upstream.base_url)),
    )
    .await;

    let first_app = context.app.clone();
    let first = tokio::spawn(async move {
        send(
            &first_app,
            "GET",
            "/repository/proxy-race/race.bin",
            None,
            Body::empty(),
        )
        .await
    });
    old_upstream.started.notified().await;

    let patched = patch_upstream(&context.app, "proxy-race", &new_upstream.base_url, &admin).await;
    assert_eq!(patched.status, StatusCode::OK);

    let second = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        send(
            &context.app,
            "GET",
            "/repository/proxy-race/race.bin",
            None,
            Body::empty(),
        ),
    )
    .await;
    old_upstream.release.notify_one();
    let second = second.expect("更新后的请求不得等待旧上游单飞任务");
    let first = tokio::time::timeout(std::time::Duration::from_secs(1), first)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(first.body, b"new-content");
    assert_eq!(second.status, StatusCode::OK);
    assert_eq!(second.body, b"new-content");
    assert_eq!(old_upstream.upstream.hits.load(Ordering::SeqCst), 1);
    assert_eq!(new_upstream.hits.load(Ordering::SeqCst), 1);

    let hit = send(
        &context.app,
        "GET",
        "/repository/proxy-race/race.bin",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(hit.status, StatusCode::OK);
    assert_eq!(hit.body, b"new-content");
    assert_eq!(new_upstream.hits.load(Ordering::SeqCst), 1);

    let cached: Vec<(String,)> = sqlx::query_as(
        "SELECT artifacts.sha256 FROM artifacts JOIN repositories ON repositories.id = repo_id WHERE repositories.name = 'proxy-race' AND path = 'race.bin'",
    )
    .fetch_all(&context.pool)
    .await
    .unwrap();
    assert_eq!(cached, vec![(digest_hex::<Sha256>(b"new-content"),)]);
    assert_ne!(cached[0].0, digest_hex::<Sha256>(b"old-content"));
}

#[tokio::test]
async fn proxy配置连续变化一次重试后稳定返回503() {
    let context = setup().await;
    let admin = login(&context.app, "admin", "admin-pw").await;
    let first_upstream = start_blocking_upstream().await;
    let second_upstream = start_blocking_upstream().await;
    let final_upstream = start_upstream().await;
    insert_repository(
        &context.pool,
        "proxy-changing",
        "raw",
        "proxy",
        "public",
        false,
        (None, Some(&first_upstream.upstream.base_url)),
    )
    .await;

    let request_app = context.app.clone();
    let request = tokio::spawn(async move {
        send(
            &request_app,
            "GET",
            "/repository/proxy-changing/race.bin",
            None,
            Body::empty(),
        )
        .await
    });
    first_upstream.started.notified().await;
    assert_eq!(
        patch_upstream(
            &context.app,
            "proxy-changing",
            &second_upstream.upstream.base_url,
            &admin,
        )
        .await
        .status,
        StatusCode::OK
    );
    first_upstream.release.notify_one();
    second_upstream.started.notified().await;
    assert_eq!(
        patch_upstream(
            &context.app,
            "proxy-changing",
            &final_upstream.base_url,
            &admin,
        )
        .await
        .status,
        StatusCode::OK
    );
    second_upstream.release.notify_one();

    let response = tokio::time::timeout(std::time::Duration::from_secs(2), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_body(&response)["code"], "PROXY_CONFIGURATION_CHANGED");
    assert_eq!(first_upstream.upstream.hits.load(Ordering::SeqCst), 1);
    assert_eq!(second_upstream.upstream.hits.load(Ordering::SeqCst), 1);
    assert_eq!(final_upstream.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn proxy未命中完整缓存_head同样回源且并发请求单飞() {
    let context = setup().await;
    let upstream = start_upstream().await;
    insert_repository(
        &context.pool,
        "raw-proxy",
        "raw",
        "proxy",
        "public",
        false,
        (None, Some(&upstream.base_url)),
    )
    .await;

    let miss = send(
        &context.app,
        "GET",
        "/repository/raw-proxy/ok.bin",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(miss.status, StatusCode::OK);
    assert_eq!(miss.body, b"proxy-content");
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 1);
    let hit = send(
        &context.app,
        "GET",
        "/repository/raw-proxy/ok.bin",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(hit.status, StatusCode::OK);
    assert_eq!(hit.body, b"proxy-content");
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 1);

    let head = send(
        &context.app,
        "HEAD",
        "/repository/raw-proxy/head.bin",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(head.status, StatusCode::OK);
    assert!(head.body.is_empty());
    assert_eq!(head.headers[header::CONTENT_LENGTH], "12");
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 2);
    let cached_after_head = send(
        &context.app,
        "GET",
        "/repository/raw-proxy/head.bin",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(cached_after_head.body, b"head-content");
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 2);

    let mut requests = Vec::new();
    for _ in 0..32 {
        let app = context.app.clone();
        requests.push(tokio::spawn(async move {
            send(
                &app,
                "GET",
                "/repository/raw-proxy/single.bin",
                None,
                Body::empty(),
            )
            .await
        }));
    }
    for request in requests {
        let response = request.await.unwrap();
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, b"single-flight");
    }
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn proxy拒绝查询和写入并稳定映射上游状态与断流() {
    let context = setup().await;
    let upstream = start_upstream().await;
    let broken = start_broken_upstream().await;
    insert_repository(
        &context.pool,
        "proxy-errors",
        "raw",
        "proxy",
        "public",
        false,
        (None, Some(&upstream.base_url)),
    )
    .await;
    insert_repository(
        &context.pool,
        "proxy-broken",
        "raw",
        "proxy",
        "public",
        false,
        (None, Some(&broken.base_url)),
    )
    .await;
    insert_repository(
        &context.pool,
        "proxy-offline",
        "raw",
        "proxy",
        "public",
        false,
        (None, Some("http://127.0.0.1:9/base")),
    )
    .await;
    let credential_repo = insert_repository(
        &context.pool,
        "proxy-credential",
        "raw",
        "proxy",
        "public",
        false,
        (None, Some(&upstream.base_url)),
    )
    .await;
    sqlx::query("UPDATE repositories SET upstream_credential_ref = ? WHERE id = ?")
        .bind("JIANARTIFACT_TEST_MISSING_UPSTREAM_AUTH")
        .bind(credential_repo)
        .execute(&context.pool)
        .await
        .unwrap();

    assert_eq!(
        send(
            &context.app,
            "GET",
            "/repository/proxy-errors/ok.bin?download=1",
            None,
            Body::empty(),
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    for method in ["PUT", "DELETE"] {
        assert_eq!(
            send(
                &context.app,
                method,
                "/repository/proxy-errors/ok.bin",
                None,
                Body::empty(),
            )
            .await
            .status,
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
    for (path, expected, code) in [
        (
            "not-found.bin",
            StatusCode::NOT_FOUND,
            "PROXY_UPSTREAM_NOT_FOUND",
        ),
        (
            "redirect.bin",
            StatusCode::BAD_GATEWAY,
            "PROXY_UPSTREAM_REDIRECT",
        ),
        (
            "client-error.bin",
            StatusCode::BAD_GATEWAY,
            "PROXY_UPSTREAM_CLIENT_ERROR",
        ),
        (
            "server-error.bin",
            StatusCode::BAD_GATEWAY,
            "PROXY_UPSTREAM_SERVER_ERROR",
        ),
    ] {
        let response = send(
            &context.app,
            "GET",
            &format!("/repository/proxy-errors/{path}"),
            None,
            Body::empty(),
        )
        .await;
        assert_eq!(response.status, expected, "path={path}");
        assert_eq!(
            serde_json::from_slice::<Value>(&response.body).unwrap()["code"],
            code
        );
    }
    let credential = send(
        &context.app,
        "GET",
        "/repository/proxy-credential/file.bin",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(credential.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        serde_json::from_slice::<Value>(&credential.body).unwrap()["code"],
        "PROXY_UPSTREAM_CONFIG"
    );

    let offline = send(
        &context.app,
        "GET",
        "/repository/proxy-offline/file.bin",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(offline.status, StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(
        serde_json::from_slice::<Value>(&offline.body).unwrap()["code"],
        "PROXY_UPSTREAM_CONNECTION"
    );

    let broken_response = send(
        &context.app,
        "GET",
        "/repository/proxy-broken/file.bin",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(broken_response.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        serde_json::from_slice::<Value>(&broken_response.body).unwrap()["code"],
        "PROXY_UPSTREAM_TRANSFER"
    );
    assert_eq!(broken.hits.load(Ordering::SeqCst), 1);
    let cached: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM artifacts JOIN repositories ON repositories.id = repo_id WHERE repositories.name = 'proxy-broken'",
    )
    .fetch_one(&context.pool)
    .await
    .unwrap();
    assert_eq!(cached.0, 0, "断流失败不得留下缓存索引");
}
