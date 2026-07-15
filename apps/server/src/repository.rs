//! 仓库领域服务：仓库元数据生命周期、group 关系与 ACL。

use std::collections::{HashMap, HashSet};

use sqlx::SqlitePool;

use crate::artifact::ArtifactService;
use crate::domain::artifact::ArtifactDirectory;
use crate::domain::repository::{
    AccessSurface, AclRecord, AuthorizationDecision, Format, OverwritePolicy, Permission,
    ProtocolRepositoryContext, RepoType, Repository, Subject, VersionPolicy, Visibility, authorize,
};
use crate::error::ApiError;
use crate::persistence::{
    self, ArtifactDirectoryEntry, ArtifactRecord, NewRepositoryRow, RepositoryDeleteOutcome,
    RepositoryRowPatch,
};

/// 创建仓库输入。
#[derive(Debug)]
pub struct CreateRepository {
    pub name: String,
    pub format: Format,
    pub repo_type: RepoType,
    pub visibility: Visibility,
    pub overwrite_policy: Option<OverwritePolicy>,
    pub version_policy: Option<VersionPolicy>,
    pub upstream_url: Option<String>,
    pub upstream_credential_ref: Option<String>,
    pub members: Option<Vec<String>>,
}

/// 仓库部分更新输入。
#[derive(Debug, Default)]
pub struct RepositoryPatch {
    pub visibility: Option<Visibility>,
    pub disabled: Option<bool>,
    pub overwrite_policy: Option<OverwritePolicy>,
    pub version_policy: Option<VersionPolicy>,
    pub upstream_url: Option<String>,
    pub upstream_credential_ref: Option<Option<String>>,
    pub members: Option<Vec<String>>,
}

/// 仓库删除模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMode {
    /// 删除仓库配置与制品索引，保留无引用 blob 等待后续 GC。
    Config,
    /// 删除仓库配置与制品索引，并回收无其他引用的 blob。
    Data,
}

/// 仓库删除结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteRepositoryResult {
    pub orphaned_blobs: i64,
    pub reclaimed_blobs: i64,
}

struct ResolvedCreate {
    row: NewRepositoryRow,
    members: Option<Vec<String>>,
}

/// 创建仓库，仅允许启用的全局管理员。
pub async fn create_repository(
    pool: &SqlitePool,
    subject: Subject,
    input: CreateRepository,
) -> Result<Repository, ApiError> {
    ensure_global_admin(subject)?;
    let resolved = resolve_create(input)?;
    let member_ids = match resolved.members.as_deref() {
        Some(names) => resolve_member_ids(pool, resolved.row.format, names, None, true).await?,
        None => Vec::new(),
    };
    persistence::insert_repository(pool, &resolved.row, &member_ids).await
}

/// 在 SQL 中按读取权限过滤并分页列出仓库。
pub async fn list_repositories(
    pool: &SqlitePool,
    subject: Subject,
    page: u32,
    page_size: u32,
    q: Option<&str>,
) -> Result<(Vec<Repository>, i64), ApiError> {
    ensure_subject_valid(subject)?;
    persistence::list_repositories(
        pool,
        subject.user_id(),
        subject.is_global_admin(),
        page,
        page_size,
        q,
    )
    .await
}

/// 读取仓库详情；private 无 read 与不存在统一返回 404。
pub async fn get_repository(
    pool: &SqlitePool,
    subject: Subject,
    name: &str,
) -> Result<Repository, ApiError> {
    authorized_repository(pool, subject, name, Permission::Read).await
}

/// 分页浏览仓库目录，仅返回直接子项。
pub async fn browse_artifacts(
    pool: &SqlitePool,
    artifacts: &ArtifactService,
    subject: Subject,
    name: &str,
    path: &str,
    page: u32,
    page_size: u32,
) -> Result<(Vec<ArtifactDirectoryEntry>, i64), ApiError> {
    let repository = authorized_repository(pool, subject, name, Permission::Read).await?;
    let directory = ArtifactDirectory::parse(path)
        .map_err(|_| ApiError::bad_request("ARTIFACT_INVALID_PATH", "制品目录路径无效"))?;
    let result = artifacts
        .list_directory(repository.id, directory.as_str(), page, page_size)
        .await?;
    if !directory.as_str().is_empty() && result.1 == 0 {
        return Err(ApiError::not_found("ARTIFACT_NOT_FOUND", "制品目录不存在"));
    }
    Ok(result)
}

/// 分页搜索仓库内制品路径。
pub async fn search_artifacts(
    pool: &SqlitePool,
    artifacts: &ArtifactService,
    subject: Subject,
    name: &str,
    query: &str,
    page: u32,
    page_size: u32,
) -> Result<(Vec<ArtifactRecord>, i64), ApiError> {
    let repository = authorized_repository(pool, subject, name, Permission::Read).await?;
    if query.is_empty() {
        return Err(ApiError::bad_request(
            "ARTIFACT_INVALID_QUERY",
            "制品搜索关键字不能为空",
        ));
    }
    artifacts
        .search(repository.id, query, page, page_size)
        .await
}

/// 部分更新仓库字段，并原子替换 group 成员关系。
pub async fn update_repository(
    pool: &SqlitePool,
    artifacts: &ArtifactService,
    subject: Subject,
    name: &str,
    patch: RepositoryPatch,
) -> Result<Repository, ApiError> {
    let repository = authorized_repository(pool, subject, name, Permission::Admin).await?;
    validate_patch(&repository, &patch)?;
    let member_ids = match patch.members.as_deref() {
        Some(names) => Some(
            resolve_member_ids(
                pool,
                repository.format,
                names,
                subject.user_id(),
                subject.is_global_admin(),
            )
            .await?,
        ),
        None => None,
    };
    let row_patch = RepositoryRowPatch {
        visibility: patch.visibility,
        disabled: patch.disabled,
        overwrite_policy: patch.overwrite_policy,
        version_policy: patch.version_policy,
        upstream_url: patch.upstream_url,
        upstream_credential_ref: patch.upstream_credential_ref,
    };
    let reclamations = persistence::update_repository_record(
        pool,
        repository.id,
        &row_patch,
        member_ids.as_deref(),
    )
    .await?;
    artifacts.reclaim_reclamations(&reclamations).await?;
    get_repository(pool, subject, name).await
}

/// 删除仓库配置与索引；data 模式同时回收无其他引用的 blob。
pub async fn delete_repository(
    pool: &SqlitePool,
    artifacts: &ArtifactService,
    subject: Subject,
    name: &str,
    mode: DeleteMode,
    confirmation: Option<&str>,
) -> Result<DeleteRepositoryResult, ApiError> {
    let repository = authorized_repository(pool, subject, name, Permission::Admin).await?;
    ensure_delete_confirmation(confirmation, name)?;
    let reclaim = mode == DeleteMode::Data;
    let deleted = persistence::delete_repository_record(pool, repository.id, reclaim).await?;
    let RepositoryDeleteOutcome::Deleted {
        hashes,
        reclamations,
    } = deleted
    else {
        return Err(ApiError::conflict("REPO_IN_USE", "仓库仍被 group 引用"));
    };
    delete_repository_blobs(artifacts, mode, &hashes, &reclamations).await
}

async fn delete_repository_blobs(
    artifacts: &ArtifactService,
    mode: DeleteMode,
    hashes: &[String],
    reclamations: &[persistence::BlobReclamation],
) -> Result<DeleteRepositoryResult, ApiError> {
    match mode {
        DeleteMode::Config => Ok(DeleteRepositoryResult {
            orphaned_blobs: blob_count(artifacts.count_unreferenced_candidates(hashes).await?)?,
            reclaimed_blobs: 0,
        }),
        DeleteMode::Data => Ok(DeleteRepositoryResult {
            orphaned_blobs: 0,
            reclaimed_blobs: blob_count(artifacts.reclaim_reclamations(reclamations).await?)?,
        }),
    }
}

fn blob_count(value: u64) -> Result<i64, ApiError> {
    i64::try_from(value).map_err(|_| ApiError::internal("blob 统计超出支持范围"))
}

/// 列出仓库 ACL。
pub async fn list_acl(
    pool: &SqlitePool,
    subject: Subject,
    name: &str,
) -> Result<Vec<AclRecord>, ApiError> {
    let repository = authorized_repository(pool, subject, name, Permission::Admin).await?;
    persistence::list_repository_acl(pool, repository.id).await
}

/// 按精确用户名新增或更新 ACL。
pub async fn set_acl(
    pool: &SqlitePool,
    subject: Subject,
    name: &str,
    username: &str,
    permission: Permission,
) -> Result<AclRecord, ApiError> {
    let repository = authorized_repository(pool, subject, name, Permission::Admin).await?;
    let user = persistence::find_user_by_username(pool, username)
        .await?
        .ok_or_else(user_not_found)?;
    persistence::upsert_repository_acl(pool, repository.id, user.id, permission).await?;
    Ok(AclRecord {
        user_id: user.id,
        username: user.username,
        permission,
    })
}

/// 按精确用户名移除 ACL。
pub async fn remove_acl(
    pool: &SqlitePool,
    subject: Subject,
    name: &str,
    username: &str,
) -> Result<(), ApiError> {
    let repository = authorized_repository(pool, subject, name, Permission::Admin).await?;
    let user = persistence::find_user_by_username(pool, username)
        .await?
        .ok_or_else(user_not_found)?;
    if !persistence::delete_repository_acl(pool, repository.id, user.id).await? {
        return Err(ApiError::not_found(
            "REPO_ACL_NOT_FOUND",
            "仓库 ACL 条目不存在",
        ));
    }
    Ok(())
}

async fn authorized_repository(
    pool: &SqlitePool,
    subject: Subject,
    name: &str,
    action: Permission,
) -> Result<Repository, ApiError> {
    authorized_repository_for(pool, subject, name, action, AccessSurface::Management).await
}

/// 在格式协议入口校验仓库权限；停用仓库与私有无权仓库统一隐藏。
pub async fn protocol_repository(
    pool: &SqlitePool,
    subject: Subject,
    name: &str,
    action: Permission,
) -> Result<Repository, ApiError> {
    authorized_repository_for(pool, subject, name, action, AccessSurface::Protocol).await
}

/// 校验 group 本身读取权限，再批量加载并过滤协议面不可读成员。
pub async fn protocol_repository_context(
    pool: &SqlitePool,
    subject: Subject,
    name: &str,
) -> Result<ProtocolRepositoryContext, ApiError> {
    let repository = protocol_repository(pool, subject, name, Permission::Read).await?;
    let members = if repository.repo_type == RepoType::Group {
        protocol_group_members(pool, subject, repository.id).await?
    } else {
        Vec::new()
    };
    Ok(ProtocolRepositoryContext {
        repository,
        members,
    })
}

async fn protocol_group_members(
    pool: &SqlitePool,
    subject: Subject,
    group_repo_id: i64,
) -> Result<Vec<Repository>, ApiError> {
    let candidates =
        persistence::find_protocol_group_member_access(pool, group_repo_id, subject.user_id())
            .await?;
    Ok(candidates
        .into_iter()
        .filter(|(repository, granted)| {
            authorize(
                subject,
                repository.visibility,
                *granted,
                Permission::Read,
                repository.disabled,
                AccessSurface::Protocol,
            ) == AuthorizationDecision::Allowed
        })
        .map(|(repository, _)| repository)
        .collect())
}

async fn authorized_repository_for(
    pool: &SqlitePool,
    subject: Subject,
    name: &str,
    action: Permission,
    surface: AccessSurface,
) -> Result<Repository, ApiError> {
    ensure_subject_valid(subject)?;
    let (repository, granted) = persistence::find_repository_access(
        pool,
        name,
        subject.user_id(),
        subject.is_global_admin(),
    )
    .await?
    .ok_or_else(repository_not_found)?;
    let decision = authorize(
        subject,
        repository.visibility,
        granted,
        action,
        repository.disabled,
        surface,
    );
    map_authorization(decision)?;
    Ok(repository)
}

fn map_authorization(decision: AuthorizationDecision) -> Result<(), ApiError> {
    match decision {
        AuthorizationDecision::Allowed => Ok(()),
        AuthorizationDecision::Unauthorized => Err(invalid_subject()),
        AuthorizationDecision::Hidden => Err(repository_not_found()),
        AuthorizationDecision::Forbidden => {
            Err(ApiError::forbidden("REPO_FORBIDDEN", "仓库操作权限不足"))
        }
    }
}

fn ensure_subject_valid(subject: Subject) -> Result<(), ApiError> {
    match subject {
        Subject::Invalid | Subject::Authenticated { enabled: false, .. } => Err(invalid_subject()),
        Subject::Anonymous | Subject::Authenticated { enabled: true, .. } => Ok(()),
    }
}

fn ensure_global_admin(subject: Subject) -> Result<(), ApiError> {
    ensure_subject_valid(subject)?;
    if subject.is_global_admin() {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "REPO_FORBIDDEN",
            "仅全局管理员可创建仓库",
        ))
    }
}

fn invalid_subject() -> ApiError {
    ApiError::unauthorized("AUTH_INVALID_SESSION", "身份凭据无效或已失效")
}

fn repository_not_found() -> ApiError {
    ApiError::not_found("REPO_NOT_FOUND", "仓库不存在或无权访问")
}

fn user_not_found() -> ApiError {
    ApiError::not_found("REPO_USER_NOT_FOUND", "目标用户不存在")
}

fn invalid_config(message: impl Into<String>) -> ApiError {
    ApiError::bad_request("REPO_INVALID_CONFIG", message)
}

fn ensure_delete_confirmation(confirmation: Option<&str>, name: &str) -> Result<(), ApiError> {
    if confirmation == Some(name) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "REPO_DELETE_CONFIRMATION",
            "删除确认头缺失或与仓库名不一致",
        ))
    }
}

fn resolve_create(input: CreateRepository) -> Result<ResolvedCreate, ApiError> {
    validate_name(&input.name)?;
    match input.repo_type {
        RepoType::Hosted => resolve_hosted(input),
        RepoType::Proxy => resolve_proxy(input),
        RepoType::Group => resolve_group(input),
    }
}

fn resolve_hosted(input: CreateRepository) -> Result<ResolvedCreate, ApiError> {
    if input.upstream_url.is_some()
        || input.upstream_credential_ref.is_some()
        || input.members.is_some()
    {
        return Err(invalid_config("hosted 仓库包含不适用的类型字段"));
    }
    if input.format != Format::Maven && input.version_policy.is_some() {
        return Err(invalid_config("仅 Maven hosted 可设置版本策略"));
    }
    let version_policy = (input.format == Format::Maven)
        .then_some(input.version_policy.unwrap_or(VersionPolicy::Mixed));
    Ok(ResolvedCreate {
        row: NewRepositoryRow {
            name: input.name,
            format: input.format,
            repo_type: input.repo_type,
            visibility: input.visibility,
            overwrite_policy: Some(default_hosted_overwrite(
                input.format,
                input.overwrite_policy,
            )),
            version_policy,
            upstream_url: None,
            upstream_credential_ref: None,
        },
        members: None,
    })
}

fn default_hosted_overwrite(
    format: Format,
    configured: Option<OverwritePolicy>,
) -> OverwritePolicy {
    configured.unwrap_or(if format == Format::Maven {
        OverwritePolicy::Deny
    } else {
        OverwritePolicy::Allow
    })
}

fn resolve_proxy(input: CreateRepository) -> Result<ResolvedCreate, ApiError> {
    if input.overwrite_policy.is_some() || input.version_policy.is_some() || input.members.is_some()
    {
        return Err(invalid_config("proxy 仓库包含不适用的类型字段"));
    }
    let upstream_url = input
        .upstream_url
        .ok_or_else(|| invalid_config("proxy 仓库必须提供非空上游地址"))?;
    validate_upstream_url(&upstream_url)?;
    validate_credential_ref(input.upstream_credential_ref.as_deref())?;
    Ok(ResolvedCreate {
        row: NewRepositoryRow {
            name: input.name,
            format: input.format,
            repo_type: input.repo_type,
            visibility: input.visibility,
            overwrite_policy: None,
            version_policy: None,
            upstream_url: Some(upstream_url),
            upstream_credential_ref: input.upstream_credential_ref,
        },
        members: None,
    })
}

fn resolve_group(input: CreateRepository) -> Result<ResolvedCreate, ApiError> {
    if input.overwrite_policy.is_some()
        || input.version_policy.is_some()
        || input.upstream_url.is_some()
        || input.upstream_credential_ref.is_some()
    {
        return Err(invalid_config("group 仓库包含不适用的类型字段"));
    }
    let members = input
        .members
        .ok_or_else(|| invalid_config("group 仓库必须提供成员"))?;
    validate_member_names(&members)?;
    Ok(ResolvedCreate {
        row: NewRepositoryRow {
            name: input.name,
            format: input.format,
            repo_type: input.repo_type,
            visibility: input.visibility,
            overwrite_policy: None,
            version_policy: None,
            upstream_url: None,
            upstream_credential_ref: None,
        },
        members: Some(members),
    })
}

fn validate_patch(repository: &Repository, patch: &RepositoryPatch) -> Result<(), ApiError> {
    match repository.repo_type {
        RepoType::Hosted => validate_hosted_patch(repository.format, patch),
        RepoType::Proxy => validate_proxy_patch(patch),
        RepoType::Group => validate_group_patch(patch),
    }
}

fn validate_hosted_patch(format: Format, patch: &RepositoryPatch) -> Result<(), ApiError> {
    if patch.upstream_url.is_some()
        || patch.upstream_credential_ref.is_some()
        || patch.members.is_some()
    {
        return Err(invalid_config("hosted 仓库包含不适用的更新字段"));
    }
    if format != Format::Maven && patch.version_policy.is_some() {
        return Err(invalid_config("仅 Maven hosted 可更新版本策略"));
    }
    Ok(())
}

fn validate_proxy_patch(patch: &RepositoryPatch) -> Result<(), ApiError> {
    if patch.overwrite_policy.is_some() || patch.version_policy.is_some() || patch.members.is_some()
    {
        return Err(invalid_config("proxy 仓库包含不适用的更新字段"));
    }
    if let Some(upstream_url) = patch.upstream_url.as_deref() {
        validate_upstream_url(upstream_url)?;
    }
    let credential = patch
        .upstream_credential_ref
        .as_ref()
        .and_then(|value| value.as_deref());
    validate_credential_ref(credential)
}

fn validate_group_patch(patch: &RepositoryPatch) -> Result<(), ApiError> {
    if patch.overwrite_policy.is_some()
        || patch.version_policy.is_some()
        || patch.upstream_url.is_some()
        || patch.upstream_credential_ref.is_some()
    {
        return Err(invalid_config("group 仓库包含不适用的更新字段"));
    }
    if let Some(members) = &patch.members {
        validate_member_names(members)?;
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), ApiError> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(invalid_config(
            "仓库名必须为 1 至 64 位小写字母、数字或连字符",
        ))
    }
}

fn validate_upstream_url(value: &str) -> Result<(), ApiError> {
    if value.trim() != value || value.chars().any(char::is_control) || value.contains('#') {
        return Err(invalid_config("proxy 上游地址格式无效"));
    }
    let remainder = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))
        .ok_or_else(|| invalid_config("proxy 上游地址格式无效"))?;
    let authority_end = remainder.find(['/', '?']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    if authority.is_empty() || authority.contains('@') || authority.chars().any(char::is_whitespace)
    {
        return Err(invalid_config("proxy 上游地址格式无效"));
    }
    Ok(())
}

fn validate_credential_ref(reference: Option<&str>) -> Result<(), ApiError> {
    let Some(reference) = reference else {
        return Ok(());
    };
    let mut bytes = reference.bytes();
    let valid_first = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_');
    let valid_rest = bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if valid_first && valid_rest {
        Ok(())
    } else {
        Err(invalid_config("上游凭据引用名不是合法环境变量名"))
    }
}

fn validate_member_names(names: &[String]) -> Result<(), ApiError> {
    if names.is_empty() {
        return Err(invalid_config("group 成员不能为空"));
    }
    let unique: HashSet<&str> = names.iter().map(String::as_str).collect();
    if unique.len() != names.len() {
        return Err(invalid_config("group 成员不能重复"));
    }
    Ok(())
}

async fn resolve_member_ids(
    pool: &SqlitePool,
    format: Format,
    names: &[String],
    user_id: Option<i64>,
    global_admin: bool,
) -> Result<Vec<i64>, ApiError> {
    validate_member_names(names)?;
    let candidates =
        persistence::find_member_candidates(pool, names, user_id, global_admin).await?;
    let by_name: HashMap<String, persistence::MemberCandidate> = candidates
        .into_iter()
        .map(|candidate| (candidate.name.clone(), candidate))
        .collect();
    names
        .iter()
        .map(|name| validate_member(by_name.get(name), name, format))
        .collect()
}

fn validate_member(
    candidate: Option<&persistence::MemberCandidate>,
    name: &str,
    format: Format,
) -> Result<i64, ApiError> {
    let candidate = candidate.ok_or_else(|| invalid_config("group 成员不存在或无权访问"))?;
    if candidate.format != format {
        return Err(invalid_config(format!("group 成员格式不一致：{name}")));
    }
    if !matches!(candidate.repo_type, RepoType::Hosted | RepoType::Proxy) {
        return Err(invalid_config(format!("group 成员不能是 group：{name}")));
    }
    Ok(candidate.id)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use sqlx::SqlitePool;

    use super::{
        CreateRepository, DeleteMode, RepositoryPatch, create_repository, delete_repository,
        get_repository, set_acl, update_repository, validate_upstream_url,
    };
    use crate::artifact::ArtifactService;
    use crate::domain::repository::{
        Format, OverwritePolicy, Permission, RepoType, Subject, VersionPolicy, Visibility,
    };
    use crate::domain::{Role, create_user};
    use crate::persistence::test_pool;
    use crate::storage::BlobStore;

    fn admin() -> Subject {
        Subject::Authenticated {
            user_id: 1,
            role: Role::Admin,
            enabled: true,
        }
    }

    fn hosted(name: &str, format: Format) -> CreateRepository {
        CreateRepository {
            name: name.to_string(),
            format,
            repo_type: RepoType::Hosted,
            visibility: Visibility::Private,
            overwrite_policy: None,
            version_policy: None,
            upstream_url: None,
            upstream_credential_ref: None,
            members: None,
        }
    }

    async fn artifact_service(pool: &SqlitePool, dir: &tempfile::TempDir) -> ArtifactService {
        let blobs = BlobStore::new(dir.path().join("blobs"), 1024, 0, 0);
        blobs.initialize().await.unwrap();
        ArtifactService::new(pool.clone(), blobs)
    }

    #[test]
    fn 上游地址允许内网但拒绝userinfo与fragment() {
        for value in [
            "http://127.0.0.1:8080/raw",
            "http://[::1]/raw",
            "https://intranet.example/raw",
        ] {
            assert!(validate_upstream_url(value).is_ok(), "应接受 {value}");
        }
        for value in [
            "https://user:password@example.invalid/raw",
            "https://example.invalid/raw#fragment",
        ] {
            assert!(validate_upstream_url(value).is_err(), "应拒绝 {value}");
        }
    }

    #[tokio::test]
    async fn 创建应用类型默认值并拒绝非法组合与非管理员() {
        let pool = test_pool().await;
        let normal = Subject::Authenticated {
            user_id: 2,
            role: Role::User,
            enabled: true,
        };
        let denied = create_repository(&pool, normal, hosted("denied", Format::Raw))
            .await
            .unwrap_err();
        assert_eq!(denied.code(), "REPO_FORBIDDEN");

        let raw = create_repository(&pool, admin(), hosted("raw-hosted", Format::Raw))
            .await
            .unwrap();
        assert_eq!(raw.overwrite_policy, Some(OverwritePolicy::Allow));
        assert_eq!(raw.version_policy, None);

        let maven = create_repository(&pool, admin(), hosted("maven-hosted", Format::Maven))
            .await
            .unwrap();
        assert_eq!(maven.overwrite_policy, Some(OverwritePolicy::Deny));
        assert_eq!(maven.version_policy, Some(VersionPolicy::Mixed));

        let invalid = CreateRepository {
            name: "bad-proxy".to_string(),
            format: Format::Raw,
            repo_type: RepoType::Proxy,
            visibility: Visibility::Public,
            overwrite_policy: Some(OverwritePolicy::Allow),
            version_policy: None,
            upstream_url: Some("".to_string()),
            upstream_credential_ref: None,
            members: None,
        };
        let error = create_repository(&pool, admin(), invalid)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "REPO_INVALID_CONFIG");
    }

    #[tokio::test]
    async fn group成员校验且失败替换保持原顺序() {
        let pool = test_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let artifacts = artifact_service(&pool, &dir).await;
        create_repository(&pool, admin(), hosted("first", Format::Raw))
            .await
            .unwrap();
        let mut proxy = hosted("second", Format::Raw);
        proxy.repo_type = RepoType::Proxy;
        proxy.upstream_url = Some("https://example.invalid/repository".to_string());
        create_repository(&pool, admin(), proxy).await.unwrap();
        create_repository(&pool, admin(), hosted("other-format", Format::Npm))
            .await
            .unwrap();

        let group = CreateRepository {
            name: "raw-group".to_string(),
            format: Format::Raw,
            repo_type: RepoType::Group,
            visibility: Visibility::Private,
            overwrite_policy: None,
            version_policy: None,
            upstream_url: None,
            upstream_credential_ref: None,
            members: Some(vec!["second".to_string(), "first".to_string()]),
        };
        create_repository(&pool, admin(), group).await.unwrap();
        let before = get_repository(&pool, admin(), "raw-group").await.unwrap();
        assert_eq!(before.members, vec!["second", "first"]);

        let error = update_repository(
            &pool,
            &artifacts,
            admin(),
            "raw-group",
            RepositoryPatch {
                members: Some(vec!["first".to_string(), "other-format".to_string()]),
                ..RepositoryPatch::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "REPO_INVALID_CONFIG");
        let after = get_repository(&pool, admin(), "raw-group").await.unwrap();
        assert_eq!(after.members, vec!["second", "first"]);
    }

    #[tokio::test]
    async fn patch显式清空凭据引用_acl向下包含与删除引用保护() {
        let pool = test_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let artifacts = artifact_service(&pool, &dir).await;
        let user = create_user(&pool, "alice", "pw", Role::User).await.unwrap();
        let mut proxy = hosted("proxy", Format::Raw);
        proxy.repo_type = RepoType::Proxy;
        proxy.upstream_url = Some("https://example.invalid/repository".to_string());
        proxy.upstream_credential_ref = Some("UPSTREAM_AUTH".to_string());
        create_repository(&pool, admin(), proxy).await.unwrap();
        set_acl(&pool, admin(), "proxy", "alice", Permission::Write)
            .await
            .unwrap();

        let actor = Subject::Authenticated {
            user_id: user.id,
            role: Role::User,
            enabled: true,
        };
        let readable = get_repository(&pool, actor, "proxy").await.unwrap();
        assert_eq!(readable.name, "proxy");
        let updated = update_repository(
            &pool,
            &artifacts,
            admin(),
            "proxy",
            RepositoryPatch {
                upstream_credential_ref: Some(None),
                ..RepositoryPatch::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.upstream_credential_ref, None);

        let group = CreateRepository {
            name: "group".to_string(),
            format: Format::Raw,
            repo_type: RepoType::Group,
            visibility: Visibility::Private,
            overwrite_policy: None,
            version_policy: None,
            upstream_url: None,
            upstream_credential_ref: None,
            members: Some(vec!["proxy".to_string()]),
        };
        create_repository(&pool, admin(), group).await.unwrap();
        let in_use = delete_repository(
            &pool,
            &artifacts,
            admin(),
            "proxy",
            DeleteMode::Config,
            Some("proxy"),
        )
        .await
        .unwrap_err();
        assert_eq!(in_use.code(), "REPO_IN_USE");

        let removed = delete_repository(
            &pool,
            &artifacts,
            admin(),
            "group",
            DeleteMode::Data,
            Some("group"),
        )
        .await
        .unwrap();
        assert_eq!(removed.orphaned_blobs, 0);
        assert_eq!(removed.reclaimed_blobs, 0);
        delete_repository(
            &pool,
            &artifacts,
            admin(),
            "proxy",
            DeleteMode::Config,
            Some("proxy"),
        )
        .await
        .unwrap();
    }
}
