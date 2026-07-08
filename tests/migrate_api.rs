//! Nexus OSS 迁移入口端点的 HTTP 集成测试（FR-36 / FR-37 / FR-39，ADR-0006）。
//!
//! 重点覆盖鉴权边界（匿名 401 / 非管理员 403 / 管理员放行）与错误映射
//! （在线：凭据引用缺失 400、连接源系统失败 502；离线：路径不存在 / 非法 400；
//! hosted 搬运：`offline_path` 为空 400）。响应 / 元数据解析与搬运编排（建仓 / blob 先落盘
//! 再写索引 / 覆盖语义 / 幂等）的正确性由 `migrate` 模块单测穷举覆盖；本文件不依赖真实 Nexus 实例。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use jianartifact::api::{build_router, AppState};
use jianartifact::auth::{self, JwtSigner, LoginGuard};
use jianartifact::config::Config;
use jianartifact::format::{ArtifactService, DockerRegistry, FormatRegistry};
use jianartifact::meta::{MetaStore, Role};
use jianartifact::proxy::HttpUpstream;
use jianartifact::storage::{BlobBackend, LocalFsStore};

/// 构造测试用状态（内存库 + 临时 blob 目录）。
async fn 测试用状态() -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let meta = MetaStore::open(&dir.path().join("test.db")).await.unwrap();
    let store = BlobBackend::Fs(LocalFsStore::new(dir.path().join("blobs")).await.unwrap());
    let jwt = JwtSigner::from_secret(b"migrate-secret-32-bytes-xxxxxxxxx", 3600);
    let upstream = HttpUpstream::new(std::time::Duration::from_secs(60)).unwrap();
    let artifacts = Arc::new(ArtifactService::new(store.clone(), meta.clone(), upstream));
    let docker = Arc::new(
        DockerRegistry::new(
            store.clone(),
            meta.clone(),
            dir.path().join("uploads"),
            None,
        )
        .await
        .unwrap(),
    );
    let (audit, audit_rx) = jianartifact::api::audit_channel();
    jianartifact::api::spawn_audit_writer(meta.clone(), audit_rx);
    // 使用分析采集：建有界 channel 并启动写入任务（关明细），使路由真实走采集链路
    let (usage, usage_rx) = jianartifact::api::usage_channel();
    jianartifact::api::spawn_usage_writer(meta.clone(), usage_rx, false);
    let state = AppState {
        config: Arc::new(Config::default()),
        meta,
        store,
        jwt,
        login_guard: Arc::new(LoginGuard::new(50, 900)),
        artifacts,
        formats: Arc::new(FormatRegistry::with_builtin()),
        docker: Some(docker),
        audit,
        usage,
        metrics: None,
        rate_limiter: Arc::new(jianartifact::api::RateLimiter::new()),
        oidc: None,
        oidc_flows: std::sync::Arc::new(jianartifact::api::OidcFlowStore::new()),
        ldap: None,
        // FR-53：测试默认名单空、封禁登记表空（异常检测默认关闭）
        protection: std::sync::Arc::new(jianartifact::api::ProtectionState::new(
            jianartifact::config::ProtectionConfig::default(),
        )),
        ban_registry: std::sync::Arc::new(jianartifact::api::BanRegistry::new()),
        // FR-54：测试默认 CC 挑战关闭；挑战器用固定密钥
        cc_challenger: std::sync::Arc::new(jianartifact::api::CcChallenger::new(
            b"test-secret-32-bytes-xxxxxxxxxxxx",
        )),
        // FR-56：防护告警默认关闭，引擎与投递端就绪（关闭时 record 直接返回）
        alerts: jianartifact::api::alert_channel().0,
        alert_engine: std::sync::Arc::new(jianartifact::api::AlertEngine::new(
            jianartifact::api::alert_channel().0,
        )),
        restart: std::sync::Arc::new(jianartifact::update::RestartHandle::default()),
        settings: std::sync::Arc::new(
            jianartifact::config::EditableSettings::new(
                jianartifact::config::NetworkProxyConfig::default(),
                std::time::Duration::from_secs(60),
                &jianartifact::config::UpdateConfig::default(),
            )
            .unwrap(),
        ),
        host_system: std::sync::Arc::new(tokio::sync::Mutex::new(sysinfo::System::new())),
        tasks: std::sync::Arc::new(jianartifact::api::TaskRegistry::default()),
    };
    (state, dir)
}

/// 发送请求并返回 (状态码, JSON 体)。
async fn send(router: Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

/// 构造带 JSON 体的在线迁移预览请求。
fn preview_req(auth: Option<&str>, body: Value) -> Request<Body> {
    json_post("/api/v1/migrate/nexus/preview", auth, body)
}

/// 构造带 JSON 体的离线 blob store 预览请求。
fn offline_req(auth: Option<&str>, body: Value) -> Request<Body> {
    json_post("/api/v1/migrate/nexus/offline/preview", auth, body)
}

/// 构造带 JSON 体的离线仓库索引双通道请求（FR-141）。
fn offline_index_req(auth: Option<&str>, body: Value) -> Request<Body> {
    json_post("/api/v1/migrate/nexus/offline/index", auth, body)
}

/// 构造带 JSON 体的 proxy 仓库搬运请求。
fn proxy_migrate_req(auth: Option<&str>, body: Value) -> Request<Body> {
    json_post("/api/v1/migrate/nexus/proxy/migrate", auth, body)
}

/// 构造带 JSON 体的 hosted 仓库搬运请求。
fn hosted_migrate_req(auth: Option<&str>, body: Value) -> Request<Body> {
    json_post("/api/v1/migrate/nexus/hosted/migrate", auth, body)
}

/// 构造带 JSON 体的在线拉取迁移请求（FR-82）。
fn online_migrate_req(auth: Option<&str>, body: Value) -> Request<Body> {
    json_post("/api/v1/migrate/nexus/online/migrate", auth, body)
}

/// 构造带可选认证头的 JSON POST 请求。
fn json_post(uri: &str, auth: Option<&str>, body: Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(a) = auth {
        builder = builder.header("authorization", a);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

/// 在临时目录下铺一个最小可用的 Nexus 文件型 blob store 布局，返回根目录临时句柄。
fn build_offline_store() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let chap = dir.path().join("content").join("vol-01").join("chap-01");
    std::fs::create_dir_all(&chap).unwrap();
    std::fs::write(
        chap.join("blob-1.properties"),
        "@Bucket.repo-name=maven-releases\n\
         @BlobStore.blob-name=/org/example/app/1.0/app-1.0.jar\n\
         sha1=a1b2c3\nsize=1024\ndeleted=false\n",
    )
    .unwrap();
    dir
}

/// 建用户并登录取回 JWT。
async fn seed_and_login(state: &AppState, username: &str, password: &str, role: Role) -> String {
    let hash = auth::hash_password(password).unwrap();
    state.meta.create_user(username, &hash, role).await.unwrap();
    let (status, body) = send(
        build_router(state.clone()),
        Request::builder()
            .method("POST")
            .uri("/api/v1/auth/login")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "username": username, "password": password }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "登录应成功: {body}");
    body["access_token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn 匿名预览返回_401() {
    let (state, _dir) = 测试用状态().await;
    let (status, _) = send(
        build_router(state),
        preview_req(None, json!({ "base_url": "https://nexus.example" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn 普通用户预览返回_403() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "user", "pw", Role::User).await;
    let (status, _) = send(
        build_router(state),
        preview_req(
            Some(&format!("Bearer {token}")),
            json!({ "base_url": "https://nexus.example" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn 管理员带未配置凭据引用返回_400() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    // auth_ref 指向一个不可能存在的环境变量名，应在调用源系统前以 400 短路
    let (status, body) = send(
        build_router(state),
        preview_req(
            Some(&format!("Bearer {token}")),
            json!({
                "base_url": "https://nexus.example",
                "auth_ref": "this_ref_is_not_configured_in_env_xyz"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}

#[tokio::test]
async fn 管理员连接不可达源系统返回_502() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    // 指向保留测试域名（不可达），连接失败应映射为 502 上游网关错误
    let (status, body) = send(
        build_router(state),
        preview_req(
            Some(&format!("Bearer {token}")),
            json!({ "base_url": "https://nexus.invalid.test.localhost.example" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "body: {body}");
}

#[tokio::test]
async fn 离线预览匿名返回_401() {
    let (state, _dir) = 测试用状态().await;
    let (status, _) = send(
        build_router(state),
        offline_req(None, json!({ "path": "/whatever" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn 离线预览普通用户返回_403() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "user", "pw", Role::User).await;
    let (status, _) = send(
        build_router(state),
        offline_req(
            Some(&format!("Bearer {token}")),
            json!({ "path": "/whatever" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn 离线预览异步_管理员枚举有效_blob_store() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    let store = build_offline_store();
    // 复用同一 router（克隆共享进程内 MigrationJobs 注册表），使提交与轮询命中同一 job
    let app = build_router(state);

    // 提交预览 → 立即返回 202 + job_id（FR-124 异步化，避免大库同步遍历在反代后 504）
    let (status, body) = send(
        app.clone(),
        offline_req(
            Some(&format!("Bearer {token}")),
            json!({ "path": store.path().to_str().unwrap() }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "body: {body}");
    let job_id = body["job_id"].as_str().expect("应返回 job_id").to_string();

    // 轮询任务进度直到完成
    let mut done = Value::Null;
    for _ in 0..100 {
        let (s, b) = send(
            app.clone(),
            get_req(
                &format!("/api/v1/migrate/jobs/{job_id}"),
                Some(&format!("Bearer {token}")),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "poll: {b}");
        // 进度字段在任务详情里展平到顶层（job_id 与 phase / offline_preview 同级）
        match b["phase"].as_str().unwrap_or("") {
            "done" | "failed" => {
                done = b;
                break;
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    assert_eq!(done["phase"], "done", "job: {done}");
    // 枚举结果经 offline_preview 字段随进度返回：maven-releases 仓库及其下 1 个 blob
    let repos = &done["offline_preview"];
    assert_eq!(repos[0]["repo_name"], "maven-releases");
    assert_eq!(repos[0]["blob_count"], 1);
    assert_eq!(
        repos[0]["blobs"][0]["blob_name"],
        "/org/example/app/1.0/app-1.0.jar"
    );
    assert_eq!(repos[0]["blobs"][0]["sha1"], "a1b2c3");
    assert_eq!(repos[0]["blobs"][0]["size"], 1024);
}

#[tokio::test]
async fn 离线预览路径不存在返回_400() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    let (status, body) = send(
        build_router(state),
        offline_req(
            Some(&format!("Bearer {token}")),
            json!({ "path": "D:/__此路径必定不存在的_blob_store__/x" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}

#[tokio::test]
async fn 离线预览空路径返回_400() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    let (status, body) = send(
        build_router(state),
        offline_req(Some(&format!("Bearer {token}")), json!({ "path": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}

// ---------- 离线仓库索引双通道端点（FR-141）鉴权与推断通道 ----------

#[tokio::test]
async fn 离线索引匿名返回_401() {
    let (state, _dir) = 测试用状态().await;
    let (status, _) = send(
        build_router(state),
        offline_index_req(None, json!({ "path": "/whatever" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn 离线索引普通用户返回_403() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "user", "pw", Role::User).await;
    let (status, _) = send(
        build_router(state),
        offline_index_req(
            Some(&format!("Bearer {token}")),
            json!({ "path": "/whatever" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn 离线索引空路径返回_400() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    let (status, body) = send(
        build_router(state),
        offline_index_req(Some(&format!("Bearer {token}")), json!({ "path": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}

#[tokio::test]
async fn 离线索引路径不存在返回_400() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    let (status, body) = send(
        build_router(state),
        offline_index_req(
            Some(&format!("Bearer {token}")),
            json!({ "path": "D:/__此路径必定不存在的_blob_store__/x" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}

#[tokio::test]
async fn 离线索引_管理员纯推断通道产出统一索引() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    let store = build_offline_store();
    // 复用同一 router（克隆共享进程内 MigrationJobs 注册表），使提交与轮询命中同一 job
    let app = build_router(state);

    // 未提供 base_url → 走纯离线推断通道；立即返回 202 + job_id
    let (status, body) = send(
        app.clone(),
        offline_index_req(
            Some(&format!("Bearer {token}")),
            json!({ "path": store.path().to_str().unwrap() }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "body: {body}");
    let job_id = body["job_id"].as_str().expect("应返回 job_id").to_string();

    // 轮询任务进度直到完成
    let mut done = Value::Null;
    for _ in 0..100 {
        let (s, b) = send(
            app.clone(),
            get_req(
                &format!("/api/v1/migrate/jobs/{job_id}"),
                Some(&format!("Bearer {token}")),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "poll: {b}");
        match b["phase"].as_str().unwrap_or("") {
            "done" | "failed" => {
                done = b;
                break;
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    assert_eq!(done["phase"], "done", "job: {done}");
    // 统一索引经 repo_index 字段随进度返回：maven-releases 仓库、格式推断 maven、来源推断、类型 / 上游留空
    let index = &done["repo_index"];
    assert_eq!(index[0]["name"], "maven-releases");
    assert_eq!(index[0]["format"], "maven");
    assert_eq!(index[0]["format_source"], "inferred");
    assert!(index[0]["type"].is_null(), "推断通道类型应留空: {done}");
    assert!(
        index[0]["upstream_url"].is_null(),
        "推断通道上游应留空: {done}"
    );
    assert_eq!(index[0]["blob_count"], 1);
    assert_eq!(index[0]["total_size"], 1024);
}

// ---------- hosted 仓库搬运端点（FR-39）鉴权与校验边界 ----------

#[tokio::test]
async fn hosted_搬运匿名返回_401() {
    let (state, _dir) = 测试用状态().await;
    let (status, _) = send(
        build_router(state),
        hosted_migrate_req(None, json!({ "offline_path": "/whatever" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn hosted_搬运普通用户返回_403() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "user", "pw", Role::User).await;
    let (status, _) = send(
        build_router(state),
        hosted_migrate_req(
            Some(&format!("Bearer {token}")),
            json!({ "offline_path": "/whatever" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn hosted_搬运管理员空_offline_path_返回_400() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    // offline_path 为空白应在启动后台搬运前以 400 短路
    let (status, body) = send(
        build_router(state),
        hosted_migrate_req(
            Some(&format!("Bearer {token}")),
            json!({ "offline_path": "   " }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}

#[tokio::test]
async fn 离线搬运带旧在线字段被拒绝() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    let auth = format!("Bearer {token}");
    let app = build_router(state);
    let body = json!({
        "base_url": "https://nexus.example",
        "auth_ref": "NEXUS_SRC",
        "offline_path": "/whatever",
        "repositories": ["maven-proxy"]
    });

    let (proxy_status, _) = send(app.clone(), proxy_migrate_req(Some(&auth), body.clone())).await;
    let (hosted_status, _) = send(app, hosted_migrate_req(Some(&auth), body)).await;

    for status in [proxy_status, hosted_status] {
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
            "旧在线字段应被拒绝，实际状态: {status}"
        );
    }
}

#[tokio::test]
async fn 匿名在线迁移返回_401() {
    let (state, _dir) = 测试用状态().await;
    let (status, _) = send(
        build_router(state),
        online_migrate_req(
            None,
            json!({ "base_url": "https://nexus.example", "repositories": [{ "source": "r3d" }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn 普通用户在线迁移返回_403() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "user", "pw", Role::User).await;
    let (status, _) = send(
        build_router(state),
        online_migrate_req(
            Some(&format!("Bearer {token}")),
            json!({ "base_url": "https://nexus.example", "repositories": [{ "source": "r3d" }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn 在线迁移管理员空仓库列表返回_400() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    // 未选择仓库应在调用源系统前以 400 短路
    let (status, body) = send(
        build_router(state),
        online_migrate_req(
            Some(&format!("Bearer {token}")),
            json!({ "base_url": "https://nexus.example", "repositories": [] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}

/// 构造带可选认证头的 GET 请求。
fn get_req(uri: &str, auth: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(a) = auth {
        builder = builder.header("authorization", a);
    }
    builder.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn 匿名查询迁移任务返回_401() {
    let (state, _dir) = 测试用状态().await;
    let (status, _) = send(
        build_router(state),
        get_req("/api/v1/migrate/jobs/any", None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn 普通用户查询迁移任务返回_403() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "user", "pw", Role::User).await;
    let (status, _) = send(
        build_router(state),
        get_req("/api/v1/migrate/jobs/any", Some(&format!("Bearer {token}"))),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn 管理员查询未知迁移任务返回_404() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    let (status, _) = send(
        build_router(state),
        get_req(
            "/api/v1/migrate/jobs/不存在",
            Some(&format!("Bearer {token}")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn 匿名列迁移任务返回_401() {
    let (state, _dir) = 测试用状态().await;
    let (status, _) = send(build_router(state), get_req("/api/v1/migrate/jobs", None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn 管理员列迁移任务空表返回_200() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    let (status, body) = send(
        build_router(state),
        get_req("/api/v1/migrate/jobs", Some(&format!("Bearer {token}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]));
}

// ---------- FR-91：任务控制端点（取消 / 暂停 / 继续）鉴权与 404 边界 ----------

/// 构造带可选认证头的无体 POST 请求（任务控制端点用）。
fn post_req(uri: &str, auth: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("POST").uri(uri);
    if let Some(a) = auth {
        builder = builder.header("authorization", a);
    }
    builder.body(Body::empty()).unwrap()
}

/// 三个控制端点的相对路径（针对同一未知 id），统一穷举鉴权与 404。
fn control_uris() -> [&'static str; 3] {
    [
        "/api/v1/migrate/jobs/任意/cancel",
        "/api/v1/migrate/jobs/任意/pause",
        "/api/v1/migrate/jobs/任意/resume",
    ]
}

#[tokio::test]
async fn 匿名控制迁移任务返回_401() {
    for uri in control_uris() {
        let (state, _dir) = 测试用状态().await;
        let (status, _) = send(build_router(state), post_req(uri, None)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "端点 {uri} 匿名应 401");
    }
}

#[tokio::test]
async fn 普通用户控制迁移任务返回_403() {
    for uri in control_uris() {
        let (state, _dir) = 测试用状态().await;
        let token = seed_and_login(&state, "user", "pw", Role::User).await;
        let (status, _) = send(
            build_router(state),
            post_req(uri, Some(&format!("Bearer {token}"))),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "端点 {uri} 普通用户应 403");
    }
}

#[tokio::test]
async fn 管理员控制未知迁移任务返回_404() {
    for uri in control_uris() {
        let (state, _dir) = 测试用状态().await;
        let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
        let (status, _) = send(
            build_router(state),
            post_req(uri, Some(&format!("Bearer {token}"))),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "端点 {uri} 未知 id 应 404");
    }
}

// ---------- 迁移任务与报告持久化（FR-140，ADR-0040）----------

/// 起一个绑定显式 DB 文件路径的状态（供「重启」重开同一 DB 断言跨重启持久）。
async fn 持久化测试用状态(db_path: &std::path::Path) -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let meta = MetaStore::open(db_path).await.unwrap();
    let store = BlobBackend::Fs(LocalFsStore::new(dir.path().join("blobs")).await.unwrap());
    let jwt = JwtSigner::from_secret(b"migrate-secret-32-bytes-xxxxxxxxx", 3600);
    let upstream = HttpUpstream::new(std::time::Duration::from_secs(60)).unwrap();
    let artifacts = Arc::new(ArtifactService::new(store.clone(), meta.clone(), upstream));
    let docker = Arc::new(
        DockerRegistry::new(
            store.clone(),
            meta.clone(),
            dir.path().join("uploads"),
            None,
        )
        .await
        .unwrap(),
    );
    let (audit, audit_rx) = jianartifact::api::audit_channel();
    jianartifact::api::spawn_audit_writer(meta.clone(), audit_rx);
    let (usage, usage_rx) = jianartifact::api::usage_channel();
    jianartifact::api::spawn_usage_writer(meta.clone(), usage_rx, false);
    let state = AppState {
        config: Arc::new(Config::default()),
        meta,
        store,
        jwt,
        login_guard: Arc::new(LoginGuard::new(50, 900)),
        artifacts,
        formats: Arc::new(FormatRegistry::with_builtin()),
        docker: Some(docker),
        audit,
        usage,
        metrics: None,
        rate_limiter: Arc::new(jianartifact::api::RateLimiter::new()),
        oidc: None,
        oidc_flows: std::sync::Arc::new(jianartifact::api::OidcFlowStore::new()),
        ldap: None,
        protection: std::sync::Arc::new(jianartifact::api::ProtectionState::new(
            jianartifact::config::ProtectionConfig::default(),
        )),
        ban_registry: std::sync::Arc::new(jianartifact::api::BanRegistry::new()),
        cc_challenger: std::sync::Arc::new(jianartifact::api::CcChallenger::new(
            b"test-secret-32-bytes-xxxxxxxxxxxx",
        )),
        alerts: jianartifact::api::alert_channel().0,
        alert_engine: std::sync::Arc::new(jianartifact::api::AlertEngine::new(
            jianartifact::api::alert_channel().0,
        )),
        restart: std::sync::Arc::new(jianartifact::update::RestartHandle::default()),
        settings: std::sync::Arc::new(
            jianartifact::config::EditableSettings::new(
                jianartifact::config::NetworkProxyConfig::default(),
                std::time::Duration::from_secs(60),
                &jianartifact::config::UpdateConfig::default(),
            )
            .unwrap(),
        ),
        host_system: std::sync::Arc::new(tokio::sync::Mutex::new(sysinfo::System::new())),
        tasks: std::sync::Arc::new(jianartifact::api::TaskRegistry::default()),
    };
    (state, dir)
}

/// 铺一个含**真实 blob 本体**（`.properties` + `.bytes` 成对）的最小离线 blob store，
/// 使 hosted 搬运能真正落地 1 个 maven 制品。blob 逻辑名为合法 maven 主构件路径。
fn build_offline_store_with_body() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let chap = dir.path().join("content").join("vol-01").join("chap-01");
    std::fs::create_dir_all(&chap).unwrap();
    let body = b"fake-jar-bytes-for-migration-test";
    std::fs::write(chap.join("blob-1.bytes"), body).unwrap();
    std::fs::write(
        chap.join("blob-1.properties"),
        format!(
            "@Bucket.repo-name=maven-releases\n\
             @BlobStore.blob-name=/org/example/app/1.0/app-1.0.jar\n\
             sha1=deadbeef\nsize={}\ndeleted=false\n",
            body.len()
        ),
    )
    .unwrap();
    dir
}

/// 轮询**库中**迁移任务直到终态落库（终态 + 报告经 finalize 写入后才可见）。
///
/// 注意：进程内 jobs 进度先于持久层终态写入变 done；本函数等的是**库中**终态（历史 / 报告已落定），
/// 才是断言持久化结果的正确同步点。
async fn 等待任务终态(app: &Router, token: &str, job_id: &str) {
    for _ in 0..300 {
        let (status, b) = send(
            app.clone(),
            get_req(
                &format!("/api/v1/migrate/tasks/{job_id}"),
                Some(&format!("Bearer {token}")),
            ),
        )
        .await;
        if status == StatusCode::OK {
            match b["state"].as_str().unwrap_or("") {
                "succeeded" | "failed" | "cancelled" | "interrupted" => return,
                _ => {}
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// 查某迁移历史任务详情 JSON。
async fn 查任务详情(app: &Router, token: &str, job_id: &str) -> Value {
    let (status, detail) = send(
        app.clone(),
        get_req(
            &format!("/api/v1/migrate/tasks/{job_id}"),
            Some(&format!("Bearer {token}")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "查详情应 200: {detail}");
    detail
}

#[tokio::test]
async fn 迁移历史匿名返回_401() {
    let (state, _dir) = 测试用状态().await;
    let (status, _) = send(build_router(state), get_req("/api/v1/migrate/tasks", None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn 迁移历史普通用户返回_403() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "user", "pw", Role::User).await;
    let (status, _) = send(
        build_router(state),
        get_req("/api/v1/migrate/tasks", Some(&format!("Bearer {token}"))),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn 迁移历史空表返回_200_空数组() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    let (status, body) = send(
        build_router(state),
        get_req("/api/v1/migrate/tasks", Some(&format!("Bearer {token}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn 迁移历史详情未知_id_返回_404() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    let (status, _) = send(
        build_router(state),
        get_req(
            "/api/v1/migrate/tasks/无此任务",
            Some(&format!("Bearer {token}")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn 离线_hosted_搬运完成后历史与分仓库报告可查() {
    // 端到端：起真实离线 hosted 搬运（内存库 + 临时 blob）→ 轮询至完成 → 查历史 + 详情报告
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    let offline = build_offline_store_with_body();
    let app = build_router(state.clone());

    let (status, body) = send(
        app.clone(),
        hosted_migrate_req(
            Some(&format!("Bearer {token}")),
            json!({ "offline_path": offline.path().to_str().unwrap() }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "body: {body}");
    let job_id = body["job_id"].as_str().unwrap().to_string();
    等待任务终态(&app, &token, &job_id).await;

    // 历史列表含该任务、终态 succeeded、有 finished_at
    let (status, list) = send(
        app.clone(),
        get_req("/api/v1/migrate/tasks", Some(&format!("Bearer {token}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let arr = list.as_array().unwrap();
    let hit = arr.iter().find(|t| t["id"] == job_id.as_str()).unwrap();
    assert_eq!(hit["kind"], "offline_hosted");
    assert_eq!(hit["state"], "succeeded", "任务应成功终态: {hit}");
    assert!(
        hit["finished_at"].as_str().is_some(),
        "终态应记 finished_at"
    );

    // 详情含分仓库报告
    let detail = 查任务详情(&app, &token, &job_id).await;
    let repos = detail["repos"].as_array().unwrap();
    assert_eq!(repos.len(), 1, "应有 1 个仓库报告: {detail}");
    assert_eq!(repos[0]["source_repo"], "maven-releases");
    assert_eq!(repos[0]["migrated_artifacts"], 1, "首次搬运新写 1 个制品");
    assert_eq!(repos[0]["skipped_existing_artifacts"], 0);
}

#[tokio::test]
async fn 重启后残留在途任务标已中断且历史报告可查() {
    // 模拟：进程 A 写入一个「运行中」任务 + 一个已完成任务（含报告）→ 退出 → 进程 B 用同一 DB 重开。
    // 断言：重开后启动中断标记把 running 归为 interrupted，已完成任务与其报告跨重启仍可查。
    use jianartifact::meta::{
        MigrationProgressSnapshot, MigrationRepoReport, MigrationTaskKind, MigrationTaskState,
        NewMigrationTask,
    };
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("persist.db");

    // 进程 A：经 meta 层写两个任务（一个 running 在途、一个 succeeded 带报告），随后 drop
    {
        let (state_a, _blob_a) = 持久化测试用状态(&db_path).await;
        state_a
            .meta
            .upsert_migration_task(&NewMigrationTask {
                id: "running-task".to_string(),
                kind: MigrationTaskKind::Online,
                label: Some("在线拉取迁移".to_string()),
                params_snapshot:
                    r#"{"kind":"online","base_url":"https://nx.example","auth_ref":"prod"}"#
                        .to_string(),
            })
            .await
            .unwrap();
        state_a
            .meta
            .upsert_migration_task(&NewMigrationTask {
                id: "done-task".to_string(),
                kind: MigrationTaskKind::OfflineHosted,
                label: None,
                params_snapshot: "{}".to_string(),
            })
            .await
            .unwrap();
        state_a
            .meta
            .finish_migration_task(
                "done-task",
                MigrationTaskState::Succeeded,
                &MigrationProgressSnapshot {
                    phase: "done".to_string(),
                    total_assets: 3,
                    done_assets: 3,
                    migrated: 2,
                    skipped_existing: 1,
                    skipped: 0,
                    total_bytes: 300,
                    done_bytes: 300,
                    migrated_bytes: 200,
                    skipped_bytes: 100,
                },
                &[MigrationRepoReport {
                    source_repo: "maven-releases".to_string(),
                    target_repo: "maven-releases".to_string(),
                    format: "maven".to_string(),
                    created: true,
                    migrated_artifacts: 2,
                    skipped_existing_artifacts: 1,
                    skipped_artifacts: 0,
                }],
                None,
            )
            .await
            .unwrap();
    }

    // 进程 B：同一 DB 重开 → 启动中断标记把残留 running/paused 归 interrupted
    let (state_b, _blob_b) = 持久化测试用状态(&db_path).await;
    let affected = state_b
        .meta
        .mark_running_migration_tasks_interrupted()
        .await
        .unwrap();
    assert_eq!(affected, 1, "只归一那个 running 在途任务");

    let token = seed_and_login(&state_b, "admin", "pw", Role::Admin).await;
    let app = build_router(state_b);

    // 历史列表：running 任务已标 interrupted、done 任务不受影响
    let (status, list) = send(
        app.clone(),
        get_req("/api/v1/migrate/tasks", Some(&format!("Bearer {token}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let arr = list.as_array().unwrap();
    let running = arr.iter().find(|t| t["id"] == "running-task").unwrap();
    assert_eq!(running["state"], "interrupted", "残留在途任务应标已中断");
    assert!(
        running["finished_at"].as_str().is_some(),
        "中断应记 finished_at"
    );
    let done = arr.iter().find(|t| t["id"] == "done-task").unwrap();
    assert_eq!(done["state"], "succeeded", "已完成任务不受重启影响");

    // 详情：重启后已完成任务的进度快照与分仓库报告仍可查
    let detail = 查任务详情(&app, &token, "done-task").await;
    assert_eq!(detail["migrated"], 2);
    assert_eq!(detail["skipped_existing"], 1);
    let repos = detail["repos"].as_array().unwrap();
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0]["migrated_artifacts"], 2);
    assert_eq!(repos[0]["skipped_existing_artifacts"], 1);
}

#[tokio::test]
async fn 已中断任务一键重跑幂等只补新增() {
    // 首跑离线 hosted 搬运（全新）→ 对该任务一键重跑 → 因 sha256 一致全部增量跳过、不再新写
    //（幂等续传：区分「跳过 vs 新搬」计数）。
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    let offline = build_offline_store_with_body();
    let app = build_router(state.clone());

    // 首跑：全新搬运，migrated=1
    let (_s, body) = send(
        app.clone(),
        hosted_migrate_req(
            Some(&format!("Bearer {token}")),
            json!({ "offline_path": offline.path().to_str().unwrap() }),
        ),
    )
    .await;
    let first_id = body["job_id"].as_str().unwrap().to_string();
    等待任务终态(&app, &token, &first_id).await;
    let first = 查任务详情(&app, &token, &first_id).await;
    assert_eq!(first["repos"][0]["migrated_artifacts"], 1, "首跑新写 1");

    // 一键重跑：据参数快照重触发；目标已存在且 sha256 一致 → 全部增量跳过、不新写
    let (status, rerun_body) = send(
        app.clone(),
        post_req(
            &format!("/api/v1/migrate/tasks/{first_id}/rerun"),
            Some(&format!("Bearer {token}")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "重跑应 202: {rerun_body}");
    let rerun_id = rerun_body["job_id"].as_str().unwrap().to_string();
    assert_ne!(rerun_id, first_id, "重跑生成新 job_id、不覆盖原历史");
    等待任务终态(&app, &token, &rerun_id).await;

    let rerun = 查任务详情(&app, &token, &rerun_id).await;
    assert_eq!(rerun["state"], "succeeded");
    // 幂等重跑：区分跳过 vs 新搬——重跑全部增量跳过、不新写
    assert_eq!(rerun["migrated"], 0, "重跑不应再新写（sha256 一致）");
    assert_eq!(rerun["skipped_existing"], 1, "重跑应全部增量跳过");
    assert_eq!(rerun["repos"][0]["migrated_artifacts"], 0);
    assert_eq!(rerun["repos"][0]["skipped_existing_artifacts"], 1);
}

#[tokio::test]
async fn 重跑未知任务返回_404() {
    let (state, _dir) = 测试用状态().await;
    let token = seed_and_login(&state, "admin", "pw", Role::Admin).await;
    let (status, _) = send(
        build_router(state),
        post_req(
            "/api/v1/migrate/tasks/无此任务/rerun",
            Some(&format!("Bearer {token}")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn 迁移参数快照不含凭据明文() {
    // 凭据红线（§2.6）：库中参数快照只存 auth_ref 引用名、不含明文密码（凭据真值只在 env）。
    let (state, _dir) = 测试用状态().await;
    use jianartifact::meta::{MigrationTaskKind, NewMigrationTask};
    state
        .meta
        .upsert_migration_task(&NewMigrationTask {
            id: "cred-task".to_string(),
            kind: MigrationTaskKind::Online,
            label: None,
            params_snapshot:
                r#"{"kind":"online","base_url":"https://nx","auth_ref":"prod","repositories":[]}"#
                    .to_string(),
        })
        .await
        .unwrap();
    let got = state
        .meta
        .get_migration_task("cred-task")
        .await
        .unwrap()
        .unwrap();
    assert!(got.params_snapshot.contains("auth_ref"), "应存引用名");
    assert!(got.params_snapshot.contains("prod"));
    // 断言不含任何凭据真值 / 明文密码字样
    assert!(!got.params_snapshot.to_lowercase().contains("password"));
    assert!(!got.params_snapshot.contains("secret"));
}
