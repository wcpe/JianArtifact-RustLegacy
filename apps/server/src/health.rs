//! 健康检查与就绪自检（M1-FR-021，API.md §5）。
//! `/healthz` 纯存活；`/readyz` 在线探测元数据库可读写与 blob 目录可写，失败明细只落中文日志。

use axum::extract::State;
use axum::http::StatusCode;
use rand::RngCore;
use rand::rngs::OsRng;
use tokio::io::AsyncWriteExt;

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
    if probe_database(&state).await.is_err() {
        tracing::error!("就绪自检失败：元数据库写入探测未通过");
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    if probe_data_dir(&state).await.is_err() {
        tracing::error!("就绪自检失败：blob 数据目录写入探测未通过");
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    StatusCode::OK
}

/// 探测元数据库可写：插入随机临时行后回滚事务。
async fn probe_database(state: &AppState) -> Result<(), sqlx::Error> {
    let mut transaction = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO revoked_sessions (jti_hash, expires_at) VALUES (?, datetime('now', '+1 minute'))",
    )
    .bind(format!("readyz-probe-{}", random_probe_id()))
    .execute(&mut *transaction)
    .await?;
    transaction.rollback().await
}

/// 探测 blob 数据目录可写：以独占方式写入随机文件并删除。
async fn probe_data_dir(state: &AppState) -> Result<(), std::io::Error> {
    let probe = state
        .config
        .data_dir
        .join(format!(".readyz-{}.probe", random_probe_id()));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .await?;
    if let Err(error) = file.write_all(b"ok").await {
        drop(file);
        let _ = tokio::fs::remove_file(&probe).await;
        return Err(error);
    }
    drop(file);
    tokio::fs::remove_file(probe).await
}

fn random_probe_id() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    format!("{:032x}", u128::from_ne_bytes(bytes))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use super::*;
    use crate::auth::AuthRuntime;
    use crate::config::Config;
    use crate::persistence;

    #[tokio::test]
    async fn 数据库只读时就绪失败恢复后通过且探针零污染() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("blobs");
        std::fs::create_dir(&data_dir).unwrap();
        let pool = persistence::test_pool().await;
        let blob_store = Arc::new(crate::storage::BlobStore::new(
            dir.path().join("blobs"),
            5 * 1024 * 1024 * 1024,
            5 * 1024 * 1024 * 1024,
            5,
        ));
        let artifacts = Arc::new(crate::artifact::ArtifactService::new(
            pool.clone(),
            blob_store.as_ref().clone(),
        ));
        let state = AppState {
            pool: pool.clone(),
            config: Arc::new(Config {
                bind: "127.0.0.1:0".to_string(),
                database_path: dir.path().join("test.db"),
                data_dir,
                log_level: "warn".to_string(),
                jwt_secret: "health-test-secret".to_string(),
                session_ttl_hours: 8,
                max_artifact_bytes: 5 * 1024 * 1024 * 1024,
                min_free_bytes: 5 * 1024 * 1024 * 1024,
                min_free_percent: 5,
            }),
            auth: Arc::new(AuthRuntime::new("health-test-secret", 8)),
            blob_store,
            artifacts,
        };

        sqlx::query("PRAGMA query_only = ON")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            readyz(State(state.clone())).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(healthz().await, StatusCode::OK);

        sqlx::query("PRAGMA query_only = OFF")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(readyz(State(state)).await, StatusCode::OK);
        let rows: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM revoked_sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows.0, 0, "数据库探针不得持久化临时记录");
    }
}
