//! Nexus **在线拉取** Docker/OCI 迁移（FR-144，增强 FR-82）。
//!
//! Docker/OCI 不是「downloadUrl → 普通文件」，而是一张按 digest 内容寻址的图：tag 指针 → manifest →
//! （config + 各 layer blob，或 manifest list → 子 manifest → …）。本模块按 **Docker Registry v2 协议**
//! 从源 Nexus 搬运整张图，逐级校验 digest，按本系统既有 docker 存储布局
//! （`{image}/blobs/sha256:...` / `{image}/manifests/sha256:...` / `{image}/tags/{tag}`）落库，
//! 使迁移后 `docker pull` 从本系统成功、digest 一致。
//!
//! 与普通文件在线搬运（[`super::online`] 的 `pull_one_asset`）**互不干扰**：docker 特有的图遍历逻辑
//! 单独封装于本文件，经统一进度 / 控制抽象（[`OnlinePullProgress`] / [`JobControl`]）接入，不把 docker
//! 分支硬塞进 raw/npm 的普通文件路径。
//!
//! 核心不变量（testing-and-quality §2.2/§2.4/§2.5）：
//! - **blob / manifest 先落盘并校验 sha256（内容寻址即校验），再写元数据索引**；失败回滚不留孤儿
//!   （由 [`DockerRegistry::ingest_blob`] / [`DockerRegistry::put_manifest`] 保证）。
//! - 流式下载 blob，不整体载入内存；manifest 通常较小，按内容读入以便按 digest 落库。
//! - 幂等重跑：已按 digest 落库且一致的 blob / manifest 跳过（内容寻址天然去重），进度区分新搬 vs 跳过。
//! - 单镜像 / 单 tag 失败记跳过、不中断整批；取消 / 暂停在 tag 边界响应（复用 FR-91）。

use crate::format::docker_registry::DockerWriteOutcome;
use crate::format::{DockerError, DockerRegistry};
use crate::meta::RepositoryRecord;
use crate::storage::BlobStore;

use super::online::{await_control, record_asset_failure};
use super::{
    parse_manifest_refs, DockerRegistryClient, JobControl, ManifestRefs, MigrateError,
    OnlinePullPhase, OnlinePullProgress,
};

/// 单个 docker manifest / blob 下载的最大尝试次数（含首次）：瞬时网络错误自动重试。
const MAX_DOCKER_ATTEMPTS: u32 = 3;

/// manifest list 递归搬运的最大深度：防御异常的自引用 / 深层嵌套（正常镜像仅 list → image 两层）。
const MAX_MANIFEST_DEPTH: u32 = 4;

/// 单个 Docker tag 的稳定、脱敏失败分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagFailureReason {
    ManifestRead,
    ManifestParse,
    ContentDigest,
    BlobDownload,
    BlobWrite,
    ManifestWrite,
}

impl TagFailureReason {
    fn message(self) -> &'static str {
        match self {
            Self::ManifestRead => "Docker manifest 读取失败",
            Self::ManifestParse => "Docker manifest 解析失败",
            Self::ContentDigest => "Docker 内容摘要不匹配",
            Self::BlobDownload => "Docker blob 下载失败",
            Self::BlobWrite => "Docker blob 写入失败",
            Self::ManifestWrite => "Docker manifest 写入失败",
        }
    }
}

/// 单源 docker 仓库在线搬运的结果计数（供上层汇总 [`super::OnlineRepoMigrationOutcome`]）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct DockerRepoPullResult {
    /// 成功新写入的 asset 数（blob + manifest + tag 指针，逐条计）。
    pub migrated: usize,
    /// 增量跳过数（目标已存在且 digest 一致）。
    pub skipped_existing: usize,
    /// 失败跳过数（镜像 / tag 拉取失败、digest 不符、layer 缺失等）。
    pub skipped: usize,
    /// 已发现的 blob 与 manifest payload 总字节数。
    pub total_bytes: u64,
    /// 已处理 blob 与 manifest payload 字节数。
    pub done_bytes: u64,
    /// 本次新写入 payload 字节数。
    pub migrated_bytes: u64,
    /// 幂等命中或失败跳过 payload 字节数。
    pub skipped_bytes: u64,
    /// 是否在 tag 边界被取消（提前结束本仓库、未搬完）。
    pub cancelled: bool,
}

/// 判断迁移网络错误是否为可重试的瞬时错误（传输失败 / 源系统 5xx；4xx 等确定性错误不重试）。
fn is_transient(e: &MigrateError) -> bool {
    match e {
        MigrateError::Transport(_) => true,
        MigrateError::Status(s) => *s >= 500,
        _ => false,
    }
}

/// 在线搬运单个源 docker 仓库（FR-144）：枚举镜像 / tag → 逐 tag 搬运 manifest 图。
///
/// 镜像枚举失败向上冒泡（整仓无法继续）；单镜像 / 单 tag 失败计跳过、不中断整批。每个 tag 处理前
/// 响应控制信号（FR-91）：取消则提前结束（`cancelled = true`）；暂停则挂起等待继续 / 取消。
/// 进度锁临界区只更新内存态，下载 / 落盘在锁外（锁外做 IO）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn migrate_online_docker_repo<C, S>(
    client: &C,
    registry: &DockerRegistry<S>,
    base_url: &str,
    source_repo: &str,
    target_repo: &RepositoryRecord,
    credential: Option<&super::NexusCredential>,
    progress: &std::sync::Mutex<OnlinePullProgress>,
    control: &JobControl,
) -> Result<DockerRepoPullResult, MigrateError>
where
    C: DockerRegistryClient,
    S: BlobStore,
{
    // 阶段1：枚举本仓库全部镜像与各镜像 tag（仅元数据，得知规模，便于进度条）
    let images = client
        .list_images(base_url, source_repo, credential)
        .await?;
    // 展平成 (image, tag) 待搬运项
    let mut targets: Vec<(String, String)> = Vec::new();
    for image in &images {
        let tags = client
            .list_tags(base_url, source_repo, image, credential)
            .await?;
        for tag in tags {
            targets.push((image.clone(), tag));
        }
    }

    {
        let mut p = lock_progress(progress);
        p.phase = OnlinePullPhase::Downloading;
    }

    // 阶段2：逐 (image, tag) 搬运 manifest 图，边搬边更新进度
    let mut result = DockerRepoPullResult::default();
    for (image, tag) in &targets {
        // 每个 tag 边界先响应控制信号（FR-91）：取消则提前结束；暂停则挂起等待
        if await_control(control, progress).await {
            result.cancelled = true;
            return Ok(result);
        }

        lock_progress(progress).current_path = Some(format!("{image}:{tag}"));

        let mut counts = TagCounts::default();
        let outcome = pull_tag(
            client,
            registry,
            base_url,
            source_repo,
            image,
            tag,
            target_repo,
            credential,
            &mut counts,
        )
        .await;

        // 失败时，已知但未完成的 manifest payload 按失败跳过计入。
        if outcome.is_err() {
            let failed_bytes = counts.total_bytes.saturating_sub(counts.done_bytes);
            counts.done_bytes = counts.total_bytes;
            counts.skipped_bytes = counts.skipped_bytes.saturating_add(failed_bytes);
        }
        // 无论 tag 整体成败，已成功搬运的 blob / manifest 计入对应计数（半途失败不回退已落定的合法 blob）
        let mut p = lock_progress(progress);
        result.migrated += counts.migrated;
        result.skipped_existing += counts.skipped_existing;
        result.total_bytes = result.total_bytes.saturating_add(counts.total_bytes);
        result.done_bytes = result.done_bytes.saturating_add(counts.done_bytes);
        result.migrated_bytes = result.migrated_bytes.saturating_add(counts.migrated_bytes);
        result.skipped_bytes = result.skipped_bytes.saturating_add(counts.skipped_bytes);
        p.migrated += counts.migrated;
        p.skipped_existing += counts.skipped_existing;
        p.total_bytes = p.total_bytes.saturating_add(counts.total_bytes);
        p.done_bytes = p.done_bytes.saturating_add(counts.done_bytes);
        p.migrated_bytes = p.migrated_bytes.saturating_add(counts.migrated_bytes);
        p.skipped_bytes = p.skipped_bytes.saturating_add(counts.skipped_bytes);
        let payloads = counts.migrated + counts.skipped_existing;
        p.total_assets += payloads;
        p.done_assets += payloads;
        if outcome.is_err() {
            result.skipped += 1;
            p.skipped += 1;
            p.total_assets += 1;
            p.done_assets += 1;
        }
        drop(p);
        if let Err(reason) = outcome {
            record_asset_failure(
                progress,
                source_repo,
                &format!("{image}:{tag}"),
                reason.message(),
            );
        }
    }

    Ok(result)
}

/// 单个 tag 搬运的中间计数（成功落定的 blob / manifest 逐条累加）。
#[derive(Default)]
struct TagCounts {
    /// 新写入数。
    migrated: usize,
    /// 增量跳过数。
    skipped_existing: usize,
    /// 已发现 payload 字节数。
    total_bytes: u64,
    /// 已处理 payload 字节数。
    done_bytes: u64,
    /// 新写入 payload 字节数。
    migrated_bytes: u64,
    /// 幂等命中 payload 字节数。
    skipped_bytes: u64,
}

/// 纯 tag 新增或改指向不重复统计 manifest payload；其余情况统计本次输入实体。
fn manifest_payload_size(
    reference: &str,
    outcome: &DockerWriteOutcome,
    payload_existed: bool,
) -> u64 {
    let tag_only = payload_existed
        && outcome.written
        && !outcome.payload_written
        && !crate::format::docker::is_digest_reference(reference);
    if tag_only {
        0
    } else {
        outcome.size
    }
}

/// 搬运单个 tag 的 manifest 图：拉 tag manifest → 遍历图搬 blob / 子 manifest → 写 manifest + tag 指针。
///
/// 任一步失败返回 `Err(())`（已记 WARN），由调用方计该 tag 失败跳过、不中断整批。已成功落定的 blob /
/// 子 manifest 保留（内容寻址、合法即有效），不因本 tag 后续失败而回滚。
#[allow(clippy::too_many_arguments)]
async fn pull_tag<C, S>(
    client: &C,
    registry: &DockerRegistry<S>,
    base_url: &str,
    source_repo: &str,
    image: &str,
    tag: &str,
    target_repo: &RepositoryRecord,
    credential: Option<&super::NexusCredential>,
    counts: &mut TagCounts,
) -> Result<(), TagFailureReason>
where
    C: DockerRegistryClient,
    S: BlobStore,
{
    // 先按 tag 拉顶层 manifest（可能是单架构 manifest 或 manifest list）
    let top = fetch_manifest_checked(client, base_url, source_repo, image, tag, credential)
        .await
        .map_err(|e| {
            tracing::warn!(仓库 = %target_repo.name, 镜像 = %image, tag = %tag, "拉取 tag manifest 失败，跳过");
            match e {
                FetchManifestError::DigestMismatch => TagFailureReason::ContentDigest,
                FetchManifestError::Read => TagFailureReason::ManifestRead,
            }
        })?;

    // 深度优先搬运 manifest 图：先把顶层 manifest 引用的 blob / 子 manifest 全搬入，再写顶层 manifest。
    // 顶层 manifest 以 tag 为 reference 写入（建立 tag 指针）；子 manifest 以 digest 为 reference 写入。
    ingest_manifest_graph(
        client,
        registry,
        base_url,
        source_repo,
        image,
        tag,
        &top,
        target_repo,
        credential,
        counts,
        0,
    )
    .await
}

/// 递归搬运一个 manifest 及其引用的图（config / layers 或子 manifests），最后写该 manifest 自身。
///
/// `reference` 为本 manifest 写入本系统时的引用（顶层用 tag、子 manifest 用其 digest）。先搬子对象、
/// 再写本 manifest，保证写 manifest 索引时其引用的 blob / 子 manifest 已就位（真源一致）。
#[allow(clippy::too_many_arguments)]
async fn ingest_manifest_graph<C, S>(
    client: &C,
    registry: &DockerRegistry<S>,
    base_url: &str,
    source_repo: &str,
    image: &str,
    reference: &str,
    manifest: &FetchedManifestChecked,
    target_repo: &RepositoryRecord,
    credential: Option<&super::NexusCredential>,
    counts: &mut TagCounts,
    depth: u32,
) -> Result<(), TagFailureReason>
where
    C: DockerRegistryClient,
    S: BlobStore,
{
    if depth > MAX_MANIFEST_DEPTH {
        tracing::warn!(仓库 = %target_repo.name, 镜像 = %image, "manifest 嵌套过深，跳过");
        return Err(TagFailureReason::ManifestParse);
    }

    counts.total_bytes = counts
        .total_bytes
        .saturating_add(u64::try_from(manifest.bytes.len()).unwrap_or(u64::MAX));
    // 解析本 manifest 引用的 blob / 子 manifest digest
    let refs: ManifestRefs = parse_manifest_refs(&manifest.bytes).map_err(|_| {
        tracing::warn!(仓库 = %target_repo.name, 镜像 = %image, 引用 = %reference, "manifest 解析失败，跳过");
        TagFailureReason::ManifestParse
    })?;

    // 单架构：搬 config + 各 layer blob（按 digest 内容寻址、校验）
    for digest in &refs.blob_digests {
        ingest_one_blob(
            client,
            registry,
            base_url,
            source_repo,
            image,
            digest,
            target_repo,
            credential,
            counts,
        )
        .await?;
    }

    // manifest list：对每个子 manifest，拉取后递归搬运（子 manifest 以其 digest 为 reference 写入）
    for sub_digest in &refs.sub_manifests {
        let sub =
            fetch_manifest_checked(client, base_url, source_repo, image, sub_digest, credential)
                .await
                .map_err(|e| {
                    tracing::warn!(仓库 = %target_repo.name, 镜像 = %image, digest = %sub_digest, "拉取子 manifest 失败，跳过");
                    match e {
                        FetchManifestError::DigestMismatch => TagFailureReason::ContentDigest,
                        FetchManifestError::Read => TagFailureReason::ManifestRead,
                    }
                })?;
        // 递归：先搬子 manifest 引用的 blob，再写子 manifest（按 digest）
        Box::pin(ingest_manifest_graph(
            client,
            registry,
            base_url,
            source_repo,
            image,
            sub_digest,
            &sub,
            target_repo,
            credential,
            counts,
            depth + 1,
        ))
        .await?;
    }

    // 图内引用就位后，写本 manifest 自身（顶层用 tag、子用 digest；put_manifest 幂等 + 内容寻址）
    let manifest_digest = format!("sha256:{}", sha256_hex(&manifest.bytes));
    let payload_existed = registry
        .get_manifest(target_repo, image, &manifest_digest)
        .await
        .is_ok();
    match registry
        .put_manifest(
            target_repo,
            image,
            reference,
            &manifest.media_type,
            manifest.bytes.clone(),
        )
        .await
    {
        Ok(outcome) => {
            let payload_size = manifest_payload_size(reference, &outcome, payload_existed);
            if payload_size == 0 {
                counts.total_bytes = counts.total_bytes.saturating_sub(outcome.size);
            }
            counts.done_bytes = counts.done_bytes.saturating_add(payload_size);
            if outcome.written {
                counts.migrated += 1;
            } else {
                counts.skipped_existing += 1;
            }
            if outcome.payload_written {
                counts.migrated_bytes = counts.migrated_bytes.saturating_add(payload_size);
            } else {
                counts.skipped_bytes = counts.skipped_bytes.saturating_add(payload_size);
            }
            Ok(())
        }
        Err(DockerError::DigestMismatch) => {
            tracing::warn!(仓库 = %target_repo.name, 镜像 = %image, 引用 = %reference, "manifest 内容摘要不匹配，跳过");
            Err(TagFailureReason::ContentDigest)
        }
        Err(_) => {
            tracing::warn!(仓库 = %target_repo.name, 镜像 = %image, 引用 = %reference, "写入 manifest 失败，跳过");
            Err(TagFailureReason::ManifestWrite)
        }
    }
}

/// 搬运单个 blob（config / layer）：幂等短路 → 流式下载 → 按 digest 校验落库。
///
/// 已按 digest 落库且一致则增量跳过（不重复下载）；否则流式下载后经
/// [`DockerRegistry::ingest_blob`] 落定（内部校验 sha256 与声明 digest 一致、失败回滚）。
#[allow(clippy::too_many_arguments)]
async fn ingest_one_blob<C, S>(
    client: &C,
    registry: &DockerRegistry<S>,
    base_url: &str,
    source_repo: &str,
    image: &str,
    digest: &str,
    target_repo: &RepositoryRecord,
    credential: Option<&super::NexusCredential>,
    counts: &mut TagCounts,
) -> Result<(), TagFailureReason>
where
    C: DockerRegistryClient,
    S: BlobStore,
{
    // 幂等短路：已按 digest 落库且一致 → 增量跳过，不重复下载
    match registry.blob_exists(target_repo, image, digest).await {
        Ok(true) => {
            let size = registry
                .stat_blob(target_repo, image, digest)
                .await
                .ok()
                .flatten()
                .and_then(|size| u64::try_from(size).ok())
                .unwrap_or(0);
            counts.skipped_existing += 1;
            counts.total_bytes = counts.total_bytes.saturating_add(size);
            counts.done_bytes = counts.done_bytes.saturating_add(size);
            counts.skipped_bytes = counts.skipped_bytes.saturating_add(size);
            return Ok(());
        }
        Ok(false) => {}
        Err(_) => {
            tracing::warn!(仓库 = %target_repo.name, 镜像 = %image, digest = %digest, "查 blob 是否存在失败，跳过");
            return Err(TagFailureReason::BlobWrite);
        }
    }

    // 流式下载 + 落库；瞬时网络错误自动重试（指数退避）
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let reader = match client
            .download_blob(base_url, source_repo, image, digest, credential)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                if is_transient(&e) && attempt < MAX_DOCKER_ATTEMPTS {
                    backoff(attempt).await;
                    continue;
                }
                tracing::warn!(仓库 = %target_repo.name, 镜像 = %image, digest = %digest, "下载 blob 失败，跳过");
                return Err(TagFailureReason::BlobDownload);
            }
        };

        match registry
            .ingest_blob(target_repo, image, digest, reader)
            .await
        {
            Ok(outcome) => {
                counts.total_bytes = counts.total_bytes.saturating_add(outcome.size);
                counts.done_bytes = counts.done_bytes.saturating_add(outcome.size);
                if outcome.written {
                    counts.migrated += 1;
                } else {
                    counts.skipped_existing += 1;
                }
                if outcome.payload_written {
                    counts.migrated_bytes = counts.migrated_bytes.saturating_add(outcome.size);
                } else {
                    counts.skipped_bytes = counts.skipped_bytes.saturating_add(outcome.size);
                }
                return Ok(());
            }
            // 内容寻址校验失败为确定性错误（源内容损坏 / digest 错配），不重试
            Err(DockerError::DigestMismatch) => {
                tracing::warn!(仓库 = %target_repo.name, 镜像 = %image, digest = %digest, "blob 内容与声明 digest 不符，回滚并跳过");
                return Err(TagFailureReason::ContentDigest);
            }
            Err(_) => {
                tracing::warn!(仓库 = %target_repo.name, 镜像 = %image, digest = %digest, "写入 blob 失败，跳过");
                return Err(TagFailureReason::BlobWrite);
            }
        }
    }
}

/// manifest 拉取阶段的稳定错误类型。
enum FetchManifestError {
    /// 源端读取或传输失败。
    Read,
    /// digest 引用与实际内容不一致。
    DigestMismatch,
}

/// 已按 digest 校验的 manifest：字节 + 媒体类型（校验通过后携带，供落库与图遍历复用）。
struct FetchedManifestChecked {
    /// manifest 原始字节。
    bytes: Vec<u8>,
    /// manifest 媒体类型。
    media_type: String,
}

/// 拉取 manifest 并校验 digest（当 reference 为 digest 引用时，落定内容 sha256 须与之一致）。
///
/// 瞬时网络错误自动重试。reference 为 tag 时不比对 digest（tag 是可变指针，无内容承诺）；为 digest 时
/// 校验拉到的字节 sha256 与 reference 一致，杜绝源返回错配内容。
async fn fetch_manifest_checked<C>(
    client: &C,
    base_url: &str,
    source_repo: &str,
    image: &str,
    reference: &str,
    credential: Option<&super::NexusCredential>,
) -> Result<FetchedManifestChecked, FetchManifestError>
where
    C: DockerRegistryClient,
{
    let mut attempt = 0u32;
    let fetched = loop {
        attempt += 1;
        match client
            .fetch_manifest(base_url, source_repo, image, reference, credential)
            .await
        {
            Ok(m) => break m,
            Err(e) => {
                if is_transient(&e) && attempt < MAX_DOCKER_ATTEMPTS {
                    backoff(attempt).await;
                    continue;
                }
                return Err(FetchManifestError::Read);
            }
        }
    };

    // reference 为 digest 引用时校验内容一致（tag 引用不校验）
    if crate::format::docker::is_digest_reference(reference) {
        let hex = sha256_hex(&fetched.bytes);
        match crate::format::docker::parse_digest(reference) {
            Some(expected) if expected == hex => {}
            _ => return Err(FetchManifestError::DigestMismatch),
        }
    }

    Ok(FetchedManifestChecked {
        bytes: fetched.bytes,
        media_type: fetched.media_type,
    })
}

/// 计算一段字节的 sha256 hex（小写）。
fn sha256_hex(data: &[u8]) -> String {
    use digest::Digest;
    let mut h = sha2::Sha256::new();
    h.update(data);
    format!("{:x}", h.finalize())
}

/// 重试前的指数退避等待 + 中文 WARN 日志（不打印凭据 / URL 明文）。
async fn backoff(attempt: u32) {
    let ms = 200u64 * 2u64.pow(attempt - 1);
    tracing::warn!(第 = attempt, "docker 迁移瞬时失败，退避后重试");
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}

/// 取进度锁（容忍中毒：恢复内部数据继续）。
fn lock_progress(
    p: &std::sync::Mutex<OnlinePullProgress>,
) -> std::sync::MutexGuard<'_, OnlinePullProgress> {
    p.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use crate::format::DockerRegistry;
    use crate::meta::{MetaStore, NewRepository, RepoType, RepositoryRecord, Visibility};
    use crate::migrate::FetchedManifest;
    use crate::storage::LocalFsStore;

    use digest::Digest;

    fn sha256_of(data: &[u8]) -> String {
        let mut h = sha2::Sha256::new();
        h.update(data);
        format!("{:x}", h.finalize())
    }

    fn digest_of(data: &[u8]) -> String {
        format!("sha256:{}", sha256_of(data))
    }

    fn assert_single_failure(progress: &Mutex<OnlinePullProgress>, reason: &str) {
        let p = progress.lock().unwrap();
        assert_eq!(p.asset_failures.len(), 1);
        assert_eq!(p.asset_failures[0].reason, reason);
    }

    /// mock registry v2 源：预置镜像 / tag、manifest（按 reference）、blob（按 digest）。
    struct MockRegistry {
        /// image → tag 列表。
        images: HashMap<String, Vec<String>>,
        /// (image, reference) → (bytes, media_type)。reference 可为 tag 或 digest。
        manifests: HashMap<(String, String), (Vec<u8>, String)>,
        /// (image, digest) → blob 字节。
        blobs: HashMap<(String, String), Vec<u8>>,
    }

    impl DockerRegistryClient for MockRegistry {
        async fn list_images(
            &self,
            _base: &str,
            _repo: &str,
            _c: Option<&super::super::NexusCredential>,
        ) -> Result<Vec<String>, MigrateError> {
            let mut v: Vec<String> = self.images.keys().cloned().collect();
            v.sort();
            Ok(v)
        }

        async fn list_tags(
            &self,
            _base: &str,
            _repo: &str,
            image: &str,
            _c: Option<&super::super::NexusCredential>,
        ) -> Result<Vec<String>, MigrateError> {
            Ok(self.images.get(image).cloned().unwrap_or_default())
        }

        async fn fetch_manifest(
            &self,
            _base: &str,
            _repo: &str,
            image: &str,
            reference: &str,
            _c: Option<&super::super::NexusCredential>,
        ) -> Result<FetchedManifest, MigrateError> {
            self.manifests
                .get(&(image.to_string(), reference.to_string()))
                .map(|(b, mt)| FetchedManifest {
                    bytes: b.clone(),
                    media_type: mt.clone(),
                })
                .ok_or(MigrateError::Status(404))
        }

        async fn download_blob(
            &self,
            _base: &str,
            _repo: &str,
            image: &str,
            digest: &str,
            _c: Option<&super::super::NexusCredential>,
        ) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>, MigrateError> {
            self.blobs
                .get(&(image.to_string(), digest.to_string()))
                .map(|b| {
                    Box::new(std::io::Cursor::new(b.clone()))
                        as Box<dyn tokio::io::AsyncRead + Send + Unpin>
                })
                .ok_or(MigrateError::Status(404))
        }
    }

    async fn 新建() -> (
        DockerRegistry<LocalFsStore>,
        MetaStore,
        RepositoryRecord,
        tempfile::TempDir,
    ) {
        新建_with_max(None).await
    }

    async fn 新建_with_max(
        max_size: Option<u64>,
    ) -> (
        DockerRegistry<LocalFsStore>,
        MetaStore,
        RepositoryRecord,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let meta = MetaStore::open_in_memory().await.unwrap();
        let store = LocalFsStore::new(dir.path().join("blobs")).await.unwrap();
        let reg = DockerRegistry::new(store, meta.clone(), dir.path().join("uploads"), max_size)
            .await
            .unwrap();
        let id = meta
            .create_repository(NewRepository {
                name: "hub",
                format: "docker",
                r#type: RepoType::Hosted,
                visibility: Visibility::Public,
                upstream_url: None,
                upstream_auth_ref: None,
            })
            .await
            .unwrap();
        let repo = meta.get_repository_by_id(&id).await.unwrap().unwrap();
        (reg, meta, repo, dir)
    }

    /// 构造一个单架构镜像的 mock：config + 一个 layer + 引用它们的 manifest，tag 为 `1.0`。
    fn 单架构镜像(image: &str) -> (MockRegistry, Vec<u8>, String) {
        let config = b"{\"architecture\":\"amd64\"}".to_vec();
        let layer = b"layer-bytes-xyz".to_vec();
        let cfg_d = digest_of(&config);
        let layer_d = digest_of(&layer);
        let manifest = format!(
            r#"{{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json","config":{{"mediaType":"application/vnd.docker.container.image.v1+json","digest":"{cfg_d}"}},"layers":[{{"digest":"{layer_d}"}}]}}"#
        )
        .into_bytes();
        let manifest_d = digest_of(&manifest);

        let mut images = HashMap::new();
        images.insert(image.to_string(), vec!["1.0".to_string()]);
        let mut manifests = HashMap::new();
        manifests.insert(
            (image.to_string(), "1.0".to_string()),
            (
                manifest.clone(),
                "application/vnd.docker.distribution.manifest.v2+json".to_string(),
            ),
        );
        let mut blobs = HashMap::new();
        blobs.insert((image.to_string(), cfg_d), config);
        blobs.insert((image.to_string(), layer_d), layer);
        (
            MockRegistry {
                images,
                manifests,
                blobs,
            },
            manifest,
            manifest_d,
        )
    }

    #[tokio::test]
    async fn 搬运单架构镜像_manifest_config_layer_落库可读回() {
        let (reg, _meta, repo, _d) = 新建().await;
        let (client, manifest_bytes, manifest_d) = 单架构镜像("app");
        let progress = Mutex::new(OnlinePullProgress::default());
        let control = JobControl::default();

        let result = migrate_online_docker_repo(
            &client,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &progress,
            &control,
        )
        .await
        .unwrap();

        // config + layer + manifest（tag 指针写入计一次）= 3 条新写
        assert_eq!(result.migrated, 3, "config + layer + manifest 各计一次");
        assert_eq!(result.skipped, 0);
        assert!(!result.cancelled);

        // 按 tag 读回 manifest，字节一致、digest 一致（迁移后 docker pull 可用）
        let by_tag = reg.get_manifest(&repo, "app", "1.0").await.unwrap();
        assert_eq!(by_tag.bytes, manifest_bytes);
        assert_eq!(by_tag.digest, manifest_d);
        // 按 digest 也可读回
        let by_digest = reg.get_manifest(&repo, "app", &manifest_d).await.unwrap();
        assert_eq!(by_digest.bytes, manifest_bytes);
    }

    #[tokio::test]
    async fn 幂等重跑_二次全增量跳过() {
        let (reg, _meta, repo, _d) = 新建().await;
        let (client, _m, _md) = 单架构镜像("app");
        let control = JobControl::default();

        let p1 = Mutex::new(OnlinePullProgress::default());
        let r1 = migrate_online_docker_repo(
            &client,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &p1,
            &control,
        )
        .await
        .unwrap();
        assert_eq!(r1.migrated, 3);

        // 二次跑：config + layer 已存在 → 增量跳过；manifest（tag 指针）每次覆盖写入
        let p2 = Mutex::new(OnlinePullProgress::default());
        let r2 = migrate_online_docker_repo(
            &client,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &p2,
            &control,
        )
        .await
        .unwrap();
        assert_eq!(
            r2.skipped_existing, 3,
            "config、layer 与 manifest 均增量跳过"
        );
        assert_eq!(r2.migrated, 0, "同 tag 状态一致时不重复写入");
        assert_eq!(r2.skipped, 0);
    }

    #[test]
    fn manifest_payload_字节区分新写幂等与纯_tag_更新() {
        let base = DockerWriteOutcome {
            written: true,
            payload_written: true,
            size: 128,
            digest: "sha256:test".to_string(),
        };
        assert_eq!(manifest_payload_size("latest", &base, false), 128);
        assert_eq!(
            manifest_payload_size(
                "latest",
                &DockerWriteOutcome {
                    written: false,
                    payload_written: false,
                    ..base.clone()
                },
                true
            ),
            128
        );
        assert_eq!(
            manifest_payload_size(
                "latest",
                &DockerWriteOutcome {
                    written: true,
                    payload_written: false,
                    ..base.clone()
                },
                true
            ),
            0
        );
        assert_eq!(
            manifest_payload_size(
                &format!("sha256:{}", "a".repeat(64)),
                &DockerWriteOutcome {
                    written: true,
                    payload_written: false,
                    ..base
                },
                true
            ),
            128
        );
    }

    #[tokio::test]
    async fn payload_已存在但_tag_缺失时_manifest_字节为零() {
        let (reg, meta, repo, _d) = 新建().await;
        let (client, _, _) = 单架构镜像("app");
        let expected_blob_bytes = client
            .blobs
            .values()
            .fold(0u64, |sum, bytes| sum.saturating_add(bytes.len() as u64));
        migrate_online_docker_repo(
            &client,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &Mutex::new(OnlinePullProgress::default()),
            &JobControl::default(),
        )
        .await
        .unwrap();
        let tag_key = crate::format::docker::tag_key("app", "1.0");
        sqlx::query("DELETE FROM artifacts WHERE repo_id = ? AND path = ?")
            .bind(&repo.id)
            .bind(tag_key)
            .execute(meta.pool())
            .await
            .unwrap();

        let progress = Mutex::new(OnlinePullProgress::default());
        let result = migrate_online_docker_repo(
            &client,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &progress,
            &JobControl::default(),
        )
        .await
        .unwrap();
        assert_eq!(result.migrated, 1, "仅补 tag 索引");
        assert_eq!(result.total_bytes, expected_blob_bytes);
        assert_eq!(result.done_bytes, expected_blob_bytes);
        assert_eq!(result.migrated_bytes, 0);
        assert_eq!(result.skipped_bytes, expected_blob_bytes);
    }

    #[tokio::test]
    async fn payload_已存在但_tag_改指向时_manifest_字节为零() {
        let (reg, _meta, repo, _d) = 新建().await;
        let (mut client, _, _) = 单架构镜像("app");
        let expected_blob_bytes = client
            .blobs
            .values()
            .fold(0u64, |sum, bytes| sum.saturating_add(bytes.len() as u64));
        migrate_online_docker_repo(
            &client,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &Mutex::new(OnlinePullProgress::default()),
            &JobControl::default(),
        )
        .await
        .unwrap();
        reg.put_manifest(
            &repo,
            "app",
            "latest",
            "application/vnd.docker.distribution.manifest.v2+json",
            br#"{"schemaVersion":2,"config":{"digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"layers":[]}"#.to_vec(),
        )
        .await
        .unwrap();
        client
            .images
            .insert("app".to_string(), vec!["latest".to_string()]);
        let source_manifest = client
            .manifests
            .remove(&("app".to_string(), "1.0".to_string()))
            .unwrap();
        client
            .manifests
            .insert(("app".to_string(), "latest".to_string()), source_manifest);

        let result = migrate_online_docker_repo(
            &client,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &Mutex::new(OnlinePullProgress::default()),
            &JobControl::default(),
        )
        .await
        .unwrap();

        assert_eq!(result.migrated, 1, "仅更新 tag 指向");
        assert_eq!(result.total_bytes, expected_blob_bytes);
        assert_eq!(result.done_bytes, expected_blob_bytes);
        assert_eq!(result.migrated_bytes, 0);
        assert_eq!(result.skipped_bytes, expected_blob_bytes);
    }

    #[tokio::test]
    async fn manifest_list_递归搬运子_manifest() {
        let (reg, _meta, repo, _d) = 新建().await;

        // 子镜像：config + layer + 子 manifest
        let config = b"{\"architecture\":\"arm64\"}".to_vec();
        let layer = b"arm64-layer".to_vec();
        let cfg_d = digest_of(&config);
        let layer_d = digest_of(&layer);
        let sub_manifest = format!(
            r#"{{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json","config":{{"digest":"{cfg_d}"}},"layers":[{{"digest":"{layer_d}"}}]}}"#
        )
        .into_bytes();
        let sub_d = digest_of(&sub_manifest);

        // manifest list 引用子 manifest
        let list = format!(
            r#"{{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.list.v2+json","manifests":[{{"digest":"{sub_d}","platform":{{"architecture":"arm64","os":"linux"}}}}]}}"#
        )
        .into_bytes();

        let mut images = HashMap::new();
        images.insert("multi".to_string(), vec!["latest".to_string()]);
        let mut manifests = HashMap::new();
        manifests.insert(
            ("multi".to_string(), "latest".to_string()),
            (
                list.clone(),
                "application/vnd.docker.distribution.manifest.list.v2+json".to_string(),
            ),
        );
        manifests.insert(
            ("multi".to_string(), sub_d.clone()),
            (
                sub_manifest.clone(),
                "application/vnd.docker.distribution.manifest.v2+json".to_string(),
            ),
        );
        let mut blobs = HashMap::new();
        blobs.insert(("multi".to_string(), cfg_d), config);
        blobs.insert(("multi".to_string(), layer_d), layer);
        let client = MockRegistry {
            images,
            manifests,
            blobs,
        };

        let progress = Mutex::new(OnlinePullProgress::default());
        let control = JobControl::default();
        let result = migrate_online_docker_repo(
            &client,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &progress,
            &control,
        )
        .await
        .unwrap();
        // config + layer + 子 manifest + list 各计一次 = 4
        assert_eq!(result.migrated, 4);
        assert_eq!(result.skipped, 0);

        // list 按 tag 读回、子 manifest 按 digest 读回，均落库
        let by_tag = reg.get_manifest(&repo, "multi", "latest").await.unwrap();
        assert_eq!(by_tag.bytes, list);
        let sub = reg.get_manifest(&repo, "multi", &sub_d).await.unwrap();
        assert_eq!(sub.bytes, sub_manifest);
    }

    #[tokio::test]
    async fn layer_缺失_该_tag_失败跳过不留半截() {
        let (reg, _meta, repo, _d) = 新建().await;
        // 构造引用一个不存在 blob 的 manifest
        let config = b"cfg".to_vec();
        let cfg_d = digest_of(&config);
        let missing_layer_d = format!("sha256:{}", "e".repeat(64));
        let manifest = format!(
            r#"{{"schemaVersion":2,"config":{{"digest":"{cfg_d}"}},"layers":[{{"digest":"{missing_layer_d}"}}]}}"#
        )
        .into_bytes();

        let mut images = HashMap::new();
        images.insert("app".to_string(), vec!["1.0".to_string()]);
        let mut manifests = HashMap::new();
        manifests.insert(
            ("app".to_string(), "1.0".to_string()),
            (
                manifest,
                "application/vnd.docker.distribution.manifest.v2+json".to_string(),
            ),
        );
        let mut blobs = HashMap::new();
        blobs.insert(("app".to_string(), cfg_d.clone()), config);
        // 缺 missing_layer_d
        let client = MockRegistry {
            images,
            manifests,
            blobs,
        };

        let progress = Mutex::new(OnlinePullProgress::default());
        let control = JobControl::default();
        let result = migrate_online_docker_repo(
            &client,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &progress,
            &control,
        )
        .await
        .unwrap();
        // 该 tag 失败跳过（layer 缺失）
        assert_eq!(result.skipped, 1);
        // manifest 未写入（layer 缺失时提前失败，不写 tag 指针）
        assert!(reg.get_manifest(&repo, "app", "1.0").await.is_err());
        // 但已成功搬运的 config blob 保留（内容寻址合法），不留半截 = 不留错配索引
        assert!(reg.blob_exists(&repo, "app", &cfg_d).await.unwrap());
        assert_single_failure(&progress, "Docker blob 下载失败");
    }

    #[tokio::test]
    async fn 损坏_manifest_digest_不符该镜像失败跳过() {
        let (reg, _meta, repo, _d) = 新建().await;
        // manifest 内容与其按 digest reference 声明的 digest 不符：按 digest 拉子 manifest 时应校验失败
        let real = b"real-manifest".to_vec();
        let wrong_d = format!("sha256:{}", "f".repeat(64)); // 与 real 内容不符的 digest
        let list = format!(
            r#"{{"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[{{"digest":"{wrong_d}"}}]}}"#
        )
        .into_bytes();

        let mut images = HashMap::new();
        images.insert("app".to_string(), vec!["latest".to_string()]);
        let mut manifests = HashMap::new();
        manifests.insert(
            ("app".to_string(), "latest".to_string()),
            (list, "application/vnd.oci.image.index.v1+json".to_string()),
        );
        // 按 wrong_d 拉到的却是 real（内容 sha256 != wrong_d）
        manifests.insert(
            ("app".to_string(), wrong_d.clone()),
            (
                real,
                "application/vnd.docker.distribution.manifest.v2+json".to_string(),
            ),
        );
        let client = MockRegistry {
            images,
            manifests,
            blobs: HashMap::new(),
        };

        let progress = Mutex::new(OnlinePullProgress::default());
        let control = JobControl::default();
        let result = migrate_online_docker_repo(
            &client,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &progress,
            &control,
        )
        .await
        .unwrap();
        // 子 manifest digest 校验失败 → 该 tag 失败跳过
        assert_eq!(result.skipped, 1);
        assert!(reg.get_manifest(&repo, "app", "latest").await.is_err());
        assert_single_failure(&progress, "Docker 内容摘要不匹配");
    }

    #[tokio::test]
    async fn manifest_读取失败记录安全分类() {
        let (reg, _meta, repo, _d) = 新建().await;
        let client = MockRegistry {
            images: HashMap::from([("app".to_string(), vec!["latest".to_string()])]),
            manifests: HashMap::new(),
            blobs: HashMap::new(),
        };
        let progress = Mutex::new(OnlinePullProgress::default());

        let result = migrate_online_docker_repo(
            &client,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &progress,
            &JobControl::default(),
        )
        .await
        .unwrap();

        assert_eq!(result.skipped, 1);
        assert_single_failure(&progress, "Docker manifest 读取失败");
    }

    #[tokio::test]
    async fn manifest_解析失败记录安全分类() {
        let (reg, _meta, repo, _d) = 新建().await;
        let client = MockRegistry {
            images: HashMap::from([("app".to_string(), vec!["latest".to_string()])]),
            manifests: HashMap::from([(
                ("app".to_string(), "latest".to_string()),
                (
                    b"not-json".to_vec(),
                    "application/vnd.oci.image.manifest.v1+json".to_string(),
                ),
            )]),
            blobs: HashMap::new(),
        };
        let progress = Mutex::new(OnlinePullProgress::default());

        migrate_online_docker_repo(
            &client,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &progress,
            &JobControl::default(),
        )
        .await
        .unwrap();

        assert_single_failure(&progress, "Docker manifest 解析失败");
    }

    #[tokio::test]
    async fn blob_摘要不匹配记录内容摘要分类() {
        let (reg, _meta, repo, _d) = 新建().await;
        let expected = b"expected";
        let digest = digest_of(expected);
        let manifest = format!(r#"{{"config":{{"digest":"{digest}"}},"layers":[]}}"#).into_bytes();
        let client = MockRegistry {
            images: HashMap::from([("app".to_string(), vec!["latest".to_string()])]),
            manifests: HashMap::from([(
                ("app".to_string(), "latest".to_string()),
                (
                    manifest,
                    "application/vnd.oci.image.manifest.v1+json".to_string(),
                ),
            )]),
            blobs: HashMap::from([(("app".to_string(), digest), b"wrong".to_vec())]),
        };
        let progress = Mutex::new(OnlinePullProgress::default());

        migrate_online_docker_repo(
            &client,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &progress,
            &JobControl::default(),
        )
        .await
        .unwrap();

        assert_single_failure(&progress, "Docker 内容摘要不匹配");
    }

    #[tokio::test]
    async fn blob_写入失败记录安全分类() {
        let (reg, _meta, repo, _d) = 新建_with_max(Some(2)).await;
        let blob = b"blob";
        let digest = digest_of(blob);
        let manifest = format!(r#"{{"config":{{"digest":"{digest}"}},"layers":[]}}"#).into_bytes();
        let client = MockRegistry {
            images: HashMap::from([("app".to_string(), vec!["latest".to_string()])]),
            manifests: HashMap::from([(
                ("app".to_string(), "latest".to_string()),
                (
                    manifest,
                    "application/vnd.oci.image.manifest.v1+json".to_string(),
                ),
            )]),
            blobs: HashMap::from([(("app".to_string(), digest), blob.to_vec())]),
        };
        let progress = Mutex::new(OnlinePullProgress::default());

        migrate_online_docker_repo(
            &client,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &progress,
            &JobControl::default(),
        )
        .await
        .unwrap();

        assert_single_failure(&progress, "Docker blob 写入失败");
    }

    #[tokio::test]
    async fn manifest_写入失败记录安全分类() {
        let (reg, _meta, repo, _d) = 新建_with_max(Some(64)).await;
        let (client, _, _) = 单架构镜像("app");
        let progress = Mutex::new(OnlinePullProgress::default());

        migrate_online_docker_repo(
            &client,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &progress,
            &JobControl::default(),
        )
        .await
        .unwrap();

        assert_single_failure(&progress, "Docker manifest 写入失败");
    }

    #[tokio::test]
    async fn 分页枚举后续镜像与_tag_仍进入搬运编排() {
        let (reg, _meta, repo, _d) = 新建().await;
        let (mut first, first_manifest, _) = 单架构镜像("first-image");
        let (mut later, later_manifest, _) = 单架构镜像("later-image");
        later
            .images
            .insert("later-image".to_string(), vec!["later-tag".to_string()]);
        let later_entry = later
            .manifests
            .remove(&("later-image".to_string(), "1.0".to_string()))
            .unwrap();
        later.manifests.insert(
            ("later-image".to_string(), "later-tag".to_string()),
            later_entry,
        );
        first.images.extend(later.images);
        first.manifests.extend(later.manifests);
        first.blobs.extend(later.blobs);

        let progress = Mutex::new(OnlinePullProgress::default());
        let result = migrate_online_docker_repo(
            &first,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &progress,
            &JobControl::default(),
        )
        .await
        .unwrap();

        assert_eq!(result.migrated, 6);
        assert_eq!(progress.lock().unwrap().total_assets, 6);
        assert_eq!(
            reg.get_manifest(&repo, "first-image", "1.0")
                .await
                .unwrap()
                .bytes,
            first_manifest
        );
        assert_eq!(
            reg.get_manifest(&repo, "later-image", "later-tag")
                .await
                .unwrap()
                .bytes,
            later_manifest
        );
    }

    #[tokio::test]
    async fn 空仓库无镜像_无搬运() {
        let (reg, _meta, repo, _d) = 新建().await;
        let client = MockRegistry {
            images: HashMap::new(),
            manifests: HashMap::new(),
            blobs: HashMap::new(),
        };
        let progress = Mutex::new(OnlinePullProgress::default());
        let control = JobControl::default();
        let result = migrate_online_docker_repo(
            &client,
            &reg,
            "https://nx",
            "docker-hosted",
            &repo,
            None,
            &progress,
            &control,
        )
        .await
        .unwrap();
        assert_eq!(result.migrated, 0);
        assert_eq!(result.skipped, 0);
        assert_eq!(progress.lock().unwrap().total_assets, 0);
    }
}
