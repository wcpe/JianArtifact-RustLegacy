//! blob 落定到索引提交之间的取消安全守卫。

use crate::meta::MetaStore;
use crate::storage::{BlobStore, BlobWriteLease};

/// 索引提交守卫：失败或 future 取消时，按最新引用计数协调清理无引用 blob。
pub(super) struct BlobIndexCommitGuard<S: BlobStore> {
    store: Option<S>,
    meta: Option<MetaStore>,
    sha256: String,
    lease: Option<BlobWriteLease>,
}

impl<S: BlobStore> BlobIndexCommitGuard<S> {
    /// 在 `put_pending` 返回后立即建立守卫。
    pub(super) fn new(store: S, meta: MetaStore, sha256: String, lease: BlobWriteLease) -> Self {
        Self {
            store: Some(store),
            meta: Some(meta),
            sha256,
            lease: Some(lease),
        }
    }

    /// 索引已成功提交，释放写入租约并解除清理责任。
    pub(super) fn commit(mut self) {
        self.lease.take();
        self.store.take();
        self.meta.take();
    }

    /// 显式失败路径：等待引用计数复查与协调清理完成。
    pub(super) async fn rollback(mut self) {
        let Some(store) = self.store.take() else {
            return;
        };
        let meta = self.meta.take().expect("索引提交守卫必须持有元数据存储");
        let lease = self.lease.take().expect("索引提交守卫必须持有写入租约");
        cleanup_blob(store, meta, self.sha256.clone(), lease).await;
    }
}

impl<S: BlobStore> Drop for BlobIndexCommitGuard<S> {
    fn drop(&mut self) {
        let (Some(store), Some(meta), Some(lease)) =
            (self.store.take(), self.meta.take(), self.lease.take())
        else {
            return;
        };
        let sha256 = self.sha256.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(cleanup_blob(store, meta, sha256, lease));
            }
            Err(error) => {
                tracing::error!(sha256 = %sha256, 错误 = %error, "取消清理 blob 时无可用 tokio runtime");
            }
        }
    }
}

/// 先释放 writer，再在 cleanup 排他期内复查引用计数并按需删除。
async fn cleanup_blob<S: BlobStore>(
    store: S,
    meta: MetaStore,
    sha256: String,
    lease: BlobWriteLease,
) {
    drop(lease);
    let count_meta = meta.clone();
    let count_sha256 = sha256.clone();
    let result = store
        .delete_if(&sha256, move || async move {
            match count_meta.count_artifacts_by_sha256(&count_sha256).await {
                Ok(0) => true,
                Ok(_) => false,
                Err(error) => {
                    tracing::warn!(sha256 = %count_sha256, 错误 = %error, "查询 blob 引用计数失败，跳过清理");
                    false
                }
            }
        })
        .await;
    if let Err(error) = result {
        tracing::warn!(sha256 = %sha256, 错误 = %error, "清理无引用 blob 失败");
    }
}
