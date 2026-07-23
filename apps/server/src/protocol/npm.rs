//! npm hosted/proxy 协议：publish、packument、tarball 与 dist-tag。

use std::time::Duration;

use axum::body::Body;
use axum::http::header::{
    AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HOST, HeaderMap, HeaderValue,
};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use serde_json::{Map, Value, json};
use tokio_util::io::ReaderStream;

use crate::api::extractor::ArtifactRoute;
use crate::artifact::{NpmHostedCommit, NpmPackageRecord, NpmProxyPackument, NpmProxyTarballInput};
use crate::domain::encoding::strict_base64_decode;
use crate::domain::repository::{
    Permission, ProtocolRepositoryContext, RepoType, Repository, Subject,
};
use crate::error::ApiError;
use crate::persistence::NpmProxyPackumentInput;
use crate::repository;
use crate::state::AppState;

const PROXY_SEND_TIMEOUT: Duration = Duration::from_secs(30);
const PROXY_CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const NPM_PACKUMENT_MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageRoute {
    package_name: String,
    action: NpmAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NpmAction {
    Packument,
    Tarball(String),
    DistTags,
    DistTag(String),
}

struct PublishVersion {
    version: String,
    manifest: Value,
    tarball_path: String,
    tarball: Vec<u8>,
    dist_tags: Vec<String>,
}

struct GroupRequest<'a> {
    repository: &'a Repository,
    members: &'a [Repository],
    route: &'a ArtifactRoute,
    uri: &'a Uri,
    headers: &'a HeaderMap,
    head: bool,
}

/// 读取 npm packument、tarball 或 dist-tag。
pub async fn get(
    state: &AppState,
    _subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
    headers: &HeaderMap,
    context: ProtocolRepositoryContext,
) -> Result<Response, ApiError> {
    let parsed = parse_route(&route, uri)?;
    match context.repository.repo_type {
        RepoType::Hosted => {
            hosted_get(
                state,
                &context.repository,
                &route,
                uri,
                headers,
                parsed,
                false,
            )
            .await
        }
        RepoType::Proxy => {
            proxy_get(
                state,
                &context.repository,
                &route,
                uri,
                headers,
                parsed,
                false,
            )
            .await
        }
        RepoType::Group => {
            group_get(
                state,
                GroupRequest {
                    repository: &context.repository,
                    members: &context.members,
                    route: &route,
                    uri,
                    headers,
                    head: false,
                },
                parsed,
            )
            .await
        }
    }
}

/// 探测 npm tarball；packument HEAD 返回对应长度。
pub async fn head(
    state: &AppState,
    _subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
    headers: &HeaderMap,
    context: ProtocolRepositoryContext,
) -> Result<Response, ApiError> {
    let parsed = parse_route(&route, uri)?;
    match context.repository.repo_type {
        RepoType::Hosted => {
            hosted_get(
                state,
                &context.repository,
                &route,
                uri,
                headers,
                parsed,
                true,
            )
            .await
        }
        RepoType::Proxy => {
            proxy_get(
                state,
                &context.repository,
                &route,
                uri,
                headers,
                parsed,
                true,
            )
            .await
        }
        RepoType::Group => {
            group_get(
                state,
                GroupRequest {
                    repository: &context.repository,
                    members: &context.members,
                    route: &route,
                    uri,
                    headers,
                    head: true,
                },
                parsed,
            )
            .await
        }
    }
}

/// 发布 npm 包或设置 dist-tag。
pub async fn put(
    state: &AppState,
    subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
    headers: &HeaderMap,
    body: Body,
    context: ProtocolRepositoryContext,
) -> Result<Response, ApiError> {
    let parsed = parse_route(&route, uri)?;
    if context.repository.repo_type != RepoType::Hosted {
        return Err(method_not_allowed());
    }
    repository::protocol_repository(&state.pool, subject, &route.repository, Permission::Write)
        .await?;
    match parsed.action {
        NpmAction::Packument => {
            publish(
                state,
                &context.repository,
                &parsed.package_name,
                headers,
                body,
            )
            .await
        }
        NpmAction::DistTag(tag) => {
            set_dist_tag(state, &context.repository, &parsed.package_name, &tag, body).await
        }
        NpmAction::Tarball(_) | NpmAction::DistTags => Err(method_not_allowed()),
    }
}

/// 删除 npm dist-tag。
pub async fn delete(
    state: &AppState,
    subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
    context: ProtocolRepositoryContext,
) -> Result<Response, ApiError> {
    let parsed = parse_route(&route, uri)?;
    if context.repository.repo_type != RepoType::Hosted {
        return Err(method_not_allowed());
    }
    repository::protocol_repository(&state.pool, subject, &route.repository, Permission::Write)
        .await?;
    let NpmAction::DistTag(tag) = parsed.action else {
        return Err(method_not_allowed());
    };
    if tag == "latest" {
        return Err(ApiError::bad_request(
            "NPM_LATEST_IMMUTABLE",
            "latest 标签不可删除",
        ));
    }
    let deleted = state
        .artifacts
        .delete_npm_dist_tag(context.repository.id, &parsed.package_name, &tag)
        .await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Err(npm_not_found())
    }
}

async fn hosted_get(
    state: &AppState,
    repository: &Repository,
    route: &ArtifactRoute,
    uri: &Uri,
    headers: &HeaderMap,
    parsed: PackageRoute,
    head: bool,
) -> Result<Response, ApiError> {
    match parsed.action {
        NpmAction::Packument => {
            hosted_packument(
                state,
                repository,
                route,
                uri,
                headers,
                &parsed.package_name,
                head,
            )
            .await
        }
        NpmAction::Tarball(path) => local_tarball(state, repository.id, &path, head).await,
        NpmAction::DistTags => {
            hosted_dist_tags(state, repository.id, &parsed.package_name, head).await
        }
        NpmAction::DistTag(_) => Err(npm_not_found()),
    }
}

async fn hosted_packument(
    state: &AppState,
    repository: &Repository,
    route: &ArtifactRoute,
    uri: &Uri,
    headers: &HeaderMap,
    package_name: &str,
    head: bool,
) -> Result<Response, ApiError> {
    let package = state
        .artifacts
        .npm_package(repository.id, package_name)
        .await?
        .ok_or_else(npm_not_found)?;
    let base_url = public_base_url(state, uri, headers)?;
    let value = build_hosted_packument(package_name, &route.repository, &base_url, &package)?;
    json_response(value, head)
}

async fn hosted_dist_tags(
    state: &AppState,
    repo_id: i64,
    package_name: &str,
    head: bool,
) -> Result<Response, ApiError> {
    let package = state
        .artifacts
        .npm_package(repo_id, package_name)
        .await?
        .ok_or_else(npm_not_found)?;
    json_response(
        serde_json::to_value(package.dist_tags).map_err(|_| invalid_metadata())?,
        head,
    )
}

async fn publish(
    state: &AppState,
    repository: &Repository,
    package_name: &str,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    reject_oversized_publish(state, headers)?;
    let bytes = collect_limited(body, state.config.npm_publish_max_bytes).await?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| invalid_publish())?;
    let published = parse_publish(package_name, &value)?;
    validate_publish_integrity(&published.manifest, &published.tarball)?;
    let mut writer = state.artifacts.begin_write().await?;
    writer.write_chunk(&published.tarball).await?;
    let pending = writer.finish().await?;
    let manifest_json =
        serde_json::to_string(&published.manifest).map_err(|_| invalid_publish())?;
    state
        .artifacts
        .commit_npm_hosted(
            NpmHostedCommit {
                repo_id: repository.id,
                package_name: package_name.to_owned(),
                version: published.version,
                manifest_json,
                tarball_path: published.tarball_path,
                dist_tags: published.dist_tags,
            },
            pending,
        )
        .await?;
    Ok((StatusCode::CREATED, axum::Json(json!({"ok": true}))).into_response())
}

fn parse_publish(package_name: &str, value: &Value) -> Result<PublishVersion, ApiError> {
    if value.get("name").and_then(Value::as_str) != Some(package_name) {
        return Err(invalid_publish());
    }
    let versions = value
        .get("versions")
        .and_then(Value::as_object)
        .ok_or_else(invalid_publish)?;
    if versions.len() != 1 {
        return Err(invalid_publish());
    }
    let (version, manifest) = versions.iter().next().ok_or_else(invalid_publish)?;
    validate_version(version)?;
    if manifest.get("name").and_then(Value::as_str) != Some(package_name)
        || manifest.get("version").and_then(Value::as_str) != Some(version)
    {
        return Err(invalid_publish());
    }
    let attachments = value
        .get("_attachments")
        .and_then(Value::as_object)
        .ok_or_else(invalid_publish)?;
    if attachments.len() != 1 {
        return Err(invalid_publish());
    }
    let (attachment_name, attachment) = attachments.iter().next().ok_or_else(invalid_publish)?;
    let filename = publish_attachment_filename(package_name, attachment_name)?;
    let data = attachment
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(invalid_publish)?;
    let tarball = strict_base64_decode(data).ok_or_else(invalid_publish)?;
    let length = attachment
        .get("length")
        .and_then(Value::as_u64)
        .ok_or_else(invalid_publish)?;
    if length != tarball.len() as u64 {
        return Err(invalid_publish());
    }
    let dist_tags = value
        .get("dist-tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .filter(|(_, target)| target.as_str() == Some(version))
                .map(|(tag, _)| tag.clone())
                .collect()
        })
        .unwrap_or_default();
    Ok(PublishVersion {
        version: version.clone(),
        manifest: manifest.clone(),
        tarball_path: tarball_path(package_name, &filename),
        tarball,
        dist_tags,
    })
}

async fn set_dist_tag(
    state: &AppState,
    repository: &Repository,
    package_name: &str,
    tag: &str,
    body: Body,
) -> Result<Response, ApiError> {
    validate_tag(tag)?;
    let bytes = collect_limited(body, 1024).await?;
    let version: String = serde_json::from_slice(&bytes).map_err(|_| invalid_publish())?;
    validate_version(&version)?;
    let updated = state
        .artifacts
        .set_npm_dist_tag(repository.id, package_name, tag, &version)
        .await?;
    if !updated {
        return Err(npm_not_found());
    }
    Ok((StatusCode::OK, axum::Json(json!({"ok": true}))).into_response())
}

async fn proxy_get(
    state: &AppState,
    repository: &Repository,
    route: &ArtifactRoute,
    uri: &Uri,
    headers: &HeaderMap,
    parsed: PackageRoute,
    head: bool,
) -> Result<Response, ApiError> {
    match parsed.action {
        NpmAction::Packument => {
            proxy_packument(
                state,
                repository,
                route,
                uri,
                headers,
                &parsed.package_name,
                head,
            )
            .await
        }
        NpmAction::Tarball(path) => {
            proxy_tarball(state, repository, &parsed.package_name, &path, head).await
        }
        NpmAction::DistTags | NpmAction::DistTag(_) => Err(method_not_allowed()),
    }
}

async fn proxy_packument(
    state: &AppState,
    repository: &Repository,
    route: &ArtifactRoute,
    uri: &Uri,
    headers: &HeaderMap,
    package_name: &str,
    head: bool,
) -> Result<Response, ApiError> {
    let cached = state
        .artifacts
        .npm_proxy_packument(repository.id, package_name)
        .await?;
    if let Some(entry) = cached.as_ref().filter(|entry| entry.fresh) {
        return cached_packument_response(entry, head);
    }
    let refreshed =
        fetch_proxy_packument(state, repository, route, uri, headers, package_name).await;
    match (refreshed, cached) {
        (Ok(value), _) => json_response(value, head),
        (Err(_), Some(stale)) => {
            tracing::warn!(
                repository_id = repository.id,
                package = package_name,
                "npm packument 回源失败，沿用过期缓存"
            );
            cached_packument_response(&stale, head)
        }
        (Err(error), None) => Err(error),
    }
}

async fn fetch_proxy_packument(
    state: &AppState,
    repository: &Repository,
    route: &ArtifactRoute,
    uri: &Uri,
    headers: &HeaderMap,
    package_name: &str,
) -> Result<Value, ApiError> {
    fetch_proxy_packument_with_idle_timeout(
        state,
        repository,
        route,
        uri,
        headers,
        package_name,
        PROXY_CHUNK_IDLE_TIMEOUT,
    )
    .await
}

/// 使用指定空闲超时回源并缓存 packument，供协议实现与快速超时测试共用。
#[doc(hidden)]
pub async fn fetch_proxy_packument_with_idle_timeout(
    state: &AppState,
    repository: &Repository,
    route: &ArtifactRoute,
    uri: &Uri,
    headers: &HeaderMap,
    package_name: &str,
    idle_timeout: Duration,
) -> Result<Value, ApiError> {
    let expected_upstream = repository
        .upstream_url
        .as_deref()
        .ok_or_else(proxy_config_error)?;
    let url = upstream_package_url(expected_upstream, package_name)?;
    let mut request = state.artifacts.proxy_client()?.get(url);
    if let Some(authorization) = upstream_authorization(repository)? {
        request = request.header(AUTHORIZATION, authorization);
    }
    let response = send_upstream(request).await?;
    validate_upstream_status(response.status())?;
    let bytes = read_proxy_packument(response, idle_timeout).await?;
    let upstream: Value = serde_json::from_slice(&bytes).map_err(|_| proxy_bad_gateway())?;
    let base_url = public_base_url(state, uri, headers)?;
    let (rewritten, tarballs) =
        rewrite_proxy_packument(&upstream, package_name, &route.repository, &base_url)?;
    let rewritten_json = serde_json::to_string(&rewritten).map_err(|_| proxy_bad_gateway())?;
    let input = NpmProxyPackumentInput {
        repo_id: repository.id,
        package_name: package_name.to_owned(),
        rewritten_json,
        upstream_url_snapshot: expected_upstream.to_owned(),
        ttl_secs: state.config.npm_packument_ttl_secs,
        tarballs,
    };
    state.artifacts.cache_npm_proxy_packument(&input).await?;
    Ok(rewritten)
}

async fn proxy_tarball(
    state: &AppState,
    repository: &Repository,
    package_name: &str,
    path: &str,
    head: bool,
) -> Result<Response, ApiError> {
    if let Some(local) = state.artifacts.read(repository.id, path).await? {
        return tarball_response(local.record, local.blob, head);
    }
    let cache = state
        .artifacts
        .npm_proxy_packument(repository.id, package_name)
        .await?
        .ok_or_else(npm_not_found)?;
    let mapping = find_tarball_mapping(&cache.tarballs, path)
        .ok_or_else(npm_not_found)?
        .clone();
    let task_state = state.clone();
    let task_repository = repository.clone();
    let task_package = package_name.to_owned();
    let task_path = path.to_owned();
    let task_mapping = mapping.clone();
    let expected_upstream = repository
        .upstream_url
        .clone()
        .ok_or_else(proxy_config_error)?;
    state
        .artifacts
        .proxy_singleflight(
            repository.id,
            path,
            &expected_upstream,
            move || async move {
                if task_state
                    .artifacts
                    .read(task_repository.id, &task_path)
                    .await?
                    .is_some()
                {
                    return Ok(());
                }
                fetch_proxy_tarball(
                    &task_state,
                    &task_repository,
                    &task_package,
                    &task_path,
                    &task_mapping,
                )
                .await
            },
        )
        .await?;
    local_tarball(state, repository.id, path, head).await
}

async fn fetch_proxy_tarball(
    state: &AppState,
    repository: &Repository,
    package_name: &str,
    path: &str,
    mapping: &NpmProxyTarballInput,
) -> Result<(), ApiError> {
    let expected_upstream = repository
        .upstream_url
        .as_deref()
        .ok_or_else(proxy_config_error)?;
    validate_proxy_tarball_url(&mapping.tarball_url)?;
    let url = reqwest::Url::parse(&mapping.tarball_url).map_err(|_| proxy_bad_gateway())?;
    let mut request = state.artifacts.proxy_client()?.get(url);
    if let Some(authorization) = upstream_authorization(repository)? {
        request = request.header(AUTHORIZATION, authorization);
    }
    let mut response = send_upstream(request).await?;
    validate_upstream_status(response.status())?;
    let mut writer = state.artifacts.begin_write().await?;
    while let Some(chunk) = tokio::time::timeout(PROXY_CHUNK_IDLE_TIMEOUT, response.chunk())
        .await
        .map_err(|_| proxy_timeout())?
        .map_err(|_| proxy_transfer_error())?
    {
        writer.write_chunk(&chunk).await?;
    }
    let pending = writer.finish().await?;
    validate_mapping_integrity(mapping, pending.metadata())?;
    state
        .artifacts
        .commit_npm_proxy_tarball(
            repository.id,
            package_name,
            &mapping.version,
            path,
            expected_upstream,
            pending,
        )
        .await?;
    Ok(())
}

fn cached_packument_response(cache: &NpmProxyPackument, head: bool) -> Result<Response, ApiError> {
    let value = serde_json::from_str(&cache.rewritten_json)
        .map_err(|_| ApiError::internal("npm 缓存内容无效"))?;
    json_response(value, head)
}

/// 为后续 group 聚合合并多个已授权成员的 packument；前序版本与标签优先。
pub fn merge_group_packuments(packuments: &[Value], package_name: &str) -> Value {
    let mut versions = Map::new();
    let mut tags = Map::new();
    let mut time = Map::new();
    for packument in packuments {
        merge_missing(
            &mut versions,
            packument.get("versions").and_then(Value::as_object),
        );
        merge_missing(
            &mut tags,
            packument.get("dist-tags").and_then(Value::as_object),
        );
        merge_missing(&mut time, packument.get("time").and_then(Value::as_object));
    }
    json!({"name": package_name, "versions": versions, "dist-tags": tags, "time": time})
}

async fn group_get(
    state: &AppState,
    request: GroupRequest<'_>,
    parsed: PackageRoute,
) -> Result<Response, ApiError> {
    let GroupRequest {
        repository,
        members,
        route,
        uri,
        headers,
        head,
    } = request;
    match parsed.action {
        NpmAction::Packument => {
            let value = group_packument(
                state,
                repository,
                members,
                route,
                uri,
                headers,
                &parsed.package_name,
            )
            .await?;
            json_response(value, head)
        }
        NpmAction::Tarball(path) => {
            group_tarball(state, members, &parsed.package_name, &path, head).await
        }
        NpmAction::DistTags => {
            let value = group_packument(
                state,
                repository,
                members,
                route,
                uri,
                headers,
                &parsed.package_name,
            )
            .await?;
            json_response(
                value.get("dist-tags").cloned().unwrap_or_else(|| json!({})),
                head,
            )
        }
        NpmAction::DistTag(_) => Err(npm_not_found()),
    }
}

async fn group_packument(
    state: &AppState,
    repository: &Repository,
    members: &[Repository],
    route: &ArtifactRoute,
    uri: &Uri,
    headers: &HeaderMap,
    package_name: &str,
) -> Result<Value, ApiError> {
    let hosted_ids = member_ids(members, RepoType::Hosted);
    let proxy_ids = member_ids(members, RepoType::Proxy);
    let hosted = state
        .artifacts
        .npm_packages_batch(&hosted_ids, package_name)
        .await?;
    let proxies = state
        .artifacts
        .npm_proxy_packuments_batch(&proxy_ids, package_name)
        .await?;
    let base_url = public_base_url(state, uri, headers)?;
    let mut packuments = Vec::new();
    for member in members {
        let value = match member.repo_type {
            RepoType::Hosted => hosted
                .get(&member.id)
                .map(|package| {
                    build_hosted_packument(package_name, &repository.name, &base_url, package)
                })
                .transpose()?,
            RepoType::Proxy => {
                group_proxy_packument(
                    state,
                    member,
                    route,
                    uri,
                    headers,
                    package_name,
                    proxies.get(&member.id),
                    &repository.name,
                    &base_url,
                )
                .await?
            }
            RepoType::Group => None,
        };
        if let Some(value) = value {
            packuments.push(value);
        }
    }
    Ok(merge_group_packuments(&packuments, package_name))
}

fn member_ids(members: &[Repository], repo_type: RepoType) -> Vec<i64> {
    members
        .iter()
        .filter(|member| member.repo_type == repo_type)
        .map(|member| member.id)
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn group_proxy_packument(
    state: &AppState,
    member: &Repository,
    route: &ArtifactRoute,
    uri: &Uri,
    headers: &HeaderMap,
    package_name: &str,
    cached: Option<&NpmProxyPackument>,
    group_name: &str,
    base_url: &str,
) -> Result<Option<Value>, ApiError> {
    let value = if let Some(entry) = cached.filter(|entry| entry.fresh) {
        serde_json::from_str(&entry.rewritten_json)
            .map_err(|_| ApiError::internal("npm 缓存内容无效"))?
    } else {
        let member_route = ArtifactRoute {
            repository: member.name.clone(),
            path: route.path.clone(),
            trailing_slash: route.trailing_slash,
        };
        match fetch_proxy_packument(state, member, &member_route, uri, headers, package_name).await
        {
            Ok(value) => value,
            Err(_) if cached.is_some() => {
                tracing::warn!(
                    repository_id = member.id,
                    package = package_name,
                    "npm group 成员 packument 回源失败，沿用过期缓存"
                );
                serde_json::from_str(
                    &cached
                        .ok_or_else(|| ApiError::internal("npm 缓存状态无效"))?
                        .rewritten_json,
                )
                .map_err(|_| ApiError::internal("npm 缓存内容无效"))?
            }
            Err(error) if error.status() == StatusCode::NOT_FOUND => return Ok(None),
            Err(error) => return Err(error),
        }
    };
    rewrite_group_packument(value, package_name, group_name, base_url).map(Some)
}

fn rewrite_group_packument(
    mut value: Value,
    package_name: &str,
    group_name: &str,
    base_url: &str,
) -> Result<Value, ApiError> {
    let versions = value
        .get_mut("versions")
        .and_then(Value::as_object_mut)
        .ok_or_else(invalid_metadata)?;
    for manifest in versions.values_mut() {
        let dist = manifest
            .get_mut("dist")
            .and_then(Value::as_object_mut)
            .ok_or_else(invalid_metadata)?;
        let filename = dist
            .get("tarball")
            .and_then(Value::as_str)
            .and_then(|url| url.split('?').next())
            .and_then(|url| url.rsplit('/').next())
            .filter(|filename| !filename.is_empty())
            .ok_or_else(invalid_metadata)?;
        dist.insert(
            "tarball".to_string(),
            Value::String(local_tarball_url(
                base_url,
                group_name,
                package_name,
                filename,
            )),
        );
    }
    Ok(value)
}

async fn group_tarball(
    state: &AppState,
    members: &[Repository],
    package_name: &str,
    group_path: &str,
    head: bool,
) -> Result<Response, ApiError> {
    let filename = group_path.rsplit('/').next().ok_or_else(npm_not_found)?;
    let hosted_ids = member_ids(members, RepoType::Hosted);
    let proxy_ids = member_ids(members, RepoType::Proxy);
    let hosted = state
        .artifacts
        .npm_packages_batch(&hosted_ids, package_name)
        .await?;
    let proxies = state
        .artifacts
        .npm_proxy_packuments_batch(&proxy_ids, package_name)
        .await?;
    for member in members {
        match member.repo_type {
            RepoType::Hosted => {
                if let Some(record) = hosted
                    .get(&member.id)
                    .into_iter()
                    .flat_map(|package| &package.versions)
                    .find(|record| record.tarball_path.rsplit('/').next() == Some(filename))
                {
                    return local_tarball(state, member.id, &record.tarball_path, head).await;
                }
            }
            RepoType::Proxy => {
                if let Some(mapping) = proxies
                    .get(&member.id)
                    .into_iter()
                    .flat_map(|cache| &cache.tarballs)
                    .find(|mapping| {
                        mapping
                            .artifact_path
                            .as_deref()
                            .and_then(|path| path.rsplit('/').next())
                            == Some(filename)
                    })
                    && let Some(path) = mapping.artifact_path.as_deref()
                {
                    return proxy_tarball(state, member, package_name, path, head).await;
                }
            }
            RepoType::Group => {}
        }
    }
    Err(npm_not_found())
}

fn merge_missing(target: &mut Map<String, Value>, source: Option<&Map<String, Value>>) {
    if let Some(source) = source {
        for (key, value) in source {
            target.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
}

fn build_hosted_packument(
    package_name: &str,
    repository_name: &str,
    base_url: &str,
    package: &NpmPackageRecord,
) -> Result<Value, ApiError> {
    let mut versions = Map::new();
    let mut time = Map::new();
    for record in &package.versions {
        let mut manifest: Value =
            serde_json::from_str(&record.manifest_json).map_err(|_| invalid_metadata())?;
        let filename = record
            .tarball_path
            .rsplit('/')
            .next()
            .ok_or_else(invalid_metadata)?;
        let tarball = local_tarball_url(base_url, repository_name, package_name, filename);
        let dist = manifest
            .as_object_mut()
            .ok_or_else(invalid_metadata)?
            .entry("dist")
            .or_insert_with(|| json!({}));
        let dist = dist.as_object_mut().ok_or_else(invalid_metadata)?;
        dist.insert("tarball".to_string(), Value::String(tarball));
        dist.insert(
            "integrity".to_string(),
            Value::String(format!("sha512-{}", hex_to_base64(&record.sha512)?)),
        );
        dist.insert("shasum".to_string(), Value::String(record.sha1.clone()));
        versions.insert(record.version.clone(), manifest);
        time.insert(
            record.version.clone(),
            Value::String(record.created_at.clone()),
        );
    }
    Ok(
        json!({"name": package_name, "versions": versions, "dist-tags": package.dist_tags, "time": time}),
    )
}

fn rewrite_proxy_packument(
    upstream: &Value,
    package_name: &str,
    repository_name: &str,
    base_url: &str,
) -> Result<(Value, Vec<NpmProxyTarballInput>), ApiError> {
    if upstream.get("name").and_then(Value::as_str) != Some(package_name) {
        return Err(proxy_bad_gateway());
    }
    let mut rewritten = upstream.clone();
    let versions = rewritten
        .get_mut("versions")
        .and_then(Value::as_object_mut)
        .ok_or_else(proxy_bad_gateway)?;
    let mut mappings = Vec::with_capacity(versions.len());
    for (version, manifest) in versions {
        let dist = manifest
            .get_mut("dist")
            .and_then(Value::as_object_mut)
            .ok_or_else(proxy_bad_gateway)?;
        let upstream_url = dist
            .get("tarball")
            .and_then(Value::as_str)
            .ok_or_else(proxy_bad_gateway)?
            .to_owned();
        validate_proxy_tarball_url(&upstream_url)?;
        let filename = proxy_tarball_filename(version)?;
        let artifact_path = tarball_path(package_name, &filename);
        dist.insert(
            "tarball".to_string(),
            Value::String(local_tarball_url(
                base_url,
                repository_name,
                package_name,
                &filename,
            )),
        );
        mappings.push(NpmProxyTarballInput {
            version: version.clone(),
            tarball_url: upstream_url,
            integrity: dist
                .get("integrity")
                .and_then(Value::as_str)
                .map(str::to_owned),
            shasum: dist
                .get("shasum")
                .and_then(Value::as_str)
                .map(str::to_owned),
            artifact_path: Some(artifact_path),
        });
    }
    Ok((rewritten, mappings))
}

fn parse_route(route: &ArtifactRoute, uri: &Uri) -> Result<PackageRoute, ApiError> {
    if route.trailing_slash || uri.query().is_some() {
        return Err(invalid_path());
    }
    let path = decode_percent(&raw_protocol_path(uri, &route.repository)?)?;
    if let Some(rest) = path.strip_prefix("-/package/") {
        let (package_name, action) = rest.split_once("/dist-tags").ok_or_else(invalid_path)?;
        let package_name = validate_package_name(package_name)?;
        let action = match action.strip_prefix('/') {
            None if action.is_empty() => NpmAction::DistTags,
            Some(tag) if !tag.is_empty() && !tag.contains('/') => {
                NpmAction::DistTag(tag.to_owned())
            }
            _ => return Err(invalid_path()),
        };
        return Ok(PackageRoute {
            package_name,
            action,
        });
    }
    if let Some((package, filename)) = path.split_once("/-/") {
        let package_name = validate_package_name(package)?;
        validate_filename(filename)?;
        return Ok(PackageRoute {
            action: NpmAction::Tarball(tarball_path(&package_name, filename)),
            package_name,
        });
    }
    Ok(PackageRoute {
        package_name: validate_package_name(&path)?,
        action: NpmAction::Packument,
    })
}

fn raw_protocol_path(uri: &Uri, repository_name: &str) -> Result<String, ApiError> {
    let prefix = format!("/repository/{repository_name}/");
    uri.path()
        .strip_prefix(&prefix)
        .map(str::to_owned)
        .ok_or_else(invalid_path)
}

fn decode_percent(input: &str) -> Result<String, ApiError> {
    let mut output = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_value(*bytes.get(index + 1).ok_or_else(invalid_path)?)?;
            let low = hex_value(*bytes.get(index + 2).ok_or_else(invalid_path)?)?;
            let byte = (high << 4) | low;
            if byte != b'/' {
                return Err(invalid_path());
            }
            output.push(byte);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).map_err(|_| invalid_path())
}

fn hex_value(byte: u8) -> Result<u8, ApiError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(invalid_path()),
    }
}

fn validate_package_name(value: &str) -> Result<String, ApiError> {
    let valid = !value.is_empty()
        && value.len() <= 214
        && !value.contains(['\\', '%'])
        && !value.contains("..")
        && match value.strip_prefix('@') {
            Some(scoped) => scoped.split_once('/').is_some_and(|(scope, name)| {
                !scope.is_empty() && !name.is_empty() && !name.contains('/')
            }),
            None => !value.contains('/'),
        };
    valid.then(|| value.to_owned()).ok_or_else(invalid_path)
}

fn validate_filename(value: &str) -> Result<(), ApiError> {
    (!value.is_empty()
        && value.len() <= 255
        && !value.contains(['/', '\\'])
        && !value.contains(".."))
    .then_some(())
    .ok_or_else(invalid_path)
}

fn publish_attachment_filename(
    package_name: &str,
    attachment_name: &str,
) -> Result<String, ApiError> {
    let filename = if attachment_name.contains('/') {
        let (scope, _) = package_name.split_once('/').ok_or_else(invalid_publish)?;
        attachment_name
            .strip_prefix(&format!("{scope}/"))
            .ok_or_else(invalid_publish)?
    } else {
        attachment_name
    };
    validate_filename(filename).map_err(|_| invalid_publish())?;
    Ok(filename.to_owned())
}

fn validate_version(value: &str) -> Result<(), ApiError> {
    (!value.is_empty()
        && value.len() <= 128
        && !value.contains(['/', '\\'])
        && !value.contains(".."))
    .then_some(())
    .ok_or_else(invalid_publish)
}

fn validate_tag(value: &str) -> Result<(), ApiError> {
    (!value.is_empty() && value.len() <= 128 && !value.contains(['/', '\\']))
        .then_some(())
        .ok_or_else(invalid_publish)
}

fn tarball_path(package_name: &str, filename: &str) -> String {
    format!("{package_name}/-/{filename}")
}

fn proxy_tarball_filename(version: &str) -> Result<String, ApiError> {
    validate_version(version).map_err(|_| proxy_bad_gateway())?;
    Ok(format!("v-{}.tgz", base64_url_encode(version.as_bytes())))
}

fn validate_proxy_tarball_url(value: &str) -> Result<(), ApiError> {
    let url = reqwest::Url::parse(value).map_err(|_| proxy_bad_gateway())?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(proxy_bad_gateway());
    }
    let filename = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .ok_or_else(proxy_bad_gateway)?;
    validate_filename(filename).map_err(|_| proxy_bad_gateway())
}

fn encode_package_name(package_name: &str) -> String {
    package_name.replace('/', "%2f")
}

fn local_tarball_url(
    base_url: &str,
    repository_name: &str,
    package_name: &str,
    filename: &str,
) -> String {
    format!(
        "{base_url}/repository/{repository_name}/{}/-/{filename}",
        encode_package_name(package_name)
    )
}

fn public_base_url(state: &AppState, uri: &Uri, headers: &HeaderMap) -> Result<String, ApiError> {
    if let Some(base) = state.config.public_base_url.as_deref() {
        return Ok(base.trim_end_matches('/').to_owned());
    }
    let authority = uri
        .authority()
        .map(ToString::to_string)
        .or_else(|| {
            headers
                .get(HOST)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        })
        .ok_or_else(invalid_host)?;
    let scheme = uri.scheme_str().unwrap_or("http");
    Ok(format!("{scheme}://{authority}"))
}

fn validate_publish_integrity(manifest: &Value, tarball: &[u8]) -> Result<(), ApiError> {
    let dist = manifest
        .get("dist")
        .and_then(Value::as_object)
        .ok_or_else(invalid_publish)?;
    validate_digest_fields(
        dist.get("integrity").and_then(Value::as_str),
        dist.get("shasum").and_then(Value::as_str),
        &hex_digest::<sha2::Sha512>(tarball),
        &hex_digest::<sha1::Sha1>(tarball),
    )
}

fn hex_digest<D: sha2::Digest + Default>(bytes: &[u8]) -> String {
    D::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_mapping_integrity(
    mapping: &NpmProxyTarballInput,
    metadata: &crate::storage::BlobMetadata,
) -> Result<(), ApiError> {
    validate_digest_fields(
        mapping.integrity.as_deref(),
        mapping.shasum.as_deref(),
        &metadata.sha512,
        &metadata.sha1,
    )
}

fn validate_digest_fields(
    integrity: Option<&str>,
    shasum: Option<&str>,
    actual_sha512: &str,
    actual_sha1: &str,
) -> Result<(), ApiError> {
    let integrity = integrity
        .and_then(|value| value.strip_prefix("sha512-"))
        .ok_or_else(invalid_integrity)?;
    let integrity_bytes = strict_base64_decode(integrity).ok_or_else(invalid_integrity)?;
    let actual_bytes = decode_hex(actual_sha512).ok_or_else(invalid_integrity)?;
    if integrity_bytes != actual_bytes || shasum != Some(actual_sha1) {
        return Err(invalid_integrity());
    }
    Ok(())
}

fn find_tarball_mapping<'a>(
    mappings: &'a [NpmProxyTarballInput],
    path: &str,
) -> Option<&'a NpmProxyTarballInput> {
    mappings
        .iter()
        .find(|mapping| mapping.artifact_path.as_deref() == Some(path))
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks(2)
        .map(|chunk| {
            let high = hex_nibble(chunk[0])?;
            let low = hex_nibble(chunk[1])?;
            Some((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn hex_to_base64(value: &str) -> Result<String, ApiError> {
    let bytes = decode_hex(value).ok_or_else(invalid_metadata)?;
    Ok(base64_encode(&bytes))
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    encode_base64(bytes, ALPHABET, true)
}

fn base64_url_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    encode_base64(bytes, ALPHABET, false)
}

fn encode_base64(bytes: &[u8], alphabet: &[u8; 64], padding: bool) -> String {
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or_default();
        let third = chunk.get(2).copied().unwrap_or_default();
        output.push(alphabet[(first >> 2) as usize] as char);
        output.push(alphabet[(((first & 3) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(alphabet[(((second & 15) << 2) | (third >> 6)) as usize] as char);
        } else if padding {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(alphabet[(third & 63) as usize] as char);
        } else if padding {
            output.push('=');
        }
    }
    output
}

async fn collect_limited(mut body: Body, limit: u64) -> Result<Vec<u8>, ApiError> {
    let limit = usize::try_from(limit).map_err(|_| payload_too_large())?;
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| invalid_publish())?;
        let Some(chunk) = frame.data_ref() else {
            continue;
        };
        let next_len = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or_else(payload_too_large)?;
        if next_len > limit {
            return Err(payload_too_large());
        }
        bytes.extend_from_slice(chunk);
    }
    Ok(bytes)
}

fn reject_oversized_publish(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    if headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|size| size > state.config.npm_publish_max_bytes)
    {
        Err(payload_too_large())
    } else {
        Ok(())
    }
}

async fn local_tarball(
    state: &AppState,
    repo_id: i64,
    path: &str,
    head: bool,
) -> Result<Response, ApiError> {
    let local = state
        .artifacts
        .read(repo_id, path)
        .await?
        .ok_or_else(npm_not_found)?;
    tarball_response(local.record, local.blob, head)
}

fn tarball_response(
    record: crate::persistence::ArtifactRecord,
    blob: crate::storage::BlobReadHandle,
    head: bool,
) -> Result<Response, ApiError> {
    let body = if head {
        Body::empty()
    } else {
        Body::from_stream(ReaderStream::new(blob))
    };
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&record.size.to_string())
            .map_err(|_| ApiError::internal("npm tarball 长度无效"))?,
    );
    Ok(response)
}

fn json_response(value: Value, head: bool) -> Result<Response, ApiError> {
    let bytes =
        serde_json::to_vec(&value).map_err(|_| ApiError::internal("序列化 npm 响应失败"))?;
    let length = bytes.len();
    let mut response = Response::new(if head {
        Body::empty()
    } else {
        Body::from(bytes)
    });
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string())
            .map_err(|_| ApiError::internal("npm 响应长度无效"))?,
    );
    Ok(response)
}

fn upstream_package_url(base: &str, package_name: &str) -> Result<reqwest::Url, ApiError> {
    let mut url = reqwest::Url::parse(base).map_err(|_| proxy_config_error())?;
    url.set_query(None);
    url.set_fragment(None);
    let mut segments = url.path_segments_mut().map_err(|_| proxy_config_error())?;
    segments.pop_if_empty();
    segments.push(package_name);
    drop(segments);
    Ok(url)
}

fn upstream_authorization(repository: &Repository) -> Result<Option<HeaderValue>, ApiError> {
    let Some(reference) = repository.upstream_credential_ref.as_deref() else {
        return Ok(None);
    };
    let value = std::env::var(reference).map_err(|_| proxy_config_error())?;
    if value.chars().any(char::is_control)
        || !(value.starts_with("Basic ") || value.starts_with("Bearer "))
    {
        return Err(proxy_config_error());
    }
    HeaderValue::from_str(&value)
        .map(Some)
        .map_err(|_| proxy_config_error())
}

async fn send_upstream(request: reqwest::RequestBuilder) -> Result<reqwest::Response, ApiError> {
    tokio::time::timeout(PROXY_SEND_TIMEOUT, request.send())
        .await
        .map_err(|_| proxy_timeout())?
        .map_err(|error| {
            if error.is_timeout() || error.is_connect() {
                proxy_timeout()
            } else {
                proxy_transfer_error()
            }
        })
}

async fn read_proxy_packument(
    mut response: reqwest::Response,
    idle_timeout: Duration,
) -> Result<Vec<u8>, ApiError> {
    if response
        .content_length()
        .is_some_and(|length| length > NPM_PACKUMENT_MAX_BYTES as u64)
    {
        return Err(proxy_bad_gateway());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = tokio::time::timeout(idle_timeout, response.chunk())
        .await
        .map_err(|_| proxy_timeout())?
        .map_err(|_| proxy_transfer_error())?
    {
        let next_len = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or_else(proxy_bad_gateway)?;
        if next_len > NPM_PACKUMENT_MAX_BYTES {
            return Err(proxy_bad_gateway());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn validate_upstream_status(status: reqwest::StatusCode) -> Result<(), ApiError> {
    if status.is_success() {
        Ok(())
    } else if status == reqwest::StatusCode::NOT_FOUND {
        Err(npm_not_found())
    } else {
        Err(proxy_bad_gateway())
    }
}

fn invalid_path() -> ApiError {
    ApiError::bad_request("NPM_INVALID_PATH", "npm 包路径无效")
}
fn invalid_host() -> ApiError {
    ApiError::bad_request("NPM_INVALID_HOST", "Host 请求头无效")
}
fn invalid_publish() -> ApiError {
    ApiError::bad_request("NPM_INVALID_PUBLISH", "npm publish 请求无效")
}
fn invalid_integrity() -> ApiError {
    ApiError::bad_request("NPM_INTEGRITY_MISMATCH", "npm tarball 摘要校验失败")
}
fn invalid_metadata() -> ApiError {
    ApiError::internal("npm 元数据无效")
}
fn npm_not_found() -> ApiError {
    ApiError::not_found("NPM_NOT_FOUND", "npm 包或版本不存在")
}
fn payload_too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "NPM_PUBLISH_TOO_LARGE",
        "npm publish 请求超过大小上限",
    )
}
fn method_not_allowed() -> ApiError {
    ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "METHOD_NOT_ALLOWED",
        "该 npm 仓库类型不允许写入",
    )
}
fn proxy_config_error() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "PROXY_UPSTREAM_CONFIG",
        "代理上游配置无效",
    )
}
fn proxy_timeout() -> ApiError {
    ApiError::new(
        StatusCode::GATEWAY_TIMEOUT,
        "PROXY_UPSTREAM_TIMEOUT",
        "代理上游响应超时",
    )
}
fn proxy_transfer_error() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "PROXY_UPSTREAM_TRANSFER",
        "代理上游传输中断",
    )
}
fn proxy_bad_gateway() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "PROXY_UPSTREAM_FAILURE",
        "代理上游响应失败",
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn 包路由安全解析scoped与unscoped() {
        assert_eq!(validate_package_name("demo").unwrap(), "demo");
        assert_eq!(decode_percent("@scope%2fpkg").unwrap(), "@scope/pkg");
        for invalid in ["", "a/b", "../a", "@scope", "@/pkg", "a%252fb"] {
            assert!(validate_package_name(invalid).is_err());
        }
    }

    #[test]
    fn proxy同名tarball按版本生成唯一确定路径() {
        let upstream = json!({
            "name": "demo",
            "versions": {
                "1.0.0": {"dist": {"tarball": "https://registry.example/v1/shared.tgz"}},
                "2.0.0": {"dist": {"tarball": "https://registry.example/v2/shared.tgz"}}
            }
        });
        let (rewritten, mappings) =
            rewrite_proxy_packument(&upstream, "demo", "npm-proxy", "https://local.example")
                .unwrap();
        let first = rewritten["versions"]["1.0.0"]["dist"]["tarball"]
            .as_str()
            .unwrap();
        let second = rewritten["versions"]["2.0.0"]["dist"]["tarball"]
            .as_str()
            .unwrap();

        assert_ne!(first, second);
        assert_eq!(
            first,
            "https://local.example/repository/npm-proxy/demo/-/v-MS4wLjA.tgz"
        );
        assert_eq!(
            mappings[0].artifact_path.as_deref(),
            Some("demo/-/v-MS4wLjA.tgz")
        );
        assert_eq!(
            mappings[1].artifact_path.as_deref(),
            Some("demo/-/v-Mi4wLjA.tgz")
        );
    }

    #[test]
    fn group聚合前序版本与标签优先() {
        let first = json!({"versions": {"1": {"source": "first"}}, "dist-tags": {"latest": "1"}});
        let second = json!({"versions": {"1": {"source": "second"}, "2": {}}, "dist-tags": {"latest": "2", "next": "2"}});
        let merged = merge_group_packuments(&[first, second], "demo");
        assert_eq!(merged["versions"]["1"]["source"], "first");
        assert_eq!(merged["versions"]["2"], json!({}));
        assert_eq!(merged["dist-tags"]["latest"], "1");
        assert_eq!(merged["dist-tags"]["next"], "2");
    }
}
