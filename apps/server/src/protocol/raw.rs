//! Raw 协议业务：托管读写、目录浏览与代理缓存。

use std::time::Duration;

use axum::Json;
use axum::body::Body;
use axum::http::header::{
    AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, ETAG, HeaderMap, HeaderValue,
};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use serde::Serialize;
use tokio_util::io::ReaderStream;
use utoipa::ToSchema;

use crate::api::extractor::ArtifactRoute;
use crate::api::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};
use crate::domain::artifact::{ArtifactDirectory, ArtifactPath};
use crate::domain::repository::{
    Format, OverwritePolicy, Permission, RepoType, Repository, Subject,
};
use crate::error::ApiError;
use crate::persistence::{ArtifactDirectoryEntry, ArtifactRecord};
use crate::repository;
use crate::state::AppState;

const PROXY_SEND_TIMEOUT: Duration = Duration::from_secs(30);
const PROXY_CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Raw 目录条目。
#[derive(Debug, Serialize, ToSchema)]
pub struct DirectoryItem {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size: Option<u64>,
    pub updated_at: String,
}

/// Raw 目录分页响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct DirectoryResponse {
    pub items: Vec<DirectoryItem>,
    pub page: u32,
    pub page_size: u32,
    pub total: i64,
}

struct DirectoryQuery {
    page: u32,
    page_size: u32,
}

/// 读取 Raw 路径或目录。
pub async fn get(
    state: &AppState,
    subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
) -> Result<Response, ApiError> {
    let repository =
        repository::protocol_repository(&state.pool, subject, &route.repository, Permission::Read)
            .await?;
    ensure_raw(&repository)?;
    match repository.repo_type {
        RepoType::Hosted => hosted_get(state, &repository, &route, uri).await,
        RepoType::Proxy => proxy_get(state, subject, repository, &route, uri, false).await,
        RepoType::Group => Err(not_implemented()),
    }
}

/// 探测 Raw 文件元数据。
pub async fn head(
    state: &AppState,
    subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
) -> Result<Response, ApiError> {
    let repository =
        repository::protocol_repository(&state.pool, subject, &route.repository, Permission::Read)
            .await?;
    ensure_raw(&repository)?;
    match repository.repo_type {
        RepoType::Hosted => hosted_head(state, &repository, &route, uri).await,
        RepoType::Proxy => proxy_get(state, subject, repository, &route, uri, true).await,
        RepoType::Group => Err(not_implemented()),
    }
}

/// 写入 Raw hosted 文件。
pub async fn put(
    state: &AppState,
    subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let repository =
        repository::protocol_repository(&state.pool, subject, &route.repository, Permission::Read)
            .await?;
    ensure_raw(&repository)?;
    let path = artifact_path(&route, uri)?;
    ensure_hosted_type(&repository, uri)?;
    repository::protocol_repository(&state.pool, subject, &route.repository, Permission::Write)
        .await?;
    reject_oversized_request(state, headers)?;
    write_hosted_body(state, &repository, &path, body).await
}

/// 删除 Raw hosted 文件。
pub async fn delete(
    state: &AppState,
    subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
) -> Result<Response, ApiError> {
    let repository =
        repository::protocol_repository(&state.pool, subject, &route.repository, Permission::Read)
            .await?;
    ensure_raw(&repository)?;
    let path = artifact_path(&route, uri)?;
    ensure_hosted_type(&repository, uri)?;
    repository::protocol_repository(&state.pool, subject, &route.repository, Permission::Write)
        .await?;
    match state.artifacts.delete(repository.id, path.as_str()).await? {
        Some(_) => Ok(StatusCode::NO_CONTENT.into_response()),
        None => Err(artifact_not_found()),
    }
}

async fn hosted_get(
    state: &AppState,
    repository: &Repository,
    route: &ArtifactRoute,
    uri: &Uri,
) -> Result<Response, ApiError> {
    if route.trailing_slash {
        return hosted_directory(state, repository, route, uri).await;
    }
    let path = artifact_path(route, uri)?;
    local_artifact_response(state, repository.id, &path, false).await
}

async fn hosted_head(
    state: &AppState,
    repository: &Repository,
    route: &ArtifactRoute,
    uri: &Uri,
) -> Result<Response, ApiError> {
    if route.trailing_slash {
        return Err(invalid_path());
    }
    let path = artifact_path(route, uri)?;
    local_artifact_response(state, repository.id, &path, true).await
}

async fn hosted_directory(
    state: &AppState,
    repository: &Repository,
    route: &ArtifactRoute,
    uri: &Uri,
) -> Result<Response, ApiError> {
    validate_encoded_path(uri)?;
    let directory = ArtifactDirectory::parse(&route.path).map_err(|_| invalid_path())?;
    let query = parse_directory_query(uri.query())?;
    let (entries, total) = state
        .artifacts
        .list_directory(
            repository.id,
            directory.as_str(),
            query.page,
            query.page_size,
        )
        .await?;
    Ok(Json(DirectoryResponse {
        items: entries.into_iter().map(DirectoryItem::from).collect(),
        page: query.page,
        page_size: query.page_size,
        total,
    })
    .into_response())
}

async fn write_hosted_body(
    state: &AppState,
    repository: &Repository,
    path: &ArtifactPath,
    mut body: Body,
) -> Result<Response, ApiError> {
    let mut writer = state.artifacts.begin_write().await?;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| invalid_body())?;
        if let Ok(data) = frame.into_data() {
            writer.write_chunk(&data).await?;
        }
    }
    let pending = writer.finish().await?;
    let allow_overwrite = repository.overwrite_policy == Some(OverwritePolicy::Allow);
    let committed = state
        .artifacts
        .commit_pending(
            repository.id,
            path.as_str(),
            pending,
            false,
            allow_overwrite,
        )
        .await?;
    let status = if committed.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok(status.into_response())
}

async fn local_artifact_response(
    state: &AppState,
    repo_id: i64,
    path: &ArtifactPath,
    head: bool,
) -> Result<Response, ApiError> {
    let artifact = state
        .artifacts
        .read(repo_id, path.as_str())
        .await?
        .ok_or_else(artifact_not_found)?;
    artifact_response(artifact.record, artifact.blob, path.as_str(), head)
}

fn artifact_response(
    record: ArtifactRecord,
    blob: crate::storage::BlobReadHandle,
    path: &str,
    head: bool,
) -> Result<Response, ApiError> {
    let body = if head {
        Body::empty()
    } else {
        Body::from_stream(ReaderStream::new(blob))
    };
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    apply_artifact_headers(response.headers_mut(), &record, path)?;
    Ok(response)
}

fn apply_artifact_headers(
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
    insert_header(headers, "x-checksum-sha512", &record.sha512)?;
    insert_header(headers, "x-checksum-md5", &record.md5)
}

fn insert_header(
    headers: &mut HeaderMap,
    name: impl axum::http::header::IntoHeaderName,
    value: &str,
) -> Result<(), ApiError> {
    let value = HeaderValue::from_str(value).map_err(|_| ApiError::internal("制品响应头无效"))?;
    headers.insert(name, value);
    Ok(())
}

async fn proxy_get(
    state: &AppState,
    subject: Subject,
    mut repository: Repository,
    route: &ArtifactRoute,
    uri: &Uri,
    head: bool,
) -> Result<Response, ApiError> {
    reject_proxy_query(uri)?;
    let path = artifact_path(route, uri)?;
    let mut retried = false;
    loop {
        if let Some(response) = cached_proxy_response(state, repository.id, &path, head).await? {
            return Ok(response);
        }
        match cache_proxy_miss(state, &repository, &path).await {
            Ok(()) => return local_artifact_response(state, repository.id, &path, head).await,
            Err(error) if is_stale_proxy_configuration(&error) && !retried => {
                repository = reload_proxy_repository(state, subject, &route.repository).await?;
                retried = true;
            }
            Err(error) => return Err(error),
        }
    }
}

async fn cached_proxy_response(
    state: &AppState,
    repo_id: i64,
    path: &ArtifactPath,
    head: bool,
) -> Result<Option<Response>, ApiError> {
    let Some(local) = state.artifacts.read(repo_id, path.as_str()).await? else {
        return Ok(None);
    };
    artifact_response(local.record, local.blob, path.as_str(), head).map(Some)
}

async fn reload_proxy_repository(
    state: &AppState,
    subject: Subject,
    name: &str,
) -> Result<Repository, ApiError> {
    let repository =
        repository::protocol_repository(&state.pool, subject, name, Permission::Read).await?;
    ensure_raw(&repository)?;
    if repository.repo_type != RepoType::Proxy {
        return Err(proxy_config_error());
    }
    Ok(repository)
}

async fn cache_proxy_miss(
    state: &AppState,
    repository: &Repository,
    path: &ArtifactPath,
) -> Result<(), ApiError> {
    let upstream_url = repository
        .upstream_url
        .as_deref()
        .ok_or_else(proxy_config_error)?;
    let task_state = state.clone();
    let task_repository = repository.clone();
    let task_path = path.clone();
    state
        .artifacts
        .proxy_singleflight(
            repository.id,
            path.as_str(),
            upstream_url,
            move || async move {
                if task_state
                    .artifacts
                    .read(task_repository.id, task_path.as_str())
                    .await?
                    .is_some()
                {
                    return Ok(());
                }
                fetch_and_cache(&task_state, &task_repository, &task_path).await
            },
        )
        .await
}

async fn fetch_and_cache(
    state: &AppState,
    repository: &Repository,
    path: &ArtifactPath,
) -> Result<(), ApiError> {
    let expected_upstream_url = repository
        .upstream_url
        .as_deref()
        .ok_or_else(proxy_config_error)?;
    let url = upstream_url(repository, path)?;
    let mut request = state.artifacts.proxy_client()?.get(url);
    if let Some(authorization) = upstream_authorization(repository)? {
        request = request.header(AUTHORIZATION, authorization);
    }
    let mut response = send_upstream(request).await?;
    validate_upstream_status(response.status())?;
    if response
        .content_length()
        .is_some_and(|size| size > state.config.max_artifact_bytes)
    {
        return Err(artifact_too_large());
    }
    let mut writer = state.artifacts.begin_write().await?;
    stream_upstream_body(&mut response, &mut writer).await?;
    let pending = writer.finish().await?;
    state
        .artifacts
        .commit_proxy_pending(repository.id, path.as_str(), pending, expected_upstream_url)
        .await?;
    Ok(())
}

async fn send_upstream(request: reqwest::RequestBuilder) -> Result<reqwest::Response, ApiError> {
    tokio::time::timeout(PROXY_SEND_TIMEOUT, request.send())
        .await
        .map_err(|_| proxy_timeout())?
        .map_err(map_upstream_transport)
}

async fn stream_upstream_body(
    response: &mut reqwest::Response,
    writer: &mut crate::artifact::ArtifactWriter,
) -> Result<(), ApiError> {
    loop {
        let chunk = tokio::time::timeout(PROXY_CHUNK_IDLE_TIMEOUT, response.chunk())
            .await
            .map_err(|_| proxy_timeout())?
            .map_err(|_| proxy_transfer_error())?;
        let Some(chunk) = chunk else {
            return Ok(());
        };
        writer.write_chunk(&chunk).await?;
    }
}

fn upstream_url(repository: &Repository, path: &ArtifactPath) -> Result<reqwest::Url, ApiError> {
    let base = repository
        .upstream_url
        .as_deref()
        .ok_or_else(proxy_config_error)?;
    let mut url = reqwest::Url::parse(base).map_err(|_| proxy_config_error())?;
    url.set_query(None);
    url.set_fragment(None);
    let mut segments = url.path_segments_mut().map_err(|_| proxy_config_error())?;
    segments.pop_if_empty();
    for segment in path.as_str().split('/') {
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
    if value.chars().any(char::is_control) {
        return false;
    }
    value
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

fn ensure_raw(repository: &Repository) -> Result<(), ApiError> {
    if repository.format == Format::Raw {
        Ok(())
    } else {
        Err(not_implemented())
    }
}

fn ensure_hosted_type(repository: &Repository, uri: &Uri) -> Result<(), ApiError> {
    match repository.repo_type {
        RepoType::Hosted => Ok(()),
        RepoType::Proxy => {
            reject_proxy_query(uri)?;
            Err(method_not_allowed())
        }
        RepoType::Group => Err(method_not_allowed()),
    }
}

fn artifact_path(route: &ArtifactRoute, uri: &Uri) -> Result<ArtifactPath, ApiError> {
    validate_encoded_path(uri)?;
    if route.trailing_slash {
        return Err(invalid_path());
    }
    ArtifactPath::parse(&route.path).map_err(|_| invalid_path())
}

fn validate_encoded_path(uri: &Uri) -> Result<(), ApiError> {
    let raw_path = uri
        .path()
        .strip_prefix("/repository/")
        .and_then(|value| value.split_once('/').map(|(_, path)| path))
        .ok_or_else(invalid_path)?;
    ArtifactPath::validate_uri_encoding(raw_path).map_err(|_| invalid_path())
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

fn reject_proxy_query(uri: &Uri) -> Result<(), ApiError> {
    if uri.query().is_some() {
        Err(ApiError::bad_request(
            "PROXY_QUERY_UNSUPPORTED",
            "代理仓库不接受查询参数",
        ))
    } else {
        Ok(())
    }
}

fn parse_directory_query(query: Option<&str>) -> Result<DirectoryQuery, ApiError> {
    let mut page = None;
    let mut page_size = None;
    for pair in query
        .unwrap_or_default()
        .split('&')
        .filter(|pair| !pair.is_empty())
    {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        match name {
            "page" => page = Some(parse_positive(value)?),
            "page_size" => page_size = Some(parse_positive(value)?),
            _ => {}
        }
    }
    Ok(DirectoryQuery {
        page: page.unwrap_or(1),
        page_size: page_size.unwrap_or(DEFAULT_PAGE_SIZE).min(MAX_PAGE_SIZE),
    })
}

fn parse_positive(value: &str) -> Result<u32, ApiError> {
    value
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| ApiError::bad_request("ARTIFACT_INVALID_QUERY", "目录分页参数无效"))
}

impl From<ArtifactDirectoryEntry> for DirectoryItem {
    fn from(entry: ArtifactDirectoryEntry) -> Self {
        Self {
            name: entry.name,
            path: entry.path,
            is_directory: entry.is_directory,
            size: entry.size,
            updated_at: entry.updated_at,
        }
    }
}

fn invalid_path() -> ApiError {
    ApiError::bad_request("ARTIFACT_INVALID_PATH", "制品路径无效")
}

fn invalid_body() -> ApiError {
    ApiError::bad_request("ARTIFACT_BODY_READ", "读取制品请求体失败")
}

fn invalid_content_length() -> ApiError {
    ApiError::bad_request("ARTIFACT_CONTENT_LENGTH", "Content-Length 无效")
}

fn artifact_not_found() -> ApiError {
    ApiError::not_found("ARTIFACT_NOT_FOUND", "制品不存在")
}

fn artifact_too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "ARTIFACT_TOO_LARGE",
        "制品大小超过上限",
    )
}

fn method_not_allowed() -> ApiError {
    ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "METHOD_NOT_ALLOWED",
        "该仓库类型不允许写入",
    )
}

fn not_implemented() -> ApiError {
    ApiError::not_implemented("该制品格式或仓库类型尚未实现")
}

fn is_stale_proxy_configuration(error: &ApiError) -> bool {
    error.code() == "PROXY_CONFIGURATION_CHANGED"
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

fn proxy_bad_gateway() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "PROXY_UPSTREAM_FAILURE",
        "代理上游响应失败",
    )
}

#[cfg(test)]
mod tests {
    use super::valid_authorization_value;

    #[test]
    fn 上游授权值仅接受无控制字符的完整basic或bearer() {
        assert!(valid_authorization_value("Basic dXNlcjpwYXNz"));
        assert!(valid_authorization_value("Bearer opaque-token"));
        for value in ["Basic ", "Digest token", "Bearer token\r\nInjected: x"] {
            assert!(!valid_authorization_value(value));
        }
    }
}
