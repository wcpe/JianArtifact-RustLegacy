//! 健康检查与就绪自检（M1-FR-021，API.md §5）。
//! `/healthz` 纯存活；`/readyz` 在线探测元数据库可读写与 blob 目录可写，失败明细只落中文日志。

use axum::extract::State;
use axum::http::StatusCode;

use crate::state::AppState;

/// 存活探针：进程存活即 200，不做依赖探测。
#[utoipa::path(
    get,
    path = "/healthz",
    tag = "health",
    responses((status = 200, description = "进程存活")),
)]
pub async fn healthz() -> StatusCode {
    StatusCode::OK
}

/// 就绪自检：关键依赖全部通过返回 200，任一失败返回 503。
#[utoipa::path(
    get,
    path = "/readyz",
    tag = "health",
    responses(
        (status = 200, description = "关键依赖就绪"),
        (status = 503, description = "存在未就绪依赖"),
    ),
)]
pub async fn readyz(State(state): State<AppState>) -> StatusCode {
    if let Err(err) = probe_database(&state).await {
        tracing::error!("就绪自检失败：元数据库不可用：{err}");
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    if let Err(err) = probe_data_dir(&state).await {
        tracing::error!("就绪自检失败：blob 数据目录不可写：{err}");
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    StatusCode::OK
}

/// 探测元数据库可读写：执行一次轻量查询。
async fn probe_database(state: &AppState) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT 1")
        .execute(&state.pool)
        .await
        .map(|_| ())
}

/// 探测 blob 数据目录可写：写入并删除一个探针文件。
async fn probe_data_dir(state: &AppState) -> Result<(), std::io::Error> {
    tokio::fs::create_dir_all(&state.config.data_dir).await?;
    let probe = state.config.data_dir.join(".readyz-probe");
    tokio::fs::write(&probe, b"ok").await?;
    tokio::fs::remove_file(&probe).await?;
    Ok(())
}
