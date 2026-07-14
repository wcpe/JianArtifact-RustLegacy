//! 阶段 1 契约骨架集成测试。
//! 核心防漂移手段：遍历 OpenAPI 文档的全部 operation 逐一真实请求——
//! 路由忘挂载报 404、方法写反报 405，两处登记（路由表 / utoipa 注解）任何漂移即红。

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use jianartifact_server::api::repos::UpdateRepoRequest;
use jianartifact_server::config::Config;
use jianartifact_server::error::StartupError;
use tower::util::ServiceExt;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// 在临时目录上构建一个完整应用（真实 SQLite 文件 + 迁移）。
async fn test_app() -> Result<(Router, tempfile::TempDir), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let config = Config {
        bind: "127.0.0.1:0".to_string(),
        database_path: dir.path().join("test.db"),
        data_dir: dir.path().join("blobs"),
        log_level: "warn".to_string(),
        // 测试用固定 JWT 密钥（非生产凭据），满足 build_app 的密钥非空校验
        jwt_secret: "test-jwt-secret-please-change".to_string(),
        session_ttl_hours: 8,
        max_artifact_bytes: 5 * 1024 * 1024 * 1024,
        min_free_bytes: 5 * 1024 * 1024 * 1024,
        min_free_percent: 5,
    };
    let app = jianartifact_server::build_app(config).await?;
    Ok((app, dir))
}

/// 向应用发送一次请求并返回响应。
async fn send(
    app: &Router,
    method: &str,
    uri: &str,
) -> Result<axum::response::Response, Box<dyn std::error::Error>> {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())?;
    Ok(app.clone().oneshot(request).await?)
}

#[tokio::test]
async fn 健康探针存活即二百() -> TestResult {
    let (app, _dir) = test_app().await?;
    let resp = send(&app, "GET", "/healthz").await?;
    assert_eq!(resp.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn 就绪自检依赖齐备时二百() -> TestResult {
    let (app, _dir) = test_app().await?;
    let resp = send(&app, "GET", "/readyz").await?;
    assert_eq!(resp.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn 启动创建数据目录且创建失败返回_io_错误() -> TestResult {
    let dir = tempfile::tempdir()?;
    let data_dir = dir.path().join("blobs");
    let config = Config {
        bind: "127.0.0.1:0".to_string(),
        database_path: dir.path().join("test.db"),
        data_dir: data_dir.clone(),
        log_level: "warn".to_string(),
        jwt_secret: "test-jwt-secret-please-change".to_string(),
        session_ttl_hours: 8,
        max_artifact_bytes: 5 * 1024 * 1024 * 1024,
        min_free_bytes: 5 * 1024 * 1024 * 1024,
        min_free_percent: 5,
    };
    let _app = jianartifact_server::build_app(config.clone()).await?;
    assert!(data_dir.is_dir(), "启动时应创建数据目录");

    let blocked_dir = dir.path().join("blocked");
    std::fs::write(&blocked_dir, b"x")?;
    let failed = jianartifact_server::build_app(Config {
        database_path: dir.path().join("blocked.db"),
        data_dir: blocked_dir,
        ..config
    })
    .await;
    assert!(matches!(failed, Err(StartupError::Io(_))));
    Ok(())
}

#[tokio::test]
async fn 就绪自检运行期目录丢失不重建且恢复后通过() -> TestResult {
    let (app, dir) = test_app().await?;
    let data_dir = dir.path().join("blobs");
    assert!(data_dir.is_dir(), "启动时应已创建数据目录");

    std::fs::remove_dir_all(&data_dir)?;
    let resp = send(&app, "GET", "/readyz").await?;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!data_dir.exists(), "运行期探针不得自动重建数据目录");
    assert_eq!(
        send(&app, "GET", "/healthz").await?.status(),
        StatusCode::OK,
        "存活探针不得受依赖故障影响"
    );

    std::fs::create_dir(&data_dir)?;
    assert_eq!(send(&app, "GET", "/readyz").await?.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn 并发就绪自检全部通过且随机探针无残留() -> TestResult {
    let (app, dir) = test_app().await?;
    let mut requests = Vec::new();
    for _ in 0..32 {
        let app = app.clone();
        let request = Request::get("/readyz").body(Body::empty())?;
        requests.push(tokio::spawn(async move { app.oneshot(request).await }));
    }
    for request in requests {
        assert_eq!(request.await??.status(), StatusCode::OK);
    }

    let blob_dir = dir.path().join("blobs");
    let entries = std::fs::read_dir(&blob_dir)?.collect::<Result<Vec<_>, _>>()?;
    assert_eq!(entries.len(), 1, "blob 根目录只应保留内部临时目录");
    assert_eq!(entries[0].file_name(), ".tmp");
    let temp_entries = std::fs::read_dir(blob_dir.join(".tmp"))?.collect::<Result<Vec<_>, _>>()?;
    assert_eq!(temp_entries.len(), 1, "临时目录只应保留守卫目录");
    assert_eq!(temp_entries[0].file_name(), "guards");
    assert!(
        std::fs::read_dir(blob_dir.join(".tmp/guards"))?
            .next()
            .is_none(),
        "就绪探针与提交守卫均不得残留"
    );
    Ok(())
}

/// 把 OpenAPI 路径中的参数占位替换为哑值（如 {id} → 1）。
fn fill_path_params(path: &str) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        match rest[start..].find('}') {
            Some(end_off) => {
                out.push('1');
                rest = &rest[start + end_off + 1..];
            }
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[tokio::test]
async fn 契约文档全部端点均已挂载且方法一致() -> TestResult {
    let (app, _dir) = test_app().await?;
    let resp = send(&app, "GET", "/api/v1/openapi.json").await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await?.to_bytes();
    let doc: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(doc["info"]["title"], "JianArtifact 管理 API");

    let paths = doc["paths"].as_object().ok_or("OpenAPI 缺少 paths")?;
    let mut operations = 0;
    for (path, methods) in paths {
        let uri = fill_path_params(path);
        let methods = methods.as_object().ok_or("路径下缺少方法对象")?;
        for method in methods.keys() {
            operations += 1;
            let resp = send(&app, &method.to_uppercase(), &uri).await?;
            let status = resp.status();
            // 方法不一致仍可直接由 405 判定；真实资源端点允许返回结构化业务 404。
            assert_ne!(
                status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path} 路由方法与契约不一致"
            );
            if status == StatusCode::NOT_FOUND {
                if method == "head" {
                    continue;
                }
                let body = resp.into_body().collect().await?.to_bytes();
                let json: serde_json::Value = serde_json::from_slice(&body)
                    .map_err(|_| format!("{method} {path} 返回裸 404，疑似路由未挂载"))?;
                assert!(
                    json["code"].as_str().is_some(),
                    "{method} {path} 返回无错误码的 404，疑似路由未挂载"
                );
                continue;
            }
            // 未实现端点必须带稳定错误码；HEAD 按 HTTP 语义不读取响应体。
            if status == StatusCode::NOT_IMPLEMENTED && method != "head" {
                let body = resp.into_body().collect().await?.to_bytes();
                let json: serde_json::Value = serde_json::from_slice(&body)?;
                assert_eq!(
                    json["code"], "NOT_IMPLEMENTED",
                    "{method} {path} 占位错误码不符"
                );
            }
        }
    }
    // 防空转：认证 + 仓库 + 迁移 + 健康的契约面至少应有这么多 operation
    assert!(
        operations >= 30,
        "OpenAPI operation 数量异常：仅 {operations} 个"
    );
    Ok(())
}

#[tokio::test]
async fn 仓库契约体现用户名_acl_显式清空_完整路径与格式端点() -> TestResult {
    let (app, _dir) = test_app().await?;
    let resp = send(&app, "GET", "/api/v1/openapi.json").await?;
    let body = resp.into_body().collect().await?.to_bytes();
    let doc: serde_json::Value = serde_json::from_slice(&body)?;

    assert!(doc["paths"]["/api/v1/repos/{name}/acl/{username}"]["put"].is_object());
    assert!(doc["paths"]["/api/v1/repos/{name}/acl/{username}"]["delete"].is_object());
    assert!(doc["paths"]["/api/v1/repos/{name}/acl/{user_id}"].is_null());

    for (path, method) in [
        ("/api/v1/repos/{name}", "patch"),
        ("/api/v1/repos/{name}", "delete"),
        ("/api/v1/repos/{name}/acl", "get"),
        ("/api/v1/repos/{name}/acl/{username}", "put"),
        ("/api/v1/repos/{name}/acl/{username}", "delete"),
    ] {
        assert!(
            doc["paths"][path][method]["responses"]["403"].is_object(),
            "{method} {path} 必须声明仓库 admin 权限不足的 403"
        );
    }
    for path in ["/api/v1/repos/{name}/files", "/api/v1/repos/{name}/search"] {
        assert!(
            doc["paths"][path]["get"]["responses"]["200"].is_object(),
            "get {path} 必须声明真实 200 响应"
        );
        assert!(doc["paths"][path]["get"]["responses"]["501"].is_null());
    }
    assert!(doc["paths"]["/api/v1/repos/{name}/usage"]["get"]["responses"]["501"].is_object());

    let update = &doc["components"]["schemas"]["UpdateRepoRequest"]["properties"];
    assert!(update["version_policy"].is_object());
    let credential_type = &update["upstream_credential_ref"]["type"];
    assert!(
        credential_type
            .as_array()
            .is_some_and(|types| types.contains(&serde_json::json!("null")))
    );

    let file_required = doc["components"]["schemas"]["FileEntry"]["required"]
        .as_array()
        .ok_or("FileEntry.required 缺失")?;
    assert!(file_required.contains(&serde_json::json!("path")));

    for method in ["get", "head", "put", "delete"] {
        assert!(
            doc["paths"]["/repository/{name}/{path}"][method].is_object(),
            "格式端点缺少 {method} 契约"
        );
        let response = send(&app, &method.to_uppercase(), "/repository/demo/file.bin").await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    Ok(())
}

#[test]
fn 仓库_patch_区分凭据引用缺失与显式_null() -> TestResult {
    let missing: UpdateRepoRequest = serde_json::from_value(serde_json::json!({}))?;
    let cleared: UpdateRepoRequest =
        serde_json::from_value(serde_json::json!({ "upstream_credential_ref": null }))?;
    assert_eq!(format!("{:?}", missing.upstream_credential_ref), "None");
    assert_eq!(
        format!("{:?}", cleared.upstream_credential_ref),
        "Some(None)"
    );
    Ok(())
}

#[tokio::test]
async fn 响应头带请求标识() -> TestResult {
    let (app, _dir) = test_app().await?;
    let resp = send(&app, "GET", "/healthz").await?;
    assert!(
        resp.headers().contains_key("x-request-id"),
        "响应应携带 x-request-id"
    );
    Ok(())
}

#[tokio::test]
async fn 迁移建立认证三表() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("test.db");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = jianartifact_server::persistence::init(&url, &db_path).await?;
    for table in ["users", "api_tokens", "revoked_sessions"] {
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?")
                .bind(table)
                .fetch_one(&pool)
                .await?;
        assert_eq!(row.0, 1, "迁移后应存在表 {table}");
    }
    Ok(())
}
