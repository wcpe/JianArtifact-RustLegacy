//! 共享制品服务：统一协调索引提交、blob 读取与全局引用回收。

use std::collections::{BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::http::StatusCode;
use sqlx::SqlitePool;
use tokio::sync::Notify;

use crate::error::ApiError;
use crate::persistence::{
    self, ArtifactCommitError, ArtifactDirectoryEntry, ArtifactRecord, ArtifactWriteInput,
    BlobReclamation,
};
use crate::storage::{
    AbandonedBlob, BlobDeletePermit, BlobMetadata, BlobReadHandle, BlobStore, BlobWriter,
    PendingBlob, StorageError,
};

/// 代理缓存单飞键。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProxyFlightKey {
    repo_id: i64,
    path: String,
    upstream_url: String,
}

/// 单次代理缓存任务的共享结果。
struct ProxyFlight {
    result: Mutex<Option<Result<(), ApiError>>>,
    notify: Notify,
}

/// 领导者请求取消时负责释放登记并唤醒等待者。
struct ProxyFlightLeader {
    service: Arc<ArtifactService>,
    key: ProxyFlightKey,
    flight: Arc<ProxyFlight>,
    completed: bool,
}

/// 共享制品服务。
pub struct ArtifactService {
    pool: SqlitePool,
    blobs: BlobStore,
    proxy_client: OnceLock<Result<reqwest::Client, ()>>,
    proxy_flights: Mutex<HashMap<ProxyFlightKey, Arc<ProxyFlight>>>,
}

/// 服务层增量写入会话，统一映射 blob 存储错误。
pub struct ArtifactWriter {
    writer: BlobWriter,
}

/// 制品提交结果。
#[derive(Debug)]
pub struct CommitPendingResult {
    pub created: bool,
    pub record: ArtifactRecord,
}

/// 制品读取结果；读取句柄存续期间内容不会被物理删除。
pub struct ArtifactRead {
    pub record: ArtifactRecord,
    pub blob: BlobReadHandle,
}

impl ArtifactService {
    /// 创建共享制品服务。
    pub fn new(pool: SqlitePool, blobs: BlobStore) -> Self {
        Self {
            pool,
            blobs,
            proxy_client: OnceLock::new(),
            proxy_flights: Mutex::new(HashMap::new()),
        }
    }

    /// 返回进程内复用的代理上游客户端。
    pub fn proxy_client(&self) -> Result<&reqwest::Client, ApiError> {
        self.proxy_client
            .get_or_init(|| {
                reqwest::Client::builder()
                    .connect_timeout(Duration::from_secs(10))
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .map_err(|_| ())
            })
            .as_ref()
            .map_err(|_| {
                ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "PROXY_CLIENT_INIT",
                    "代理上游客户端初始化失败",
                )
            })
    }

    /// 对同一仓库与路径只允许一个代理缓存任务执行，其余请求等待同一结果。
    pub async fn proxy_singleflight<F, Fut>(
        self: &Arc<Self>,
        repo_id: i64,
        path: &str,
        upstream_url: &str,
        leader: F,
    ) -> Result<(), ApiError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), ApiError>> + Send + 'static,
    {
        let key = ProxyFlightKey {
            repo_id,
            path: path.to_owned(),
            upstream_url: upstream_url.to_owned(),
        };
        let (flight, is_leader) = self.register_proxy_flight(&key);
        if is_leader {
            let leader_guard = ProxyFlightLeader {
                service: Arc::clone(self),
                key,
                flight: Arc::clone(&flight),
                completed: false,
            };
            std::mem::drop(tokio::spawn(async move {
                let result = leader().await;
                leader_guard.complete(result);
            }));
        }
        wait_for_proxy_flight(&flight).await
    }

    /// 开始一次增量 blob 写入。
    pub async fn begin_write(&self) -> Result<ArtifactWriter, ApiError> {
        self.blobs
            .begin()
            .await
            .map(|writer| ArtifactWriter { writer })
            .map_err(map_storage_error)
    }

    /// 原子提交 hosted 待定 blob 的索引，并在覆盖后回收旧内容。
    pub async fn commit_pending(
        &self,
        repo_id: i64,
        path: &str,
        pending: PendingBlob,
        cached: bool,
        allow_overwrite: bool,
    ) -> Result<CommitPendingResult, ApiError> {
        self.commit_pending_inner(repo_id, path, pending, cached, allow_overwrite, None)
            .await
    }

    /// 仅当上游地址仍等于预期快照时提交 proxy 正缓存。
    pub async fn commit_proxy_pending(
        &self,
        repo_id: i64,
        path: &str,
        pending: PendingBlob,
        expected_upstream_url: &str,
    ) -> Result<CommitPendingResult, ApiError> {
        self.commit_pending_inner(
            repo_id,
            path,
            pending,
            true,
            true,
            Some(expected_upstream_url),
        )
        .await
    }

    async fn commit_pending_inner(
        &self,
        repo_id: i64,
        path: &str,
        pending: PendingBlob,
        cached: bool,
        allow_overwrite: bool,
        expected_upstream_url: Option<&str>,
    ) -> Result<CommitPendingResult, ApiError> {
        let input = artifact_write_input(repo_id, path, pending.metadata(), cached);
        let committed = match expected_upstream_url {
            Some(expected) => {
                persistence::commit_proxy_artifact(&self.pool, &input, expected).await
            }
            None => persistence::commit_artifact(&self.pool, &input, allow_overwrite).await,
        };
        let committed = match committed {
            Ok(committed) => committed,
            Err(error) => {
                let error = map_commit_error(error);
                self.cleanup_abandoned(pending.abandon()).await;
                return Err(error);
            }
        };
        pending.complete().await;
        if let Some(reclamation) = &committed.reclamation {
            self.reclaim_reclamations(std::slice::from_ref(reclamation))
                .await?;
        }
        Ok(CommitPendingResult {
            created: committed.created,
            record: committed.artifact,
        })
    }

    /// 按仓库与路径读取索引及带租约的 blob 句柄。
    pub async fn read(&self, repo_id: i64, path: &str) -> Result<Option<ArtifactRead>, ApiError> {
        let Some(record) = persistence::find_artifact_by_path(&self.pool, repo_id, path).await?
        else {
            return Ok(None);
        };
        let blob = self
            .blobs
            .open(&record.sha256)
            .await
            .map_err(map_storage_error)?;
        Ok(Some(ArtifactRead { record, blob }))
    }

    /// 分页列出目录的直接子项。
    pub async fn list_directory(
        &self,
        repo_id: i64,
        prefix: &str,
        page: u32,
        page_size: u32,
    ) -> Result<(Vec<ArtifactDirectoryEntry>, i64), ApiError> {
        persistence::list_artifact_directory(&self.pool, repo_id, prefix, page, page_size).await
    }

    /// 分页搜索仓库内制品路径。
    pub async fn search(
        &self,
        repo_id: i64,
        query: &str,
        page: u32,
        page_size: u32,
    ) -> Result<(Vec<ArtifactRecord>, i64), ApiError> {
        persistence::search_artifacts(&self.pool, repo_id, query, page, page_size).await
    }

    /// 删除单个索引；不存在返回 None，存在时按持久化令牌回收。
    pub async fn delete(
        &self,
        repo_id: i64,
        path: &str,
    ) -> Result<Option<ArtifactRecord>, ApiError> {
        let Some(deleted) = persistence::delete_artifact_by_path(&self.pool, repo_id, path).await?
        else {
            return Ok(None);
        };
        self.reclaim_reclamations(std::slice::from_ref(&deleted.reclamation))
            .await?;
        Ok(Some(deleted.artifact))
    }

    /// 统计候选摘要中当前已无任何索引引用的数量。
    pub async fn count_unreferenced_candidates(
        &self,
        candidates: &[String],
    ) -> Result<u64, ApiError> {
        let unique: HashSet<&str> = candidates.iter().map(String::as_str).collect();
        let referenced = persistence::referenced_candidate_hashes(&self.pool, candidates).await?;
        Ok((unique.len().saturating_sub(referenced.len())) as u64)
    }

    /// 处理启动时遗留的全部持久化回收意图。
    pub async fn recover_reclamations(&self) -> Result<u64, ApiError> {
        let reclamations = persistence::pending_blob_reclamations(&self.pool).await?;
        self.reclaim_reclamations(&reclamations).await
    }

    /// 按当前代次和引用状态处理一批持久化回收意图。
    pub async fn reclaim_reclamations(
        &self,
        reclamations: &[BlobReclamation],
    ) -> Result<u64, ApiError> {
        let permits = self.acquire_reclamation_permits(reclamations).await?;
        let states = persistence::inspect_blob_reclamations(&self.pool, reclamations).await?;
        let (reclaimed, completed) = remove_reclaimable_blobs(permits, states).await?;
        persistence::complete_blob_reclamations(&self.pool, &completed).await?;
        Ok(reclaimed)
    }

    async fn acquire_reclamation_permits(
        &self,
        reclamations: &[BlobReclamation],
    ) -> Result<Vec<BlobDeletePermit>, ApiError> {
        let hashes: BTreeSet<&str> = reclamations
            .iter()
            .map(|reclamation| reclamation.sha256.as_str())
            .collect();
        let mut permits = Vec::with_capacity(hashes.len());
        for sha256 in hashes {
            permits.push(
                self.blobs
                    .begin_delete(sha256)
                    .await
                    .map_err(map_storage_error)?,
            );
        }
        Ok(permits)
    }

    fn register_proxy_flight(&self, key: &ProxyFlightKey) -> (Arc<ProxyFlight>, bool) {
        let mut flights = match self.proxy_flights.lock() {
            Ok(flights) => flights,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(flight) = flights.get(key) {
            return (Arc::clone(flight), false);
        }
        let flight = Arc::new(ProxyFlight {
            result: Mutex::new(None),
            notify: Notify::new(),
        });
        flights.insert(key.clone(), Arc::clone(&flight));
        (flight, true)
    }

    fn remove_proxy_flight(&self, key: &ProxyFlightKey, completed: &Arc<ProxyFlight>) {
        let mut flights = match self.proxy_flights.lock() {
            Ok(flights) => flights,
            Err(poisoned) => poisoned.into_inner(),
        };
        if flights
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, completed))
        {
            flights.remove(key);
        }
    }

    async fn cleanup_abandoned(&self, abandoned: AbandonedBlob) {
        if let Err(error) = self.cleanup_abandoned_inner(abandoned).await {
            tracing::warn!(code = error.code(), "索引提交失败后的 blob 将留待启动恢复");
        }
    }

    async fn cleanup_abandoned_inner(&self, abandoned: AbandonedBlob) -> Result<(), ApiError> {
        if !abandoned.is_guarded() {
            return Ok(());
        }
        let permit = self
            .blobs
            .begin_delete(abandoned.sha256())
            .await
            .map_err(map_storage_error)?;
        let references =
            persistence::count_artifact_references(&self.pool, abandoned.sha256()).await?;
        if references == 0 {
            permit.remove().await.map_err(map_storage_error)?;
        } else {
            drop(permit);
        }
        abandoned.clear_guard().await.map_err(map_storage_error)
    }
}

async fn remove_reclaimable_blobs(
    permits: Vec<BlobDeletePermit>,
    states: Vec<persistence::BlobReclamationState>,
) -> Result<(u64, Vec<BlobReclamation>), ApiError> {
    let mut states: HashMap<_, _> = states
        .into_iter()
        .map(|state| (state.reclamation.sha256.clone(), state))
        .collect();
    let mut reclaimed = 0_u64;
    let mut completed = Vec::with_capacity(states.len());
    for permit in permits {
        let Some(state) = states.remove(permit.sha256()) else {
            continue;
        };
        if !state.referenced {
            reclaimed = reclaimed
                .saturating_add(u64::from(permit.remove().await.map_err(map_storage_error)?));
        }
        completed.push(state.reclamation);
    }
    Ok((reclaimed, completed))
}

impl ProxyFlightLeader {
    fn complete(mut self, result: Result<(), ApiError>) {
        set_proxy_flight_result(&self.flight, result);
        self.service.remove_proxy_flight(&self.key, &self.flight);
        self.flight.notify.notify_waiters();
        self.completed = true;
    }
}

impl Drop for ProxyFlightLeader {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        set_proxy_flight_result(
            &self.flight,
            Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "PROXY_UPSTREAM_FAILURE",
                "代理上游响应失败",
            )),
        );
        self.service.remove_proxy_flight(&self.key, &self.flight);
        self.flight.notify.notify_waiters();
    }
}

fn set_proxy_flight_result(flight: &ProxyFlight, result: Result<(), ApiError>) {
    let mut stored = match flight.result.lock() {
        Ok(stored) => stored,
        Err(poisoned) => poisoned.into_inner(),
    };
    *stored = Some(result);
}

async fn wait_for_proxy_flight(flight: &ProxyFlight) -> Result<(), ApiError> {
    loop {
        let mut notified = Box::pin(flight.notify.notified());
        notified.as_mut().enable();
        let result = match flight.result.lock() {
            Ok(stored) => stored.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        if let Some(result) = result {
            return result;
        }
        notified.await;
    }
}

impl ArtifactWriter {
    /// 写入一段内容并更新摘要。
    pub async fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), ApiError> {
        self.writer
            .write_chunk(chunk)
            .await
            .map_err(map_storage_error)
    }

    /// 完成 blob 落定，交由 ArtifactService 提交索引。
    pub async fn finish(self) -> Result<PendingBlob, ApiError> {
        self.writer.finish().await.map_err(map_storage_error)
    }
}

fn artifact_write_input(
    repo_id: i64,
    path: &str,
    metadata: &BlobMetadata,
    cached: bool,
) -> ArtifactWriteInput {
    ArtifactWriteInput {
        repo_id,
        path: path.to_owned(),
        sha256: metadata.sha256.clone(),
        sha1: metadata.sha1.clone(),
        md5: metadata.md5.clone(),
        sha512: metadata.sha512.clone(),
        size: metadata.size,
        cached,
    }
}

fn map_commit_error(error: ArtifactCommitError) -> ApiError {
    match error {
        ArtifactCommitError::Conflict => ApiError::conflict("ARTIFACT_CONFLICT", "制品路径已存在"),
        ArtifactCommitError::StaleProxyConfiguration => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "PROXY_CONFIGURATION_CHANGED",
            "代理仓库配置连续变化，请稍后重试",
        ),
        ArtifactCommitError::Internal(error) => error,
    }
}

fn map_storage_error(error: StorageError) -> ApiError {
    match error {
        StorageError::SizeLimit => ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "ARTIFACT_TOO_LARGE",
            "制品大小超过上限",
        ),
        StorageError::LowSpace => ApiError::new(
            StatusCode::INSUFFICIENT_STORAGE,
            "ARTIFACT_STORAGE_FULL",
            "制品存储可用空间不足",
        ),
        error => {
            tracing::error!(error = %error, "制品 blob 存储操作失败");
            ApiError::internal("制品存储操作失败")
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use tokio::io::AsyncReadExt;
    use tokio::sync::oneshot;

    use super::*;

    async fn service(dir: &tempfile::TempDir) -> ArtifactService {
        let pool = persistence::test_pool().await;
        let blobs = BlobStore::new(dir.path().join("blobs"), 1024, 0, 0);
        blobs.initialize().await.unwrap();
        ArtifactService::new(pool, blobs)
    }

    async fn create_repo(service: &ArtifactService, name: &str) -> i64 {
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO repositories (name, format, repo_type, visibility, overwrite_policy) VALUES (?, 'raw', 'hosted', 'private', 'allow') RETURNING id",
        )
        .bind(name)
        .fetch_one(&service.pool)
        .await
        .unwrap();
        row.0
    }

    async fn commit_bytes(
        service: &ArtifactService,
        repo_id: i64,
        path: &str,
        bytes: &[u8],
        allow_overwrite: bool,
    ) -> CommitPendingResult {
        let mut writer = service.begin_write().await.unwrap();
        writer.write_chunk(bytes).await.unwrap();
        let pending = writer.finish().await.unwrap();
        service
            .commit_pending(repo_id, path, pending, false, allow_overwrite)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn 提交后可读取原始字节() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir).await;
        let repo_id = create_repo(&service, "read-bytes").await;

        let committed = commit_bytes(&service, repo_id, "a.bin", b"artifact-bytes", false).await;
        let mut read = service.read(repo_id, "a.bin").await.unwrap().unwrap();
        let mut bytes = Vec::new();
        read.blob.read_to_end(&mut bytes).await.unwrap();

        assert!(committed.created);
        assert_eq!(read.record.id, committed.record.id);
        assert_eq!(bytes, b"artifact-bytes");
    }

    #[tokio::test]
    async fn deny覆盖返回稳定冲突且保留原索引() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir).await;
        let repo_id = create_repo(&service, "deny-overwrite").await;
        let original = commit_bytes(&service, repo_id, "a.bin", b"old", false).await;
        let mut writer = service.begin_write().await.unwrap();
        writer.write_chunk(b"new").await.unwrap();
        let pending = writer.finish().await.unwrap();
        let rejected_blob = service.blobs.blob_path(&pending.metadata().sha256).unwrap();

        let error = service
            .commit_pending(repo_id, "a.bin", pending, false, false)
            .await
            .unwrap_err();
        let current = service.read(repo_id, "a.bin").await.unwrap().unwrap();

        assert_eq!(error.status(), StatusCode::CONFLICT);
        assert_eq!(error.code(), "ARTIFACT_CONFLICT");
        assert_eq!(current.record.sha256, original.record.sha256);
        assert!(!rejected_blob.exists());
    }

    #[tokio::test]
    async fn 提交冲突不删除已被其他索引引用的新blob() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir).await;
        let first_repo = create_repo(&service, "conflict-first").await;
        let second_repo = create_repo(&service, "conflict-second").await;
        commit_bytes(&service, first_repo, "a.bin", b"old", false).await;

        let mut conflicting_writer = service.begin_write().await.unwrap();
        conflicting_writer.write_chunk(b"shared-new").await.unwrap();
        let conflicting_pending = conflicting_writer.finish().await.unwrap();
        let shared_hash = conflicting_pending.metadata().sha256.clone();
        commit_bytes(&service, second_repo, "b.bin", b"shared-new", false).await;

        let error = service
            .commit_pending(first_repo, "a.bin", conflicting_pending, false, false)
            .await
            .unwrap_err();
        let blob_path = service.blobs.blob_path(&shared_hash).unwrap();
        let mut read = service.read(second_repo, "b.bin").await.unwrap().unwrap();
        let mut bytes = Vec::new();
        read.blob.read_to_end(&mut bytes).await.unwrap();

        assert_eq!(error.status(), StatusCode::CONFLICT);
        assert!(blob_path.exists());
        assert_eq!(bytes, b"shared-new");
    }

    #[tokio::test]
    async fn allow覆盖后回收旧blob() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir).await;
        let repo_id = create_repo(&service, "allow-overwrite").await;
        let original = commit_bytes(&service, repo_id, "a.bin", b"old", false).await;
        let old_path = service.blobs.blob_path(&original.record.sha256).unwrap();

        let replaced = commit_bytes(&service, repo_id, "a.bin", b"new", true).await;

        assert!(!replaced.created);
        assert_ne!(replaced.record.sha256, original.record.sha256);
        assert!(!old_path.exists());
        assert!(
            service
                .blobs
                .blob_path(&replaced.record.sha256)
                .unwrap()
                .exists()
        );
    }

    #[tokio::test]
    async fn 同摘要覆盖不回收当前blob() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir).await;
        let repo_id = create_repo(&service, "same-overwrite").await;
        let original = commit_bytes(&service, repo_id, "a.bin", b"same", false).await;
        let blob_path = service.blobs.blob_path(&original.record.sha256).unwrap();

        let replaced = commit_bytes(&service, repo_id, "a.bin", b"same", true).await;

        assert!(!replaced.created);
        assert_eq!(replaced.record.sha256, original.record.sha256);
        assert!(blob_path.exists());
    }

    #[tokio::test]
    async fn 跨仓同hash仅最后一个索引删除时回收() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir).await;
        let first_repo = create_repo(&service, "shared-first").await;
        let second_repo = create_repo(&service, "shared-second").await;
        let first = commit_bytes(&service, first_repo, "a.bin", b"shared", false).await;
        commit_bytes(&service, second_repo, "b.bin", b"shared", false).await;
        let blob_path = service.blobs.blob_path(&first.record.sha256).unwrap();

        assert!(service.delete(first_repo, "a.bin").await.unwrap().is_some());
        assert!(blob_path.exists());
        assert!(
            service
                .delete(second_repo, "b.bin")
                .await
                .unwrap()
                .is_some()
        );
        assert!(!blob_path.exists());
        assert!(
            service
                .delete(second_repo, "b.bin")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn 删除等待并发读取lease后才物理回收() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir).await;
        let repo_id = create_repo(&service, "delete-read-race").await;
        let committed = commit_bytes(&service, repo_id, "a.bin", b"leased", false).await;
        let blob_path = service.blobs.blob_path(&committed.record.sha256).unwrap();
        let read = service.read(repo_id, "a.bin").await.unwrap().unwrap();
        let deleting_service = ArtifactService::new(service.pool.clone(), service.blobs.clone());
        let deleting =
            tokio::spawn(async move { deleting_service.delete(repo_id, "a.bin").await.unwrap() });

        tokio::task::yield_now().await;
        assert!(!deleting.is_finished(), "活跃读取期间删除必须等待");
        assert!(blob_path.exists());
        drop(read);

        let deleted = tokio::time::timeout(Duration::from_secs(1), deleting)
            .await
            .unwrap()
            .unwrap();
        assert!(deleted.is_some());
        assert!(!blob_path.exists());
    }

    #[tokio::test]
    async fn 覆盖提交后取消回收由启动恢复收口() {
        let dir = tempfile::tempdir().unwrap();
        let service = Arc::new(service(&dir).await);
        let repo_id = create_repo(&service, "overwrite-recovery").await;
        let original = commit_bytes(&service, repo_id, "a.bin", b"old", false).await;
        let old_path = service.blobs.blob_path(&original.record.sha256).unwrap();
        let read = service.read(repo_id, "a.bin").await.unwrap().unwrap();

        let replacing_service = Arc::clone(&service);
        let replacing = tokio::spawn(async move {
            commit_bytes(&replacing_service, repo_id, "a.bin", b"new", true).await
        });
        let new_hash = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let record = persistence::find_artifact_by_path(&service.pool, repo_id, "a.bin")
                    .await
                    .unwrap()
                    .unwrap();
                if record.sha256 != original.record.sha256 {
                    break record.sha256;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let intent: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM blob_reclamations WHERE sha256 = ? AND generation = 1",
        )
        .bind(&original.record.sha256)
        .fetch_one(&service.pool)
        .await
        .unwrap();
        assert_eq!(intent.0, 1, "索引提交时必须已经持久化回收意图");
        assert!(!replacing.is_finished(), "旧读取租约存续时回收必须等待");

        replacing.abort();
        assert!(replacing.await.unwrap_err().is_cancelled());
        drop(read);

        let recovered = ArtifactService::new(service.pool.clone(), service.blobs.clone());
        assert_eq!(recovered.recover_reclamations().await.unwrap(), 1);
        assert!(!old_path.exists());
        assert!(service.blobs.blob_path(&new_hash).unwrap().exists());
        assert_eq!(
            persistence::find_artifact_by_path(&service.pool, repo_id, "a.bin")
                .await
                .unwrap()
                .unwrap()
                .sha256,
            new_hash
        );
        let remaining: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM blob_reclamations")
            .fetch_one(&service.pool)
            .await
            .unwrap();
        assert_eq!(remaining.0, 0);
    }

    #[tokio::test]
    async fn 单飞领导任务独立于首个等待请求取消() {
        let dir = tempfile::tempdir().unwrap();
        let service = Arc::new(service(&dir).await);
        let runs = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let first_service = Arc::clone(&service);
        let first_runs = Arc::clone(&runs);
        let first = tokio::spawn(async move {
            first_service
                .proxy_singleflight(
                    1,
                    "detached",
                    "https://old.example/base",
                    move || async move {
                        let _sent = started_tx.send(());
                        release_rx
                            .await
                            .map_err(|_| ApiError::internal("单飞测试释放信号丢失"))?;
                        first_runs.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    },
                )
                .await
        });
        started_rx.await.unwrap();
        first.abort();

        let second_runs = Arc::clone(&runs);
        let mut second = Box::pin(service.proxy_singleflight(
            1,
            "detached",
            "https://old.example/base",
            move || async move {
                second_runs.fetch_add(100, Ordering::SeqCst);
                Ok(())
            },
        ));
        let completed_early = tokio::select! {
            _ = &mut second => true,
            () = tokio::task::yield_now() => false,
        };
        assert!(!completed_early, "领导任务释放前等待者不应完成");
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn 写入错误映射稳定状态码() {
        let dir = tempfile::tempdir().unwrap();
        let pool = persistence::test_pool().await;
        let blobs = BlobStore::new(dir.path().join("blobs"), 2, 0, 0);
        blobs.initialize().await.unwrap();
        let service = ArtifactService::new(pool, blobs);
        let mut writer = service.begin_write().await.unwrap();

        let error = writer.write_chunk(b"abc").await.unwrap_err();

        assert_eq!(error.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(error.code(), "ARTIFACT_TOO_LARGE");
    }
}
