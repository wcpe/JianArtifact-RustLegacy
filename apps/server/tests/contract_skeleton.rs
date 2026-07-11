//! 阶段 1 契约骨架集成测试。
//! 核心防漂移手段：遍历 OpenAPI 文档的全部 operation 逐一真实请求——
//! 路由忘挂载报 404、方法写反报 405，两处登记（路由表 / utoipa 注解）任何漂移即红。

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use jianartifact_server::config::Config;
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
async fn 就绪自检数据目录不可写时五百零三() -> TestResult {
    // 把 data_dir 指到一个「文件」之下，create_dir_all 必然失败
    let dir = tempfile::tempdir()?;
    let blocker = dir.path().join("occupied");
    std::fs::write(&blocker, b"x")?;
    let config = Config {
        bind: "127.0.0.1:0".to_string(),
        database_path: dir.path().join("test.db"),
        data_dir: blocker.join("blobs"),
        log_level: "warn".to_string(),
    };
    let app = jianartifact_server::build_app(config).await?;
    let resp = send(&app, "GET", "/readyz").await?;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
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
            // 漂移判据：404=路由未挂载，405=方法不一致
            assert_ne!(
                status,
                StatusCode::NOT_FOUND,
                "{method} {path} 未挂载到路由"
            );
            assert_ne!(
                status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path} 路由方法与契约不一致"
            );
            // 阶段 1 语义：健康端点真实现（200/503），其余为 501 占位且带稳定错误码
            if status == StatusCode::NOT_IMPLEMENTED {
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
