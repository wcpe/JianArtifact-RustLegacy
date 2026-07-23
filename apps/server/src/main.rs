//! 服务与本地管理命令入口。

use std::error::Error;
use std::io::Write;
use std::process::ExitCode;

use jianartifact_server::admin::{CliCommand, parse_args, reset_password};
use jianartifact_server::api::ApiDoc;
use jianartifact_server::config::Config;
use jianartifact_server::error::StartupError;
use jianartifact_server::migration::export::export_native_package;
use jianartifact_server::{build_app, logging};
use utoipa::OpenApi;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(std::io::stderr().lock(), "{error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    match parse_args(std::env::args_os().skip(1))? {
        CliCommand::Serve => serve(Config::load()?).await?,
        CliCommand::OpenApi => print_openapi()?,
        CliCommand::ResetPassword {
            username,
            password_stdin,
        } => {
            let config = Config::load()?;
            reset_password(&config.database_path, &username, password_stdin).await?;
            writeln!(std::io::stdout().lock(), "口令重置成功")?;
        }
        CliCommand::MigrationExport {
            source,
            output,
            repository_config,
            nexus_version,
        } => {
            let config = Config::load()?;
            export_native_package(
                config.migration_workdir.as_deref(),
                &source,
                &output,
                &repository_config,
                &nexus_version,
            )?;
            writeln!(std::io::stdout().lock(), "离线迁移包导出成功")?;
        }
    }
    Ok(())
}

fn print_openapi() -> Result<(), StartupError> {
    let doc = ApiDoc::openapi()
        .to_pretty_json()
        .map_err(|error| StartupError::BindAddr(format!("契约序列化失败：{error}")))?;
    println!("{doc}");
    Ok(())
}

async fn serve(config: Config) -> Result<(), StartupError> {
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
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::error!("注册 SIGTERM 信号处理失败：{error}"),
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
