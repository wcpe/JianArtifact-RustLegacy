//! npm hosted/proxy 协议真实 HTTP 集成测试。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Request, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use http_body_util::BodyExt;
use jianartifact_server::api::extractor::ArtifactRoute;
use jianartifact_server::api::router;
use jianartifact_server::artifact::ArtifactService;
use jianartifact_server::auth::AuthRuntime;
use jianartifact_server::config::Config;
use jianartifact_server::domain::repository::{Permission, Subject};
use jianartifact_server::domain::{Role, create_user};
use jianartifact_server::persistence;
use jianartifact_server::protocol::npm;
use jianartifact_server::repository;
use jianartifact_server::state::AppState;
use jianartifact_server::storage::BlobStore;
use serde_json::{Value, json};
use sha1::Sha1;
use sha2::{Digest, Sha512};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tower::ServiceExt;

const JWT_SECRET: &str = "npm-api-test-jwt-secret";
const TARBALL: &[u8] = b"npm-test-tarball";

struct TestContext {
    app: Router,
    pool: SqlitePool,
    state: AppState,
    _dir: TempDir,
}

struct TestResponse {
    status: StatusCode,
    body: Vec<u8>,
}

struct Upstream {
    base_url: String,
    hits: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
struct FixtureUpstreamState {
    hits: Arc<AtomicUsize>,
    packument: Value,
    tarballs: Arc<HashMap<String, Vec<u8>>>,
}

#[derive(Clone, Copy)]
enum PackumentStreamMode {
    DeclaredOversized,
    ChunkedOversized,
    Idle,
}

#[derive(Clone)]
struct PackumentStreamState {
    hits: Arc<AtomicUsize>,
    mode: PackumentStreamMode,
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn setup(ttl_secs: u64) -> TestContext {
    setup_with_publish_limit(ttl_secs, 1024 * 1024).await
}

async fn setup_with_publish_limit(ttl_secs: u64, publish_max_bytes: u64) -> TestContext {
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
        migration_workdir: Some(dir.path().join("migration")),
        migration_object_concurrency: 4,
        log_level: "warn".to_string(),
        jwt_secret: JWT_SECRET.to_string(),
        session_ttl_hours: 8,
        public_base_url: None,
        maven_metadata_ttl_secs: 1800,
        npm_packument_ttl_secs: ttl_secs,
        npm_publish_max_bytes: publish_max_bytes,
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
        app: router(state.clone()),
        pool,
        state,
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
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "registry.test");
    if let Some(auth) = auth {
        builder = builder.header(header::AUTHORIZATION, auth);
    }
    let response = app
        .clone()
        .oneshot(builder.body(body.into()).unwrap())
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

async fn login(app: &Router) -> String {
    let request = Request::post("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({"username": "admin", "password": "admin-pw"})).unwrap(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&body).unwrap();
    format!("Bearer {}", value["token"].as_str().unwrap())
}

async fn insert_repository(
    pool: &SqlitePool,
    name: &str,
    repo_type: &str,
    upstream_url: Option<&str>,
) -> i64 {
    sqlx::query_as::<_, (i64,)>(
        "INSERT INTO repositories (name, format, repo_type, visibility, overwrite_policy, upstream_url) VALUES (?, 'npm', ?, 'public', ?, ?) RETURNING id",
    )
    .bind(name)
    .bind(repo_type)
    .bind((repo_type == "hosted").then_some("deny"))
    .bind(upstream_url)
    .fetch_one(pool)
    .await
    .unwrap()
    .0
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

fn digest_hex<D: Digest + Default>(bytes: &[u8]) -> String {
    let digest = D::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn publish_body(name: &str, version: &str, tarball: &[u8]) -> Vec<u8> {
    let filename = format!("{}-{version}.tgz", name.rsplit('/').next().unwrap());
    serde_json::to_vec(&json!({
        "name": name,
        "dist-tags": {"latest": version},
        "versions": {
            version: {
                "name": name,
                "version": version,
                "description": "测试包",
                "dist": {
                    "tarball": format!("http://source.invalid/{filename}"),
                    "integrity": format!("sha512-{}", base64(&Sha512::digest(tarball))),
                    "shasum": digest_hex::<Sha1>(tarball)
                }
            }
        },
        "_attachments": {
            filename: {
                "content_type": "application/octet-stream",
                "data": base64(tarball),
                "length": tarball.len()
            }
        }
    }))
    .unwrap()
}

fn json_body(response: &TestResponse) -> Value {
    serde_json::from_slice(&response.body).unwrap()
}

#[tokio::test]
async fn hosted无content_length流式超限立即返回413() {
    let context = setup_with_publish_limit(1800, 8).await;
    let admin = login(&context.app).await;
    insert_repository(&context.pool, "npm-limited", "hosted", None).await;
    let (mut sender, receiver) = tokio::io::duplex(32);
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(receiver));
    let app = context.app.clone();
    let request = tokio::spawn(async move {
        send(
            &app,
            "PUT",
            "/repository/npm-limited/demo",
            Some(&admin),
            body,
        )
        .await
    });
    sender.write_all(b"123456789").await.unwrap();

    let response = tokio::time::timeout(Duration::from_secs(1), request)
        .await
        .expect("服务端应在流结束前拒绝超限请求")
        .unwrap();
    assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json_body(&response)["code"], "NPM_PUBLISH_TOO_LARGE");
}

#[tokio::test]
async fn hosted发布权威packument下载与不可变版本() {
    let context = setup(1800).await;
    let admin = login(&context.app).await;
    insert_repository(&context.pool, "npm-hosted", "hosted", None).await;

    let publish = send(
        &context.app,
        "PUT",
        "/repository/npm-hosted/demo",
        Some(&admin),
        publish_body("demo", "1.0.0", TARBALL),
    )
    .await;
    assert_eq!(publish.status, StatusCode::CREATED);

    let packument = send(
        &context.app,
        "GET",
        "/repository/npm-hosted/demo",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(packument.status, StatusCode::OK);
    let packument_json = json_body(&packument);
    assert_eq!(packument_json["name"], "demo");
    assert_eq!(packument_json["dist-tags"]["latest"], "1.0.0");
    assert_eq!(
        packument_json["versions"]["1.0.0"]["dist"]["tarball"],
        "http://registry.test/repository/npm-hosted/demo/-/demo-1.0.0.tgz"
    );

    let tarball = send(
        &context.app,
        "GET",
        "/repository/npm-hosted/demo/-/demo-1.0.0.tgz",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(tarball.status, StatusCode::OK);
    assert_eq!(tarball.body, TARBALL);

    let repeated = send(
        &context.app,
        "PUT",
        "/repository/npm-hosted/demo",
        Some(&admin),
        publish_body("demo", "1.0.0", TARBALL),
    )
    .await;
    assert_eq!(repeated.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn hosted严格摘要_scoped包与dist_tag增删() {
    let context = setup(1800).await;
    let admin = login(&context.app).await;
    insert_repository(&context.pool, "npm-scoped", "hosted", None).await;

    let mut damaged: Value =
        serde_json::from_slice(&publish_body("bad", "1.0.0", TARBALL)).unwrap();
    damaged["versions"]["1.0.0"]["dist"]["shasum"] = Value::String("0".repeat(40));
    let rejected = send(
        &context.app,
        "PUT",
        "/repository/npm-scoped/bad",
        Some(&admin),
        serde_json::to_vec(&damaged).unwrap(),
    )
    .await;
    assert_eq!(rejected.status, StatusCode::BAD_REQUEST);

    let publish = send(
        &context.app,
        "PUT",
        "/repository/npm-scoped/@scope%2fpkg",
        Some(&admin),
        publish_body("@scope/pkg", "2.0.0", TARBALL),
    )
    .await;
    assert_eq!(publish.status, StatusCode::CREATED);

    let put_tag = send(
        &context.app,
        "PUT",
        "/repository/npm-scoped/-/package/@scope%2fpkg/dist-tags/stable",
        Some(&admin),
        Body::from("\"2.0.0\""),
    )
    .await;
    assert_eq!(put_tag.status, StatusCode::OK);
    let tags = send(
        &context.app,
        "GET",
        "/repository/npm-scoped/-/package/@scope%2fpkg/dist-tags",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(json_body(&tags)["stable"], "2.0.0");

    let deleted = send(
        &context.app,
        "DELETE",
        "/repository/npm-scoped/-/package/@scope%2fpkg/dist-tags/stable",
        Some(&admin),
        Body::empty(),
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    let latest = send(
        &context.app,
        "DELETE",
        "/repository/npm-scoped/-/package/@scope%2fpkg/dist-tags/latest",
        Some(&admin),
        Body::empty(),
    )
    .await;
    assert_eq!(latest.status, StatusCode::BAD_REQUEST);
}

async fn upstream_handler(
    State(hits): State<Arc<AtomicUsize>>,
    Path(path): Path<String>,
) -> Response {
    hits.fetch_add(1, Ordering::SeqCst);
    match path.as_str() {
        "base/demo" => axum::Json(json!({
            "name": "demo",
            "dist-tags": {"latest": "1.0.0"},
            "versions": {"1.0.0": {
                "name": "demo",
                "version": "1.0.0",
                "dist": {
                    "tarball": "PLACEHOLDER",
                    "integrity": format!("sha512-{}", base64(&Sha512::digest(TARBALL))),
                    "shasum": digest_hex::<Sha1>(TARBALL)
                }
            }}
        }))
        .into_response(),
        "base/demo/-/demo-1.0.0.tgz" => (StatusCode::OK, TARBALL).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn fixture_upstream_handler(
    State(state): State<FixtureUpstreamState>,
    Path(path): Path<String>,
) -> Response {
    state.hits.fetch_add(1, Ordering::SeqCst);
    if path == "base/demo" {
        return axum::Json(state.packument).into_response();
    }
    state
        .tarballs
        .get(&path)
        .map(|bytes| (StatusCode::OK, bytes.clone()).into_response())
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

async fn start_fixture_upstream(
    packument: impl FnOnce(&str) -> Value,
    tarballs: HashMap<String, Vec<u8>>,
) -> Upstream {
    let hits = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let origin = format!("http://{address}");
    let state = FixtureUpstreamState {
        hits: Arc::clone(&hits),
        packument: packument(&origin),
        tarballs: Arc::new(tarballs),
    };
    let app = Router::new()
        .route("/{*path}", get(fixture_upstream_handler))
        .with_state(state);
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Upstream {
        base_url: format!("{origin}/base"),
        hits,
        task,
    }
}

async fn write_prefix_then_wait(mut sender: tokio::io::DuplexStream, mut remaining: usize) {
    let chunk = vec![b' '; 64 * 1024];
    while remaining > 0 {
        let length = remaining.min(chunk.len());
        if sender.write_all(&chunk[..length]).await.is_err() {
            return;
        }
        remaining -= length;
    }
    let _sender = sender;
    std::future::pending::<()>().await;
}

fn pending_packument_body(prefix_bytes: usize) -> Body {
    let (sender, receiver) = tokio::io::duplex(64 * 1024);
    tokio::spawn(write_prefix_then_wait(sender, prefix_bytes));
    Body::from_stream(tokio_util::io::ReaderStream::new(receiver))
}

async fn packument_stream_handler(
    State(state): State<PackumentStreamState>,
    Path(path): Path<String>,
) -> Response {
    const OVERSIZED_BYTES: usize = 16 * 1024 * 1024 + 1;
    state.hits.fetch_add(1, Ordering::SeqCst);
    if path != "base/demo" {
        return StatusCode::NOT_FOUND.into_response();
    }
    let (body, content_length) = match state.mode {
        PackumentStreamMode::DeclaredOversized => {
            (pending_packument_body(0), Some(OVERSIZED_BYTES))
        }
        PackumentStreamMode::ChunkedOversized => (pending_packument_body(OVERSIZED_BYTES), None),
        PackumentStreamMode::Idle => (pending_packument_body(0), None),
    };
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(length) = content_length {
        response = response.header(header::CONTENT_LENGTH, length);
    }
    response.body(body).unwrap()
}

async fn start_packument_stream_upstream(mode: PackumentStreamMode) -> Upstream {
    let hits = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = PackumentStreamState {
        hits: Arc::clone(&hits),
        mode,
    };
    let app = Router::new()
        .route("/{*path}", get(packument_stream_handler))
        .with_state(state);
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Upstream {
        base_url: format!("http://{address}/base"),
        hits,
        task,
    }
}

async fn assert_proxy_packument_too_large(mode: PackumentStreamMode) {
    let context = setup(1800).await;
    let upstream = start_packument_stream_upstream(mode).await;
    insert_repository(
        &context.pool,
        "npm-oversized",
        "proxy",
        Some(&upstream.base_url),
    )
    .await;

    let response = tokio::time::timeout(
        Duration::from_secs(2),
        send(
            &context.app,
            "GET",
            "/repository/npm-oversized/demo",
            None,
            Body::empty(),
        ),
    )
    .await
    .expect("服务端应在上游流结束前立即拒绝超限 packument");
    assert_eq!(response.status, StatusCode::BAD_GATEWAY);
    assert_eq!(json_body(&response)["code"], "PROXY_UPSTREAM_FAILURE");
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 1);
}

async fn start_upstream() -> Upstream {
    let hits = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let tarball_url = format!("http://{address}/base/demo/-/demo-1.0.0.tgz");
    let state_hits = Arc::clone(&hits);
    let app = Router::new()
        .route(
            "/{*path}",
            get(move |state: State<Arc<AtomicUsize>>, path: Path<String>| {
                let tarball_url = tarball_url.clone();
                async move {
                    let mut response = upstream_handler(state, path).await;
                    if response.status() == StatusCode::OK {
                        let (parts, body) = response.into_parts();
                        let bytes = body.collect().await.unwrap().to_bytes();
                        if let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) {
                            value["versions"]["1.0.0"]["dist"]["tarball"] =
                                Value::String(tarball_url);
                            response = Response::from_parts(
                                parts,
                                Body::from(serde_json::to_vec(&value).unwrap()),
                            );
                        } else {
                            response = Response::from_parts(parts, Body::from(bytes));
                        }
                    }
                    response
                }
            }),
        )
        .with_state(state_hits);
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Upstream {
        base_url: format!("http://{address}/base"),
        hits,
        task,
    }
}

#[tokio::test]
async fn proxy_packument声明长度超过十六mib时读取body前拒绝() {
    assert_proxy_packument_too_large(PackumentStreamMode::DeclaredOversized).await;
}

#[tokio::test]
async fn proxy_packument分块累计超限后不等待流结束() {
    assert_proxy_packument_too_large(PackumentStreamMode::ChunkedOversized).await;
}

#[tokio::test]
async fn proxy_packument空闲超时映射且失败不写缓存() {
    let context = setup(1800).await;
    let upstream = start_packument_stream_upstream(PackumentStreamMode::Idle).await;
    let repo_id = insert_repository(
        &context.pool,
        "npm-timeout",
        "proxy",
        Some(&upstream.base_url),
    )
    .await;
    let proxy = repository::protocol_repository(
        &context.pool,
        Subject::Anonymous,
        "npm-timeout",
        Permission::Read,
    )
    .await
    .unwrap();
    let route = ArtifactRoute {
        repository: "npm-timeout".to_string(),
        path: "demo".to_string(),
        trailing_slash: false,
    };
    let uri = Uri::from_static("/repository/npm-timeout/demo");
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, "registry.test".parse().unwrap());

    let error = tokio::time::timeout(
        Duration::from_secs(2),
        npm::fetch_proxy_packument_with_idle_timeout(
            &context.state,
            &proxy,
            &route,
            &uri,
            &headers,
            "demo",
            Duration::from_millis(20),
        ),
    )
    .await
    .expect("测试注入的短空闲超时应及时生效")
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(error.code(), "PROXY_UPSTREAM_TIMEOUT");
    assert!(
        context
            .state
            .artifacts
            .npm_proxy_packument(repo_id, "demo")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn proxy改写地址_tarball三十二并发单飞且断源后缓存可用() {
    let context = setup(1800).await;
    let upstream = start_upstream().await;
    insert_repository(
        &context.pool,
        "npm-proxy",
        "proxy",
        Some(&upstream.base_url),
    )
    .await;

    let packument = send(
        &context.app,
        "GET",
        "/repository/npm-proxy/demo",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(packument.status, StatusCode::OK);
    let body_text = String::from_utf8(packument.body.clone()).unwrap();
    assert!(!body_text.contains(&upstream.base_url));
    let tarball_url = json_body(&packument)["versions"]["1.0.0"]["dist"]["tarball"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(tarball_url.starts_with("http://registry.test/repository/npm-proxy/demo/-/"));
    let tarball_path = reqwest::Url::parse(&tarball_url)
        .unwrap()
        .path()
        .to_string();

    let mut requests = Vec::new();
    for _ in 0..32 {
        let app = context.app.clone();
        let path = tarball_path.clone();
        requests.push(tokio::spawn(async move {
            send(&app, "GET", &path, None, Body::empty()).await
        }));
    }
    for request in requests {
        let response = request.await.unwrap();
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, TARBALL);
    }
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 2);
    drop(upstream);

    let cached = send(&context.app, "GET", &tarball_path, None, Body::empty()).await;
    assert_eq!(cached.status, StatusCode::OK);
    assert_eq!(cached.body, TARBALL);
}

#[tokio::test]
async fn proxy两版本同名tarball使用不同本地地址并正确缓存() {
    let first_tarball = b"first-version-tarball".to_vec();
    let second_tarball = b"second-version-tarball".to_vec();
    let tarballs = HashMap::from([
        ("base/v1/shared.tgz".to_string(), first_tarball.clone()),
        ("base/v2/shared.tgz".to_string(), second_tarball.clone()),
    ]);
    let first_digest = first_tarball.clone();
    let second_digest = second_tarball.clone();
    let upstream = start_fixture_upstream(
        move |origin| {
            json!({
                "name": "demo",
                "dist-tags": {"latest": "2.0.0"},
                "versions": {
                    "1.0.0": {
                        "name": "demo",
                        "version": "1.0.0",
                        "dist": {
                            "tarball": format!("{origin}/base/v1/shared.tgz"),
                            "integrity": format!("sha512-{}", base64(&Sha512::digest(&first_digest))),
                            "shasum": digest_hex::<Sha1>(&first_digest)
                        }
                    },
                    "2.0.0": {
                        "name": "demo",
                        "version": "2.0.0",
                        "dist": {
                            "tarball": format!("{origin}/base/v2/shared.tgz"),
                            "integrity": format!("sha512-{}", base64(&Sha512::digest(&second_digest))),
                            "shasum": digest_hex::<Sha1>(&second_digest)
                        }
                    }
                }
            })
        },
        tarballs,
    )
    .await;
    let context = setup(1800).await;
    insert_repository(
        &context.pool,
        "npm-collision",
        "proxy",
        Some(&upstream.base_url),
    )
    .await;

    let packument = send(
        &context.app,
        "GET",
        "/repository/npm-collision/demo",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(packument.status, StatusCode::OK);
    let value = json_body(&packument);
    let first_url = value["versions"]["1.0.0"]["dist"]["tarball"]
        .as_str()
        .unwrap();
    let second_url = value["versions"]["2.0.0"]["dist"]["tarball"]
        .as_str()
        .unwrap();
    assert_ne!(first_url, second_url);

    for (url, expected) in [(first_url, first_tarball), (second_url, second_tarball)] {
        let path = reqwest::Url::parse(url).unwrap().path().to_string();
        let response = send(&context.app, "GET", &path, None, Body::empty()).await;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, expected);
    }
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn proxy拒绝含userinfo的tarball地址并稳定返回502() {
    let upstream = start_fixture_upstream(
        |origin| {
            let address = origin.strip_prefix("http://").unwrap();
            json!({
                "name": "demo",
                "dist-tags": {"latest": "1.0.0"},
                "versions": {
                    "1.0.0": {
                        "name": "demo",
                        "version": "1.0.0",
                        "dist": {
                            "tarball": format!("http://user:secret@{address}/base/demo.tgz"),
                            "integrity": format!("sha512-{}", base64(&Sha512::digest(TARBALL))),
                            "shasum": digest_hex::<Sha1>(TARBALL)
                        }
                    }
                }
            })
        },
        HashMap::new(),
    )
    .await;
    let context = setup(1800).await;
    insert_repository(
        &context.pool,
        "npm-userinfo",
        "proxy",
        Some(&upstream.base_url),
    )
    .await;

    let response = send(
        &context.app,
        "GET",
        "/repository/npm-userinfo/demo",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(response.status, StatusCode::BAD_GATEWAY);
    assert_eq!(json_body(&response)["code"], "PROXY_UPSTREAM_FAILURE");
}

#[tokio::test]
async fn proxy过期刷新失败沿用重写旧缓存且所有写入为405() {
    let context = setup(0).await;
    let upstream = start_upstream().await;
    insert_repository(
        &context.pool,
        "npm-stale",
        "proxy",
        Some(&upstream.base_url),
    )
    .await;
    let first = send(
        &context.app,
        "GET",
        "/repository/npm-stale/demo",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(first.status, StatusCode::OK);
    drop(upstream);

    let stale = send(
        &context.app,
        "GET",
        "/repository/npm-stale/demo",
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(stale.status, StatusCode::OK);
    assert!(!String::from_utf8(stale.body).unwrap().contains("127.0.0.1"));

    for (method, path, body) in [
        ("PUT", "/repository/npm-stale/demo", Body::empty()),
        (
            "PUT",
            "/repository/npm-stale/-/package/demo/dist-tags/latest",
            Body::from("\"1.0.0\""),
        ),
        (
            "DELETE",
            "/repository/npm-stale/-/package/demo/dist-tags/latest",
            Body::empty(),
        ),
    ] {
        assert_eq!(
            send(&context.app, method, path, None, body).await.status,
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
}
