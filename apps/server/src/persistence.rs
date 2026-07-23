//! 元数据库连接与迁移框架（SQLite 唯一真源，ADR-0002）。迁移脚本只向前追加。
//! 兼含认证域仓储：用户、API Token、会话撤销的增改查，均只做数据存取、不含业务规则。

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{QueryBuilder, Sqlite, SqliteConnection, SqlitePool, Transaction};

use crate::domain::Role;
use crate::domain::encoding::strict_base64_decode;
use crate::domain::repository::{
    AclRecord, Format, OverwritePolicy, Permission, RepoType, Repository, VersionPolicy, Visibility,
};
use crate::error::{ApiError, StartupError};

pub(crate) mod migration;

/// 编译期内嵌的迁移集合（`apps/server/migrations`）。
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// 建立连接池并执行全部未应用的迁移。
pub async fn init(_database_url: &str, database_path: &Path) -> Result<SqlitePool, StartupError> {
    // 确保库文件所在目录存在（SQLite 只创建文件，不创建父目录）
    if let Some(parent) = database_path.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    let options = SqliteConnectOptions::new()
        .filename(database_path)
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;
    MIGRATOR.run(&pool).await?;
    tracing::info!("元数据库已就绪并完成迁移");
    Ok(pool)
}

/// 读取当前仍被制品元数据引用的全部 blob 摘要，供启动恢复判定。
pub async fn referenced_blob_hashes(pool: &SqlitePool) -> Result<HashSet<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT DISTINCT sha256 FROM artifacts")
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|row| row.0).collect())
}

// ==================== 制品索引仓储 ====================

const ARTIFACT_COLUMNS: &str =
    "id, repo_id, path, sha256, sha1, md5, sha512, size, cached, created_at, updated_at";
const BATCH_ARTIFACT_COLUMNS: &str = "artifacts.id, artifacts.repo_id, artifacts.path, artifacts.sha256, artifacts.sha1, artifacts.md5, artifacts.sha512, artifacts.size, artifacts.cached, artifacts.created_at, artifacts.updated_at";

/// 制品索引记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRecord {
    pub id: i64,
    pub repo_id: i64,
    pub path: String,
    pub sha256: String,
    pub sha1: String,
    pub md5: String,
    pub sha512: String,
    pub size: u64,
    pub cached: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// 制品索引写入参数。
pub struct ArtifactWriteInput {
    pub repo_id: i64,
    pub path: String,
    pub sha256: String,
    pub sha1: String,
    pub md5: String,
    pub sha512: String,
    pub size: u64,
    pub cached: bool,
}

/// 制品缓存状态，携带更新时间与按 TTL 计算的新鲜度。
#[derive(Debug, PartialEq, Eq)]
pub struct ArtifactCacheStatus {
    pub record: ArtifactRecord,
    pub fresh: bool,
}

/// npm hosted 版本原子提交输入。
pub struct NpmHostedVersionInput {
    pub artifact: ArtifactWriteInput,
    pub package_name: String,
    pub version: String,
    pub manifest_json: String,
    pub dist_tags: Vec<String>,
}

/// 迁移 npm hosted 包的单版本元数据。
pub struct NpmMigrationVersionInput {
    pub version: String,
    pub manifest_json: String,
    pub tarball_path: String,
    pub shasum: Option<String>,
    pub integrity: Option<String>,
}

/// 迁移 npm hosted 包元数据，引用已经校验提交的 tarball 制品。
pub struct NpmMigrationPackageInput {
    pub repo_id: i64,
    pub package_name: String,
    pub versions: Vec<NpmMigrationVersionInput>,
    pub dist_tags: BTreeMap<String, String>,
}

/// npm 迁移单类校验计数。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NpmMigrationValidationCounts {
    pub checked: u64,
    pub passed: u64,
    pub failed: u64,
}

/// npm 迁移协议元数据与 proxy 正缓存校验结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NpmMigrationValidationReport {
    #[serde(default)]
    pub proxy_positive_cache: NpmMigrationValidationCounts,
    #[serde(default)]
    pub protocol_metadata: NpmMigrationValidationCounts,
}

/// npm proxy tarball 映射输入。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NpmProxyTarballInput {
    pub version: String,
    pub tarball_url: String,
    pub integrity: Option<String>,
    pub shasum: Option<String>,
    pub artifact_path: Option<String>,
}

/// npm proxy 重写 packument 原子替换输入。
pub struct NpmProxyPackumentInput {
    pub repo_id: i64,
    pub package_name: String,
    pub rewritten_json: String,
    pub upstream_url_snapshot: String,
    pub ttl_secs: u64,
    pub tarballs: Vec<NpmProxyTarballInput>,
}

/// npm proxy packument 缓存记录。
#[derive(Debug, PartialEq, Eq)]
pub struct NpmProxyPackument {
    pub rewritten_json: String,
    pub upstream_url_snapshot: String,
    pub ttl_secs: u64,
    pub fetched_at: String,
    pub fresh: bool,
    pub tarballs: Vec<NpmProxyTarballInput>,
}

/// npm hosted 版本记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpmVersionRecord {
    pub package_name: String,
    pub version: String,
    pub manifest_json: String,
    pub tarball_path: String,
    pub sha1: String,
    pub sha512: String,
    pub created_at: String,
    pub updated_at: String,
}

/// npm 包元数据记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpmPackageRecord {
    pub versions: Vec<NpmVersionRecord>,
    pub dist_tags: BTreeMap<String, String>,
}

/// npm hosted 版本提交错误。
#[derive(Debug, thiserror::Error)]
pub enum NpmVersionCommitError {
    #[error("npm 版本已存在")]
    Conflict,
    #[error("代理仓库配置已变化")]
    StaleProxyConfiguration,
    #[error("npm 版本提交失败")]
    Internal(ApiError),
}

/// 持久化 blob 回收令牌。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BlobReclamation {
    pub sha256: String,
    pub generation: i64,
}

/// 回收令牌的当前状态。
#[derive(Debug, PartialEq, Eq)]
pub struct BlobReclamationState {
    pub reclamation: BlobReclamation,
    pub referenced: bool,
}

/// 原子提交结果。
#[derive(Debug, PartialEq, Eq)]
pub struct ArtifactCommitResult {
    pub artifact: ArtifactRecord,
    pub reclamation: Option<BlobReclamation>,
    pub created: bool,
}

/// 单个制品索引删除结果。
#[derive(Debug, PartialEq, Eq)]
pub struct ArtifactDeleteResult {
    pub artifact: ArtifactRecord,
    pub reclamation: BlobReclamation,
}

/// 目录索引中的直接子条目。
#[derive(Debug, PartialEq, Eq)]
pub struct ArtifactDirectoryEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    pub sha1: Option<String>,
    pub md5: Option<String>,
    pub sha512: Option<String>,
    pub updated_at: String,
}

#[derive(sqlx::FromRow)]
struct ArtifactDirectoryRow {
    name: String,
    is_directory: bool,
    size: Option<i64>,
    sha256: Option<String>,
    sha1: Option<String>,
    md5: Option<String>,
    sha512: Option<String>,
    updated_at: String,
}

/// 制品提交错误，路径冲突可由调用方单独处理。
#[derive(Debug, thiserror::Error)]
pub enum ArtifactCommitError {
    #[error("制品路径已存在")]
    Conflict,
    #[error("代理仓库配置已变化")]
    StaleProxyConfiguration,
    #[error("制品提交失败")]
    Internal(ApiError),
}

#[derive(sqlx::FromRow)]
struct BlobReclamationRow {
    sha256: String,
    generation: i64,
}

#[derive(sqlx::FromRow)]
struct BlobReclamationStateRow {
    sha256: String,
    generation: i64,
    referenced: bool,
}

#[derive(sqlx::FromRow)]
struct ArtifactRow {
    id: i64,
    repo_id: i64,
    path: String,
    sha256: String,
    sha1: String,
    md5: String,
    sha512: String,
    size: i64,
    cached: bool,
    created_at: String,
    updated_at: String,
}

#[derive(sqlx::FromRow)]
struct ArtifactCacheStatusRow {
    id: i64,
    repo_id: i64,
    path: String,
    sha256: String,
    sha1: String,
    md5: String,
    sha512: String,
    size: i64,
    cached: bool,
    created_at: String,
    updated_at: String,
    fresh: bool,
}

#[derive(sqlx::FromRow)]
struct NpmVersionRow {
    package_name: String,
    version: String,
    manifest_json: String,
    tarball_path: String,
    sha1: String,
    sha512: String,
    created_at: String,
    updated_at: String,
}

#[derive(sqlx::FromRow)]
struct NpmProxyPackumentRow {
    rewritten_json: String,
    upstream_url_snapshot: String,
    ttl_secs: i64,
    fetched_at: String,
    fresh: bool,
}

#[derive(sqlx::FromRow)]
struct BatchNpmVersionRow {
    repo_id: i64,
    package_name: String,
    version: String,
    manifest_json: String,
    tarball_path: String,
    sha1: String,
    sha512: String,
    created_at: String,
    updated_at: String,
}

#[derive(sqlx::FromRow)]
struct BatchNpmProxyPackumentRow {
    repo_id: i64,
    rewritten_json: String,
    upstream_url_snapshot: String,
    ttl_secs: i64,
    fetched_at: String,
    fresh: bool,
}

#[derive(sqlx::FromRow)]
struct BatchNpmProxyTarballRow {
    repo_id: i64,
    version: String,
    tarball_url: String,
    integrity: Option<String>,
    shasum: Option<String>,
    artifact_path: Option<String>,
}

impl From<BlobReclamationRow> for BlobReclamation {
    fn from(row: BlobReclamationRow) -> Self {
        Self {
            sha256: row.sha256,
            generation: row.generation,
        }
    }
}

impl From<BlobReclamationStateRow> for BlobReclamationState {
    fn from(row: BlobReclamationStateRow) -> Self {
        Self {
            reclamation: BlobReclamation {
                sha256: row.sha256,
                generation: row.generation,
            },
            referenced: row.referenced,
        }
    }
}

impl TryFrom<ArtifactRow> for ArtifactRecord {
    type Error = ApiError;

    fn try_from(row: ArtifactRow) -> Result<Self, Self::Error> {
        let size = u64::try_from(row.size).map_err(|_| ApiError::internal("制品大小非法"))?;
        Ok(Self {
            id: row.id,
            repo_id: row.repo_id,
            path: row.path,
            sha256: row.sha256,
            sha1: row.sha1,
            md5: row.md5,
            sha512: row.sha512,
            size,
            cached: row.cached,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

impl TryFrom<ArtifactCacheStatusRow> for ArtifactCacheStatus {
    type Error = ApiError;

    fn try_from(row: ArtifactCacheStatusRow) -> Result<Self, Self::Error> {
        let fresh = row.fresh;
        let record = ArtifactRecord::try_from(ArtifactRow {
            id: row.id,
            repo_id: row.repo_id,
            path: row.path,
            sha256: row.sha256,
            sha1: row.sha1,
            md5: row.md5,
            sha512: row.sha512,
            size: row.size,
            cached: row.cached,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })?;
        Ok(Self { record, fresh })
    }
}

impl From<NpmVersionRow> for NpmVersionRecord {
    fn from(row: NpmVersionRow) -> Self {
        Self {
            package_name: row.package_name,
            version: row.version,
            manifest_json: row.manifest_json,
            tarball_path: row.tarball_path,
            sha1: row.sha1,
            sha512: row.sha512,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

fn artifact_size_i64(size: u64) -> Result<i64, ApiError> {
    i64::try_from(size).map_err(|_| ApiError::internal("制品大小超出数据库范围"))
}

fn artifact_commit_internal(
    message: &'static str,
) -> impl FnOnce(sqlx::Error) -> ArtifactCommitError {
    move |_| ArtifactCommitError::Internal(ApiError::internal(message))
}

fn sorted_reclamations(rows: Vec<BlobReclamationRow>) -> Vec<BlobReclamation> {
    let mut reclamations: Vec<_> = rows.into_iter().map(BlobReclamation::from).collect();
    reclamations.sort_unstable_by(|left, right| left.sha256.cmp(&right.sha256));
    reclamations
}

async fn register_blob_reclamation(
    tx: &mut Transaction<'_, Sqlite>,
    sha256: &str,
) -> Result<BlobReclamation, ApiError> {
    let row = sqlx::query_as::<_, BlobReclamationRow>(
        "INSERT INTO blob_reclamations (sha256, generation) VALUES (?, 1) ON CONFLICT(sha256) DO UPDATE SET generation = generation + 1, updated_at = datetime('now') RETURNING sha256, generation",
    )
    .bind(sha256)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| ApiError::internal("登记 blob 回收意图失败"))?;
    Ok(row.into())
}

async fn register_repository_reclamations(
    tx: &mut Transaction<'_, Sqlite>,
    repository_id: i64,
    cached_only: bool,
) -> Result<Vec<BlobReclamation>, ApiError> {
    let filter = if cached_only {
        "repo_id = ? AND cached = 1"
    } else {
        "repo_id = ?"
    };
    let sql = format!(
        "INSERT INTO blob_reclamations (sha256, generation) SELECT DISTINCT sha256, 1 FROM artifacts WHERE {filter} ON CONFLICT(sha256) DO UPDATE SET generation = generation + 1, updated_at = datetime('now') RETURNING sha256, generation"
    );
    let rows = sqlx::query_as::<_, BlobReclamationRow>(&sql)
        .bind(repository_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|_| ApiError::internal("批量登记 blob 回收意图失败"))?;
    Ok(sorted_reclamations(rows))
}

async fn cancel_repository_reclamations(
    tx: &mut Transaction<'_, Sqlite>,
    repository_id: i64,
) -> Result<(), ApiError> {
    sqlx::query(
        "DELETE FROM blob_reclamations WHERE sha256 IN (SELECT DISTINCT sha256 FROM artifacts WHERE repo_id = ?)",
    )
    .bind(repository_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| ApiError::internal("取消 blob 回收意图失败"))?;
    Ok(())
}

/// 列出全部待处理 blob 回收令牌，按摘要稳定排序。
pub async fn pending_blob_reclamations(
    pool: &SqlitePool,
) -> Result<Vec<BlobReclamation>, ApiError> {
    let rows = sqlx::query_as::<_, BlobReclamationRow>(
        "SELECT sha256, generation FROM blob_reclamations ORDER BY sha256",
    )
    .fetch_all(pool)
    .await
    .map_err(|_| ApiError::internal("查询待处理 blob 回收意图失败"))?;
    Ok(rows.into_iter().map(BlobReclamation::from).collect())
}

const RECLAMATION_REQUEST_CTE: &str = "WITH requested(sha256, generation) AS (SELECT json_extract(value, '$.sha256'), CAST(json_extract(value, '$.generation') AS INTEGER) FROM json_each(?))";

/// 批量核对回收令牌当前代次及全局引用状态。
pub async fn inspect_blob_reclamations(
    pool: &SqlitePool,
    reclamations: &[BlobReclamation],
) -> Result<Vec<BlobReclamationState>, ApiError> {
    if reclamations.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "{RECLAMATION_REQUEST_CTE} SELECT blob_reclamations.sha256, blob_reclamations.generation, EXISTS(SELECT 1 FROM artifacts WHERE artifacts.sha256 = blob_reclamations.sha256) AS referenced FROM blob_reclamations JOIN requested ON requested.sha256 = blob_reclamations.sha256 AND requested.generation = blob_reclamations.generation ORDER BY blob_reclamations.sha256"
    );
    let rows = sqlx::query_as::<_, BlobReclamationStateRow>(&sql)
        .bind(reclamation_payload(reclamations)?)
        .fetch_all(pool)
        .await
        .map_err(|_| ApiError::internal("批量核对 blob 回收意图失败"))?;
    Ok(rows.into_iter().map(BlobReclamationState::from).collect())
}

/// 仅按匹配代次批量完成回收意图。
pub async fn complete_blob_reclamations(
    pool: &SqlitePool,
    reclamations: &[BlobReclamation],
) -> Result<u64, ApiError> {
    if reclamations.is_empty() {
        return Ok(0);
    }
    let sql = format!(
        "{RECLAMATION_REQUEST_CTE} DELETE FROM blob_reclamations WHERE EXISTS (SELECT 1 FROM requested WHERE requested.sha256 = blob_reclamations.sha256 AND requested.generation = blob_reclamations.generation)"
    );
    let result = sqlx::query(&sql)
        .bind(reclamation_payload(reclamations)?)
        .execute(pool)
        .await
        .map_err(|_| ApiError::internal("完成 blob 回收意图失败"))?;
    Ok(result.rows_affected())
}

fn reclamation_payload(reclamations: &[BlobReclamation]) -> Result<String, ApiError> {
    serde_json::to_string(reclamations).map_err(|_| ApiError::internal("序列化 blob 回收意图失败"))
}

/// 按仓库与路径读取制品索引。
pub async fn find_artifact_by_path(
    pool: &SqlitePool,
    repo_id: i64,
    path: &str,
) -> Result<Option<ArtifactRecord>, ApiError> {
    let sql = format!("SELECT {ARTIFACT_COLUMNS} FROM artifacts WHERE repo_id = ? AND path = ?");
    sqlx::query_as::<_, ArtifactRow>(&sql)
        .bind(repo_id)
        .bind(path)
        .fetch_optional(pool)
        .await
        .map_err(|_| ApiError::internal("查询制品失败"))?
        .map(ArtifactRecord::try_from)
        .transpose()
}

/// 按多个仓库与同一路径批量读取制品索引。
pub async fn find_artifacts_by_path(
    pool: &SqlitePool,
    repo_ids: &[i64],
    path: &str,
) -> Result<BTreeMap<i64, ArtifactRecord>, ApiError> {
    if repo_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let sql = format!(
        "WITH requested(repo_id) AS (SELECT DISTINCT CAST(value AS INTEGER) FROM json_each(?)) SELECT {BATCH_ARTIFACT_COLUMNS} FROM artifacts JOIN requested ON requested.repo_id = artifacts.repo_id WHERE artifacts.path = ? ORDER BY artifacts.repo_id, artifacts.id"
    );
    let rows = sqlx::query_as::<_, ArtifactRow>(&sql)
        .bind(repo_id_payload(repo_ids)?)
        .bind(path)
        .fetch_all(pool)
        .await
        .map_err(|_| ApiError::internal("批量查询制品失败"))?;
    artifact_rows_by_repo(rows)
}

/// 按多个仓库与同一路径前缀批量列出制品索引。
pub async fn list_artifacts_by_prefix_batch(
    pool: &SqlitePool,
    repo_ids: &[i64],
    prefix: &str,
) -> Result<BTreeMap<i64, Vec<ArtifactRecord>>, ApiError> {
    if repo_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let pattern = format!("{}%", escape_like(prefix));
    let sql = format!(
        "WITH requested(repo_id) AS (SELECT DISTINCT CAST(value AS INTEGER) FROM json_each(?)) SELECT {BATCH_ARTIFACT_COLUMNS} FROM artifacts JOIN requested ON requested.repo_id = artifacts.repo_id WHERE artifacts.path LIKE ? ESCAPE '\\' ORDER BY artifacts.repo_id, artifacts.path, artifacts.id"
    );
    let rows = sqlx::query_as::<_, ArtifactRow>(&sql)
        .bind(repo_id_payload(repo_ids)?)
        .bind(pattern)
        .fetch_all(pool)
        .await
        .map_err(|_| ApiError::internal("批量按前缀查询制品失败"))?;
    let mut records = BTreeMap::<i64, Vec<ArtifactRecord>>::new();
    for row in rows {
        let record = ArtifactRecord::try_from(row)?;
        records.entry(record.repo_id).or_default().push(record);
    }
    Ok(records)
}

fn artifact_rows_by_repo(
    rows: Vec<ArtifactRow>,
) -> Result<BTreeMap<i64, ArtifactRecord>, ApiError> {
    rows.into_iter()
        .map(|row| {
            let record = ArtifactRecord::try_from(row)?;
            Ok((record.repo_id, record))
        })
        .collect()
}

fn repo_id_payload(repo_ids: &[i64]) -> Result<String, ApiError> {
    serde_json::to_string(repo_ids).map_err(|_| ApiError::internal("序列化仓库标识列表失败"))
}

/// 按路径前缀列出完整制品记录，稳定按路径与标识排序。
pub async fn list_artifacts_by_prefix(
    pool: &SqlitePool,
    repo_id: i64,
    prefix: &str,
) -> Result<Vec<ArtifactRecord>, ApiError> {
    let pattern = format!("{}%", escape_like(prefix));
    let sql = format!(
        "SELECT {ARTIFACT_COLUMNS} FROM artifacts WHERE repo_id = ? AND path LIKE ? ESCAPE '\\' ORDER BY path, id"
    );
    let rows = sqlx::query_as::<_, ArtifactRow>(&sql)
        .bind(repo_id)
        .bind(pattern)
        .fetch_all(pool)
        .await
        .map_err(|_| ApiError::internal("按前缀查询制品失败"))?;
    rows.into_iter().map(ArtifactRecord::try_from).collect()
}

/// 读取制品更新时间，并按调用方 TTL 计算缓存新鲜度。
pub async fn find_artifact_cache_status(
    pool: &SqlitePool,
    repo_id: i64,
    path: &str,
    ttl_secs: u64,
) -> Result<Option<ArtifactCacheStatus>, ApiError> {
    let ttl = i64::try_from(ttl_secs).map_err(|_| ApiError::internal("缓存 TTL 超出数据库范围"))?;
    let sql = format!(
        "SELECT {ARTIFACT_COLUMNS}, (unixepoch('now') - unixepoch(updated_at) <= ?) AS fresh FROM artifacts WHERE repo_id = ? AND path = ?"
    );
    let row = sqlx::query_as::<_, ArtifactCacheStatusRow>(&sql)
        .bind(ttl)
        .bind(repo_id)
        .bind(path)
        .fetch_optional(pool)
        .await
        .map_err(|_| ApiError::internal("查询制品缓存状态失败"))?;
    row.map(ArtifactCacheStatus::try_from).transpose()
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// 统计指定内容摘要的全局索引引用数。
pub async fn count_artifact_references(pool: &SqlitePool, sha256: &str) -> Result<i64, ApiError> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM artifacts WHERE sha256 = ?")
        .bind(sha256)
        .fetch_one(pool)
        .await
        .map_err(|_| ApiError::internal("统计制品引用失败"))?;
    Ok(row.0)
}

/// 批量返回候选摘要中仍被任意制品索引引用的集合。
pub async fn referenced_candidate_hashes(
    pool: &SqlitePool,
    candidates: &[String],
) -> Result<HashSet<String>, ApiError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "WITH requested(sha256) AS (SELECT value FROM json_each(?)) SELECT DISTINCT artifacts.sha256 FROM artifacts JOIN requested ON requested.sha256 = artifacts.sha256",
    )
    .bind(string_list_payload(candidates)?)
    .fetch_all(pool)
    .await
    .map_err(|_| ApiError::internal("批量统计制品引用失败"))?;
    Ok(rows.into_iter().map(|row| row.0).collect())
}

fn string_list_payload(values: &[String]) -> Result<String, ApiError> {
    serde_json::to_string(values).map_err(|_| ApiError::internal("序列化摘要列表失败"))
}

/// 列出仓库当前引用的去重内容摘要。
pub async fn list_repository_blob_hashes(
    pool: &SqlitePool,
    repo_id: i64,
) -> Result<Vec<String>, ApiError> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT DISTINCT sha256 FROM artifacts WHERE repo_id = ? ORDER BY sha256")
            .bind(repo_id)
            .fetch_all(pool)
            .await
            .map_err(|_| ApiError::internal("查询仓库制品摘要失败"))?;
    Ok(rows.into_iter().map(|row| row.0).collect())
}

/// 删除单个路径，并在同一事务登记旧 blob 回收意图。
pub async fn delete_artifact_by_path(
    pool: &SqlitePool,
    repo_id: i64,
    path: &str,
) -> Result<Option<ArtifactDeleteResult>, ApiError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ApiError::internal("开启制品删除事务失败"))?;
    let sql = format!(
        "DELETE FROM artifacts WHERE repo_id = ? AND path = ? RETURNING {ARTIFACT_COLUMNS}"
    );
    let row = sqlx::query_as::<_, ArtifactRow>(&sql)
        .bind(repo_id)
        .bind(path)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| ApiError::internal("删除制品失败"))?;
    let Some(row) = row else {
        tx.rollback()
            .await
            .map_err(|_| ApiError::internal("回滚制品删除事务失败"))?;
        return Ok(None);
    };
    let artifact = ArtifactRecord::try_from(row)?;
    let reclamation = register_blob_reclamation(&mut tx, &artifact.sha256).await?;
    tx.commit()
        .await
        .map_err(|_| ApiError::internal("提交制品删除事务失败"))?;
    Ok(Some(ArtifactDeleteResult {
        artifact,
        reclamation,
    }))
}

/// 原子提交 hosted 制品索引；覆盖时同事务登记旧 blob 回收意图。
pub async fn commit_artifact(
    pool: &SqlitePool,
    input: &ArtifactWriteInput,
    allow_overwrite: bool,
) -> Result<ArtifactCommitResult, ArtifactCommitError> {
    if !allow_overwrite {
        return insert_artifact_without_overwrite(pool, input).await;
    }
    upsert_artifact_in_transaction(pool, input).await
}

/// 仅当 proxy 仓库上游地址仍等于预期快照时提交正缓存。
pub async fn commit_proxy_artifact(
    pool: &SqlitePool,
    input: &ArtifactWriteInput,
    expected_upstream_url: &str,
    allow_overwrite: bool,
) -> Result<ArtifactCommitResult, ArtifactCommitError> {
    let size = artifact_size_i64(input.size).map_err(ArtifactCommitError::Internal)?;
    if !allow_overwrite {
        return insert_proxy_artifact_without_overwrite(pool, input, size, expected_upstream_url)
            .await;
    }

    let mut tx = pool
        .begin()
        .await
        .map_err(artifact_commit_internal("开启代理缓存提交事务失败"))?;
    let old = load_replaced_sha256(&mut tx, input).await?;
    let row = conditional_proxy_upsert(&mut tx, input, size, expected_upstream_url, true).await?;
    let Some(row) = row else {
        tx.rollback()
            .await
            .map_err(artifact_commit_internal("回滚过期代理缓存事务失败"))?;
        return Err(ArtifactCommitError::StaleProxyConfiguration);
    };
    finish_artifact_upsert(tx, row, old, input).await
}

async fn insert_proxy_artifact_without_overwrite(
    pool: &SqlitePool,
    input: &ArtifactWriteInput,
    size: i64,
    expected_upstream_url: &str,
) -> Result<ArtifactCommitResult, ArtifactCommitError> {
    let mut connection = pool
        .acquire()
        .await
        .map_err(artifact_commit_internal("获取代理缓存提交连接失败"))?;
    let row = conditional_proxy_upsert(&mut connection, input, size, expected_upstream_url, false)
        .await?
        .ok_or(ArtifactCommitError::StaleProxyConfiguration)?;
    let artifact = ArtifactRecord::try_from(row).map_err(ArtifactCommitError::Internal)?;
    Ok(ArtifactCommitResult {
        artifact,
        reclamation: None,
        created: true,
    })
}

async fn insert_artifact_without_overwrite(
    pool: &SqlitePool,
    input: &ArtifactWriteInput,
) -> Result<ArtifactCommitResult, ArtifactCommitError> {
    let size = artifact_size_i64(input.size).map_err(ArtifactCommitError::Internal)?;
    let sql = format!(
        "INSERT INTO artifacts (repo_id, path, sha256, sha1, md5, sha512, size, cached) VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING {ARTIFACT_COLUMNS}"
    );
    let row = bind_artifact_write(sqlx::query_as::<_, ArtifactRow>(&sql), input, size)
        .fetch_one(pool)
        .await
        .map_err(|error| match error {
            sqlx::Error::Database(database_error) if database_error.is_unique_violation() => {
                ArtifactCommitError::Conflict
            }
            _ => ArtifactCommitError::Internal(ApiError::internal("写入制品失败")),
        })?;
    let artifact = ArtifactRecord::try_from(row).map_err(ArtifactCommitError::Internal)?;
    Ok(ArtifactCommitResult {
        artifact,
        reclamation: None,
        created: true,
    })
}

async fn upsert_artifact_in_transaction(
    pool: &SqlitePool,
    input: &ArtifactWriteInput,
) -> Result<ArtifactCommitResult, ArtifactCommitError> {
    let size = artifact_size_i64(input.size).map_err(ArtifactCommitError::Internal)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(artifact_commit_internal("开启制品提交事务失败"))?;
    let old = load_replaced_sha256(&mut tx, input).await?;
    let sql = format!(
        "INSERT INTO artifacts (repo_id, path, sha256, sha1, md5, sha512, size, cached) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(repo_id, path) DO UPDATE SET sha256 = excluded.sha256, sha1 = excluded.sha1, md5 = excluded.md5, sha512 = excluded.sha512, size = excluded.size, cached = excluded.cached, updated_at = datetime('now') RETURNING {ARTIFACT_COLUMNS}"
    );
    let row = bind_artifact_write(sqlx::query_as::<_, ArtifactRow>(&sql), input, size)
        .fetch_one(&mut *tx)
        .await
        .map_err(artifact_commit_internal("提交制品索引失败"))?;
    finish_artifact_upsert(tx, row, old, input).await
}

async fn load_replaced_sha256(
    tx: &mut Transaction<'_, Sqlite>,
    input: &ArtifactWriteInput,
) -> Result<Option<String>, ArtifactCommitError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT sha256 FROM artifacts WHERE repo_id = ? AND path = ?")
            .bind(input.repo_id)
            .bind(&input.path)
            .fetch_optional(&mut **tx)
            .await
            .map_err(artifact_commit_internal("读取旧制品失败"))?;
    Ok(row.map(|value| value.0))
}

async fn conditional_proxy_upsert(
    connection: &mut SqliteConnection,
    input: &ArtifactWriteInput,
    size: i64,
    expected_upstream_url: &str,
    allow_overwrite: bool,
) -> Result<Option<ArtifactRow>, ArtifactCommitError> {
    let conflict_clause = if allow_overwrite {
        " ON CONFLICT(repo_id, path) DO UPDATE SET sha256 = excluded.sha256, sha1 = excluded.sha1, md5 = excluded.md5, sha512 = excluded.sha512, size = excluded.size, cached = excluded.cached, updated_at = datetime('now')"
    } else {
        ""
    };
    let sql = format!(
        "INSERT INTO artifacts (repo_id, path, sha256, sha1, md5, sha512, size, cached) SELECT ?, ?, ?, ?, ?, ?, ?, ? FROM repositories WHERE id = ? AND repo_type = 'proxy' AND upstream_url = ?{conflict_clause} RETURNING {ARTIFACT_COLUMNS}"
    );
    bind_artifact_write(sqlx::query_as::<_, ArtifactRow>(&sql), input, size)
        .bind(input.repo_id)
        .bind(expected_upstream_url)
        .fetch_optional(connection)
        .await
        .map_err(|error| match error {
            sqlx::Error::Database(database_error) if database_error.is_unique_violation() => {
                ArtifactCommitError::Conflict
            }
            _ => ArtifactCommitError::Internal(ApiError::internal("提交代理缓存索引失败")),
        })
}

fn bind_artifact_write<'q>(
    query: sqlx::query::QueryAs<'q, Sqlite, ArtifactRow, sqlx::sqlite::SqliteArguments<'q>>,
    input: &'q ArtifactWriteInput,
    size: i64,
) -> sqlx::query::QueryAs<'q, Sqlite, ArtifactRow, sqlx::sqlite::SqliteArguments<'q>> {
    query
        .bind(input.repo_id)
        .bind(&input.path)
        .bind(&input.sha256)
        .bind(&input.sha1)
        .bind(&input.md5)
        .bind(&input.sha512)
        .bind(size)
        .bind(input.cached)
}

async fn finish_artifact_upsert(
    mut tx: Transaction<'_, Sqlite>,
    row: ArtifactRow,
    old: Option<String>,
    input: &ArtifactWriteInput,
) -> Result<ArtifactCommitResult, ArtifactCommitError> {
    let reclamation = match old.as_deref() {
        Some(sha256) if sha256 != input.sha256 => Some(
            register_blob_reclamation(&mut tx, sha256)
                .await
                .map_err(ArtifactCommitError::Internal)?,
        ),
        _ => None,
    };
    tx.commit()
        .await
        .map_err(artifact_commit_internal("提交制品事务失败"))?;
    let artifact = ArtifactRecord::try_from(row).map_err(ArtifactCommitError::Internal)?;
    Ok(ArtifactCommitResult {
        artifact,
        reclamation,
        created: old.is_none(),
    })
}

/// 原子提交 npm hosted 的制品索引、版本清单与 dist-tag。
pub async fn commit_npm_hosted_version(
    pool: &SqlitePool,
    input: &NpmHostedVersionInput,
) -> Result<ArtifactRecord, NpmVersionCommitError> {
    let size = artifact_size_i64(input.artifact.size).map_err(NpmVersionCommitError::Internal)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| npm_internal("开启 npm 发布事务失败"))?;
    let sql = format!(
        "INSERT INTO artifacts (repo_id, path, sha256, sha1, md5, sha512, size, cached) VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING {ARTIFACT_COLUMNS}"
    );
    let artifact = bind_artifact_write(
        sqlx::query_as::<_, ArtifactRow>(&sql),
        &input.artifact,
        size,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(map_npm_commit_error)?;
    insert_npm_version(&mut tx, input, artifact.id).await?;
    replace_npm_tags(&mut tx, input).await?;
    tx.commit()
        .await
        .map_err(|_| npm_internal("提交 npm 发布事务失败"))?;
    ArtifactRecord::try_from(artifact).map_err(NpmVersionCommitError::Internal)
}

/// 校验迁移 npm packument 引用的 tarball 与声明摘要一致。
pub async fn validate_npm_migration_package(
    pool: &SqlitePool,
    input: &NpmMigrationPackageInput,
) -> Result<(), ApiError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ApiError::internal("开启 npm 迁移摘要校验事务失败"))?;
    for version in &input.versions {
        let artifact = load_npm_migration_artifact(&mut tx, input, version).await?;
        validate_npm_migration_digests(version, &artifact)?;
    }
    tx.commit()
        .await
        .map_err(|_| ApiError::internal("提交 npm 迁移摘要校验事务失败"))?;
    Ok(())
}

/// 为迁移已提交的 npm tarball 重建 hosted 版本与 dist-tag 元数据。
pub async fn import_npm_migration_package(
    pool: &SqlitePool,
    input: &NpmMigrationPackageInput,
) -> Result<(), ApiError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ApiError::internal("开启 npm 迁移元数据事务失败"))?;
    for version in &input.versions {
        import_npm_migration_version(&mut tx, input, version).await?;
    }
    sqlx::query("DELETE FROM npm_dist_tags WHERE repo_id = ? AND package_name = ?")
        .bind(input.repo_id)
        .bind(&input.package_name)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal("更新 npm 迁移 dist-tag 失败"))?;
    for (tag, version) in &input.dist_tags {
        sqlx::query(
            "INSERT INTO npm_dist_tags (repo_id, package_name, tag, version) VALUES (?, ?, ?, ?)",
        )
        .bind(input.repo_id)
        .bind(&input.package_name)
        .bind(tag)
        .bind(version)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal("更新 npm 迁移 dist-tag 失败"))?;
    }
    tx.commit()
        .await
        .map_err(|_| ApiError::internal("提交 npm 迁移元数据事务失败"))?;
    Ok(())
}

/// 将 npm 迁移校验计数写入任务报告，供报告接口读取。
pub async fn store_npm_migration_validation_report(
    pool: &SqlitePool,
    task_id: &str,
    validation: NpmMigrationValidationReport,
) -> Result<(), ApiError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ApiError::internal("开启 npm 迁移报告事务失败"))?;
    let report_json: Option<String> =
        sqlx::query_scalar("SELECT report_json FROM migration_tasks WHERE id = ?")
            .bind(task_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| ApiError::internal("读取 npm 迁移报告失败"))?
            .ok_or_else(|| ApiError::internal("npm 迁移任务不存在"))?;
    let mut report = match report_json {
        Some(value) => serde_json::from_str(&value)
            .map_err(|_| ApiError::internal("npm 迁移报告不是有效 JSON"))?,
        None => serde_json::json!({}),
    };
    merge_npm_migration_validation(&mut report, validation)?;
    let serialized = serde_json::to_string(&report)
        .map_err(|_| ApiError::internal("序列化 npm 迁移报告失败"))?;
    sqlx::query(
        "UPDATE migration_tasks SET report_json = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(serialized)
    .bind(task_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApiError::internal("写入 npm 迁移报告失败"))?;
    tx.commit()
        .await
        .map_err(|_| ApiError::internal("提交 npm 迁移报告事务失败"))?;
    Ok(())
}

/// 读取任务报告中已持久化的 npm 迁移校验计数。
pub async fn load_npm_migration_validation_report(
    pool: &SqlitePool,
    task_id: &str,
) -> Result<Option<NpmMigrationValidationReport>, ApiError> {
    let report_json: Option<String> =
        sqlx::query_scalar("SELECT report_json FROM migration_tasks WHERE id = ?")
            .bind(task_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| ApiError::internal("读取 npm 迁移报告失败"))?
            .ok_or_else(|| ApiError::internal("npm 迁移任务不存在"))?;
    let Some(report_json) = report_json else {
        return Ok(None);
    };
    let report: serde_json::Value = serde_json::from_str(&report_json)
        .map_err(|_| ApiError::internal("npm 迁移报告不是有效 JSON"))?;
    report
        .get("validation")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| ApiError::internal("npm 迁移校验计数无效"))
}

fn merge_npm_migration_validation(
    report: &mut serde_json::Value,
    validation: NpmMigrationValidationReport,
) -> Result<(), ApiError> {
    let report = report
        .as_object_mut()
        .ok_or_else(|| ApiError::internal("npm 迁移报告必须为对象"))?;
    let validation_value = report
        .entry("validation")
        .or_insert_with(|| serde_json::json!({}));
    let validation_object = validation_value
        .as_object_mut()
        .ok_or_else(|| ApiError::internal("npm 迁移报告校验字段必须为对象"))?;
    validation_object.insert(
        "proxy_positive_cache".to_string(),
        serde_json::to_value(validation.proxy_positive_cache)
            .map_err(|_| ApiError::internal("序列化 proxy 正缓存校验计数失败"))?,
    );
    validation_object.insert(
        "protocol_metadata".to_string(),
        serde_json::to_value(validation.protocol_metadata)
            .map_err(|_| ApiError::internal("序列化 npm 协议元数据校验计数失败"))?,
    );
    Ok(())
}

async fn import_npm_migration_version(
    tx: &mut Transaction<'_, Sqlite>,
    package: &NpmMigrationPackageInput,
    version: &NpmMigrationVersionInput,
) -> Result<(), ApiError> {
    let artifact = load_npm_migration_artifact(tx, package, version).await?;
    validate_npm_migration_digests(version, &artifact)?;
    sqlx::query("INSERT INTO npm_versions (repo_id, package_name, version, manifest_json, tarball_path, artifact_id, sha256, sha1, md5, sha512) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(repo_id, package_name, version) DO NOTHING")
        .bind(package.repo_id)
        .bind(&package.package_name)
        .bind(&version.version)
        .bind(&version.manifest_json)
        .bind(&version.tarball_path)
        .bind(artifact.id)
        .bind(&artifact.sha256)
        .bind(&artifact.sha1)
        .bind(&artifact.md5)
        .bind(&artifact.sha512)
        .execute(&mut **tx)
        .await
        .map_err(|_| ApiError::internal("写入 npm 迁移版本元数据失败"))?;
    let current: (String, String, i64, String) = sqlx::query_as("SELECT manifest_json, tarball_path, artifact_id, sha256 FROM npm_versions WHERE repo_id = ? AND package_name = ? AND version = ?")
        .bind(package.repo_id)
        .bind(&package.package_name)
        .bind(&version.version)
        .fetch_one(&mut **tx)
        .await
        .map_err(|_| ApiError::internal("核对 npm 迁移版本元数据失败"))?;
    if current
        != (
            version.manifest_json.clone(),
            version.tarball_path.clone(),
            artifact.id,
            artifact.sha256,
        )
    {
        return Err(ApiError::conflict(
            "NPM_MIGRATION_METADATA_CONFLICT",
            "目标 npm 版本元数据与迁移来源不一致",
        ));
    }
    Ok(())
}

async fn load_npm_migration_artifact(
    tx: &mut Transaction<'_, Sqlite>,
    package: &NpmMigrationPackageInput,
    version: &NpmMigrationVersionInput,
) -> Result<ArtifactRow, ApiError> {
    let sql = format!("SELECT {ARTIFACT_COLUMNS} FROM artifacts WHERE repo_id = ? AND path = ?");
    sqlx::query_as::<_, ArtifactRow>(&sql)
        .bind(package.repo_id)
        .bind(&version.tarball_path)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| ApiError::internal("查询 npm 迁移 tarball 失败"))?
        .ok_or_else(|| ApiError::internal("npm 迁移元数据引用的 tarball 不存在"))
}

fn validate_npm_migration_digests(
    version: &NpmMigrationVersionInput,
    artifact: &ArtifactRow,
) -> Result<(), ApiError> {
    if version
        .shasum
        .as_deref()
        .is_some_and(|value| value != artifact.sha1)
    {
        return Err(npm_migration_digest_conflict());
    }
    if let Some(integrity) = version.integrity.as_deref() {
        let expected = integrity
            .strip_prefix("sha512-")
            .and_then(strict_base64_decode)
            .ok_or_else(npm_migration_digest_conflict)?;
        let actual =
            decode_lower_hex(&artifact.sha512).ok_or_else(npm_migration_digest_conflict)?;
        if expected != actual {
            return Err(npm_migration_digest_conflict());
        }
    }
    Ok(())
}

fn npm_migration_digest_conflict() -> ApiError {
    ApiError::conflict(
        "NPM_MIGRATION_DIGEST_CONFLICT",
        "npm packument 摘要与迁移 tarball 不一致",
    )
}

fn decode_lower_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Some((lower_hex_nibble(pair[0])? << 4) | lower_hex_nibble(pair[1])?))
        .collect()
}

fn lower_hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

async fn insert_npm_version(
    tx: &mut Transaction<'_, Sqlite>,
    input: &NpmHostedVersionInput,
    artifact_id: i64,
) -> Result<(), NpmVersionCommitError> {
    sqlx::query("INSERT INTO npm_versions (repo_id, package_name, version, manifest_json, tarball_path, artifact_id, sha256, sha1, md5, sha512) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(input.artifact.repo_id)
        .bind(&input.package_name)
        .bind(&input.version)
        .bind(&input.manifest_json)
        .bind(&input.artifact.path)
        .bind(artifact_id)
        .bind(&input.artifact.sha256)
        .bind(&input.artifact.sha1)
        .bind(&input.artifact.md5)
        .bind(&input.artifact.sha512)
        .execute(&mut **tx)
        .await
        .map_err(map_npm_commit_error)?;
    Ok(())
}

async fn replace_npm_tags(
    tx: &mut Transaction<'_, Sqlite>,
    input: &NpmHostedVersionInput,
) -> Result<(), NpmVersionCommitError> {
    let tags = serde_json::to_string(&input.dist_tags)
        .map_err(|_| npm_internal("序列化 npm dist-tag 失败"))?;
    sqlx::query("INSERT INTO npm_dist_tags (repo_id, package_name, tag, version) SELECT ?, ?, value, ? FROM json_each(?) WHERE true ON CONFLICT(repo_id, package_name, tag) DO UPDATE SET version = excluded.version, updated_at = datetime('now')")
        .bind(input.artifact.repo_id)
        .bind(&input.package_name)
        .bind(&input.version)
        .bind(tags)
        .execute(&mut **tx)
        .await
        .map_err(map_npm_commit_error)?;
    Ok(())
}

fn map_npm_commit_error(error: sqlx::Error) -> NpmVersionCommitError {
    match error {
        sqlx::Error::Database(database_error) if database_error.is_unique_violation() => {
            NpmVersionCommitError::Conflict
        }
        _ => npm_internal("写入 npm 发布元数据失败"),
    }
}

fn npm_internal(message: &'static str) -> NpmVersionCommitError {
    NpmVersionCommitError::Internal(ApiError::internal(message))
}

/// 读取 npm hosted 包的版本与 dist-tag。
pub async fn find_npm_package(
    pool: &SqlitePool,
    repo_id: i64,
    package_name: &str,
) -> Result<Option<NpmPackageRecord>, ApiError> {
    let versions = sqlx::query_as::<_, NpmVersionRow>("SELECT package_name, version, manifest_json, tarball_path, sha1, sha512, created_at, updated_at FROM npm_versions WHERE repo_id = ? AND package_name = ? ORDER BY created_at, version")
        .bind(repo_id)
        .bind(package_name)
        .fetch_all(pool)
        .await
        .map_err(|_| ApiError::internal("查询 npm 包版本失败"))?;
    if versions.is_empty() {
        return Ok(None);
    }
    let tags: Vec<(String, String)> = sqlx::query_as("SELECT tag, version FROM npm_dist_tags WHERE repo_id = ? AND package_name = ? ORDER BY tag")
        .bind(repo_id)
        .bind(package_name)
        .fetch_all(pool)
        .await
        .map_err(|_| ApiError::internal("查询 npm dist-tag 失败"))?;
    Ok(Some(NpmPackageRecord {
        versions: versions.into_iter().map(NpmVersionRecord::from).collect(),
        dist_tags: tags.into_iter().collect(),
    }))
}

/// 按多个仓库与同一包名批量读取 npm hosted 版本与 dist-tag。
pub async fn find_npm_packages(
    pool: &SqlitePool,
    repo_ids: &[i64],
    package_name: &str,
) -> Result<BTreeMap<i64, NpmPackageRecord>, ApiError> {
    if repo_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let payload = repo_id_payload(repo_ids)?;
    let versions = batch_npm_versions(pool, &payload, package_name).await?;
    let tags = batch_npm_tags(pool, &payload, package_name).await?;
    let mut packages = BTreeMap::<i64, NpmPackageRecord>::new();
    for row in versions {
        packages
            .entry(row.repo_id)
            .or_insert_with(empty_npm_package)
            .versions
            .push(row.into_version());
    }
    for (repo_id, tag, version) in tags {
        if let Some(package) = packages.get_mut(&repo_id) {
            package.dist_tags.insert(tag, version);
        }
    }
    Ok(packages)
}

async fn batch_npm_versions(
    pool: &SqlitePool,
    payload: &str,
    package_name: &str,
) -> Result<Vec<BatchNpmVersionRow>, ApiError> {
    sqlx::query_as("WITH requested(repo_id) AS (SELECT DISTINCT CAST(value AS INTEGER) FROM json_each(?)) SELECT npm_versions.repo_id, npm_versions.package_name, npm_versions.version, npm_versions.manifest_json, npm_versions.tarball_path, npm_versions.sha1, npm_versions.sha512, npm_versions.created_at, npm_versions.updated_at FROM npm_versions JOIN requested ON requested.repo_id = npm_versions.repo_id WHERE npm_versions.package_name = ? ORDER BY npm_versions.repo_id, npm_versions.created_at, npm_versions.version")
        .bind(payload)
        .bind(package_name)
        .fetch_all(pool)
        .await
        .map_err(|_| ApiError::internal("批量查询 npm 包版本失败"))
}

async fn batch_npm_tags(
    pool: &SqlitePool,
    payload: &str,
    package_name: &str,
) -> Result<Vec<(i64, String, String)>, ApiError> {
    sqlx::query_as("WITH requested(repo_id) AS (SELECT DISTINCT CAST(value AS INTEGER) FROM json_each(?)) SELECT npm_dist_tags.repo_id, npm_dist_tags.tag, npm_dist_tags.version FROM npm_dist_tags JOIN requested ON requested.repo_id = npm_dist_tags.repo_id WHERE npm_dist_tags.package_name = ? ORDER BY npm_dist_tags.repo_id, npm_dist_tags.tag")
        .bind(payload)
        .bind(package_name)
        .fetch_all(pool)
        .await
        .map_err(|_| ApiError::internal("批量查询 npm dist-tag 失败"))
}

fn empty_npm_package() -> NpmPackageRecord {
    NpmPackageRecord {
        versions: Vec::new(),
        dist_tags: BTreeMap::new(),
    }
}

impl BatchNpmVersionRow {
    fn into_version(self) -> NpmVersionRecord {
        NpmVersionRecord {
            package_name: self.package_name,
            version: self.version,
            manifest_json: self.manifest_json,
            tarball_path: self.tarball_path,
            sha1: self.sha1,
            sha512: self.sha512,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// 设置 npm dist-tag，目标版本不存在时不写入。
pub async fn set_npm_dist_tag(
    pool: &SqlitePool,
    repo_id: i64,
    package_name: &str,
    tag: &str,
    version: &str,
) -> Result<bool, ApiError> {
    let result = sqlx::query("INSERT INTO npm_dist_tags (repo_id, package_name, tag, version) SELECT ?, ?, ?, ? WHERE EXISTS (SELECT 1 FROM npm_versions WHERE repo_id = ? AND package_name = ? AND version = ?) ON CONFLICT(repo_id, package_name, tag) DO UPDATE SET version = excluded.version, updated_at = datetime('now')")
        .bind(repo_id)
        .bind(package_name)
        .bind(tag)
        .bind(version)
        .bind(repo_id)
        .bind(package_name)
        .bind(version)
        .execute(pool)
        .await
        .map_err(|_| ApiError::internal("设置 npm dist-tag 失败"))?;
    Ok(result.rows_affected() == 1)
}

/// 删除 npm dist-tag。
pub async fn delete_npm_dist_tag(
    pool: &SqlitePool,
    repo_id: i64,
    package_name: &str,
    tag: &str,
) -> Result<bool, ApiError> {
    let result =
        sqlx::query("DELETE FROM npm_dist_tags WHERE repo_id = ? AND package_name = ? AND tag = ?")
            .bind(repo_id)
            .bind(package_name)
            .bind(tag)
            .execute(pool)
            .await
            .map_err(|_| ApiError::internal("删除 npm dist-tag 失败"))?;
    Ok(result.rows_affected() == 1)
}

/// 原子替换 npm proxy 的重写 packument 与最小 tarball 上游映射。
pub async fn upsert_npm_proxy_packument(
    pool: &SqlitePool,
    input: &NpmProxyPackumentInput,
) -> Result<(), NpmVersionCommitError> {
    let ttl = i64::try_from(input.ttl_secs.max(1))
        .map_err(|_| npm_internal("npm 缓存 TTL 超出数据库范围"))?;
    let tarballs = serde_json::to_string(&input.tarballs)
        .map_err(|_| npm_internal("序列化 npm tarball 映射失败"))?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| npm_internal("开启 npm packument 缓存事务失败"))?;
    let current: Option<(String,)> = sqlx::query_as(
        "SELECT upstream_url FROM repositories WHERE id = ? AND repo_type = 'proxy'",
    )
    .bind(input.repo_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| npm_internal("核对 npm 上游快照失败"))?;
    if current.as_ref().map(|row| row.0.as_str()) != Some(input.upstream_url_snapshot.as_str()) {
        return Err(NpmVersionCommitError::StaleProxyConfiguration);
    }
    sqlx::query("INSERT INTO npm_proxy_packuments (repo_id, package_name, rewritten_json, upstream_url_snapshot, ttl_secs, fetched_at, expires_at) VALUES (?, ?, ?, ?, ?, datetime('now'), datetime('now', '+' || ? || ' seconds')) ON CONFLICT(repo_id, package_name) DO UPDATE SET rewritten_json = excluded.rewritten_json, upstream_url_snapshot = excluded.upstream_url_snapshot, ttl_secs = excluded.ttl_secs, fetched_at = excluded.fetched_at, expires_at = excluded.expires_at")
        .bind(input.repo_id)
        .bind(&input.package_name)
        .bind(&input.rewritten_json)
        .bind(&input.upstream_url_snapshot)
        .bind(ttl)
        .bind(ttl)
        .execute(&mut *tx)
        .await
        .map_err(|_| npm_internal("写入 npm packument 缓存失败"))?;
    sqlx::query("DELETE FROM npm_proxy_tarballs WHERE repo_id = ? AND package_name = ?")
        .bind(input.repo_id)
        .bind(&input.package_name)
        .execute(&mut *tx)
        .await
        .map_err(|_| npm_internal("清理旧 npm tarball 映射失败"))?;
    sqlx::query("INSERT INTO npm_proxy_tarballs (repo_id, package_name, version, tarball_url, integrity, shasum, artifact_path) SELECT ?, ?, json_extract(value, '$.version'), json_extract(value, '$.tarball_url'), json_extract(value, '$.integrity'), json_extract(value, '$.shasum'), json_extract(value, '$.artifact_path') FROM json_each(?)")
        .bind(input.repo_id)
        .bind(&input.package_name)
        .bind(tarballs)
        .execute(&mut *tx)
        .await
        .map_err(|_| npm_internal("写入 npm tarball 映射失败"))?;
    tx.commit()
        .await
        .map_err(|_| npm_internal("提交 npm packument 缓存事务失败"))?;
    Ok(())
}

/// 读取 npm proxy packument 与 tarball 映射。
pub async fn find_npm_proxy_packument(
    pool: &SqlitePool,
    repo_id: i64,
    package_name: &str,
) -> Result<Option<NpmProxyPackument>, ApiError> {
    let row = sqlx::query_as::<_, NpmProxyPackumentRow>("SELECT rewritten_json, upstream_url_snapshot, ttl_secs, fetched_at, (unixepoch('now') < unixepoch(expires_at)) AS fresh FROM npm_proxy_packuments WHERE repo_id = ? AND package_name = ?")
        .bind(repo_id)
        .bind(package_name)
        .fetch_optional(pool)
        .await
        .map_err(|_| ApiError::internal("查询 npm packument 缓存失败"))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let tarballs = sqlx::query_as::<_, (String, String, Option<String>, Option<String>, Option<String>)>("SELECT version, tarball_url, integrity, shasum, artifact_path FROM npm_proxy_tarballs WHERE repo_id = ? AND package_name = ? ORDER BY version")
        .bind(repo_id)
        .bind(package_name)
        .fetch_all(pool)
        .await
        .map_err(|_| ApiError::internal("查询 npm tarball 映射失败"))?
        .into_iter()
        .map(|row| NpmProxyTarballInput {
            version: row.0,
            tarball_url: row.1,
            integrity: row.2,
            shasum: row.3,
            artifact_path: row.4,
        })
        .collect();
    let ttl_secs =
        u64::try_from(row.ttl_secs).map_err(|_| ApiError::internal("npm 缓存 TTL 非法"))?;
    Ok(Some(NpmProxyPackument {
        rewritten_json: row.rewritten_json,
        upstream_url_snapshot: row.upstream_url_snapshot,
        ttl_secs,
        fetched_at: row.fetched_at,
        fresh: row.fresh,
        tarballs,
    }))
}

/// 按多个仓库与同一包名批量读取 npm proxy packument 与 tarball 映射。
pub async fn find_npm_proxy_packuments(
    pool: &SqlitePool,
    repo_ids: &[i64],
    package_name: &str,
) -> Result<BTreeMap<i64, NpmProxyPackument>, ApiError> {
    if repo_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let payload = repo_id_payload(repo_ids)?;
    let packuments = batch_npm_proxy_packuments(pool, &payload, package_name).await?;
    let tarballs = batch_npm_proxy_tarballs(pool, &payload, package_name).await?;
    let mut result = BTreeMap::new();
    for row in packuments {
        result.insert(row.repo_id, row.try_into_packument()?);
    }
    for row in tarballs {
        if let Some(packument) = result.get_mut(&row.repo_id) {
            packument.tarballs.push(row.into_tarball());
        }
    }
    Ok(result)
}

async fn batch_npm_proxy_packuments(
    pool: &SqlitePool,
    payload: &str,
    package_name: &str,
) -> Result<Vec<BatchNpmProxyPackumentRow>, ApiError> {
    sqlx::query_as("WITH requested(repo_id) AS (SELECT DISTINCT CAST(value AS INTEGER) FROM json_each(?)) SELECT npm_proxy_packuments.repo_id, npm_proxy_packuments.rewritten_json, npm_proxy_packuments.upstream_url_snapshot, npm_proxy_packuments.ttl_secs, npm_proxy_packuments.fetched_at, (unixepoch('now') < unixepoch(npm_proxy_packuments.expires_at)) AS fresh FROM npm_proxy_packuments JOIN requested ON requested.repo_id = npm_proxy_packuments.repo_id WHERE npm_proxy_packuments.package_name = ? ORDER BY npm_proxy_packuments.repo_id")
        .bind(payload)
        .bind(package_name)
        .fetch_all(pool)
        .await
        .map_err(|_| ApiError::internal("批量查询 npm packument 缓存失败"))
}

async fn batch_npm_proxy_tarballs(
    pool: &SqlitePool,
    payload: &str,
    package_name: &str,
) -> Result<Vec<BatchNpmProxyTarballRow>, ApiError> {
    sqlx::query_as("WITH requested(repo_id) AS (SELECT DISTINCT CAST(value AS INTEGER) FROM json_each(?)) SELECT npm_proxy_tarballs.repo_id, npm_proxy_tarballs.version, npm_proxy_tarballs.tarball_url, npm_proxy_tarballs.integrity, npm_proxy_tarballs.shasum, npm_proxy_tarballs.artifact_path FROM npm_proxy_tarballs JOIN requested ON requested.repo_id = npm_proxy_tarballs.repo_id WHERE npm_proxy_tarballs.package_name = ? ORDER BY npm_proxy_tarballs.repo_id, npm_proxy_tarballs.version")
        .bind(payload)
        .bind(package_name)
        .fetch_all(pool)
        .await
        .map_err(|_| ApiError::internal("批量查询 npm tarball 映射失败"))
}

impl BatchNpmProxyPackumentRow {
    fn try_into_packument(self) -> Result<NpmProxyPackument, ApiError> {
        let ttl_secs =
            u64::try_from(self.ttl_secs).map_err(|_| ApiError::internal("npm 缓存 TTL 非法"))?;
        Ok(NpmProxyPackument {
            rewritten_json: self.rewritten_json,
            upstream_url_snapshot: self.upstream_url_snapshot,
            ttl_secs,
            fetched_at: self.fetched_at,
            fresh: self.fresh,
            tarballs: Vec::new(),
        })
    }
}

impl BatchNpmProxyTarballRow {
    fn into_tarball(self) -> NpmProxyTarballInput {
        NpmProxyTarballInput {
            version: self.version,
            tarball_url: self.tarball_url,
            integrity: self.integrity,
            shasum: self.shasum,
            artifact_path: self.artifact_path,
        }
    }
}

/// 原子提交 npm proxy tarball 制品索引并绑定已验证的本地路径。
pub async fn commit_npm_proxy_tarball(
    pool: &SqlitePool,
    input: &ArtifactWriteInput,
    package_name: &str,
    version: &str,
    expected_upstream_url: &str,
) -> Result<ArtifactRecord, NpmVersionCommitError> {
    let size = artifact_size_i64(input.size).map_err(NpmVersionCommitError::Internal)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| npm_internal("开启 npm tarball 缓存事务失败"))?;
    let row = conditional_proxy_upsert(&mut tx, input, size, expected_upstream_url, true)
        .await
        .map_err(|error| match error {
            ArtifactCommitError::StaleProxyConfiguration => {
                NpmVersionCommitError::StaleProxyConfiguration
            }
            ArtifactCommitError::Conflict => NpmVersionCommitError::Conflict,
            ArtifactCommitError::Internal(error) => NpmVersionCommitError::Internal(error),
        })?
        .ok_or(NpmVersionCommitError::StaleProxyConfiguration)?;
    let updated = sqlx::query("UPDATE npm_proxy_tarballs SET artifact_path = ? WHERE repo_id = ? AND package_name = ? AND version = ?")
        .bind(&input.path)
        .bind(input.repo_id)
        .bind(package_name)
        .bind(version)
        .execute(&mut *tx)
        .await
        .map_err(|_| npm_internal("绑定 npm tarball 缓存路径失败"))?;
    if updated.rows_affected() != 1 {
        return Err(npm_internal("npm tarball 映射已变化"));
    }
    tx.commit()
        .await
        .map_err(|_| npm_internal("提交 npm tarball 缓存事务失败"))?;
    ArtifactRecord::try_from(row).map_err(NpmVersionCommitError::Internal)
}

/// 删除仓库内全部缓存索引，并在同一事务批量登记回收意图。
pub async fn delete_cached_artifacts_by_repo(
    pool: &SqlitePool,
    repo_id: i64,
) -> Result<Vec<BlobReclamation>, ApiError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ApiError::internal("开启缓存制品删除事务失败"))?;
    let reclamations = register_repository_reclamations(&mut tx, repo_id, true).await?;
    sqlx::query("DELETE FROM artifacts WHERE repo_id = ? AND cached = 1")
        .bind(repo_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ApiError::internal("清理仓库缓存制品失败"))?;
    tx.commit()
        .await
        .map_err(|_| ApiError::internal("提交缓存制品删除事务失败"))?;
    Ok(reclamations)
}

fn unique_hashes(mut rows: Vec<(String,)>) -> Vec<String> {
    rows.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    rows.dedup_by(|left, right| left.0 == right.0);
    rows.into_iter().map(|row| row.0).collect()
}

/// 分页列出目录的直接子项，目录由后续路径段实时聚合而成。
pub async fn list_artifact_directory(
    pool: &SqlitePool,
    repo_id: i64,
    prefix: &str,
    page: u32,
    page_size: u32,
) -> Result<(Vec<ArtifactDirectoryEntry>, i64), ApiError> {
    let pattern = directory_pattern(prefix);
    let start = i64::try_from(prefix.chars().count() + usize::from(!prefix.is_empty()) + 1)
        .map_err(|_| ApiError::bad_request("ARTIFACT_INVALID_PATH", "制品目录路径无效"))?;
    let offset = i64::from(page.saturating_sub(1)) * i64::from(page_size);
    let sql = directory_entries_sql();
    let total: (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM ({sql})"))
        .bind(start)
        .bind(repo_id)
        .bind(&pattern)
        .fetch_one(pool)
        .await
        .map_err(|_| ApiError::internal("统计制品目录失败"))?;
    let rows = sqlx::query_as::<_, ArtifactDirectoryRow>(&format!(
        "SELECT name, is_directory, size, sha256, sha1, md5, sha512, updated_at FROM ({sql}) ORDER BY is_directory DESC, name LIMIT ? OFFSET ?"
    ))
    .bind(start)
    .bind(repo_id)
    .bind(pattern)
    .bind(i64::from(page_size))
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(|_| ApiError::internal("查询制品目录失败"))?;
    let entries = rows
        .into_iter()
        .map(|row| directory_entry(prefix, row))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((entries, total.0))
}

fn directory_entries_sql() -> &'static str {
    "WITH descendants AS (SELECT substr(path, ?) AS rest, size, sha256, sha1, md5, sha512, updated_at FROM artifacts WHERE repo_id = ? AND path LIKE ? ESCAPE '\\'), normalized AS (SELECT CASE WHEN instr(rest, '/') = 0 THEN rest ELSE substr(rest, 1, instr(rest, '/') - 1) END AS name, CASE WHEN instr(rest, '/') = 0 THEN 0 ELSE 1 END AS is_directory, size, sha256, sha1, md5, sha512, updated_at FROM descendants) SELECT name, MAX(is_directory) AS is_directory, CASE WHEN MAX(is_directory) = 1 THEN NULL ELSE MAX(size) END AS size, CASE WHEN MAX(is_directory) = 1 THEN NULL ELSE MAX(sha256) END AS sha256, CASE WHEN MAX(is_directory) = 1 THEN NULL ELSE MAX(sha1) END AS sha1, CASE WHEN MAX(is_directory) = 1 THEN NULL ELSE MAX(md5) END AS md5, CASE WHEN MAX(is_directory) = 1 THEN NULL ELSE MAX(sha512) END AS sha512, MAX(updated_at) AS updated_at FROM normalized WHERE name <> '' GROUP BY name"
}

fn directory_pattern(prefix: &str) -> String {
    if prefix.is_empty() {
        "%".to_string()
    } else {
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        format!("{escaped}/%")
    }
}

/// 分页搜索仓库内路径子串，返回完整制品记录。
pub async fn search_artifacts(
    pool: &SqlitePool,
    repo_id: i64,
    query: &str,
    page: u32,
    page_size: u32,
) -> Result<(Vec<ArtifactRecord>, i64), ApiError> {
    let pattern = like_pattern(Some(query));
    let (limit, offset) = page_bounds(page, page_size);
    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM artifacts WHERE repo_id = ? AND path LIKE ? ESCAPE '\\'",
    )
    .bind(repo_id)
    .bind(&pattern)
    .fetch_one(pool)
    .await
    .map_err(|_| ApiError::internal("统计制品搜索结果失败"))?;
    let sql = format!(
        "SELECT {ARTIFACT_COLUMNS} FROM artifacts WHERE repo_id = ? AND path LIKE ? ESCAPE '\\' ORDER BY path LIMIT ? OFFSET ?"
    );
    let rows = sqlx::query_as::<_, ArtifactRow>(&sql)
        .bind(repo_id)
        .bind(pattern)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(|_| ApiError::internal("搜索制品失败"))?;
    let artifacts = rows
        .into_iter()
        .map(ArtifactRecord::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((artifacts, total.0))
}

fn directory_entry(
    prefix: &str,
    row: ArtifactDirectoryRow,
) -> Result<ArtifactDirectoryEntry, ApiError> {
    let size = row
        .size
        .map(|value| u64::try_from(value).map_err(|_| ApiError::internal("制品大小非法")))
        .transpose()?;
    let path = if prefix.is_empty() {
        row.name.clone()
    } else {
        format!("{prefix}/{}", row.name)
    };
    Ok(ArtifactDirectoryEntry {
        name: row.name,
        path,
        is_directory: row.is_directory,
        size,
        sha256: row.sha256,
        sha1: row.sha1,
        md5: row.md5,
        sha512: row.sha512,
        updated_at: row.updated_at,
    })
}

/// 用户读取列（统一列表，避免多处重复书写并保证 FromRow 列名一致）。
const USER_COLUMNS: &str = "id, username, role, enabled, created_at, token_version";

/// 用户实体（仓储读出，领域与接口层消费；不含口令哈希）。
#[derive(Debug)]
pub struct User {
    /// 用户标识。
    pub id: i64,
    /// 用户名。
    pub username: String,
    /// 全局角色。
    pub role: Role,
    /// 是否启用。
    pub enabled: bool,
    /// 创建时间（SQLite datetime 文本）。
    pub created_at: String,
    /// 会话版本号。
    pub token_version: i64,
}

/// 数据库原始用户行：role 为文本，转换为领域 Role 后对外暴露 User。
#[derive(sqlx::FromRow)]
struct UserRow {
    id: i64,
    username: String,
    role: String,
    enabled: bool,
    created_at: String,
    token_version: i64,
}

impl UserRow {
    /// 将原始行转换为领域 User；角色文本非法时归为内部错误。
    fn into_user(self) -> Result<User, ApiError> {
        let role =
            Role::from_db_str(&self.role).ok_or_else(|| ApiError::internal("用户角色值非法"))?;
        Ok(User {
            id: self.id,
            username: self.username,
            role,
            enabled: self.enabled,
            created_at: self.created_at,
            token_version: self.token_version,
        })
    }
}

/// API Token 元数据行（列表返回，永不含明文或哈希）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TokenRow {
    /// Token 标识。
    pub id: i64,
    /// Token 名称。
    pub name: String,
    /// 创建时间。
    pub created_at: String,
}

/// 分页边界：page 从 1、page_size 下限 1 上限 200（NFR-16），返回 (limit, offset)。
fn page_bounds(page: u32, page_size: u32) -> (i64, i64) {
    let page = page.max(1);
    let size = page_size.clamp(1, 200);
    let offset = page.saturating_sub(1).saturating_mul(size);
    (i64::from(size), i64::from(offset))
}

/// 构造 LIKE 子串匹配模式，转义 `\` `%` `_` 通配符；q 为空时以 `%` 匹配全部。
fn like_pattern(q: Option<&str>) -> String {
    match q {
        Some(s) if !s.is_empty() => {
            let escaped = s
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            format!("%{escaped}%")
        }
        _ => "%".to_string(),
    }
}

// ==================== 用户仓储 ====================

/// 按用户名查用户；不存在返回 None。
pub async fn find_user_by_username(
    pool: &SqlitePool,
    username: &str,
) -> Result<Option<User>, ApiError> {
    let row = sqlx::query_as::<_, UserRow>(&format!(
        "SELECT {USER_COLUMNS} FROM users WHERE username = ?"
    ))
    .bind(username)
    .fetch_optional(pool)
    .await
    .map_err(|_| ApiError::internal("查询用户失败"))?;
    row.map(UserRow::into_user).transpose()
}

/// 按 id 查用户；不存在返回 None。
pub async fn find_user_by_id(pool: &SqlitePool, id: i64) -> Result<Option<User>, ApiError> {
    let row =
        sqlx::query_as::<_, UserRow>(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"))
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(|_| ApiError::internal("查询用户失败"))?;
    row.map(UserRow::into_user).transpose()
}

/// 插入用户并回读完整行；用户名唯一约束冲突映射为稳定的 409 业务错误。
pub async fn insert_user(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
    role: Role,
    enabled: bool,
) -> Result<User, ApiError> {
    let row = sqlx::query_as::<_, UserRow>(&format!(
        "INSERT INTO users (username, password_hash, role, enabled) VALUES (?, ?, ?, ?) RETURNING {USER_COLUMNS}"
    ))
    .bind(username)
    .bind(password_hash)
    .bind(role.as_db_str())
    .bind(enabled)
    .fetch_one(pool)
    .await
    .map_err(|error| match error {
        sqlx::Error::Database(database_error) if database_error.is_unique_violation() => {
            ApiError::conflict("AUTH_USERNAME_TAKEN", "用户名已存在")
        }
        _ => ApiError::internal("创建用户失败"),
    })?;
    row.into_user()
}

/// 按给出的字段部分更新用户；改口令或禁用时原子递增 token_version，禁用同时永久吊销 API Token。
/// 全部为 None 时不写库。
pub async fn update_user_fields(
    pool: &SqlitePool,
    id: i64,
    enabled: Option<bool>,
    role: Option<Role>,
    password_hash: Option<String>,
) -> Result<(), ApiError> {
    if enabled.is_none() && role.is_none() && password_hash.is_none() {
        return Ok(());
    }
    let role_str = role.map(|r| r.as_db_str().to_string());
    let reset_sessions = password_hash.is_some() || enabled == Some(false);
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ApiError::internal("开启用户更新事务失败"))?;
    sqlx::query(
        "UPDATE users SET \
         enabled = COALESCE(?, enabled), \
         role = COALESCE(?, role), \
         password_hash = COALESCE(?, password_hash), \
         token_version = token_version + (CASE WHEN ? THEN 1 ELSE 0 END) \
         WHERE id = ?",
    )
    .bind(enabled)
    .bind(role_str)
    .bind(password_hash.as_deref())
    .bind(reset_sessions)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApiError::internal("更新用户失败"))?;
    if enabled == Some(false) {
        sqlx::query("DELETE FROM api_tokens WHERE user_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal("吊销用户 Token 失败"))?;
    }
    tx.commit()
        .await
        .map_err(|_| ApiError::internal("提交用户更新失败"))?;
    Ok(())
}

/// 条件更新用户，并由同一事务保护最后一名启用管理员；改口令或禁用会推进会话版本，禁用永久吊销 API Token。
pub async fn update_user_preserving_admin(
    pool: &SqlitePool,
    id: i64,
    enabled: Option<bool>,
    role: Option<Role>,
    password_hash: Option<String>,
) -> Result<bool, ApiError> {
    if enabled.is_none() && role.is_none() && password_hash.is_none() {
        return Ok(true);
    }
    let role_str = role.map(|value| value.as_db_str().to_string());
    let reset_sessions = password_hash.is_some() || enabled == Some(false);
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ApiError::internal("开启用户更新事务失败"))?;
    let result = sqlx::query(
        "UPDATE users SET \
         enabled = COALESCE(?, enabled), \
         role = COALESCE(?, role), \
         password_hash = COALESCE(?, password_hash), \
         token_version = token_version + (CASE WHEN ? THEN 1 ELSE 0 END) \
         WHERE id = ? AND (\
           role <> 'admin' OR enabled <> 1 \
           OR (COALESCE(?, role) = 'admin' AND COALESCE(?, enabled) = 1) \
           OR EXISTS (\
             SELECT 1 FROM users AS other \
             WHERE other.id <> users.id AND other.role = 'admin' AND other.enabled = 1\
           )\
         )",
    )
    .bind(enabled)
    .bind(role_str.as_deref())
    .bind(password_hash.as_deref())
    .bind(reset_sessions)
    .bind(id)
    .bind(role_str.as_deref())
    .bind(enabled)
    .execute(&mut *tx)
    .await
    .map_err(|_| ApiError::internal("更新用户失败"))?;
    let updated = result.rows_affected() == 1;
    if updated && enabled == Some(false) {
        sqlx::query("DELETE FROM api_tokens WHERE user_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|_| ApiError::internal("吊销用户 Token 失败"))?;
    }
    tx.commit()
        .await
        .map_err(|_| ApiError::internal("提交用户更新失败"))?;
    Ok(updated)
}

/// 仅当用户表仍为空时插入首名管理员；插入成功返回 true。
pub async fn insert_admin_if_users_empty(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
) -> Result<bool, ApiError> {
    let result = sqlx::query(
        "INSERT INTO users (username, password_hash, role, enabled) \
         SELECT ?, ?, 'admin', 1 WHERE NOT EXISTS (SELECT 1 FROM users)",
    )
    .bind(username)
    .bind(password_hash)
    .execute(pool)
    .await
    .map_err(|_| ApiError::internal("创建管理员失败"))?;
    Ok(result.rows_affected() == 1)
}

/// 分页列出用户，可选按用户名子串（大小写不敏感）筛选，稳定按 id 排序。返回 (当前页, 筛选后总数)。
pub async fn list_users(
    pool: &SqlitePool,
    page: u32,
    page_size: u32,
    q: Option<&str>,
) -> Result<(Vec<User>, i64), ApiError> {
    let (limit, offset) = page_bounds(page, page_size);
    let pattern = like_pattern(q);
    let total: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM users WHERE username LIKE ? ESCAPE '\\'")
            .bind(&pattern)
            .fetch_one(pool)
            .await
            .map_err(|_| ApiError::internal("统计用户失败"))?;
    let rows = sqlx::query_as::<_, UserRow>(&format!(
        "SELECT {USER_COLUMNS} FROM users WHERE username LIKE ? ESCAPE '\\' ORDER BY id LIMIT ? OFFSET ?"
    ))
    .bind(&pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(|_| ApiError::internal("查询用户列表失败"))?;
    let users = rows
        .into_iter()
        .map(UserRow::into_user)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((users, total.0))
}

/// 用户总数。
pub async fn count_users(pool: &SqlitePool) -> Result<i64, ApiError> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await
        .map_err(|_| ApiError::internal("统计用户失败"))?;
    Ok(row.0)
}

/// 当前启用的管理员数量（最后一名启用 Admin 保护据此判定）。
pub async fn count_enabled_admins(pool: &SqlitePool) -> Result<i64, ApiError> {
    let row: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM users WHERE role = 'admin' AND enabled = 1")
            .fetch_one(pool)
            .await
            .map_err(|_| ApiError::internal("统计管理员失败"))?;
    Ok(row.0)
}

/// 递增用户会话版本号（使其所有既有会话失配失效）。
pub async fn increment_token_version(pool: &SqlitePool, id: i64) -> Result<(), ApiError> {
    sqlx::query("UPDATE users SET token_version = token_version + 1 WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|_| ApiError::internal("更新会话版本失败"))?;
    Ok(())
}

/// 登录用：按用户名读出 (id, 口令哈希, 是否启用, 角色, 会话版本)；不存在返回 None。
pub async fn load_password_and_version_by_username(
    pool: &SqlitePool,
    username: &str,
) -> Result<Option<(i64, String, bool, Role, i64)>, ApiError> {
    let row: Option<(i64, String, bool, String, i64)> = sqlx::query_as(
        "SELECT id, password_hash, enabled, role, token_version FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
    .map_err(|_| ApiError::internal("查询登录凭据失败"))?;
    match row {
        Some((id, hash, enabled, role_str, ver)) => {
            let role =
                Role::from_db_str(&role_str).ok_or_else(|| ApiError::internal("用户角色值非法"))?;
            Ok(Some((id, hash, enabled, role, ver)))
        }
        None => Ok(None),
    }
}

/// 改口令用：按 id 读出当前口令哈希与会话版本；不存在返回 None。
pub async fn load_password_and_version_by_id(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<(String, i64)>, ApiError> {
    sqlx::query_as("SELECT password_hash, token_version FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|_| ApiError::internal("查询口令凭据失败"))
}

/// 仅当旧口令哈希与会话版本均未变化时更新口令并推进版本，防止覆盖并发管理员重置。
pub async fn compare_and_update_password(
    pool: &SqlitePool,
    id: i64,
    expected_hash: &str,
    expected_version: i64,
    new_hash: &str,
) -> Result<bool, ApiError> {
    let result = sqlx::query(
        "UPDATE users SET password_hash = ?, token_version = token_version + 1 \
         WHERE id = ? AND enabled = 1 AND password_hash = ? AND token_version = ?",
    )
    .bind(new_hash)
    .bind(id)
    .bind(expected_hash)
    .bind(expected_version)
    .execute(pool)
    .await
    .map_err(|_| ApiError::internal("更新口令失败"))?;
    Ok(result.rows_affected() == 1)
}

/// 本地管理员 CLI 按用户名单条更新口令并推进会话版本；未命中返回 false。
pub async fn reset_password_by_username(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
) -> Result<bool, ApiError> {
    let result = sqlx::query(
        "UPDATE users SET password_hash = ?, token_version = token_version + 1 WHERE username = ?",
    )
    .bind(password_hash)
    .bind(username)
    .execute(pool)
    .await
    .map_err(|_| ApiError::internal("本地重置口令失败"))?;
    Ok(result.rows_affected() == 1)
}

/// 测试与只读校验用：按 id 读出口令哈希；不存在返回 None。
pub async fn load_password_by_id(pool: &SqlitePool, id: i64) -> Result<Option<String>, ApiError> {
    Ok(load_password_and_version_by_id(pool, id)
        .await?
        .map(|(hash, _)| hash))
}

// ==================== API Token 仓储 ====================

/// 插入 Token 并回读 (id, created_at)。持久化只存哈希，不存明文。
pub async fn insert_token(
    pool: &SqlitePool,
    user_id: i64,
    name: &str,
    token_hash: &str,
) -> Result<(i64, String), ApiError> {
    let row: (i64, String) = sqlx::query_as(
        "INSERT INTO api_tokens (user_id, name, token_hash) VALUES (?, ?, ?) RETURNING id, created_at",
    )
    .bind(user_id)
    .bind(name)
    .bind(token_hash)
    .fetch_one(pool)
    .await
    .map_err(|_| ApiError::internal("创建 Token 失败"))?;
    Ok(row)
}

/// 按 token_hash 查归属用户 id（Bearer/Basic 中 Token 校验用）；未命中返回 None。
pub async fn find_token_owner_by_hash(
    pool: &SqlitePool,
    token_hash: &str,
) -> Result<Option<i64>, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT user_id FROM api_tokens WHERE token_hash = ?")
        .bind(token_hash)
        .fetch_optional(pool)
        .await
        .map_err(|_| ApiError::internal("查询 Token 失败"))?;
    Ok(row.map(|r| r.0))
}

/// 分页列出某用户的 Token 元数据，可选按名称子串筛选，稳定按 id 排序。返回 (当前页, 总数)。
pub async fn list_tokens_by_user(
    pool: &SqlitePool,
    user_id: i64,
    page: u32,
    page_size: u32,
    q: Option<&str>,
) -> Result<(Vec<TokenRow>, i64), ApiError> {
    let (limit, offset) = page_bounds(page, page_size);
    let pattern = like_pattern(q);
    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM api_tokens WHERE user_id = ? AND name LIKE ? ESCAPE '\\'",
    )
    .bind(user_id)
    .bind(&pattern)
    .fetch_one(pool)
    .await
    .map_err(|_| ApiError::internal("统计 Token 失败"))?;
    let rows = sqlx::query_as::<_, TokenRow>(
        "SELECT id, name, created_at FROM api_tokens WHERE user_id = ? AND name LIKE ? ESCAPE '\\' ORDER BY id LIMIT ? OFFSET ?",
    )
    .bind(user_id)
    .bind(&pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(|_| ApiError::internal("查询 Token 列表失败"))?;
    Ok((rows, total.0))
}

/// 删除本人 Token（id 与 user_id 双重限定，防越权删除他人 Token）；命中返回 true。
pub async fn delete_token(pool: &SqlitePool, id: i64, user_id: i64) -> Result<bool, ApiError> {
    let result = sqlx::query("DELETE FROM api_tokens WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|_| ApiError::internal("删除 Token 失败"))?;
    Ok(result.rows_affected() > 0)
}

// ==================== 会话撤销仓储 ====================

/// 写入会话撤销记录（幂等：已存在则忽略）。expires_at 传 Unix 秒，落库为可比较的 datetime 文本。
pub async fn insert_revoked(
    pool: &SqlitePool,
    jti_hash: &str,
    expires_at_unix: i64,
) -> Result<(), ApiError> {
    sqlx::query("INSERT OR IGNORE INTO revoked_sessions (jti_hash, expires_at) VALUES (?, datetime(?, 'unixepoch'))")
        .bind(jti_hash)
        .bind(expires_at_unix)
        .execute(pool)
        .await
        .map_err(|_| ApiError::internal("写入会话撤销记录失败"))?;
    Ok(())
}

/// 刷新会话时条件撤销旧 jti：仅当用户仍启用、会话版本仍等于声明版本且 jti 尚未撤销时插入。
/// 条件检查与插入由 SQLite 单条语句原子完成；返回 true 表示本次请求唯一取得刷新资格。
pub async fn revoke_session_for_refresh(
    pool: &SqlitePool,
    user_id: i64,
    token_version: i64,
    jti_hash: &str,
    family_hash: &str,
    expires_at_unix: i64,
) -> Result<bool, ApiError> {
    let result = sqlx::query(
        "INSERT OR IGNORE INTO revoked_sessions (jti_hash, expires_at) \
         SELECT ?, datetime(?, 'unixepoch') FROM users \
         WHERE id = ? AND enabled = 1 AND token_version = ? \
         AND NOT EXISTS (SELECT 1 FROM revoked_sessions WHERE jti_hash IN (?, ?))",
    )
    .bind(jti_hash)
    .bind(expires_at_unix)
    .bind(user_id)
    .bind(token_version)
    .bind(jti_hash)
    .bind(family_hash)
    .execute(pool)
    .await
    .map_err(|_| ApiError::internal("刷新会话撤销失败"))?;
    Ok(result.rows_affected() == 1)
}

/// 查询会话是否已被撤销。
pub async fn is_revoked(pool: &SqlitePool, jti_hash: &str) -> Result<bool, ApiError> {
    let row: (bool,) =
        sqlx::query_as("SELECT EXISTS(SELECT 1 FROM revoked_sessions WHERE jti_hash = ?)")
            .bind(jti_hash)
            .fetch_one(pool)
            .await
            .map_err(|_| ApiError::internal("查询会话撤销状态失败"))?;
    Ok(row.0)
}

/// 清理已过期的撤销记录（now 传 Unix 秒）；返回删除行数。
pub async fn cleanup_expired_revoked(pool: &SqlitePool, now_unix: i64) -> Result<u64, ApiError> {
    let result =
        sqlx::query("DELETE FROM revoked_sessions WHERE expires_at < datetime(?, 'unixepoch')")
            .bind(now_unix)
            .execute(pool)
            .await
            .map_err(|_| ApiError::internal("清理过期撤销记录失败"))?;
    Ok(result.rows_affected())
}

// ==================== 仓库核心仓储 ====================

/// 仓库读取列，制品统计由同一聚合子查询提供。
const REPOSITORY_COLUMNS: &str = "repositories.id, repositories.name, repositories.format, repositories.repo_type, repositories.visibility, repositories.disabled, repositories.overwrite_policy, repositories.version_policy, repositories.upstream_url, repositories.upstream_credential_ref, COALESCE(artifact_stats.object_count, 0) AS object_count, COALESCE(artifact_stats.total_bytes, 0) AS total_bytes, repositories.created_at";
const REPOSITORY_FROM: &str = "repositories LEFT JOIN (SELECT repo_id, COUNT(*) AS object_count, SUM(size) AS total_bytes FROM artifacts GROUP BY repo_id) AS artifact_stats ON artifact_stats.repo_id = repositories.id";

/// 待写入的仓库配置。
pub(crate) struct NewRepositoryRow {
    pub name: String,
    pub format: Format,
    pub repo_type: RepoType,
    pub visibility: Visibility,
    pub overwrite_policy: Option<OverwritePolicy>,
    pub version_policy: Option<VersionPolicy>,
    pub upstream_url: Option<String>,
    pub upstream_credential_ref: Option<String>,
}

/// 仓库部分更新字段。
#[derive(Default)]
pub(crate) struct RepositoryRowPatch {
    pub visibility: Option<Visibility>,
    pub disabled: Option<bool>,
    pub overwrite_policy: Option<OverwritePolicy>,
    pub version_policy: Option<VersionPolicy>,
    pub upstream_url: Option<String>,
    pub upstream_credential_ref: Option<Option<String>>,
}

/// group 成员候选的不可变属性。
pub(crate) struct MemberCandidate {
    pub id: i64,
    pub name: String,
    pub format: Format,
    pub repo_type: RepoType,
}

/// 条件删除结果。
pub(crate) enum RepositoryDeleteOutcome {
    Deleted {
        hashes: Vec<String>,
        reclamations: Vec<BlobReclamation>,
    },
    InUse,
}

#[derive(sqlx::FromRow)]
struct RepositoryRow {
    id: i64,
    name: String,
    format: String,
    repo_type: String,
    visibility: String,
    disabled: bool,
    overwrite_policy: Option<String>,
    version_policy: Option<String>,
    upstream_url: Option<String>,
    upstream_credential_ref: Option<String>,
    object_count: i64,
    total_bytes: i64,
    created_at: String,
}

impl RepositoryRow {
    fn into_repository(self, members: Vec<String>) -> Result<Repository, ApiError> {
        Ok(Repository {
            id: self.id,
            name: self.name,
            format: parse_format(&self.format)?,
            repo_type: parse_repo_type(&self.repo_type)?,
            visibility: parse_visibility(&self.visibility)?,
            disabled: self.disabled,
            overwrite_policy: self
                .overwrite_policy
                .as_deref()
                .map(parse_overwrite)
                .transpose()?,
            version_policy: self
                .version_policy
                .as_deref()
                .map(parse_version)
                .transpose()?,
            upstream_url: self.upstream_url,
            upstream_credential_ref: self.upstream_credential_ref,
            members,
            object_count: self.object_count,
            total_bytes: self.total_bytes,
            created_at: self.created_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct RepositoryAccessRow {
    id: i64,
    name: String,
    format: String,
    repo_type: String,
    visibility: String,
    disabled: bool,
    overwrite_policy: Option<String>,
    version_policy: Option<String>,
    upstream_url: Option<String>,
    upstream_credential_ref: Option<String>,
    object_count: i64,
    total_bytes: i64,
    created_at: String,
    acl_permission: Option<String>,
}

impl RepositoryAccessRow {
    fn split(self, members: Vec<String>) -> Result<(Repository, Option<Permission>), ApiError> {
        let permission = self
            .acl_permission
            .as_deref()
            .map(parse_permission)
            .transpose()?;
        let repository = RepositoryRow {
            id: self.id,
            name: self.name,
            format: self.format,
            repo_type: self.repo_type,
            visibility: self.visibility,
            disabled: self.disabled,
            overwrite_policy: self.overwrite_policy,
            version_policy: self.version_policy,
            upstream_url: self.upstream_url,
            upstream_credential_ref: self.upstream_credential_ref,
            object_count: self.object_count,
            total_bytes: self.total_bytes,
            created_at: self.created_at,
        }
        .into_repository(members)?;
        Ok((repository, permission))
    }
}

#[derive(sqlx::FromRow)]
struct GroupMemberNameRow {
    group_repo_id: i64,
    name: String,
}

#[derive(sqlx::FromRow)]
struct MemberCandidateRow {
    id: i64,
    name: String,
    format: String,
    repo_type: String,
}

fn parse_format(value: &str) -> Result<Format, ApiError> {
    Format::from_db_str(value).ok_or_else(|| ApiError::internal("仓库格式值非法"))
}

fn parse_repo_type(value: &str) -> Result<RepoType, ApiError> {
    RepoType::from_db_str(value).ok_or_else(|| ApiError::internal("仓库类型值非法"))
}

fn parse_visibility(value: &str) -> Result<Visibility, ApiError> {
    Visibility::from_db_str(value).ok_or_else(|| ApiError::internal("仓库可见性值非法"))
}

fn parse_permission(value: &str) -> Result<Permission, ApiError> {
    Permission::from_db_str(value).ok_or_else(|| ApiError::internal("仓库权限值非法"))
}

fn parse_overwrite(value: &str) -> Result<OverwritePolicy, ApiError> {
    OverwritePolicy::from_db_str(value).ok_or_else(|| ApiError::internal("仓库覆盖策略值非法"))
}

fn parse_version(value: &str) -> Result<VersionPolicy, ApiError> {
    VersionPolicy::from_db_str(value).ok_or_else(|| ApiError::internal("仓库版本策略值非法"))
}

/// 创建仓库，并在同一事务中写入 group 有序成员关系。
pub(crate) async fn insert_repository(
    pool: &SqlitePool,
    row: &NewRepositoryRow,
    member_ids: &[i64],
) -> Result<Repository, ApiError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ApiError::internal("开启仓库创建事务失败"))?;
    let inserted: (i64,) = sqlx::query_as(
        "INSERT INTO repositories (name, format, repo_type, visibility, overwrite_policy, version_policy, upstream_url, upstream_credential_ref) VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&row.name)
    .bind(row.format.as_db_str())
    .bind(row.repo_type.as_db_str())
    .bind(row.visibility.as_db_str())
    .bind(row.overwrite_policy.map(OverwritePolicy::as_db_str))
    .bind(row.version_policy.map(VersionPolicy::as_db_str))
    .bind(row.upstream_url.as_deref())
    .bind(row.upstream_credential_ref.as_deref())
    .fetch_one(&mut *tx)
    .await
    .map_err(map_repository_insert_error)?;
    insert_group_members(&mut tx, inserted.0, member_ids).await?;
    tx.commit()
        .await
        .map_err(|_| ApiError::internal("提交仓库创建事务失败"))?;
    find_repository_by_id(pool, inserted.0)
        .await?
        .ok_or_else(|| ApiError::internal("创建后未能读回仓库"))
}

fn map_repository_insert_error(error: sqlx::Error) -> ApiError {
    match error {
        sqlx::Error::Database(database_error) if database_error.is_unique_violation() => {
            ApiError::conflict("REPO_NAME_TAKEN", "仓库名已存在")
        }
        _ => ApiError::internal("创建仓库失败"),
    }
}

async fn insert_group_members(
    tx: &mut Transaction<'_, Sqlite>,
    group_repo_id: i64,
    member_ids: &[i64],
) -> Result<(), ApiError> {
    if member_ids.is_empty() {
        return Ok(());
    }
    let mut query = QueryBuilder::<Sqlite>::new(
        "INSERT INTO repository_group_members (group_repo_id, member_repo_id, position) ",
    );
    query.push_values(
        member_ids.iter().enumerate(),
        |mut values, (position, member_id)| {
            values
                .push_bind(group_repo_id)
                .push_bind(*member_id)
                .push_bind(position as i64);
        },
    );
    query
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|_| ApiError::internal("写入 group 成员失败"))?;
    Ok(())
}

/// 按仓库名读取元数据与指定用户的 ACL。
pub(crate) async fn find_repository_access(
    pool: &SqlitePool,
    name: &str,
    user_id: Option<i64>,
    global_admin: bool,
) -> Result<Option<(Repository, Option<Permission>)>, ApiError> {
    let sql = format!(
        "SELECT {REPOSITORY_COLUMNS}, (SELECT permission FROM repository_acl WHERE repo_id = repositories.id AND user_id = ?) AS acl_permission FROM {REPOSITORY_FROM} WHERE repositories.name = ?"
    );
    let row = sqlx::query_as::<_, RepositoryAccessRow>(&sql)
        .bind(user_id)
        .bind(name)
        .fetch_optional(pool)
        .await
        .map_err(|_| ApiError::internal("查询仓库失败"))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let members = load_member_names(pool, &[row.id], user_id, global_admin)
        .await?
        .remove(&row.id)
        .unwrap_or_default();
    row.split(members).map(Some)
}

async fn find_repository_by_id(
    pool: &SqlitePool,
    repository_id: i64,
) -> Result<Option<Repository>, ApiError> {
    let sql =
        format!("SELECT {REPOSITORY_COLUMNS} FROM {REPOSITORY_FROM} WHERE repositories.id = ?");
    let row = sqlx::query_as::<_, RepositoryRow>(&sql)
        .bind(repository_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| ApiError::internal("查询仓库失败"))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let members = load_member_names(pool, &[repository_id], None, true)
        .await?
        .remove(&repository_id)
        .unwrap_or_default();
    row.into_repository(members).map(Some)
}

/// 在 SQL 中完成权限过滤、字面查询、总数统计与稳定分页，并批量加载当前页 group 成员。
pub(crate) async fn list_repositories(
    pool: &SqlitePool,
    user_id: Option<i64>,
    global_admin: bool,
    page: u32,
    page_size: u32,
    q: Option<&str>,
) -> Result<(Vec<Repository>, i64), ApiError> {
    let (limit, offset) = page_bounds(page, page_size);
    let pattern = like_pattern(q);
    let filter = "repositories.name LIKE ? ESCAPE '\\' AND (? OR repositories.visibility = 'public' OR (? IS NOT NULL AND EXISTS (SELECT 1 FROM repository_acl WHERE repo_id = repositories.id AND user_id = ?)))";
    let total_sql = format!("SELECT COUNT(*) FROM repositories WHERE {filter}");
    let total: (i64,) = sqlx::query_as(&total_sql)
        .bind(&pattern)
        .bind(global_admin)
        .bind(user_id)
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(|_| ApiError::internal("统计仓库失败"))?;
    let list_sql = format!(
        "SELECT {REPOSITORY_COLUMNS} FROM {REPOSITORY_FROM} WHERE {filter} ORDER BY repositories.name, repositories.id LIMIT ? OFFSET ?"
    );
    let rows = sqlx::query_as::<_, RepositoryRow>(&list_sql)
        .bind(&pattern)
        .bind(global_admin)
        .bind(user_id)
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(|_| ApiError::internal("查询仓库列表失败"))?;
    attach_member_names(pool, rows, user_id, global_admin)
        .await
        .map(|items| (items, total.0))
}

async fn attach_member_names(
    pool: &SqlitePool,
    rows: Vec<RepositoryRow>,
    user_id: Option<i64>,
    global_admin: bool,
) -> Result<Vec<Repository>, ApiError> {
    let group_ids: Vec<i64> = rows
        .iter()
        .filter(|row| row.repo_type == "group")
        .map(|row| row.id)
        .collect();
    let mut members = load_member_names(pool, &group_ids, user_id, global_admin).await?;
    rows.into_iter()
        .map(|row| {
            let names = members.remove(&row.id).unwrap_or_default();
            row.into_repository(names)
        })
        .collect()
}

fn append_readable_repository_filter(
    query: &mut QueryBuilder<'_, Sqlite>,
    user_id: Option<i64>,
    global_admin: bool,
) {
    query
        .push(" AND (")
        .push_bind(global_admin)
        .push(" OR repositories.visibility = 'public' OR (")
        .push_bind(user_id)
        .push(" IS NOT NULL AND EXISTS (SELECT 1 FROM repository_acl WHERE repo_id = repositories.id AND user_id = ")
        .push_bind(user_id)
        .push(")))");
}

async fn load_member_names(
    pool: &SqlitePool,
    group_ids: &[i64],
    user_id: Option<i64>,
    global_admin: bool,
) -> Result<BTreeMap<i64, Vec<String>>, ApiError> {
    if group_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT repository_group_members.group_repo_id, repositories.name FROM repository_group_members JOIN repositories ON repositories.id = repository_group_members.member_repo_id WHERE repository_group_members.group_repo_id IN (",
    );
    {
        let mut separated = query.separated(", ");
        for id in group_ids {
            separated.push_bind(*id);
        }
    }
    query.push(")");
    append_readable_repository_filter(&mut query, user_id, global_admin);
    query.push(
        " ORDER BY repository_group_members.group_repo_id, repository_group_members.position",
    );
    let rows = query
        .build_query_as::<GroupMemberNameRow>()
        .fetch_all(pool)
        .await
        .map_err(|_| ApiError::internal("查询 group 成员失败"))?;
    let mut grouped = BTreeMap::<i64, Vec<String>>::new();
    for row in rows {
        grouped.entry(row.group_repo_id).or_default().push(row.name);
    }
    Ok(grouped)
}

/// 单条 SQL 读取 group 有序成员及调用方 ACL，供协议面统一过滤。
pub(crate) async fn find_protocol_group_member_access(
    pool: &SqlitePool,
    group_repo_id: i64,
    user_id: Option<i64>,
) -> Result<Vec<(Repository, Option<Permission>)>, ApiError> {
    let sql = format!(
        "SELECT {REPOSITORY_COLUMNS}, (SELECT permission FROM repository_acl WHERE repo_id = repositories.id AND user_id = ?) AS acl_permission FROM repository_group_members JOIN repositories ON repositories.id = repository_group_members.member_repo_id LEFT JOIN (SELECT repo_id, COUNT(*) AS object_count, SUM(size) AS total_bytes FROM artifacts GROUP BY repo_id) AS artifact_stats ON artifact_stats.repo_id = repositories.id WHERE repository_group_members.group_repo_id = ? ORDER BY repository_group_members.position"
    );
    let rows = sqlx::query_as::<_, RepositoryAccessRow>(&sql)
        .bind(user_id)
        .bind(group_repo_id)
        .fetch_all(pool)
        .await
        .map_err(|_| ApiError::internal("查询 group 协议成员失败"))?;
    rows.into_iter().map(|row| row.split(Vec::new())).collect()
}

/// 批量读取 group 成员候选，供服务层验证存在性、格式与类型。
pub(crate) async fn find_member_candidates(
    pool: &SqlitePool,
    names: &[String],
    user_id: Option<i64>,
    global_admin: bool,
) -> Result<Vec<MemberCandidate>, ApiError> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT id, name, format, repo_type FROM repositories WHERE name IN (",
    );
    {
        let mut separated = query.separated(", ");
        for name in names {
            separated.push_bind(name);
        }
    }
    query.push(")");
    append_readable_repository_filter(&mut query, user_id, global_admin);
    let rows = query
        .build_query_as::<MemberCandidateRow>()
        .fetch_all(pool)
        .await
        .map_err(|_| ApiError::internal("查询 group 成员候选失败"))?;
    rows.into_iter()
        .map(|row| {
            Ok(MemberCandidate {
                id: row.id,
                name: row.name,
                format: parse_format(&row.format)?,
                repo_type: parse_repo_type(&row.repo_type)?,
            })
        })
        .collect()
}

/// 原子更新仓库字段，并在上游地址变化时登记并失效全部正缓存索引。
pub(crate) async fn update_repository_record(
    pool: &SqlitePool,
    repository_id: i64,
    patch: &RepositoryRowPatch,
    member_ids: Option<&[i64]>,
) -> Result<Vec<BlobReclamation>, ApiError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ApiError::internal("开启仓库更新事务失败"))?;
    let invalidate_cache = upstream_url_changed(&mut tx, repository_id, patch).await?;
    update_repository_fields(&mut tx, repository_id, patch).await?;
    let reclamations = if invalidate_cache {
        delete_cached_artifacts_in_transaction(&mut tx, repository_id).await?
    } else {
        Vec::new()
    };
    if let Some(ids) = member_ids {
        replace_group_members(&mut tx, repository_id, ids).await?;
    }
    tx.commit()
        .await
        .map_err(|_| ApiError::internal("提交仓库更新事务失败"))?;
    Ok(reclamations)
}

async fn upstream_url_changed(
    tx: &mut Transaction<'_, Sqlite>,
    repository_id: i64,
    patch: &RepositoryRowPatch,
) -> Result<bool, ApiError> {
    let Some(next_url) = patch.upstream_url.as_deref() else {
        return Ok(false);
    };
    let current: Option<(Option<String>,)> =
        sqlx::query_as("SELECT upstream_url FROM repositories WHERE id = ?")
            .bind(repository_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|_| ApiError::internal("读取仓库上游地址失败"))?;
    Ok(current.is_some_and(|row| row.0.as_deref() != Some(next_url)))
}

async fn delete_cached_artifacts_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    repository_id: i64,
) -> Result<Vec<BlobReclamation>, ApiError> {
    let reclamations = register_repository_reclamations(tx, repository_id, true).await?;
    sqlx::query("DELETE FROM artifacts WHERE repo_id = ? AND cached = 1")
        .bind(repository_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ApiError::internal("失效仓库缓存制品失败"))?;
    sqlx::query("DELETE FROM npm_proxy_packuments WHERE repo_id = ?")
        .bind(repository_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ApiError::internal("失效 npm packument 缓存失败"))?;
    Ok(reclamations)
}

async fn replace_group_members(
    tx: &mut Transaction<'_, Sqlite>,
    repository_id: i64,
    member_ids: &[i64],
) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM repository_group_members WHERE group_repo_id = ?")
        .bind(repository_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ApiError::internal("清理旧 group 成员失败"))?;
    insert_group_members(tx, repository_id, member_ids).await
}

async fn update_repository_fields(
    tx: &mut Transaction<'_, Sqlite>,
    repository_id: i64,
    patch: &RepositoryRowPatch,
) -> Result<(), ApiError> {
    let credential_provided = patch.upstream_credential_ref.is_some();
    let credential_value = patch
        .upstream_credential_ref
        .as_ref()
        .and_then(|value| value.as_deref());
    sqlx::query(
        "UPDATE repositories SET visibility = COALESCE(?, visibility), disabled = COALESCE(?, disabled), overwrite_policy = CASE WHEN ? THEN ? ELSE overwrite_policy END, version_policy = CASE WHEN ? THEN ? ELSE version_policy END, upstream_url = CASE WHEN ? THEN ? ELSE upstream_url END, upstream_credential_ref = CASE WHEN ? THEN ? ELSE upstream_credential_ref END WHERE id = ?",
    )
    .bind(patch.visibility.map(Visibility::as_db_str))
    .bind(patch.disabled)
    .bind(patch.overwrite_policy.is_some())
    .bind(patch.overwrite_policy.map(OverwritePolicy::as_db_str))
    .bind(patch.version_policy.is_some())
    .bind(patch.version_policy.map(VersionPolicy::as_db_str))
    .bind(patch.upstream_url.is_some())
    .bind(patch.upstream_url.as_deref())
    .bind(credential_provided)
    .bind(credential_value)
    .bind(repository_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| ApiError::internal("更新仓库失败"))?;
    Ok(())
}

/// 删除未被 group 引用的仓库，并按模式同事务登记或取消 blob 回收意图。
pub(crate) async fn delete_repository_record(
    pool: &SqlitePool,
    repository_id: i64,
    reclaim_blobs: bool,
) -> Result<RepositoryDeleteOutcome, ApiError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| ApiError::internal("开启仓库删除事务失败"))?;
    let hashes = repository_blob_hashes_in_transaction(&mut tx, repository_id).await?;
    let reclamations = if reclaim_blobs {
        register_repository_reclamations(&mut tx, repository_id, false).await?
    } else {
        cancel_repository_reclamations(&mut tx, repository_id).await?;
        Vec::new()
    };
    let result = delete_unreferenced_repository(&mut tx, repository_id).await?;
    if result.rows_affected() == 0 {
        tx.rollback()
            .await
            .map_err(|_| ApiError::internal("回滚仓库删除事务失败"))?;
        return Ok(RepositoryDeleteOutcome::InUse);
    }
    tx.commit()
        .await
        .map_err(|_| ApiError::internal("提交仓库删除事务失败"))?;
    Ok(RepositoryDeleteOutcome::Deleted {
        hashes,
        reclamations,
    })
}

async fn repository_blob_hashes_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    repository_id: i64,
) -> Result<Vec<String>, ApiError> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT DISTINCT sha256 FROM artifacts WHERE repo_id = ?")
            .bind(repository_id)
            .fetch_all(&mut **tx)
            .await
            .map_err(|_| ApiError::internal("读取仓库制品摘要失败"))?;
    Ok(unique_hashes(rows))
}

async fn delete_unreferenced_repository(
    tx: &mut Transaction<'_, Sqlite>,
    repository_id: i64,
) -> Result<sqlx::sqlite::SqliteQueryResult, ApiError> {
    sqlx::query(
        "DELETE FROM repositories WHERE id = ? AND NOT EXISTS (SELECT 1 FROM repository_group_members WHERE member_repo_id = ?)",
    )
    .bind(repository_id)
    .bind(repository_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| ApiError::internal("删除仓库失败"))
}

/// 列出仓库 ACL，稳定按精确用户名与用户标识排序。
pub(crate) async fn list_repository_acl(
    pool: &SqlitePool,
    repository_id: i64,
) -> Result<Vec<AclRecord>, ApiError> {
    let rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT users.id, users.username, repository_acl.permission FROM repository_acl JOIN users ON users.id = repository_acl.user_id WHERE repository_acl.repo_id = ? ORDER BY users.username, users.id",
    )
    .bind(repository_id)
    .fetch_all(pool)
    .await
    .map_err(|_| ApiError::internal("查询仓库 ACL 失败"))?;
    rows.into_iter()
        .map(|(user_id, username, permission)| {
            Ok(AclRecord {
                user_id,
                username,
                permission: parse_permission(&permission)?,
            })
        })
        .collect()
}

/// 按仓库与用户联合键新增或更新 ACL。
pub(crate) async fn upsert_repository_acl(
    pool: &SqlitePool,
    repository_id: i64,
    user_id: i64,
    permission: Permission,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO repository_acl (repo_id, user_id, permission) VALUES (?, ?, ?) ON CONFLICT(repo_id, user_id) DO UPDATE SET permission = excluded.permission",
    )
    .bind(repository_id)
    .bind(user_id)
    .bind(permission.as_db_str())
    .execute(pool)
    .await
    .map_err(|_| ApiError::internal("设置仓库 ACL 失败"))?;
    Ok(())
}

/// 删除仓库 ACL；命中返回 true。
pub(crate) async fn delete_repository_acl(
    pool: &SqlitePool,
    repository_id: i64,
    user_id: i64,
) -> Result<bool, ApiError> {
    let result = sqlx::query("DELETE FROM repository_acl WHERE repo_id = ? AND user_id = ?")
        .bind(repository_id)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|_| ApiError::internal("删除仓库 ACL 失败"))?;
    Ok(result.rows_affected() == 1)
}

/// 测试辅助：内存 SQLite（单连接，确保迁移与查询同库）+ 迁移，供普通单测复用。
#[cfg(test)]
pub(crate) async fn test_pool() -> SqlitePool {
    #![allow(clippy::expect_used)]
    let options = "sqlite::memory:"
        .parse::<SqliteConnectOptions>()
        .expect("解析内存测试库连接失败")
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("创建内存测试库失败");
    MIGRATOR.run(&pool).await.expect("测试库迁移失败");
    pool
}

/// 测试辅助：多连接文件 SQLite + 迁移，供并发不变量单测复用。
#[cfg(test)]
pub(crate) async fn concurrent_test_pool() -> (SqlitePool, PathBuf) {
    #![allow(clippy::expect_used)]
    static NEXT_DATABASE_ID: AtomicU64 = AtomicU64::new(0);

    let database_id = NEXT_DATABASE_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "jianartifact-{}-{database_id}.sqlite",
        std::process::id()
    ));
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .expect("创建并发测试库失败");
    MIGRATOR.run(&pool).await.expect("并发测试库迁移失败");
    (pool, path)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::auth::password::hash_password;
    use crate::auth::token::generate_token;

    #[tokio::test]
    async fn 用户增改查往返一致() {
        let pool = test_pool().await;
        let hash = hash_password("pw").unwrap();
        let user = insert_user(&pool, "alice", &hash, Role::User, true)
            .await
            .unwrap();
        assert_eq!(user.username, "alice");
        assert!(matches!(user.role, Role::User));
        assert!(user.enabled);
        assert_eq!(user.token_version, 0);

        let by_id = find_user_by_id(&pool, user.id).await.unwrap().unwrap();
        assert_eq!(by_id.username, "alice");
        let by_name = find_user_by_username(&pool, "alice")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(by_name.id, user.id);
        assert!(find_user_by_id(&pool, 9999).await.unwrap().is_none());

        // 部分更新：启停与角色
        update_user_fields(&pool, user.id, Some(false), Some(Role::Admin), None)
            .await
            .unwrap();
        let updated = find_user_by_id(&pool, user.id).await.unwrap().unwrap();
        assert!(!updated.enabled);
        assert!(matches!(updated.role, Role::Admin));
        assert_eq!(updated.token_version, 1, "禁用应推进会话版本");
    }

    #[tokio::test]
    async fn 改口令原子递增会话版本() {
        let pool = test_pool().await;
        let user = insert_user(
            &pool,
            "bob",
            &hash_password("old").unwrap(),
            Role::User,
            true,
        )
        .await
        .unwrap();
        let new_hash = hash_password("new").unwrap();
        update_user_fields(&pool, user.id, None, None, Some(new_hash))
            .await
            .unwrap();
        let updated = find_user_by_id(&pool, user.id).await.unwrap().unwrap();
        assert_eq!(updated.token_version, 1, "写口令应同步递增版本");
    }

    #[tokio::test]
    async fn 口令条件更新拒绝覆盖并发管理员重置() {
        let pool = test_pool().await;
        let original_hash = hash_password("old").unwrap();
        let user = insert_user(&pool, "cas-user", &original_hash, Role::User, true)
            .await
            .unwrap();
        let observed = load_password_and_version_by_id(&pool, user.id)
            .await
            .unwrap()
            .unwrap();

        let admin_hash = hash_password("admin-reset").unwrap();
        update_user_fields(&pool, user.id, None, None, Some(admin_hash.clone()))
            .await
            .unwrap();
        let stale_write = compare_and_update_password(
            &pool,
            user.id,
            &observed.0,
            observed.1,
            &hash_password("self-change").unwrap(),
        )
        .await
        .unwrap();

        assert!(!stale_write, "旧快照不得覆盖并发管理员重置");
        assert_eq!(
            load_password_by_id(&pool, user.id).await.unwrap(),
            Some(admin_hash)
        );
    }

    #[tokio::test]
    async fn 分页与用户名子串筛选() {
        let pool = test_pool().await;
        for name in ["alpha", "alberta", "beta", "gamma"] {
            insert_user(&pool, name, &hash_password("pw").unwrap(), Role::User, true)
                .await
                .unwrap();
        }
        // 首页两条
        let (page1, total) = list_users(&pool, 1, 2, None).await.unwrap();
        assert_eq!(total, 4);
        assert_eq!(page1.len(), 2);
        // 次页无重叠
        let (page2, _) = list_users(&pool, 2, 2, None).await.unwrap();
        assert_ne!(page1[0].id, page2[0].id);
        // 子串筛选（大小写不敏感）
        let (filtered, ftotal) = list_users(&pool, 1, 50, Some("AL")).await.unwrap();
        assert_eq!(ftotal, 2, "alpha/alberta 命中");
        assert_eq!(filtered.len(), 2);
    }

    #[tokio::test]
    async fn like通配符按字面匹配不放大() {
        let pool = test_pool().await;
        insert_user(
            &pool,
            "a_b",
            &hash_password("pw").unwrap(),
            Role::User,
            true,
        )
        .await
        .unwrap();
        insert_user(
            &pool,
            "axb",
            &hash_password("pw").unwrap(),
            Role::User,
            true,
        )
        .await
        .unwrap();
        // `_` 应作字面下划线匹配，不得匹配到 axb
        let (rows, total) = list_users(&pool, 1, 50, Some("a_b")).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].username, "a_b");
    }

    #[tokio::test]
    async fn 统计启用管理员与递增版本() {
        let pool = test_pool().await;
        insert_user(
            &pool,
            "admin1",
            &hash_password("pw").unwrap(),
            Role::Admin,
            true,
        )
        .await
        .unwrap();
        let admin2 = insert_user(
            &pool,
            "admin2",
            &hash_password("pw").unwrap(),
            Role::Admin,
            true,
        )
        .await
        .unwrap();
        insert_user(
            &pool,
            "admin3",
            &hash_password("pw").unwrap(),
            Role::Admin,
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            count_enabled_admins(&pool).await.unwrap(),
            2,
            "仅启用的 admin 计入"
        );
        assert_eq!(count_users(&pool).await.unwrap(), 3);

        increment_token_version(&pool, admin2.id).await.unwrap();
        let after = find_user_by_id(&pool, admin2.id).await.unwrap().unwrap();
        assert_eq!(after.token_version, 1);
    }

    #[tokio::test]
    async fn token增查删且本人隔离() {
        let pool = test_pool().await;
        let owner = insert_user(
            &pool,
            "owner",
            &hash_password("pw").unwrap(),
            Role::User,
            true,
        )
        .await
        .unwrap();
        let other = insert_user(
            &pool,
            "other",
            &hash_password("pw").unwrap(),
            Role::User,
            true,
        )
        .await
        .unwrap();
        let (_, hash) = generate_token();
        let (tid, _created) = insert_token(&pool, owner.id, "ci", &hash).await.unwrap();

        assert_eq!(
            find_token_owner_by_hash(&pool, &hash).await.unwrap(),
            Some(owner.id)
        );
        let (rows, total) = list_tokens_by_user(&pool, owner.id, 1, 50, None)
            .await
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].name, "ci");
        // 他人列表为空
        let (_, other_total) = list_tokens_by_user(&pool, other.id, 1, 50, None)
            .await
            .unwrap();
        assert_eq!(other_total, 0);
        // 他人不得删除本 Token
        assert!(!delete_token(&pool, tid, other.id).await.unwrap());
        // 本人删除成功
        assert!(delete_token(&pool, tid, owner.id).await.unwrap());
        assert_eq!(find_token_owner_by_hash(&pool, &hash).await.unwrap(), None);
    }

    #[tokio::test]
    async fn 撤销写入查询与清理() {
        let pool = test_pool().await;
        let far_future = 4_102_444_800; // 2100-01-01
        insert_revoked(&pool, "hash-a", far_future).await.unwrap();
        assert!(is_revoked(&pool, "hash-a").await.unwrap());
        assert!(!is_revoked(&pool, "hash-b").await.unwrap());
        // 幂等写入不报错
        insert_revoked(&pool, "hash-a", far_future).await.unwrap();

        // 过期记录：expires_at 在过去
        insert_revoked(&pool, "hash-expired", 1).await.unwrap();
        let removed = cleanup_expired_revoked(&pool, 1_000).await.unwrap();
        assert_eq!(removed, 1, "仅过期记录被清理");
        assert!(!is_revoked(&pool, "hash-expired").await.unwrap());
        assert!(is_revoked(&pool, "hash-a").await.unwrap(), "未过期记录保留");
    }

    #[tokio::test]
    async fn 刷新撤销原子校验状态版本与唯一性() {
        let pool = test_pool().await;
        let user = insert_user(
            &pool,
            "refresh-user",
            &hash_password("old").unwrap(),
            Role::User,
            true,
        )
        .await
        .unwrap();
        let expires_at = 4_102_444_800;

        assert!(
            revoke_session_for_refresh(
                &pool,
                user.id,
                0,
                "hash-first",
                "family-active",
                expires_at,
            )
            .await
            .unwrap(),
            "首次有效刷新应取得资格"
        );
        assert!(
            !revoke_session_for_refresh(
                &pool,
                user.id,
                0,
                "hash-first",
                "family-active",
                expires_at,
            )
            .await
            .unwrap(),
            "同一 jti 不得重复取得刷新资格"
        );
        insert_revoked(&pool, "family-revoked", expires_at)
            .await
            .unwrap();
        assert!(
            !revoke_session_for_refresh(
                &pool,
                user.id,
                0,
                "hash-family-child",
                "family-revoked",
                expires_at,
            )
            .await
            .unwrap(),
            "已撤销会话家族不得继续刷新"
        );

        update_user_fields(
            &pool,
            user.id,
            None,
            None,
            Some(hash_password("new").unwrap()),
        )
        .await
        .unwrap();
        assert!(
            !revoke_session_for_refresh(
                &pool,
                user.id,
                0,
                "hash-stale",
                "family-active",
                expires_at,
            )
            .await
            .unwrap(),
            "改口令后的旧声明版本不得刷新"
        );
        assert!(!is_revoked(&pool, "hash-stale").await.unwrap());

        update_user_fields(&pool, user.id, Some(false), None, None)
            .await
            .unwrap();
        assert!(
            !revoke_session_for_refresh(
                &pool,
                user.id,
                2,
                "hash-disabled",
                "family-active",
                expires_at,
            )
            .await
            .unwrap(),
            "禁用用户不得刷新"
        );
        assert!(!is_revoked(&pool, "hash-disabled").await.unwrap());
    }

    #[tokio::test]
    async fn 零泄露_口令仅存phc_token仅存哈希() {
        let pool = test_pool().await;
        let user = insert_user(
            &pool,
            "secretive",
            &hash_password("pw").unwrap(),
            Role::User,
            true,
        )
        .await
        .unwrap();
        let stored: (String,) = sqlx::query_as("SELECT password_hash FROM users WHERE id = ?")
            .bind(user.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            stored.0.starts_with("$argon2id$"),
            "口令列应为 Argon2 PHC 串"
        );

        let (plain, hash) = generate_token();
        insert_token(&pool, user.id, "t", &hash).await.unwrap();
        let stored_hash: (String,) =
            sqlx::query_as("SELECT token_hash FROM api_tokens WHERE user_id = ?")
                .bind(user.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            stored_hash.0.len(),
            64,
            "Token 列应为 64 位 sha256 十六进制"
        );
        assert_ne!(stored_hash.0, plain, "Token 列不得为明文");
        assert!(!stored_hash.0.starts_with("jat_"), "Token 列不得含明文前缀");
    }

    #[tokio::test]
    async fn artifacts迁移建立唯一键摘要约束与索引() {
        let pool = test_pool().await;
        let repo_id: (i64,) = sqlx::query_as(
            "INSERT INTO repositories (name, format, repo_type, visibility, overwrite_policy) VALUES ('artifact-test', 'raw', 'hosted', 'private', 'allow') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let insert = "INSERT INTO artifacts (repo_id, path, sha256, sha1, md5, sha512, size, cached) VALUES (?, ?, ?, ?, ?, ?, ?, ?)";
        sqlx::query(insert)
            .bind(repo_id.0)
            .bind("a/file.bin")
            .bind("a".repeat(64))
            .bind("b".repeat(40))
            .bind("c".repeat(32))
            .bind("d".repeat(128))
            .bind(3_i64)
            .bind(false)
            .execute(&pool)
            .await
            .unwrap();

        let duplicate = sqlx::query(insert)
            .bind(repo_id.0)
            .bind("a/file.bin")
            .bind("e".repeat(64))
            .bind("b".repeat(40))
            .bind("c".repeat(32))
            .bind("d".repeat(128))
            .bind(3_i64)
            .bind(false)
            .execute(&pool)
            .await;
        assert!(duplicate.is_err(), "仓库内路径必须唯一");

        for (sha256, size, cached) in [
            ("z".repeat(64), 1_i64, 0_i64),
            ("a".repeat(63), 1, 0),
            ("a".repeat(64), -1, 0),
            ("a".repeat(64), 1, 2),
        ] {
            let invalid = sqlx::query(insert)
                .bind(repo_id.0)
                .bind(format!("invalid-{sha256}-{size}-{cached}"))
                .bind(sha256)
                .bind("b".repeat(40))
                .bind("c".repeat(32))
                .bind("d".repeat(128))
                .bind(size)
                .bind(cached)
                .execute(&pool)
                .await;
            assert!(invalid.is_err(), "摘要、大小与缓存标志约束必须生效");
        }

        for index in ["idx_artifacts_blob", "idx_artifacts_repo_path"] {
            let row: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?",
            )
            .bind(index)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(row.0, 1, "应建立索引 {index}");
        }
    }

    async fn 创建制品测试仓库(pool: &SqlitePool, name: &str) -> i64 {
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO repositories (name, format, repo_type, visibility, overwrite_policy) VALUES (?, 'raw', 'hosted', 'private', 'allow') RETURNING id",
        )
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap();
        row.0
    }

    fn 制品输入(
        repo_id: i64,
        path: &str,
        marker: char,
        cached: bool,
        size: u64,
    ) -> ArtifactWriteInput {
        ArtifactWriteInput {
            repo_id,
            path: path.to_owned(),
            sha256: marker.to_string().repeat(64),
            sha1: marker.to_string().repeat(40),
            md5: marker.to_string().repeat(32),
            sha512: marker.to_string().repeat(128),
            size,
            cached,
        }
    }

    #[tokio::test]
    async fn 制品拒绝覆盖返回可区分冲突() {
        let pool = test_pool().await;
        let repo_id = 创建制品测试仓库(&pool, "deny-conflict").await;
        commit_artifact(&pool, &制品输入(repo_id, "a.bin", 'a', false, 1), false)
            .await
            .unwrap();

        let error = commit_artifact(&pool, &制品输入(repo_id, "a.bin", 'b', false, 2), false)
            .await
            .unwrap_err();

        assert!(matches!(error, ArtifactCommitError::Conflict));
    }

    #[tokio::test]
    async fn 制品允许覆盖返回旧摘要() {
        let pool = test_pool().await;
        let repo_id = 创建制品测试仓库(&pool, "allow-replace").await;
        commit_artifact(&pool, &制品输入(repo_id, "a.bin", 'a', false, 1), false)
            .await
            .unwrap();

        let result = commit_artifact(&pool, &制品输入(repo_id, "a.bin", 'b', false, 2), true)
            .await
            .unwrap();

        assert!(!result.created);
        assert_eq!(result.reclamation.unwrap().sha256, "a".repeat(64));
        assert_eq!(result.artifact.sha256, "b".repeat(64));
    }

    #[tokio::test]
    async fn 同摘要跨仓引用计数与仓库去重列表() {
        let pool = test_pool().await;
        let first_repo = 创建制品测试仓库(&pool, "refs-first").await;
        let second_repo = 创建制品测试仓库(&pool, "refs-second").await;
        for (repo_id, path) in [
            (first_repo, "a.bin"),
            (first_repo, "b.bin"),
            (second_repo, "c.bin"),
        ] {
            commit_artifact(&pool, &制品输入(repo_id, path, 'a', false, 1), false)
                .await
                .unwrap();
        }

        assert_eq!(
            count_artifact_references(&pool, &"a".repeat(64))
                .await
                .unwrap(),
            3
        );
        assert_eq!(
            list_repository_blob_hashes(&pool, first_repo)
                .await
                .unwrap(),
            vec!["a".repeat(64)]
        );
    }

    #[tokio::test]
    async fn 删除制品路径返回旧记录() {
        let pool = test_pool().await;
        let repo_id = 创建制品测试仓库(&pool, "delete-path").await;
        commit_artifact(&pool, &制品输入(repo_id, "a.bin", 'a', false, 7), false)
            .await
            .unwrap();

        let deleted = delete_artifact_by_path(&pool, repo_id, "a.bin")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(deleted.artifact.path, "a.bin");
        assert_eq!(deleted.artifact.size, 7);
    }

    #[tokio::test]
    async fn 批量删除只清理缓存制品并返回去重摘要() {
        let pool = test_pool().await;
        let repo_id = 创建制品测试仓库(&pool, "delete-cached").await;
        for (path, marker, cached) in [
            ("cached-a.bin", 'a', true),
            ("cached-a-copy.bin", 'a', true),
            ("hosted.bin", 'b', false),
        ] {
            commit_artifact(&pool, &制品输入(repo_id, path, marker, cached, 1), false)
                .await
                .unwrap();
        }

        let candidates = delete_cached_artifacts_by_repo(&pool, repo_id)
            .await
            .unwrap();

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].sha256, "a".repeat(64));
        assert!(
            find_artifact_by_path(&pool, repo_id, "cached-a.bin")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            find_artifact_by_path(&pool, repo_id, "hosted.bin")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn 仓库列表与详情聚合制品统计且分页总数不变() {
        let pool = test_pool().await;
        let repo_id = 创建制品测试仓库(&pool, "stats-repo").await;
        创建制品测试仓库(&pool, "stats-empty").await;
        commit_artifact(&pool, &制品输入(repo_id, "a.bin", 'a', false, 3), false)
            .await
            .unwrap();
        commit_artifact(&pool, &制品输入(repo_id, "b.bin", 'b', false, 5), false)
            .await
            .unwrap();

        let (repositories, total) = list_repositories(&pool, None, true, 1, 1, Some("stats-repo"))
            .await
            .unwrap();
        let detail = find_repository_access(&pool, "stats-repo", None, true)
            .await
            .unwrap()
            .unwrap()
            .0;

        assert_eq!(total, 1);
        assert_eq!(repositories[0].object_count, 2);
        assert_eq!(repositories[0].total_bytes, 8);
        assert_eq!(detail.object_count, 2);
        assert_eq!(detail.total_bytes, 8);
    }

    #[tokio::test]
    async fn 目录索引只返回直接子项并支持utf8前缀分页() {
        let pool = test_pool().await;
        let repo_id = 创建制品测试仓库(&pool, "directory-list").await;
        for (path, marker, size) in [
            ("根.bin", 'a', 1),
            ("目录/甲.bin", 'b', 2),
            ("目录/子目录/乙.bin", 'c', 3),
            ("目录/丙.bin", 'd', 4),
        ] {
            commit_artifact(&pool, &制品输入(repo_id, path, marker, false, size), false)
                .await
                .unwrap();
        }

        let (root, root_total) = list_artifact_directory(&pool, repo_id, "", 1, 10)
            .await
            .unwrap();
        assert_eq!(root_total, 2);
        assert!(root[0].is_directory);
        assert_eq!(root[0].path, "目录");
        assert_eq!(root[1].path, "根.bin");

        let (page, total) = list_artifact_directory(&pool, repo_id, "目录", 2, 2)
            .await
            .unwrap();
        assert_eq!(total, 3);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].path, "目录/甲.bin");
    }

    #[tokio::test]
    async fn 制品路径搜索分页且通配符按字面匹配() {
        let pool = test_pool().await;
        let repo_id = 创建制品测试仓库(&pool, "artifact-search").await;
        for (path, marker) in [("a%file.bin", 'a'), ("a-file.bin", 'b'), ("other.bin", 'c')] {
            commit_artifact(&pool, &制品输入(repo_id, path, marker, false, 1), false)
                .await
                .unwrap();
        }

        let (items, total) = search_artifacts(&pool, repo_id, "%", 1, 1).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].path, "a%file.bin");
    }

    #[tokio::test]
    async fn proxy仅在上游地址真实变化时事务失效正缓存() {
        let pool = test_pool().await;
        let repo_id: (i64,) = sqlx::query_as(
            "INSERT INTO repositories (name, format, repo_type, visibility, upstream_url) VALUES ('proxy-invalidate', 'npm', 'proxy', 'private', 'https://old.example/base') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        commit_artifact(
            &pool,
            &制品输入(repo_id.0, "cached.bin", 'a', true, 3),
            false,
        )
        .await
        .unwrap();
        upsert_npm_proxy_packument(
            &pool,
            &NpmProxyPackumentInput {
                repo_id: repo_id.0,
                package_name: "demo".to_string(),
                rewritten_json: r#"{"name":"demo"}"#.to_string(),
                upstream_url_snapshot: "https://old.example/base".to_string(),
                ttl_secs: 1800,
                tarballs: Vec::new(),
            },
        )
        .await
        .unwrap();

        let credential_only = RepositoryRowPatch {
            upstream_credential_ref: Some(Some("UPSTREAM_AUTH".to_string())),
            ..RepositoryRowPatch::default()
        };
        assert!(
            update_repository_record(&pool, repo_id.0, &credential_only, None)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            find_artifact_by_path(&pool, repo_id.0, "cached.bin")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            find_npm_proxy_packument(&pool, repo_id.0, "demo")
                .await
                .unwrap()
                .is_some()
        );

        let changed_url = RepositoryRowPatch {
            upstream_url: Some("https://new.example/base".to_string()),
            ..RepositoryRowPatch::default()
        };
        let reclamations = update_repository_record(&pool, repo_id.0, &changed_url, None)
            .await
            .unwrap();
        assert_eq!(reclamations.len(), 1);
        assert_eq!(reclamations[0].sha256, "a".repeat(64));
        assert!(
            find_artifact_by_path(&pool, repo_id.0, "cached.bin")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            find_npm_proxy_packument(&pool, repo_id.0, "demo")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn 删除仓库事务返回级联前去重blob候选() {
        let pool = test_pool().await;
        let repo_id = 创建制品测试仓库(&pool, "delete-with-artifacts").await;
        for path in ["a.bin", "a-copy.bin"] {
            commit_artifact(&pool, &制品输入(repo_id, path, 'a', false, 1), false)
                .await
                .unwrap();
        }

        let deleted = delete_repository_record(&pool, repo_id, true)
            .await
            .unwrap();
        let (hashes, reclamations) = match deleted {
            RepositoryDeleteOutcome::Deleted {
                hashes,
                reclamations,
            } => Some((hashes, reclamations)),
            RepositoryDeleteOutcome::InUse => None,
        }
        .unwrap();
        assert_eq!(hashes, vec!["a".repeat(64)]);
        assert_eq!(reclamations.len(), 1);
        assert_eq!(reclamations[0].sha256, "a".repeat(64));
        assert_eq!(
            sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM artifacts WHERE repo_id = ?")
                .bind(repo_id)
                .fetch_one(&pool)
                .await
                .unwrap()
                .0,
            0
        );
    }

    #[tokio::test]
    async fn blob回收迁移约束与generation条件完成() {
        let pool = test_pool().await;
        let table: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'blob_reclamations'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(table.0, 1);
        for (sha256, generation) in [("z".repeat(64), 1_i64), ("a".repeat(64), 0_i64)] {
            assert!(
                sqlx::query("INSERT INTO blob_reclamations (sha256, generation) VALUES (?, ?)")
                    .bind(sha256)
                    .bind(generation)
                    .execute(&pool)
                    .await
                    .is_err()
            );
        }

        let repo_id = 创建制品测试仓库(&pool, "reclamation-generation").await;
        commit_artifact(&pool, &制品输入(repo_id, "a.bin", 'a', false, 1), false)
            .await
            .unwrap();
        let first = delete_artifact_by_path(&pool, repo_id, "a.bin")
            .await
            .unwrap()
            .unwrap()
            .reclamation;
        commit_artifact(&pool, &制品输入(repo_id, "a.bin", 'a', false, 1), false)
            .await
            .unwrap();
        let second = delete_artifact_by_path(&pool, repo_id, "a.bin")
            .await
            .unwrap()
            .unwrap()
            .reclamation;

        assert_eq!(second.generation, first.generation + 1);
        assert_eq!(
            complete_blob_reclamations(&pool, &[first]).await.unwrap(),
            0
        );
        assert_eq!(
            pending_blob_reclamations(&pool).await.unwrap(),
            vec![second.clone()]
        );
        assert_eq!(
            complete_blob_reclamations(&pool, &[second]).await.unwrap(),
            1
        );
        assert!(pending_blob_reclamations(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn blob回收批量操作不受sqlite变量上限限制() {
        let pool = test_pool().await;
        let reclamations: Vec<_> = (0..16_385)
            .map(|index| BlobReclamation {
                sha256: format!("{index:064x}"),
                generation: 1,
            })
            .collect();

        assert!(
            inspect_blob_reclamations(&pool, &reclamations)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            complete_blob_reclamations(&pool, &reclamations)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn 候选blob引用统计支持大批量并保持去重() {
        let pool = test_pool().await;
        let repo_id = 创建制品测试仓库(&pool, "candidate-references").await;
        commit_artifact(&pool, &制品输入(repo_id, "a.bin", 'a', false, 1), false)
            .await
            .unwrap();
        let referenced = "a".repeat(64);
        let mut candidates: Vec<_> = (0..16_385).map(|index| format!("{index:064x}")).collect();
        candidates.push(referenced.clone());
        candidates.push(referenced.clone());

        assert_eq!(
            referenced_candidate_hashes(&pool, &candidates)
                .await
                .unwrap(),
            HashSet::from([referenced])
        );
    }

    #[tokio::test]
    async fn 仓库data登记且config取消同摘要回收意图() {
        let pool = test_pool().await;
        let source_id = 创建制品测试仓库(&pool, "reclamation-source").await;
        commit_artifact(&pool, &制品输入(source_id, "a.bin", 'a', false, 1), false)
            .await
            .unwrap();
        delete_artifact_by_path(&pool, source_id, "a.bin")
            .await
            .unwrap()
            .unwrap();

        let config_id = 创建制品测试仓库(&pool, "reclamation-config").await;
        commit_artifact(&pool, &制品输入(config_id, "a.bin", 'a', false, 1), false)
            .await
            .unwrap();
        let config = delete_repository_record(&pool, config_id, false)
            .await
            .unwrap();
        let (hashes, reclamations) = match config {
            RepositoryDeleteOutcome::Deleted {
                hashes,
                reclamations,
            } => Some((hashes, reclamations)),
            RepositoryDeleteOutcome::InUse => None,
        }
        .unwrap();
        assert_eq!(hashes, vec!["a".repeat(64)]);
        assert!(reclamations.is_empty());
        assert!(pending_blob_reclamations(&pool).await.unwrap().is_empty());

        let data_id = 创建制品测试仓库(&pool, "reclamation-data").await;
        commit_artifact(&pool, &制品输入(data_id, "b.bin", 'b', false, 1), false)
            .await
            .unwrap();
        let data = delete_repository_record(&pool, data_id, true)
            .await
            .unwrap();
        let reclamations = match data {
            RepositoryDeleteOutcome::Deleted { reclamations, .. } => Some(reclamations),
            RepositoryDeleteOutcome::InUse => None,
        }
        .unwrap();
        assert_eq!(reclamations.len(), 1);
        assert_eq!(reclamations[0].sha256, "b".repeat(64));
        assert_eq!(
            pending_blob_reclamations(&pool).await.unwrap(),
            reclamations
        );
    }

    #[tokio::test]
    async fn proxy缓存提交在事务内拒绝旧上游快照() {
        let pool = test_pool().await;
        let repo_id: (i64,) = sqlx::query_as(
            "INSERT INTO repositories (name, format, repo_type, visibility, upstream_url) VALUES ('proxy-commit-snapshot', 'raw', 'proxy', 'private', 'https://old.example/base') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE repositories SET upstream_url = 'https://new.example/base' WHERE id = ?",
        )
        .bind(repo_id.0)
        .execute(&pool)
        .await
        .unwrap();

        let error = commit_proxy_artifact(
            &pool,
            &制品输入(repo_id.0, "cached.bin", 'a', true, 1),
            "https://old.example/base",
            true,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            ArtifactCommitError::StaleProxyConfiguration
        ));
        assert!(
            find_artifact_by_path(&pool, repo_id.0, "cached.bin")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn proxy拒绝覆盖并发提交不同路径全部成功() {
        const 并发数: usize = 4;
        const 轮数: usize = 64;
        const 上游地址: &str = "https://upstream.example/base";

        let (pool, database_path) = concurrent_test_pool().await;
        let repo_id: (i64,) = sqlx::query_as(
            "INSERT INTO repositories (name, format, repo_type, visibility, upstream_url) VALUES ('proxy-concurrent-commit', 'raw', 'proxy', 'private', ?) RETURNING id",
        )
        .bind(上游地址)
        .fetch_one(&pool)
        .await
        .unwrap();

        for 轮次 in 0..轮数 {
            let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(并发数));
            let mut tasks = tokio::task::JoinSet::new();
            for 对象序号 in 0..并发数 {
                let pool = pool.clone();
                let barrier = std::sync::Arc::clone(&barrier);
                tasks.spawn(async move {
                    barrier.wait().await;
                    commit_proxy_artifact(
                        &pool,
                        &制品输入(
                            repo_id.0,
                            &format!("并发/{轮次}/{对象序号}.bin"),
                            ['a', 'b', 'c', 'd'][对象序号],
                            true,
                            1,
                        ),
                        上游地址,
                        false,
                    )
                    .await
                });
            }

            let mut 成功数 = 0;
            while let Some(result) = tasks.join_next().await {
                if result.expect("并发提交任务不应崩溃").is_ok() {
                    成功数 += 1;
                }
            }
            assert_eq!(成功数, 并发数, "第 {轮次} 轮四个不同对象应全部提交成功");
        }

        let object_count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM artifacts WHERE repo_id = ?")
                .bind(repo_id.0)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            object_count.0,
            (轮数 * 并发数) as i64,
            "并发提交完成后对象总数应与轮数和并发数一致"
        );

        pool.close().await;
        tokio::fs::remove_file(database_path).await.unwrap();
    }

    #[tokio::test]
    async fn 迁移proxy提交保留外部改写冲突守卫() {
        let pool = test_pool().await;
        let repo_id: (i64,) = sqlx::query_as(
            "INSERT INTO repositories (name, format, repo_type, visibility, upstream_url) VALUES ('migration-proxy-conflict', 'raw', 'proxy', 'private', 'https://upstream.example/base') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        commit_proxy_artifact(
            &pool,
            &制品输入(repo_id.0, "cached.bin", 'a', true, 1),
            "https://upstream.example/base",
            true,
        )
        .await
        .unwrap();

        let error = commit_proxy_artifact(
            &pool,
            &制品输入(repo_id.0, "cached.bin", 'b', true, 1),
            "https://upstream.example/base",
            false,
        )
        .await
        .unwrap_err();

        assert!(matches!(error, ArtifactCommitError::Conflict));
        assert_eq!(
            find_artifact_by_path(&pool, repo_id.0, "cached.bin")
                .await
                .unwrap()
                .unwrap()
                .sha256,
            "a".repeat(64)
        );
    }

    #[tokio::test]
    async fn npm迁移校验计数写入报告且保留既有字段() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO migration_discoveries (id, source_kind, source_instance_id, snapshot_id, snapshot_time, source_locator, discovery_json) VALUES ('discovery-report', 'native-dir', 'source', 'snapshot', '0', '/source', '{}')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO migration_tasks (id, discovery_id, plan_id, source_kind, status, phase, snapshot_json, report_json, total_objects, total_bytes) VALUES ('task-report', 'discovery-report', 'plan-report', 'native-dir', 'queued', 'initial-transfer', '{}', '{\"existing\":true,\"validation\":{\"blob\":{\"checked\":1,\"passed\":1,\"failed\":0}}}', 0, 0)")
            .execute(&pool)
            .await
            .unwrap();
        let validation = NpmMigrationValidationReport {
            proxy_positive_cache: NpmMigrationValidationCounts {
                checked: 2,
                passed: 1,
                failed: 1,
            },
            protocol_metadata: NpmMigrationValidationCounts {
                checked: 3,
                passed: 2,
                failed: 1,
            },
        };

        store_npm_migration_validation_report(&pool, "task-report", validation)
            .await
            .unwrap();

        assert_eq!(
            load_npm_migration_validation_report(&pool, "task-report")
                .await
                .unwrap(),
            Some(validation)
        );
        let report: String =
            sqlx::query_scalar("SELECT report_json FROM migration_tasks WHERE id = 'task-report'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let report: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(
            report.get("existing").and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            report
                .pointer("/validation/blob/checked")
                .and_then(serde_json::Value::as_u64),
            Some(1)
        );
    }

    #[tokio::test]
    async fn npm迁移摘要冲突不写入版本与标签() {
        let pool = test_pool().await;
        let repo_id = 创建制品测试仓库(&pool, "npm-migration-digest").await;
        commit_artifact(
            &pool,
            &制品输入(repo_id, "pkg/-/pkg-1.0.0.tgz", 'a', false, 1),
            false,
        )
        .await
        .unwrap();
        let mut input = NpmMigrationPackageInput {
            repo_id,
            package_name: "pkg".to_string(),
            versions: vec![NpmMigrationVersionInput {
                version: "1.0.0".to_string(),
                manifest_json: "{}".to_string(),
                tarball_path: "pkg/-/pkg-1.0.0.tgz".to_string(),
                shasum: Some("b".repeat(40)),
                integrity: Some("sha512-YQ==".to_string()),
            }],
            dist_tags: BTreeMap::from([("latest".to_string(), "1.0.0".to_string())]),
        };

        let error = import_npm_migration_package(&pool, &input)
            .await
            .unwrap_err();

        assert_eq!(error.code(), "NPM_MIGRATION_DIGEST_CONFLICT");
        input.versions[0].shasum = None;
        let integrity_error = validate_npm_migration_package(&pool, &input)
            .await
            .unwrap_err();
        assert_eq!(integrity_error.code(), "NPM_MIGRATION_DIGEST_CONFLICT");
        assert!(
            find_npm_package(&pool, repo_id, "pkg")
                .await
                .unwrap()
                .is_none()
        );
    }
}
