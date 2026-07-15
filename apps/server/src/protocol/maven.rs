//! Maven2 协议业务：标准布局、权威元数据、动态校验和与代理缓存。

use std::cmp::Ordering;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::header::{
    AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, ETAG, HeaderMap, HeaderValue,
};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use quick_xml::se::to_string;
use serde::Serialize;
use tokio_util::io::ReaderStream;

use crate::api::extractor::ArtifactRoute;
use crate::artifact::{ArtifactRead, ArtifactWriter};
use crate::domain::artifact::ArtifactPath;
use crate::domain::repository::{
    Format, OverwritePolicy, Permission, ProtocolRepositoryContext, RepoType, Repository, Subject,
    VersionPolicy,
};
use crate::error::ApiError;
use crate::persistence::ArtifactRecord;
use crate::repository;
use crate::state::AppState;
use crate::storage::PendingBlob;

const METADATA_FILE: &str = "maven-metadata.xml";
const PROXY_SEND_TIMEOUT: Duration = Duration::from_secs(30);
const PROXY_CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_CHECKSUM_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
struct MavenCoordinate {
    group_id: String,
    artifact_id: String,
    version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MavenArtifact {
    coordinate: MavenCoordinate,
    extension: String,
    classifier: Option<String>,
    snapshot: Option<SnapshotIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SnapshotIdentity {
    timestamp: String,
    build_number: u64,
    value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChecksumKind {
    Sha1,
    Md5,
    Sha256,
}

#[derive(Debug, Serialize)]
#[serde(rename = "metadata")]
struct ArtifactMetadata {
    #[serde(rename = "modelVersion")]
    model_version: &'static str,
    #[serde(rename = "groupId")]
    group_id: String,
    #[serde(rename = "artifactId")]
    artifact_id: String,
    versioning: ArtifactVersioning,
}

#[derive(Debug, Serialize)]
struct ArtifactVersioning {
    #[serde(skip_serializing_if = "Option::is_none")]
    latest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    release: Option<String>,
    versions: Versions,
    #[serde(rename = "lastUpdated")]
    last_updated: String,
}

#[derive(Debug, Serialize)]
struct Versions {
    version: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename = "metadata")]
struct SnapshotMetadata {
    #[serde(rename = "modelVersion")]
    model_version: &'static str,
    #[serde(rename = "groupId")]
    group_id: String,
    #[serde(rename = "artifactId")]
    artifact_id: String,
    version: String,
    versioning: SnapshotVersioning,
}

#[derive(Debug, Serialize)]
struct SnapshotVersioning {
    snapshot: SnapshotValue,
    #[serde(rename = "lastUpdated")]
    last_updated: String,
    #[serde(rename = "snapshotVersions")]
    snapshot_versions: SnapshotVersions,
}

#[derive(Debug, Serialize)]
struct SnapshotValue {
    timestamp: String,
    #[serde(rename = "buildNumber")]
    build_number: u64,
}

#[derive(Debug, Serialize)]
struct SnapshotVersions {
    #[serde(rename = "snapshotVersion")]
    snapshot_version: Vec<SnapshotVersion>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
struct SnapshotVersion {
    extension: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    classifier: Option<String>,
    value: String,
    updated: String,
}

/// 读取 Maven 构件、元数据或动态校验和。
pub async fn get(
    state: &AppState,
    subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
    context: ProtocolRepositoryContext,
) -> Result<Response, ApiError> {
    dispatch_get(state, subject, route, uri, context, false).await
}

/// 探测 Maven 构件、元数据或动态校验和。
pub async fn head(
    state: &AppState,
    subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
    context: ProtocolRepositoryContext,
) -> Result<Response, ApiError> {
    dispatch_get(state, subject, route, uri, context, true).await
}

async fn dispatch_get(
    state: &AppState,
    subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
    context: ProtocolRepositoryContext,
    head: bool,
) -> Result<Response, ApiError> {
    ensure_maven(&context.repository)?;
    let path = maven_path(&route, uri)?;
    match context.repository.repo_type {
        RepoType::Hosted => hosted_get(state, &context.repository, &path, head).await,
        RepoType::Proxy => proxy_get(state, subject, context.repository, &route, &path, head).await,
        RepoType::Group => group_get(state, &context.members, &path, head).await,
    }
}

/// 写入 Maven hosted 构件；元数据由服务端生成，客户端上传被忽略。
pub async fn put(
    state: &AppState,
    subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
    headers: &HeaderMap,
    body: Body,
    context: ProtocolRepositoryContext,
) -> Result<Response, ApiError> {
    let repository = context.repository;
    ensure_maven(&repository)?;
    let path = maven_path(&route, uri)?;
    ensure_hosted(&repository)?;
    repository::protocol_repository(&state.pool, subject, &route.repository, Permission::Write)
        .await?;
    reject_oversized_request(state, headers)?;
    if ignored_metadata_upload(path.as_str()) {
        return Ok(StatusCode::OK.into_response());
    }
    if let Some((base, kind)) = checksum_target(path.as_str()) {
        return verify_sidecar_upload(state, repository.id, base, kind, body).await;
    }
    let artifact = parse_artifact_path(path.as_str())?;
    enforce_version_policy(&repository, &artifact.coordinate)?;
    write_hosted_artifact(state, &repository, &artifact, &path, body).await
}

/// 删除 Maven hosted 构件；动态元数据与 sidecar 不持久化。
pub async fn delete(
    state: &AppState,
    subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
    context: ProtocolRepositoryContext,
) -> Result<Response, ApiError> {
    let repository = context.repository;
    ensure_maven(&repository)?;
    let path = maven_path(&route, uri)?;
    ensure_hosted(&repository)?;
    repository::protocol_repository(&state.pool, subject, &route.repository, Permission::Write)
        .await?;
    if checksum_target(path.as_str()).is_some() || metadata_coordinate(path.as_str()).is_some() {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    parse_artifact_path(path.as_str())?;
    match state.artifacts.delete(repository.id, path.as_str()).await? {
        Some(_) => Ok(StatusCode::NO_CONTENT.into_response()),
        None => Err(artifact_not_found()),
    }
}

/// 供 Maven group 聚合器调用的单仓读取入口。
pub(crate) async fn member_get(
    state: &AppState,
    repository: &Repository,
    path: &ArtifactPath,
    head: bool,
) -> Result<Response, ApiError> {
    match repository.repo_type {
        RepoType::Hosted => hosted_get(state, repository, path, head).await,
        RepoType::Proxy => proxy_get_without_reload(state, repository, path, head).await,
        RepoType::Group => Err(artifact_not_found()),
    }
}

async fn group_get(
    state: &AppState,
    members: &[Repository],
    path: &ArtifactPath,
    head: bool,
) -> Result<Response, ApiError> {
    if let Some(coordinate) =
        metadata_coordinate(path.as_str()).filter(|value| value.version.is_none())
    {
        return group_artifact_metadata(state, members, &coordinate, path, head).await;
    }
    group_first_match(state, members, path, head).await
}

async fn group_artifact_metadata(
    state: &AppState,
    members: &[Repository],
    coordinate: &MavenCoordinate,
    path: &ArtifactPath,
    head: bool,
) -> Result<Response, ApiError> {
    let hosted_ids = members
        .iter()
        .filter(|member| member.repo_type == RepoType::Hosted)
        .map(|member| member.id)
        .collect::<Vec<_>>();
    let records_by_repo = state
        .artifacts
        .list_by_prefix_batch(&hosted_ids, &coordinate_prefix(coordinate))
        .await?;
    let records = members
        .iter()
        .filter_map(|member| records_by_repo.get(&member.id))
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    if records.is_empty()
        && let Some(response) = first_proxy_match(state, members, path, head).await?
    {
        return Ok(response);
    }
    let bytes = artifact_metadata_xml(coordinate, &records)?.into_bytes();
    bytes_response(bytes, "application/xml", head)
}

async fn first_proxy_match(
    state: &AppState,
    members: &[Repository],
    path: &ArtifactPath,
    head: bool,
) -> Result<Option<Response>, ApiError> {
    for member in members
        .iter()
        .filter(|member| member.repo_type == RepoType::Proxy)
    {
        match member_get(state, member, path, head).await {
            Err(error) if error.status() == StatusCode::NOT_FOUND => continue,
            Err(error) => return Err(error),
            Ok(response) => return Ok(Some(response)),
        }
    }
    Ok(None)
}

async fn group_first_match(
    state: &AppState,
    members: &[Repository],
    path: &ArtifactPath,
    head: bool,
) -> Result<Response, ApiError> {
    let lookup_path = checksum_target(path.as_str()).map_or(path.as_str(), |(base, _)| base);
    let repo_ids = members.iter().map(|member| member.id).collect::<Vec<_>>();
    let cached = state
        .artifacts
        .find_by_path_batch(&repo_ids, lookup_path)
        .await?;
    let dynamic =
        checksum_target(path.as_str()).is_some() || metadata_coordinate(path.as_str()).is_some();
    for member in members {
        if member.repo_type == RepoType::Hosted && !dynamic && !cached.contains_key(&member.id) {
            continue;
        }
        match member_get(state, member, path, head).await {
            Err(error) if error.status() == StatusCode::NOT_FOUND => continue,
            result => return result,
        }
    }
    Err(artifact_not_found())
}

async fn hosted_get(
    state: &AppState,
    repository: &Repository,
    path: &ArtifactPath,
    head: bool,
) -> Result<Response, ApiError> {
    if let Some((base, kind)) = checksum_target(path.as_str()) {
        return generated_target_response(state, repository.id, base, kind, head).await;
    }
    if let Some(coordinate) = metadata_coordinate(path.as_str()) {
        let bytes = generate_metadata(state, repository.id, &coordinate).await?;
        return bytes_response(bytes, "application/xml", head);
    }
    parse_artifact_path(path.as_str())?;
    local_artifact_response(state, repository.id, path.as_str(), head).await
}

async fn generated_target_response(
    state: &AppState,
    repo_id: i64,
    base: &str,
    kind: ChecksumKind,
    head: bool,
) -> Result<Response, ApiError> {
    let bytes = if let Some(coordinate) = metadata_coordinate(base) {
        let metadata = generate_metadata(state, repo_id, &coordinate).await?;
        checksum_bytes(kind, &metadata).into_bytes()
    } else {
        let record = state
            .artifacts
            .read(repo_id, base)
            .await?
            .ok_or_else(artifact_not_found)?
            .record;
        record_checksum(&record, kind).as_bytes().to_vec()
    };
    bytes_response(bytes, "text/plain", head)
}

async fn generate_metadata(
    state: &AppState,
    repo_id: i64,
    coordinate: &MavenCoordinate,
) -> Result<Vec<u8>, ApiError> {
    let prefix = coordinate_prefix(coordinate);
    let records = state.artifacts.list_by_prefix(repo_id, &prefix).await?;
    let xml = match coordinate.version.as_deref() {
        Some(version) => snapshot_metadata_xml(coordinate, version, &records)?,
        None => artifact_metadata_xml(coordinate, &records)?,
    };
    Ok(xml.into_bytes())
}

async fn verify_sidecar_upload(
    state: &AppState,
    repo_id: i64,
    base: &str,
    kind: ChecksumKind,
    body: Body,
) -> Result<Response, ApiError> {
    let expected = if let Some(coordinate) = metadata_coordinate(base) {
        checksum_bytes(kind, &generate_metadata(state, repo_id, &coordinate).await?)
    } else {
        let record = state
            .artifacts
            .read(repo_id, base)
            .await?
            .ok_or_else(artifact_not_found)?
            .record;
        record_checksum(&record, kind).to_string()
    };
    let supplied = read_small_body(body).await?;
    if supplied.trim().eq_ignore_ascii_case(&expected) {
        Ok(StatusCode::OK.into_response())
    } else {
        Err(checksum_mismatch())
    }
}

async fn write_hosted_artifact(
    state: &AppState,
    repository: &Repository,
    artifact: &MavenArtifact,
    path: &ArtifactPath,
    body: Body,
) -> Result<Response, ApiError> {
    let pending = stream_body(state, body).await?;
    let snapshot_timestamped = artifact.snapshot.is_some();
    let allow = snapshot_timestamped || repository.overwrite_policy == Some(OverwritePolicy::Allow);
    let committed = state
        .artifacts
        .commit_pending(repository.id, path.as_str(), pending, false, allow)
        .await?;
    Ok(if committed.created {
        StatusCode::CREATED.into_response()
    } else {
        StatusCode::OK.into_response()
    })
}

async fn stream_body(state: &AppState, mut body: Body) -> Result<PendingBlob, ApiError> {
    let mut writer = state.artifacts.begin_write().await?;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| invalid_body())?;
        if let Ok(data) = frame.into_data() {
            writer.write_chunk(&data).await?;
        }
    }
    writer.finish().await
}

async fn local_artifact_response(
    state: &AppState,
    repo_id: i64,
    path: &str,
    head: bool,
) -> Result<Response, ApiError> {
    let artifact = state
        .artifacts
        .read(repo_id, path)
        .await?
        .ok_or_else(artifact_not_found)?;
    artifact_response(artifact, path, head)
}

fn artifact_response(artifact: ArtifactRead, path: &str, head: bool) -> Result<Response, ApiError> {
    let body = if head {
        Body::empty()
    } else {
        Body::from_stream(ReaderStream::new(artifact.blob))
    };
    let mut response = Response::new(body);
    apply_headers(response.headers_mut(), &artifact.record, path)?;
    Ok(response)
}

fn bytes_response(bytes: Vec<u8>, content_type: &str, head: bool) -> Result<Response, ApiError> {
    let length = bytes.len();
    let mut response = Response::new(if head {
        Body::empty()
    } else {
        Body::from(bytes)
    });
    insert_header(response.headers_mut(), CONTENT_TYPE, content_type)?;
    insert_header(response.headers_mut(), CONTENT_LENGTH, &length.to_string())?;
    Ok(response)
}

fn apply_headers(
    headers: &mut HeaderMap,
    record: &ArtifactRecord,
    path: &str,
) -> Result<(), ApiError> {
    let content_type = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();
    insert_header(headers, CONTENT_TYPE, &content_type)?;
    insert_header(headers, CONTENT_LENGTH, &record.size.to_string())?;
    insert_header(headers, ETAG, &format!("\"{}\"", record.sha256))?;
    insert_header(headers, "x-checksum-sha256", &record.sha256)?;
    insert_header(headers, "x-checksum-sha1", &record.sha1)?;
    insert_header(headers, "x-checksum-md5", &record.md5)
}

async fn proxy_get(
    state: &AppState,
    subject: Subject,
    mut repository: Repository,
    route: &ArtifactRoute,
    path: &ArtifactPath,
    head: bool,
) -> Result<Response, ApiError> {
    let mut retried = false;
    loop {
        match proxy_get_without_reload(state, &repository, path, head).await {
            Err(error) if error.code() == "PROXY_CONFIGURATION_CHANGED" && !retried => {
                repository = reload_proxy(state, subject, &route.repository).await?;
                retried = true;
            }
            result => return result,
        }
    }
}

async fn proxy_get_without_reload(
    state: &AppState,
    repository: &Repository,
    path: &ArtifactPath,
    head: bool,
) -> Result<Response, ApiError> {
    validate_proxy_path(path.as_str())?;
    if let Some((base, kind)) = checksum_target(path.as_str()) {
        ensure_proxy_cached(state, repository, base).await?;
        return cached_sidecar_response(state, repository.id, base, kind, head).await;
    }
    ensure_proxy_cached(state, repository, path.as_str()).await?;
    local_artifact_response(state, repository.id, path.as_str(), head).await
}

async fn cached_sidecar_response(
    state: &AppState,
    repo_id: i64,
    base: &str,
    kind: ChecksumKind,
    head: bool,
) -> Result<Response, ApiError> {
    let record = state
        .artifacts
        .read(repo_id, base)
        .await?
        .ok_or_else(artifact_not_found)?
        .record;
    bytes_response(
        record_checksum(&record, kind).as_bytes().to_vec(),
        "text/plain",
        head,
    )
}

async fn ensure_proxy_cached(
    state: &AppState,
    repository: &Repository,
    path: &str,
) -> Result<(), ApiError> {
    let is_metadata = metadata_coordinate(path).is_some();
    if cache_is_usable(state, repository.id, path, is_metadata).await? {
        return Ok(());
    }
    refresh_proxy_cache(state, repository, path, is_metadata).await
}

async fn cache_is_usable(
    state: &AppState,
    repo_id: i64,
    path: &str,
    is_metadata: bool,
) -> Result<bool, ApiError> {
    if !is_metadata {
        return Ok(state.artifacts.read(repo_id, path).await?.is_some());
    }
    Ok(state
        .artifacts
        .cache_status(repo_id, path, state.config.maven_metadata_ttl_secs)
        .await?
        .is_some_and(|status| status.fresh))
}

async fn refresh_proxy_cache(
    state: &AppState,
    repository: &Repository,
    path: &str,
    stale_allowed: bool,
) -> Result<(), ApiError> {
    let upstream = repository
        .upstream_url
        .as_deref()
        .ok_or_else(proxy_config_error)?;
    let task_state = state.clone();
    let task_repository = repository.clone();
    let task_path = path.to_string();
    let result = state
        .artifacts
        .proxy_singleflight(repository.id, path, upstream, move || async move {
            if cache_is_usable(&task_state, task_repository.id, &task_path, stale_allowed).await? {
                return Ok(());
            }
            fetch_and_cache(&task_state, &task_repository, &task_path).await
        })
        .await;
    if result.is_err()
        && stale_allowed
        && state.artifacts.read(repository.id, path).await?.is_some()
    {
        tracing::warn!(repository = %repository.name, path, "Maven 元数据回源失败，继续使用旧缓存");
        return Ok(());
    }
    result
}

async fn fetch_and_cache(
    state: &AppState,
    repository: &Repository,
    path: &str,
) -> Result<(), ApiError> {
    let expected_upstream = repository
        .upstream_url
        .as_deref()
        .ok_or_else(proxy_config_error)?;
    let mut response = send_upstream(upstream_request(state, repository, path)?).await?;
    validate_upstream_status(response.status())?;
    reject_upstream_size(state, &response)?;
    let mut writer = state.artifacts.begin_write().await?;
    stream_upstream_body(&mut response, &mut writer).await?;
    let pending = writer.finish().await?;
    if let Err(error) = verify_upstream_sha1(state, repository, path, &pending).await {
        state.artifacts.discard_pending(pending).await;
        return Err(error);
    }
    state
        .artifacts
        .commit_proxy_pending(repository.id, path, pending, expected_upstream)
        .await?;
    Ok(())
}

fn upstream_request(
    state: &AppState,
    repository: &Repository,
    path: &str,
) -> Result<reqwest::RequestBuilder, ApiError> {
    let mut request = state
        .artifacts
        .proxy_client()?
        .get(upstream_url(repository, path)?);
    if let Some(authorization) = upstream_authorization(repository)? {
        request = request.header(AUTHORIZATION, authorization);
    }
    Ok(request)
}

async fn verify_upstream_sha1(
    state: &AppState,
    repository: &Repository,
    path: &str,
    pending: &PendingBlob,
) -> Result<(), ApiError> {
    let sidecar_path = format!("{path}.sha1");
    let mut response = send_upstream(upstream_request(state, repository, &sidecar_path)?).await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(());
    }
    validate_upstream_status(response.status())?;
    let supplied = read_small_upstream(&mut response).await?;
    if supplied
        .trim()
        .eq_ignore_ascii_case(&pending.metadata().sha1)
    {
        Ok(())
    } else {
        Err(proxy_checksum_mismatch())
    }
}

async fn send_upstream(request: reqwest::RequestBuilder) -> Result<reqwest::Response, ApiError> {
    tokio::time::timeout(PROXY_SEND_TIMEOUT, request.send())
        .await
        .map_err(|_| proxy_timeout())?
        .map_err(map_upstream_transport)
}

async fn stream_upstream_body(
    response: &mut reqwest::Response,
    writer: &mut ArtifactWriter,
) -> Result<(), ApiError> {
    while let Some(chunk) = next_upstream_chunk(response).await? {
        writer.write_chunk(&chunk).await?;
    }
    Ok(())
}

async fn next_upstream_chunk(response: &mut reqwest::Response) -> Result<Option<Bytes>, ApiError> {
    tokio::time::timeout(PROXY_CHUNK_IDLE_TIMEOUT, response.chunk())
        .await
        .map_err(|_| proxy_timeout())?
        .map_err(|_| proxy_transfer_error())
}

async fn read_small_upstream(response: &mut reqwest::Response) -> Result<String, ApiError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = next_upstream_chunk(response).await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_CHECKSUM_BYTES {
            return Err(proxy_checksum_mismatch());
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| proxy_checksum_mismatch())
}

fn reject_upstream_size(state: &AppState, response: &reqwest::Response) -> Result<(), ApiError> {
    if response
        .content_length()
        .is_some_and(|size| size > state.config.max_artifact_bytes)
    {
        Err(artifact_too_large())
    } else {
        Ok(())
    }
}

async fn reload_proxy(
    state: &AppState,
    subject: Subject,
    name: &str,
) -> Result<Repository, ApiError> {
    let repository =
        repository::protocol_repository(&state.pool, subject, name, Permission::Read).await?;
    ensure_maven(&repository)?;
    if repository.repo_type == RepoType::Proxy {
        Ok(repository)
    } else {
        Err(proxy_config_error())
    }
}

fn upstream_url(repository: &Repository, path: &str) -> Result<reqwest::Url, ApiError> {
    let base = repository
        .upstream_url
        .as_deref()
        .ok_or_else(proxy_config_error)?;
    let mut url = reqwest::Url::parse(base).map_err(|_| proxy_config_error())?;
    url.set_query(None);
    url.set_fragment(None);
    let mut segments = url.path_segments_mut().map_err(|_| proxy_config_error())?;
    segments.pop_if_empty();
    for segment in path.split('/') {
        segments.push(segment);
    }
    drop(segments);
    Ok(url)
}

fn upstream_authorization(repository: &Repository) -> Result<Option<HeaderValue>, ApiError> {
    let Some(reference) = repository.upstream_credential_ref.as_deref() else {
        return Ok(None);
    };
    let value = std::env::var(reference).map_err(|_| proxy_config_error())?;
    if !valid_authorization_value(&value) {
        return Err(proxy_config_error());
    }
    HeaderValue::from_str(&value)
        .map(Some)
        .map_err(|_| proxy_config_error())
}

fn valid_authorization_value(value: &str) -> bool {
    !value.chars().any(char::is_control)
        && value
            .strip_prefix("Basic ")
            .or_else(|| value.strip_prefix("Bearer "))
            .is_some_and(|credential| {
                !credential.is_empty() && !credential.chars().any(char::is_whitespace)
            })
}

fn validate_upstream_status(status: reqwest::StatusCode) -> Result<(), ApiError> {
    if status.is_success() {
        Ok(())
    } else if status == reqwest::StatusCode::NOT_FOUND {
        Err(proxy_not_found())
    } else if status.is_redirection() {
        Err(proxy_redirect_error())
    } else if status.is_client_error() {
        Err(proxy_client_error())
    } else if status.is_server_error() {
        Err(proxy_server_error())
    } else {
        Err(proxy_bad_gateway())
    }
}

fn map_upstream_transport(error: reqwest::Error) -> ApiError {
    if error.is_timeout() {
        proxy_timeout()
    } else if error.is_connect() {
        proxy_connection_error()
    } else {
        proxy_transfer_error()
    }
}

fn maven_path(route: &ArtifactRoute, uri: &Uri) -> Result<ArtifactPath, ApiError> {
    validate_encoded_path(uri)?;
    if route.trailing_slash || uri.query().is_some() {
        return Err(invalid_path());
    }
    ArtifactPath::parse(&route.path).map_err(|_| invalid_path())
}

fn validate_encoded_path(uri: &Uri) -> Result<(), ApiError> {
    let raw = uri
        .path()
        .strip_prefix("/repository/")
        .and_then(|value| value.split_once('/').map(|(_, path)| path))
        .ok_or_else(invalid_path)?;
    ArtifactPath::validate_uri_encoding(raw).map_err(|_| invalid_path())
}

fn parse_artifact_path(path: &str) -> Result<MavenArtifact, ApiError> {
    let segments: Vec<_> = path.split('/').collect();
    if segments.len() < 4 || !segments.iter().all(valid_maven_segment) {
        return Err(invalid_path());
    }
    let file = segments[segments.len() - 1];
    let version = segments[segments.len() - 2];
    let artifact_id = segments[segments.len() - 3];
    let group = &segments[..segments.len() - 3];
    let coordinate = MavenCoordinate {
        group_id: group.join("."),
        artifact_id: artifact_id.to_string(),
        version: Some(version.to_string()),
    };
    let (extension, classifier, snapshot) = artifact_filename(&coordinate, file)?;
    Ok(MavenArtifact {
        coordinate,
        extension,
        classifier,
        snapshot,
    })
}

fn artifact_filename(
    coordinate: &MavenCoordinate,
    file: &str,
) -> Result<(String, Option<String>, Option<SnapshotIdentity>), ApiError> {
    let version = coordinate.version.as_deref().ok_or_else(invalid_path)?;
    if version.ends_with("-SNAPSHOT")
        && let Some(parsed) = timestamped_filename(coordinate, file)
    {
        return Ok(parsed);
    }
    let prefix = format!("{}-{version}", coordinate.artifact_id);
    filename_suffix(file.strip_prefix(&prefix).ok_or_else(invalid_path)?)
        .map(|(extension, classifier)| (extension, classifier, None))
}

fn timestamped_filename(
    coordinate: &MavenCoordinate,
    file: &str,
) -> Option<(String, Option<String>, Option<SnapshotIdentity>)> {
    let version = coordinate.version.as_deref()?;
    version.strip_suffix("-SNAPSHOT")?;
    let suffix = file.strip_prefix(&format!("{}-", coordinate.artifact_id))?;
    let identity = parse_snapshot_identity(version, suffix)?;
    let remainder = suffix.strip_prefix(&identity.value)?;
    let (extension, classifier) = filename_suffix(remainder).ok()?;
    Some((extension, classifier, Some(identity)))
}

fn filename_suffix(value: &str) -> Result<(String, Option<String>), ApiError> {
    if let Some(extension) = value.strip_prefix('.')
        && !extension.is_empty()
    {
        return Ok((extension.to_string(), None));
    }
    let classified = value.strip_prefix('-').ok_or_else(invalid_path)?;
    let (classifier, extension) = classified.split_once('.').ok_or_else(invalid_path)?;
    if classifier.is_empty() || extension.is_empty() {
        return Err(invalid_path());
    }
    Ok((extension.to_string(), Some(classifier.to_string())))
}

fn parse_snapshot_identity(version: &str, suffix: &str) -> Option<SnapshotIdentity> {
    let base = version.strip_suffix("-SNAPSHOT")?;
    let remainder = suffix.strip_prefix(&format!("{base}-"))?;
    let (timestamp, remainder) = remainder.split_once('-')?;
    let build = remainder
        .split(['.', '-'])
        .next()
        .filter(|value| !value.is_empty())?;
    if timestamp.len() != 15 || timestamp.as_bytes().get(8) != Some(&b'.') {
        return None;
    }
    let digits = timestamp
        .chars()
        .filter(|value| *value != '.')
        .all(|value| value.is_ascii_digit());
    let build_number = build.parse().ok()?;
    if !digits || build_number == 0 {
        return None;
    }
    Some(SnapshotIdentity {
        timestamp: timestamp.to_string(),
        build_number,
        value: format!("{base}-{timestamp}-{build}"),
    })
}

fn metadata_coordinate(path: &str) -> Option<MavenCoordinate> {
    let segments: Vec<_> = path.split('/').collect();
    if segments.last()? != &METADATA_FILE || segments.len() < 3 {
        return None;
    }
    let parent = &segments[..segments.len() - 1];
    let version_level = parent.last()?.ends_with("-SNAPSHOT");
    let artifact_index = parent.len().checked_sub(usize::from(version_level) + 1)?;
    if artifact_index == 0 || !segments.iter().all(valid_maven_segment) {
        return None;
    }
    Some(MavenCoordinate {
        group_id: parent[..artifact_index].join("."),
        artifact_id: parent[artifact_index].to_string(),
        version: version_level.then(|| parent[parent.len() - 1].to_string()),
    })
}

fn coordinate_prefix(coordinate: &MavenCoordinate) -> String {
    let mut prefix = format!(
        "{}/{}/",
        coordinate.group_id.replace('.', "/"),
        coordinate.artifact_id
    );
    if let Some(version) = &coordinate.version {
        prefix.push_str(version);
        prefix.push('/');
    }
    prefix
}

fn valid_maven_segment(segment: &&str) -> bool {
    !segment.is_empty()
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn checksum_target(path: &str) -> Option<(&str, ChecksumKind)> {
    [
        (".sha256", ChecksumKind::Sha256),
        (".sha1", ChecksumKind::Sha1),
        (".md5", ChecksumKind::Md5),
    ]
    .into_iter()
    .find_map(|(suffix, kind)| path.strip_suffix(suffix).map(|base| (base, kind)))
}

fn validate_proxy_path(path: &str) -> Result<(), ApiError> {
    if path.ends_with(".sha512") {
        return Err(invalid_path());
    }
    let base = checksum_target(path).map_or(path, |(base, _)| base);
    if checksum_target(base).is_some() {
        return Err(invalid_path());
    }
    if metadata_coordinate(base).is_some() || parse_artifact_path(base).is_ok() {
        Ok(())
    } else {
        Err(invalid_path())
    }
}

fn ignored_metadata_upload(path: &str) -> bool {
    metadata_coordinate(path).is_some()
        || checksum_target(path).is_some_and(|(base, _)| metadata_coordinate(base).is_some())
}

fn enforce_version_policy(
    repository: &Repository,
    coordinate: &MavenCoordinate,
) -> Result<(), ApiError> {
    let snapshot = coordinate
        .version
        .as_deref()
        .is_some_and(|version| version.ends_with("-SNAPSHOT"));
    match (repository.version_policy, snapshot) {
        (Some(VersionPolicy::Release), true) => Err(version_policy_error()),
        (Some(VersionPolicy::Snapshot), false) => Err(version_policy_error()),
        _ => Ok(()),
    }
}

fn artifact_metadata_xml(
    coordinate: &MavenCoordinate,
    records: &[ArtifactRecord],
) -> Result<String, ApiError> {
    let prefix = coordinate_prefix(coordinate);
    let mut versions: Vec<_> = records
        .iter()
        .filter_map(|record| record.path.strip_prefix(&prefix))
        .filter_map(|rest| rest.split_once('/').map(|value| value.0.to_string()))
        .collect();
    versions.sort_by(|left, right| comparable_version(left, right));
    versions.dedup();
    let latest = versions.last().cloned();
    let release = versions
        .iter()
        .rev()
        .find(|version| !version.ends_with("-SNAPSHOT"))
        .cloned();
    let metadata = ArtifactMetadata {
        model_version: "1.1.0",
        group_id: coordinate.group_id.clone(),
        artifact_id: coordinate.artifact_id.clone(),
        versioning: ArtifactVersioning {
            latest,
            release,
            versions: Versions { version: versions },
            last_updated: latest_update(records),
        },
    };
    serialize_metadata(&metadata)
}

fn snapshot_metadata_xml(
    coordinate: &MavenCoordinate,
    version: &str,
    records: &[ArtifactRecord],
) -> Result<String, ApiError> {
    let artifacts: Vec<_> = records
        .iter()
        .filter_map(|record| {
            parse_artifact_path(&record.path)
                .ok()
                .map(|artifact| (record, artifact))
        })
        .filter(|(_, artifact)| artifact.coordinate.version.as_deref() == Some(version))
        .collect();
    let latest = artifacts
        .iter()
        .filter_map(|(_, artifact)| artifact.snapshot.as_ref())
        .max()
        .cloned()
        .ok_or_else(artifact_not_found)?;
    let versions = latest_snapshot_versions(&artifacts);
    let metadata = SnapshotMetadata {
        model_version: "1.1.0",
        group_id: coordinate.group_id.clone(),
        artifact_id: coordinate.artifact_id.clone(),
        version: version.to_string(),
        versioning: SnapshotVersioning {
            snapshot: SnapshotValue {
                timestamp: latest.timestamp,
                build_number: latest.build_number,
            },
            last_updated: latest_update(records),
            snapshot_versions: SnapshotVersions {
                snapshot_version: versions,
            },
        },
    };
    serialize_metadata(&metadata)
}

fn latest_snapshot_versions(records: &[(&ArtifactRecord, MavenArtifact)]) -> Vec<SnapshotVersion> {
    let mut versions: Vec<_> = records
        .iter()
        .filter_map(|(record, artifact)| {
            artifact.snapshot.as_ref().map(|identity| {
                (
                    artifact.extension.clone(),
                    artifact.classifier.clone(),
                    identity.clone(),
                    normalize_updated(&record.updated_at),
                )
            })
        })
        .collect();
    versions
        .sort_by(|left, right| (&left.0, &left.1, &left.2).cmp(&(&right.0, &right.1, &right.2)));
    versions.dedup_by(|left, right| {
        if left.0 == right.0 && left.1 == right.1 {
            *left = right.clone();
            true
        } else {
            false
        }
    });
    versions
        .into_iter()
        .map(
            |(extension, classifier, identity, updated)| SnapshotVersion {
                extension,
                classifier,
                value: identity.value,
                updated,
            },
        )
        .collect()
}

fn latest_update(records: &[ArtifactRecord]) -> String {
    records
        .iter()
        .map(|record| normalize_updated(&record.updated_at))
        .max()
        .unwrap_or_else(|| "19700101000000".to_string())
}

fn normalize_updated(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_digit)
        .take(14)
        .collect()
}

fn serialize_metadata<T: Serialize>(value: &T) -> Result<String, ApiError> {
    to_string(value)
        .map(|xml| format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{xml}"))
        .map_err(|_| ApiError::internal("生成 Maven 元数据失败"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VersionSeparator {
    Dot,
    Hyphen,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum VersionItem {
    Number(VersionSeparator, String),
    Qualifier(u8, String),
}

fn comparable_version(left: &str, right: &str) -> Ordering {
    let left = version_items(left);
    let right = version_items(right);
    for index in 0..left.len().max(right.len()) {
        let ordering = compare_version_item(left.get(index), right.get(index));
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

fn version_items(version: &str) -> Vec<VersionItem> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut numeric = None;
    let mut separator = VersionSeparator::Dot;
    for character in version.to_ascii_lowercase().chars() {
        if matches!(character, '.' | '-' | '_') {
            push_version_item(&mut parts, &mut current, separator);
            separator = if character == '-' {
                VersionSeparator::Hyphen
            } else {
                VersionSeparator::Dot
            };
            numeric = None;
            continue;
        }
        let next_numeric = character.is_ascii_digit();
        if numeric.is_some_and(|value| value != next_numeric) {
            push_version_item(&mut parts, &mut current, separator);
            separator = VersionSeparator::Hyphen;
        }
        current.push(character);
        numeric = Some(next_numeric);
    }
    push_version_item(&mut parts, &mut current, separator);
    trim_version_items(&mut parts);
    parts
}

fn push_version_item(
    parts: &mut Vec<VersionItem>,
    current: &mut String,
    separator: VersionSeparator,
) {
    if current.is_empty() {
        return;
    }
    let value = std::mem::take(current);
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        let normalized = value.trim_start_matches('0');
        parts.push(VersionItem::Number(
            separator,
            if normalized.is_empty() {
                "0".to_string()
            } else {
                normalized.to_string()
            },
        ));
    } else {
        let canonical = canonical_qualifier(&value);
        let (rank, text) = qualifier_rank(canonical);
        parts.push(VersionItem::Qualifier(rank, text));
    }
}

fn canonical_qualifier(value: &str) -> &str {
    match value {
        "a" => "alpha",
        "b" => "beta",
        "m" => "milestone",
        "cr" => "rc",
        "ga" | "final" | "release" => "",
        other => other,
    }
}

fn qualifier_rank(value: &str) -> (u8, String) {
    let rank = match value {
        "alpha" => 1,
        "beta" => 2,
        "milestone" => 3,
        "rc" => 4,
        "snapshot" => 5,
        "" => 6,
        "sp" => 7,
        _ => 8,
    };
    (rank, value.to_string())
}

fn trim_version_items(items: &mut Vec<VersionItem>) {
    while items.last().is_some_and(|item| {
        matches!(item, VersionItem::Number(_, value) if value == "0")
            || matches!(item, VersionItem::Qualifier(6, _))
    }) {
        items.pop();
    }
}

fn compare_number_separator(left: VersionSeparator, right: VersionSeparator) -> Ordering {
    match (left, right) {
        (VersionSeparator::Hyphen, VersionSeparator::Dot) => Ordering::Less,
        (VersionSeparator::Dot, VersionSeparator::Hyphen) => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

fn compare_version_item(left: Option<&VersionItem>, right: Option<&VersionItem>) -> Ordering {
    match (left, right) {
        (
            Some(VersionItem::Number(left_separator, left)),
            Some(VersionItem::Number(right_separator, right)),
        ) => compare_number_separator(*left_separator, *right_separator)
            .then_with(|| left.len().cmp(&right.len()))
            .then_with(|| left.cmp(right)),
        (
            Some(VersionItem::Qualifier(left_rank, left)),
            Some(VersionItem::Qualifier(right_rank, right)),
        ) => left_rank.cmp(right_rank).then_with(|| left.cmp(right)),
        (Some(VersionItem::Number(_, _)), Some(VersionItem::Qualifier(_, _))) => Ordering::Greater,
        (Some(VersionItem::Qualifier(_, _)), Some(VersionItem::Number(_, _))) => Ordering::Less,
        (Some(item), None) => {
            compare_version_item(Some(item), Some(&VersionItem::Qualifier(6, String::new())))
        }
        (None, Some(item)) => {
            compare_version_item(Some(&VersionItem::Qualifier(6, String::new())), Some(item))
        }
        (None, None) => Ordering::Equal,
    }
}

async fn read_small_body(mut body: Body) -> Result<String, ApiError> {
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| invalid_body())?;
        if let Ok(data) = frame.into_data() {
            if bytes.len().saturating_add(data.len()) > MAX_CHECKSUM_BYTES {
                return Err(checksum_mismatch());
            }
            bytes.extend_from_slice(&data);
        }
    }
    String::from_utf8(bytes).map_err(|_| checksum_mismatch())
}

fn checksum_bytes(kind: ChecksumKind, bytes: &[u8]) -> String {
    use sha1::Sha1;
    use sha2::{Digest, Sha256};
    match kind {
        ChecksumKind::Sha1 => format!("{:x}", Sha1::digest(bytes)),
        ChecksumKind::Md5 => format!("{:x}", md5::Md5::digest(bytes)),
        ChecksumKind::Sha256 => format!("{:x}", Sha256::digest(bytes)),
    }
}

fn record_checksum(record: &ArtifactRecord, kind: ChecksumKind) -> &str {
    match kind {
        ChecksumKind::Sha1 => &record.sha1,
        ChecksumKind::Md5 => &record.md5,
        ChecksumKind::Sha256 => &record.sha256,
    }
}

fn insert_header(
    headers: &mut HeaderMap,
    name: impl axum::http::header::IntoHeaderName,
    value: &str,
) -> Result<(), ApiError> {
    let value = HeaderValue::from_str(value).map_err(|_| ApiError::internal("Maven 响应头无效"))?;
    headers.insert(name, value);
    Ok(())
}

fn reject_oversized_request(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(value) = headers.get(CONTENT_LENGTH) else {
        return Ok(());
    };
    let size = value
        .to_str()
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(invalid_content_length)?;
    if size > state.config.max_artifact_bytes {
        Err(artifact_too_large())
    } else {
        Ok(())
    }
}

fn ensure_maven(repository: &Repository) -> Result<(), ApiError> {
    if repository.format == Format::Maven {
        Ok(())
    } else {
        Err(ApiError::not_implemented("该制品格式尚未实现"))
    }
}

fn ensure_hosted(repository: &Repository) -> Result<(), ApiError> {
    if repository.repo_type == RepoType::Hosted {
        Ok(())
    } else {
        Err(method_not_allowed())
    }
}

fn invalid_path() -> ApiError {
    ApiError::bad_request("MAVEN_INVALID_PATH", "Maven 制品路径无效")
}

fn invalid_body() -> ApiError {
    ApiError::bad_request("ARTIFACT_BODY_READ", "读取制品请求体失败")
}

fn invalid_content_length() -> ApiError {
    ApiError::bad_request("ARTIFACT_CONTENT_LENGTH", "Content-Length 无效")
}

fn artifact_not_found() -> ApiError {
    ApiError::not_found("ARTIFACT_NOT_FOUND", "Maven 制品不存在")
}

fn checksum_mismatch() -> ApiError {
    ApiError::bad_request("MAVEN_CHECKSUM_MISMATCH", "Maven 校验和与制品不一致")
}

fn version_policy_error() -> ApiError {
    ApiError::bad_request("MAVEN_VERSION_POLICY", "Maven 版本不符合仓库版本策略")
}

fn method_not_allowed() -> ApiError {
    ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "METHOD_NOT_ALLOWED",
        "该仓库类型不允许写入",
    )
}

fn artifact_too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "ARTIFACT_TOO_LARGE",
        "制品大小超过上限",
    )
}

fn proxy_config_error() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "PROXY_UPSTREAM_CONFIG",
        "代理上游配置无效",
    )
}

fn proxy_not_found() -> ApiError {
    ApiError::not_found("PROXY_UPSTREAM_NOT_FOUND", "代理上游制品不存在")
}

fn proxy_timeout() -> ApiError {
    ApiError::new(
        StatusCode::GATEWAY_TIMEOUT,
        "PROXY_UPSTREAM_TIMEOUT",
        "代理上游响应超时",
    )
}

fn proxy_connection_error() -> ApiError {
    ApiError::new(
        StatusCode::GATEWAY_TIMEOUT,
        "PROXY_UPSTREAM_CONNECTION",
        "无法连接代理上游",
    )
}

fn proxy_redirect_error() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "PROXY_UPSTREAM_REDIRECT",
        "代理上游返回了不允许的重定向",
    )
}

fn proxy_client_error() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "PROXY_UPSTREAM_CLIENT_ERROR",
        "代理上游拒绝了请求",
    )
}

fn proxy_server_error() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "PROXY_UPSTREAM_SERVER_ERROR",
        "代理上游服务异常",
    )
}

fn proxy_transfer_error() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "PROXY_UPSTREAM_TRANSFER",
        "代理上游传输中断",
    )
}

fn proxy_checksum_mismatch() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "MAVEN_UPSTREAM_CHECKSUM",
        "Maven 上游校验和不一致",
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
    fn 标准布局解析release_snapshot_classifier与多段扩展名() {
        let release = parse_artifact_path("org/example/demo/1.0/demo-1.0-sources.jar.asc").unwrap();
        assert_eq!(release.coordinate.group_id, "org.example");
        assert_eq!(release.coordinate.artifact_id, "demo");
        assert_eq!(release.coordinate.version.as_deref(), Some("1.0"));
        assert_eq!(release.classifier.as_deref(), Some("sources"));
        assert_eq!(release.extension, "jar.asc");

        let snapshot = parse_artifact_path(
            "org/example/demo/1.1-SNAPSHOT/demo-1.1-20250101.010203-7-linux.zip",
        )
        .unwrap();
        assert_eq!(snapshot.snapshot.unwrap().build_number, 7);
        assert_eq!(snapshot.classifier.as_deref(), Some("linux"));
    }

    #[test]
    fn 标准布局拒绝错坐标与危险路径() {
        for path in [
            "demo.jar",
            "org/example/demo/1.0/other-1.0.jar",
            "org/example/demo/1.0/demo-2.0.jar",
            "org/example/demo/1.0/demo-1.0",
            "org/example/demo/../demo-1.0.jar",
        ] {
            assert!(parse_artifact_path(path).is_err(), "应拒绝 {path}");
        }
    }

    #[test]
    fn comparable_version覆盖官方常用顺序与别名() {
        let ordered = [
            "1-alpha1",
            "1-beta1",
            "1-milestone1",
            "1-rc1",
            "1-SNAPSHOT",
            "1",
            "1-sp1",
            "2",
        ];
        for pair in ordered.windows(2) {
            assert_eq!(comparable_version(pair[0], pair[1]), Ordering::Less);
        }
        for pair in [
            ("1-1", "1.0.1"),
            ("1.0.1", "1.1"),
            ("1.1", "1.2"),
            ("1.9", "1.10"),
        ] {
            assert_eq!(comparable_version(pair.0, pair.1), Ordering::Less);
        }
        for equal in [
            ("1", "1.0"),
            ("1-ga", "1"),
            ("1-final", "1"),
            ("1-cr1", "1-rc1"),
        ] {
            assert_eq!(comparable_version(equal.0, equal.1), Ordering::Equal);
        }
    }

    #[test]
    fn proxy路径仅接受标准资源及指定sidecar() {
        for path in [
            "org/example/demo/1.0/demo-1.0.jar",
            "org/example/demo/1.0/demo-1.0.jar.sha1",
            "org/example/demo/maven-metadata.xml",
            "org/example/demo/maven-metadata.xml.md5",
            "org/example/demo/1.1-SNAPSHOT/maven-metadata.xml",
            "org/example/demo/1.1-SNAPSHOT/maven-metadata.xml.sha256",
        ] {
            assert!(validate_proxy_path(path).is_ok(), "应接受 {path}");
        }
        for path in [
            "arbitrary/path.txt",
            "org/example/demo/1.0/other-1.0.jar",
            "org/example/demo/1.0/other-1.0.jar.sha1",
            "org/example/demo/1.0/demo-1.0.jar.sha1.md5",
            "org/example/demo/1.0/demo-1.0.jar.sha512",
            "org/maven-metadata.xml",
        ] {
            assert!(validate_proxy_path(path).is_err(), "应拒绝 {path}");
        }
    }

    #[test]
    fn artifact_metadata纯函数排序并生成release() {
        let coordinate = MavenCoordinate {
            group_id: "com.example".to_string(),
            artifact_id: "demo".to_string(),
            version: None,
        };
        let records = [
            record("com/example/demo/2.0/demo-2.0.jar", "2025-01-02 03:04:05"),
            record("com/example/demo/1.0/demo-1.0.jar", "2025-01-01 00:00:00"),
        ];
        let xml = artifact_metadata_xml(&coordinate, &records).unwrap();
        assert!(xml.contains("<latest>2.0</latest>"));
        assert!(xml.contains("<release>2.0</release>"));
        assert!(
            xml.find("<version>1.0</version>").unwrap()
                < xml.find("<version>2.0</version>").unwrap()
        );
        assert!(xml.contains("<lastUpdated>20250102030405</lastUpdated>"));
    }

    fn record(path: &str, updated_at: &str) -> ArtifactRecord {
        ArtifactRecord {
            id: 1,
            repo_id: 1,
            path: path.to_string(),
            sha256: "a".repeat(64),
            sha1: "b".repeat(40),
            md5: "c".repeat(32),
            sha512: "d".repeat(128),
            size: 1,
            cached: false,
            created_at: updated_at.to_string(),
            updated_at: updated_at.to_string(),
        }
    }
}
