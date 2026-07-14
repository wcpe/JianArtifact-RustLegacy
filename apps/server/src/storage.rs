//! 内容寻址 blob 存储：增量写入、四摘要计算、提交守卫与启动恢复。

use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};

use md5::Md5;
use rand::RngCore;
use rand::rngs::OsRng;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};
use tokio::sync::Notify;

/// 单次内部写入处理的固定块大小：8 MiB。
pub const BLOB_CHUNK_BYTES: usize = 8 * 1024 * 1024;
const CREATE_ATTEMPTS: usize = 16;

/// blob 存储错误。
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("blob 文件系统操作失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("制品大小超过上限")]
    SizeLimit,
    #[error("blob 可用磁盘空间不足")]
    LowSpace,
    #[error("提交守卫中的摘要无效")]
    InvalidGuard,
    #[error("无法分配随机临时文件")]
    TemporaryNameExhausted,
    #[error("blob 写入者已关闭")]
    WriterClosed,
}

/// 完成一次 blob 写入后得到的摘要与实际大小。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobMetadata {
    pub sha256: String,
    pub sha1: String,
    pub md5: String,
    pub sha512: String,
    pub size: u64,
}

#[derive(Debug, Default)]
struct BlobCoordinator {
    states: Mutex<HashMap<String, BlobState>>,
}

#[derive(Debug, Default)]
struct BlobState {
    active: usize,
    deleting: bool,
    waiting_deleters: usize,
    notify: Arc<Notify>,
}

impl BlobCoordinator {
    async fn acquire(self: &Arc<Self>, sha256: &str) -> BlobLease {
        loop {
            let notified = {
                let mut states = self.lock_states();
                let state = states.entry(sha256.to_owned()).or_default();
                if !state.deleting && state.waiting_deleters == 0 {
                    state.active = state.active.saturating_add(1);
                    return BlobLease {
                        coordinator: Arc::clone(self),
                        sha256: sha256.to_owned(),
                    };
                }
                let mut notified = Box::pin(Arc::clone(&state.notify).notified_owned());
                notified.as_mut().enable();
                notified
            };
            notified.await;
        }
    }

    async fn begin_delete(self: &Arc<Self>, sha256: &str) {
        let mut request = DeleteRequest::new(Arc::clone(self), sha256.to_owned());
        loop {
            let notified = {
                let mut states = self.lock_states();
                let state = states.entry(sha256.to_owned()).or_default();
                if state.active == 0 && !state.deleting {
                    state.waiting_deleters = state.waiting_deleters.saturating_sub(1);
                    state.deleting = true;
                    request.registered = false;
                    return;
                }
                let mut notified = Box::pin(Arc::clone(&state.notify).notified_owned());
                notified.as_mut().enable();
                notified
            };
            notified.await;
        }
    }

    fn try_begin_delete(&self, sha256: &str) -> bool {
        let mut states = self.lock_states();
        let state = states.entry(sha256.to_owned()).or_default();
        if state.active != 0 || state.deleting || state.waiting_deleters != 0 {
            return false;
        }
        state.deleting = true;
        true
    }

    fn lock_states(&self) -> MutexGuard<'_, HashMap<String, BlobState>> {
        self.states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct DeleteRequest {
    coordinator: Arc<BlobCoordinator>,
    sha256: String,
    registered: bool,
}

impl DeleteRequest {
    fn new(coordinator: Arc<BlobCoordinator>, sha256: String) -> Self {
        {
            let mut states = coordinator.lock_states();
            let state = states.entry(sha256.clone()).or_default();
            state.waiting_deleters = state.waiting_deleters.saturating_add(1);
        }
        Self {
            coordinator,
            sha256,
            registered: true,
        }
    }
}

impl Drop for DeleteRequest {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }
        let notify = {
            let mut states = self.coordinator.lock_states();
            let state = states.entry(self.sha256.clone()).or_default();
            state.waiting_deleters = state.waiting_deleters.saturating_sub(1);
            let notify = Arc::clone(&state.notify);
            if state.active == 0 && !state.deleting && state.waiting_deleters == 0 {
                states.remove(&self.sha256);
            }
            notify
        };
        notify.notify_waiters();
    }
}

struct BlobLease {
    coordinator: Arc<BlobCoordinator>,
    sha256: String,
}

impl Drop for BlobLease {
    fn drop(&mut self) {
        let notify = {
            let mut states = self.coordinator.lock_states();
            let Some(state) = states.get_mut(&self.sha256) else {
                return;
            };
            if state.active == 0 {
                return;
            }
            state.active -= 1;
            if state.active != 0 {
                return;
            }
            let notify = Arc::clone(&state.notify);
            if !state.deleting && state.waiting_deleters == 0 {
                states.remove(&self.sha256);
            }
            Some(notify)
        };
        if let Some(notify) = notify {
            notify.notify_waiters();
        }
    }
}

/// 正式 blob 读取句柄；句柄存续期间同摘要的物理删除会等待。
pub struct BlobReadHandle {
    file: tokio::fs::File,
    _lease: BlobLease,
}

impl AsyncRead for BlobReadHandle {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.file).poll_read(context, buffer)
    }
}

/// 正式 blob 删除许可；许可存续期间同摘要的新使用者会等待。
pub struct BlobDeletePermit {
    coordinator: Arc<BlobCoordinator>,
    sha256: String,
    blob_path: PathBuf,
}

impl BlobDeletePermit {
    /// 返回此许可唯一对应的 SHA-256。
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// 删除许可对应的正式 blob；文件已不存在时返回 false。
    pub async fn remove(self) -> Result<bool, StorageError> {
        match tokio::fs::remove_file(&self.blob_path).await {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}

impl Drop for BlobDeletePermit {
    fn drop(&mut self) {
        let notify = {
            let mut states = self.coordinator.lock_states();
            let state = states.entry(self.sha256.clone()).or_default();
            state.deleting = false;
            let notify = Arc::clone(&state.notify);
            if state.active == 0 && state.waiting_deleters == 0 {
                states.remove(&self.sha256);
            }
            notify
        };
        notify.notify_waiters();
    }
}

/// blob 存储根与非功能阈值。
#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
    max_artifact_bytes: u64,
    min_free_bytes: u64,
    min_free_percent: u8,
    coordinator: Arc<BlobCoordinator>,
}

impl BlobStore {
    /// 创建 blob 存储配置；目录由 initialize 显式建立。
    pub fn new(
        root: PathBuf,
        max_artifact_bytes: u64,
        min_free_bytes: u64,
        min_free_percent: u8,
    ) -> Self {
        Self {
            root,
            max_artifact_bytes,
            min_free_bytes,
            min_free_percent,
            coordinator: Arc::new(BlobCoordinator::default()),
        }
    }

    /// 建立临时目录与守卫目录。
    pub async fn initialize(&self) -> Result<(), StorageError> {
        tokio::fs::create_dir_all(self.guard_dir()).await?;
        Ok(())
    }

    /// 开始一次随机且独占的增量写入。
    pub async fn begin(&self) -> Result<BlobWriter, StorageError> {
        self.ensure_space()?;
        for _ in 0..CREATE_ATTEMPTS {
            let id = random_id();
            let part_path = self.temp_dir().join(format!("{id}.part"));
            match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&part_path)
                .await
            {
                Ok(file) => return Ok(BlobWriter::new(self.clone(), id, part_path, file)),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(StorageError::TemporaryNameExhausted)
    }

    /// 返回已校验摘要对应的正式内容寻址路径。
    pub fn blob_path(&self, sha256: &str) -> Result<PathBuf, StorageError> {
        if !valid_sha256(sha256) {
            return Err(StorageError::InvalidGuard);
        }
        Ok(self.root.join(&sha256[..2]).join(&sha256[2..]))
    }

    /// 先取得使用租约，再打开正式 blob，避免读取期间被物理删除。
    pub async fn open(&self, sha256: &str) -> Result<BlobReadHandle, StorageError> {
        let blob_path = self.blob_path(sha256)?;
        let lease = self.acquire_lease(sha256).await;
        let file = tokio::fs::File::open(blob_path).await?;
        Ok(BlobReadHandle {
            file,
            _lease: lease,
        })
    }

    /// 请求正式 blob 删除许可；请求建立后阻止同摘要的新租约，并等待现有租约释放。
    pub async fn begin_delete(&self, sha256: &str) -> Result<BlobDeletePermit, StorageError> {
        let blob_path = self.blob_path(sha256)?;
        self.coordinator.begin_delete(sha256).await;
        Ok(BlobDeletePermit {
            coordinator: Arc::clone(&self.coordinator),
            sha256: sha256.to_owned(),
            blob_path,
        })
    }

    /// 尝试取得正式 blob 删除许可；仅供启动恢复跳过当前活跃内容。
    pub fn try_begin_delete(&self, sha256: &str) -> Option<BlobDeletePermit> {
        let blob_path = self.blob_path(sha256).ok()?;
        self.coordinator
            .try_begin_delete(sha256)
            .then(|| BlobDeletePermit {
                coordinator: Arc::clone(&self.coordinator),
                sha256: sha256.to_owned(),
                blob_path,
            })
    }

    async fn acquire_lease(&self, sha256: &str) -> BlobLease {
        self.coordinator.acquire(sha256).await
    }

    #[cfg(test)]
    fn coordinator_state_count(&self) -> usize {
        self.coordinator.lock_states().len()
    }

    /// 清理所有 part，并仅删除有守卫且无数据库引用的正式 blob。
    pub async fn recover(
        &self,
        referenced: &HashSet<String>,
    ) -> Result<RecoveryReport, StorageError> {
        let mut report = RecoveryReport::default();
        let mut entries = tokio::fs::read_dir(self.temp_dir()).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|value| value == "part")
            {
                tokio::fs::remove_file(entry.path()).await?;
                report.removed_parts += 1;
            }
        }
        let mut guards = tokio::fs::read_dir(self.guard_dir()).await?;
        while let Some(entry) = guards.next_entry().await? {
            if !entry.file_type().await?.is_file() {
                continue;
            }
            let hash = tokio::fs::read_to_string(entry.path()).await?;
            if !valid_sha256(&hash) {
                return Err(StorageError::InvalidGuard);
            }
            if !referenced.contains(&hash) {
                let Some(permit) = self.try_begin_delete(&hash) else {
                    continue;
                };
                if permit.remove().await? {
                    report.removed_blobs += 1;
                }
            }
            tokio::fs::remove_file(entry.path()).await?;
            report.removed_guards += 1;
        }
        Ok(report)
    }

    fn ensure_space(&self) -> Result<(), StorageError> {
        let available = fs4::available_space(&self.root)?;
        let total = fs4::total_space(&self.root)?;
        let percent = if total == 0 {
            0
        } else {
            ((u128::from(available) * 100) / u128::from(total)) as u8
        };
        if available < self.min_free_bytes || percent < self.min_free_percent {
            return Err(StorageError::LowSpace);
        }
        Ok(())
    }

    fn temp_dir(&self) -> PathBuf {
        self.root.join(".tmp")
    }
    fn guard_dir(&self) -> PathBuf {
        self.temp_dir().join("guards")
    }
}

/// 尚未完成的增量 blob 写入者。
pub struct BlobWriter {
    store: BlobStore,
    id: String,
    part_path: PathBuf,
    file: Option<tokio::fs::File>,
    guard_path: Option<PathBuf>,
    guard_owns_target: bool,
    owns_part: bool,
    sha256: Sha256,
    sha1: Sha1,
    md5: Md5,
    sha512: Sha512,
    size: u64,
}

impl BlobWriter {
    fn new(store: BlobStore, id: String, part_path: PathBuf, file: tokio::fs::File) -> Self {
        Self {
            store,
            id,
            part_path,
            file: Some(file),
            guard_path: None,
            guard_owns_target: false,
            owns_part: true,
            sha256: Sha256::new(),
            sha1: Sha1::new(),
            md5: Md5::new(),
            sha512: Sha512::new(),
            size: 0,
        }
    }

    /// 写入一段数据并更新全部摘要与实际字节计数。
    pub async fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), StorageError> {
        let next_size = self
            .size
            .checked_add(chunk.len() as u64)
            .ok_or(StorageError::SizeLimit)?;
        if next_size > self.store.max_artifact_bytes {
            return Err(StorageError::SizeLimit);
        }
        self.store.ensure_space()?;
        for block in chunk.chunks(BLOB_CHUNK_BYTES) {
            self.file
                .as_mut()
                .ok_or(StorageError::WriterClosed)?
                .write_all(block)
                .await?;
            self.sha256.update(block);
            self.sha1.update(block);
            self.md5.update(block);
            self.sha512.update(block);
        }
        self.size = next_size;
        Ok(())
    }

    /// 刷盘、持久化守卫并争用最终路径，等待调用方提交数据库索引。
    pub async fn finish(mut self) -> Result<PendingBlob, StorageError> {
        let file = self.file.as_mut().ok_or(StorageError::WriterClosed)?;
        file.flush().await?;
        file.sync_all().await?;
        drop(self.file.take());
        let metadata = BlobMetadata {
            sha256: format!("{:x}", std::mem::take(&mut self.sha256).finalize()),
            sha1: format!("{:x}", std::mem::take(&mut self.sha1).finalize()),
            md5: format!("{:x}", std::mem::take(&mut self.md5).finalize()),
            sha512: format!("{:x}", std::mem::take(&mut self.sha512).finalize()),
            size: self.size,
        };
        let lease = self.store.acquire_lease(&metadata.sha256).await;
        let target = self.store.blob_path(&metadata.sha256)?;
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let guard_path = self.store.guard_dir().join(format!("{}.guard", self.id));
        let mut guard = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&guard_path)
            .await?;
        self.guard_path = Some(guard_path.clone());
        guard.write_all(metadata.sha256.as_bytes()).await?;
        guard.sync_all().await?;
        drop(guard);
        let created_new = match tokio::fs::hard_link(&self.part_path, &target).await {
            Ok(()) => {
                self.guard_owns_target = true;
                true
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                tokio::fs::remove_file(&guard_path).await?;
                self.guard_path = None;
                false
            }
            Err(error) => return Err(error.into()),
        };
        tokio::fs::remove_file(&self.part_path).await?;
        self.owns_part = false;
        self.guard_path = None;
        Ok(PendingBlob {
            metadata,
            guard_path: created_new.then_some(guard_path),
            _lease: lease,
        })
    }
}

impl Drop for BlobWriter {
    fn drop(&mut self) {
        drop(self.file.take());
        if !self.owns_part {
            return;
        }
        if let Some(guard_path) = &self.guard_path
            && !self.guard_owns_target
        {
            drop(std::fs::remove_file(guard_path));
        }
        drop(std::fs::remove_file(&self.part_path));
    }
}

/// 已进入正式路径、等待数据库索引提交结果的 blob。
pub struct PendingBlob {
    metadata: BlobMetadata,
    guard_path: Option<PathBuf>,
    _lease: BlobLease,
}

impl PendingBlob {
    /// 只读访问待提交 blob 的摘要与实际大小。
    pub fn metadata(&self) -> &BlobMetadata {
        &self.metadata
    }

    /// 索引提交失败后转为可协调清理的遗留 blob。
    pub(crate) fn abandon(self) -> AbandonedBlob {
        let Self {
            metadata,
            guard_path,
            _lease,
        } = self;
        drop(_lease);
        AbandonedBlob {
            sha256: metadata.sha256,
            guard_path,
        }
    }

    /// 数据库索引提交成功后尽力移除守卫并返回元数据。
    pub async fn complete(self) -> BlobMetadata {
        if let Some(guard_path) = &self.guard_path
            && let Err(error) = tokio::fs::remove_file(guard_path).await
            && error.kind() != ErrorKind::NotFound
        {
            tracing::warn!(error = %error, "制品索引已提交，但 blob 守卫清理失败，将在下次启动恢复");
        }
        self.metadata
    }
}

/// 索引提交失败后仍带提交守卫的 blob。
pub(crate) struct AbandonedBlob {
    sha256: String,
    guard_path: Option<PathBuf>,
}

impl AbandonedBlob {
    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }

    pub(crate) fn is_guarded(&self) -> bool {
        self.guard_path.is_some()
    }

    pub(crate) async fn clear_guard(self) -> Result<(), StorageError> {
        let Some(guard_path) = self.guard_path else {
            return Ok(());
        };
        match tokio::fs::remove_file(guard_path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

/// 启动恢复清理统计。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub removed_parts: u64,
    pub removed_guards: u64,
    pub removed_blobs: u64,
}

fn random_id() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    format!("{:032x}", u128::from_ne_bytes(bytes))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn store(dir: &tempfile::TempDir, max: u64, min_bytes: u64, min_percent: u8) -> BlobStore {
        BlobStore::new(dir.path().join("blobs"), max, min_bytes, min_percent)
    }

    async fn guard_count(store: &BlobStore) -> usize {
        let mut entries = tokio::fs::read_dir(store.guard_dir()).await.unwrap();
        let mut count = 0;
        while entries.next_entry().await.unwrap().is_some() {
            count += 1;
        }
        count
    }

    #[tokio::test]
    async fn finish保留守卫直到complete并返回四摘要() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 1024, 0, 0);
        store.initialize().await.unwrap();
        let mut writer = store.begin().await.unwrap();
        writer.write_chunk(b"a").await.unwrap();
        writer.write_chunk(b"bc").await.unwrap();
        let pending = writer.finish().await.unwrap();
        assert_eq!(guard_count(&store).await, 1);
        assert_eq!(pending.metadata().size, 3);
        assert_eq!(
            pending.metadata().sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            pending.metadata().sha1,
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(pending.metadata().md5, "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            pending.metadata().sha512,
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        assert_eq!(
            tokio::fs::read(store.blob_path(&pending.metadata().sha256).unwrap())
                .await
                .unwrap(),
            b"abc"
        );

        let metadata = pending.complete().await;
        assert_eq!(metadata.size, 3);
        assert_eq!(guard_count(&store).await, 0);
    }

    #[tokio::test]
    async fn drop未完成pending后由恢复清理零引用正式blob() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 1024, 0, 0);
        store.initialize().await.unwrap();
        let mut writer = store.begin().await.unwrap();
        writer.write_chunk(b"abandoned").await.unwrap();
        let pending = writer.finish().await.unwrap();
        let blob_path = store.blob_path(&pending.metadata().sha256).unwrap();
        assert!(blob_path.exists());
        assert_eq!(guard_count(&store).await, 1);

        drop(pending);
        assert!(blob_path.exists(), "drop PendingBlob 不得同步删除正式 blob");
        assert_eq!(guard_count(&store).await, 1);
        let report = store.recover(&HashSet::new()).await.unwrap();
        assert_eq!(report.removed_blobs, 1);
        assert_eq!(report.removed_guards, 1);
        assert!(!blob_path.exists());
    }

    #[tokio::test]
    async fn 崩溃在正式落定后且part未删时恢复仍清理零引用blob() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 1024, 0, 0);
        store.initialize().await.unwrap();
        let hash = "d".repeat(64);
        let blob_path = store.blob_path(&hash).unwrap();
        tokio::fs::create_dir_all(blob_path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&blob_path, b"crash").await.unwrap();
        tokio::fs::write(store.root.join(".tmp/crash.part"), b"crash")
            .await
            .unwrap();
        tokio::fs::write(store.root.join(".tmp/guards/crash.guard"), &hash)
            .await
            .unwrap();

        let report = store.recover(&HashSet::new()).await.unwrap();
        assert_eq!(report.removed_parts, 1);
        assert_eq!(report.removed_guards, 1);
        assert_eq!(report.removed_blobs, 1);
        assert!(!blob_path.exists());
    }

    #[tokio::test]
    async fn 预存目标不保留守卫且丢弃pending不能删除既有blob() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 1024, 0, 0);
        store.initialize().await.unwrap();
        let mut first = store.begin().await.unwrap();
        first.write_chunk(b"shared").await.unwrap();
        let metadata = first.finish().await.unwrap().complete().await;
        let blob_path = store.blob_path(&metadata.sha256).unwrap();

        let mut second = store.begin().await.unwrap();
        second.write_chunk(b"shared").await.unwrap();
        let pending = second.finish().await.unwrap();
        assert_eq!(guard_count(&store).await, 0);
        drop(pending);
        assert!(blob_path.exists());

        let report = store.recover(&HashSet::new()).await.unwrap();
        assert_eq!(report.removed_blobs, 0);
        assert!(blob_path.exists());
    }

    #[tokio::test]
    async fn 恢复遇到同摘要活跃使用者时保留守卫等待重试() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 1024, 0, 0);
        store.initialize().await.unwrap();
        let mut writer = store.begin().await.unwrap();
        writer.write_chunk(b"shared-active").await.unwrap();
        let pending = writer.finish().await.unwrap();
        let hash = pending.metadata().sha256.clone();
        let blob_path = store.blob_path(&hash).unwrap();
        let other_lease = store.acquire_lease(&hash).await;
        drop(pending);

        let skipped = store.recover(&HashSet::new()).await.unwrap();
        assert_eq!(skipped.removed_blobs, 0);
        assert!(blob_path.exists());
        assert_eq!(guard_count(&store).await, 1);

        drop(other_lease);
        let report = store.recover(&HashSet::new()).await.unwrap();
        assert_eq!(report.removed_blobs, 1);
        assert!(!blob_path.exists());
    }

    #[tokio::test]
    async fn 写入失败后drop立即清理自己的part() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 2, 0, 0);
        store.initialize().await.unwrap();
        let mut writer = store.begin().await.unwrap();
        let part_path = writer.part_path.clone();
        writer.write_chunk(b"a").await.unwrap();
        assert!(matches!(
            writer.write_chunk(b"bc").await,
            Err(StorageError::SizeLimit)
        ));

        drop(writer);
        assert!(!part_path.exists());
    }

    #[tokio::test]
    async fn 以实际字节计数拒绝超过上限() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 2, 0, 0);
        store.initialize().await.unwrap();
        let mut writer = store.begin().await.unwrap();
        assert!(matches!(
            writer.write_chunk(b"abc").await,
            Err(StorageError::SizeLimit)
        ));
    }

    #[tokio::test]
    async fn 磁盘可用空间低于字节阈值时拒绝开始() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 1024, u64::MAX, 0);
        store.initialize().await.unwrap();
        assert!(matches!(store.begin().await, Err(StorageError::LowSpace)));
    }

    #[tokio::test]
    async fn 摘要协调状态在最后租约释放后回收() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 1024, 0, 0);
        store.initialize().await.unwrap();
        let mut writer = store.begin().await.unwrap();
        writer.write_chunk(b"state-cleanup").await.unwrap();
        let pending = writer.finish().await.unwrap();
        assert_eq!(store.coordinator_state_count(), 1);

        pending.complete().await;
        assert_eq!(store.coordinator_state_count(), 0);
    }

    #[tokio::test]
    async fn 读取句柄生命周期阻止删除() {
        use tokio::io::AsyncReadExt;

        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 1024, 0, 0);
        store.initialize().await.unwrap();
        let mut writer = store.begin().await.unwrap();
        writer.write_chunk(b"read-lease").await.unwrap();
        let metadata = writer.finish().await.unwrap().complete().await;
        let mut handle = store.open(&metadata.sha256).await.unwrap();
        let mut bytes = Vec::new();
        handle.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"read-lease");

        let deleting_store = store.clone();
        let deleting_hash = metadata.sha256.clone();
        let deleting =
            tokio::spawn(async move { deleting_store.begin_delete(&deleting_hash).await.unwrap() });
        tokio::task::yield_now().await;
        assert!(!deleting.is_finished(), "读取句柄存续时删除必须等待");

        drop(handle);
        let permit = tokio::time::timeout(std::time::Duration::from_secs(1), deleting)
            .await
            .unwrap()
            .unwrap();
        drop(permit);
    }

    #[tokio::test]
    async fn begin_delete等待活跃lease并阻止新lease() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 1024, 0, 0);
        store.initialize().await.unwrap();
        let mut writer = store.begin().await.unwrap();
        writer.write_chunk(b"delete-wait").await.unwrap();
        let metadata = writer.finish().await.unwrap().complete().await;
        let active = store.open(&metadata.sha256).await.unwrap();

        let deleting_store = store.clone();
        let deleting_hash = metadata.sha256.clone();
        let deleting =
            tokio::spawn(async move { deleting_store.begin_delete(&deleting_hash).await.unwrap() });
        tokio::task::yield_now().await;
        assert!(!deleting.is_finished());

        let reading_store = store.clone();
        let reading_hash = metadata.sha256.clone();
        let reading = tokio::spawn(async move { reading_store.open(&reading_hash).await });
        tokio::task::yield_now().await;
        assert!(!reading.is_finished(), "删除请求后不得取得新 lease");

        drop(active);
        let permit = tokio::time::timeout(std::time::Duration::from_secs(1), deleting)
            .await
            .unwrap()
            .unwrap();
        assert!(!reading.is_finished(), "删除许可存续时新 lease 仍须等待");
        drop(permit);
        let handle = tokio::time::timeout(std::time::Duration::from_secs(1), reading)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(handle);
    }

    #[tokio::test]
    async fn 多个begin_delete串行取得许可且不丢通知() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 1024, 0, 0);
        let hash = "a".repeat(64);
        let first_store = store.clone();
        let first_hash = hash.clone();
        let first =
            tokio::spawn(async move { first_store.begin_delete(&first_hash).await.unwrap() });
        let first_permit = first.await.unwrap();

        let second_store = store.clone();
        let second_hash = hash.clone();
        let second =
            tokio::spawn(async move { second_store.begin_delete(&second_hash).await.unwrap() });
        tokio::task::yield_now().await;
        assert!(!second.is_finished());

        drop(first_permit);
        let second_permit = tokio::time::timeout(std::time::Duration::from_secs(1), second)
            .await
            .unwrap()
            .unwrap();
        drop(second_permit);
    }

    #[tokio::test]
    async fn 活跃lease阻止try_begin_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 1024, 0, 0);
        let hash = "b".repeat(64);
        let lease = store.acquire_lease(&hash).await;

        assert!(store.try_begin_delete(&hash).is_none());
        drop(lease);
        assert!(store.try_begin_delete(&hash).is_some());
    }

    #[tokio::test]
    async fn 删除permit生命周期内新lease阻塞且释放后完成() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 1024, 0, 0);
        let hash = "b".repeat(64);
        let permit = store.try_begin_delete(&hash).unwrap();
        let waiting_store = store.clone();
        let waiting_hash = hash.clone();
        let waiter = tokio::spawn(async move { waiting_store.acquire_lease(&waiting_hash).await });

        tokio::task::yield_now().await;
        assert!(!waiter.is_finished(), "deleting 期间新 lease 必须等待");
        drop(permit);
        let lease = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
        drop(lease);
    }

    #[tokio::test]
    async fn 不同sha不互相阻塞() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 1024, 0, 0);
        let deleting_hash = "c".repeat(64);
        let other_hash = "d".repeat(64);
        let _permit = store.try_begin_delete(&deleting_hash).unwrap();

        let lease = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            store.acquire_lease(&other_hash),
        )
        .await
        .unwrap();
        drop(lease);
    }

    #[tokio::test]
    async fn 恢复只清理part与守卫指向的零引用正式blob() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 1024, 0, 0);
        store.initialize().await.unwrap();
        let referenced = "a".repeat(64);
        let abandoned = "b".repeat(64);
        let unguarded = "c".repeat(64);
        for hash in [&referenced, &abandoned, &unguarded] {
            let path = store.blob_path(hash).unwrap();
            tokio::fs::create_dir_all(path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(path, hash.as_bytes()).await.unwrap();
        }
        tokio::fs::write(store.root.join(".tmp/left.part"), b"partial")
            .await
            .unwrap();
        tokio::fs::write(store.root.join(".tmp/guards/ref.guard"), &referenced)
            .await
            .unwrap();
        tokio::fs::write(store.root.join(".tmp/guards/abandoned.guard"), &abandoned)
            .await
            .unwrap();
        let report = store
            .recover(&HashSet::from([referenced.clone()]))
            .await
            .unwrap();
        assert_eq!(
            report,
            RecoveryReport {
                removed_parts: 1,
                removed_guards: 2,
                removed_blobs: 1
            }
        );
        assert!(store.blob_path(&referenced).unwrap().exists());
        assert!(!store.blob_path(&abandoned).unwrap().exists());
        assert!(
            store.blob_path(&unguarded).unwrap().exists(),
            "无守卫的无引用正式 blob 必须保留"
        );
    }
}
