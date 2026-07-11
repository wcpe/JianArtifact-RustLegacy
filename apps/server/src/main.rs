//! 服务入口：加载配置 → 初始化日志与基础设施 → 启动 HTTP 服务（支持优雅停机）。
//! 附带 `openapi` 模式：将契约 JSON 打印到标准输出后退出，供前端类型生成与 devmock 比对。

use jianartifact_server::api::ApiDoc;
use jianartifact_server::config::Config;
use jianartifact_server::error::StartupError;
use jianartifact_server::{build_app, logging};
use utoipa::OpenApi;

#[tokio::main]
async fn main() -> Result<(), StartupError> {
    // openapi 模式：只输出契约，不启动服务（此输出是数据产物，非日志）
    if std::env::args().nth(1).as_deref() == Some("openapi") {
        let doc = ApiDoc::openapi()
            .to_pretty_json()
            .map_err(|err| StartupError::BindAddr(format!("契约序列化失败：{err}")))?;
        println!("{doc}");
        return Ok(());
    }

    let config = Config::load()?;
    logging::init(&config.log_level);
    let bind = config.bind.clone();
    let app = build_app(config).await?;

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("服务已启动，监听 {bind}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("服务已优雅停机");
    Ok(())
}

/// 等待 Ctrl+C / SIGTERM，触发优雅停机。
async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            tracing::error!("注册 Ctrl+C 信号处理失败");
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => tracing::error!("注册 SIGTERM 信号处理失败：{err}"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("收到停机信号，开始优雅停机");
}
