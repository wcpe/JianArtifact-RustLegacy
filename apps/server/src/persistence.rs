//! 元数据库连接与迁移框架（SQLite 唯一真源，ADR-0002）。迁移脚本只向前追加。

use std::path::Path;

use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;

use crate::error::StartupError;

/// 编译期内嵌的迁移集合（`apps/server/migrations`）。
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// 建立连接池并执行全部未应用的迁移。
pub async fn init(database_url: &str, database_path: &Path) -> Result<SqlitePool, StartupError> {
    // 确保库文件所在目录存在（sqlite rwc 模式只创建文件不创建目录）
    if let Some(parent) = database_path.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect(database_url)
        .await?;
    MIGRATOR.run(&pool).await?;
    tracing::info!("元数据库已就绪并完成迁移");
    Ok(pool)
}
