//! 制品格式协议中央分发与格式领域服务。

pub mod maven;
pub mod npm;
pub mod raw;

use axum::body::Body;
use axum::http::{HeaderMap, Uri};
use axum::response::Response;

use crate::api::extractor::ArtifactRoute;
use crate::domain::repository::{Format, Subject};
use crate::error::ApiError;
use crate::repository;
use crate::state::AppState;

/// 中央分发 GET 请求。
pub async fn get(
    state: &AppState,
    subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let context =
        repository::protocol_repository_context(&state.pool, subject, &route.repository).await?;
    match context.repository.format {
        Format::Raw => raw::get(state, subject, route, uri, context).await,
        Format::Npm => npm::get(state, subject, route, uri, headers, context).await,
        Format::Maven => maven::get(state, subject, route, uri, context).await,
    }
}

/// 中央分发 HEAD 请求。
pub async fn head(
    state: &AppState,
    subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let context =
        repository::protocol_repository_context(&state.pool, subject, &route.repository).await?;
    match context.repository.format {
        Format::Raw => raw::head(state, subject, route, uri, context).await,
        Format::Npm => npm::head(state, subject, route, uri, headers, context).await,
        Format::Maven => maven::head(state, subject, route, uri, context).await,
    }
}

/// 中央分发 PUT 请求。
pub async fn put(
    state: &AppState,
    subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let context =
        repository::protocol_repository_context(&state.pool, subject, &route.repository).await?;
    match context.repository.format {
        Format::Raw => raw::put(state, subject, route, uri, headers, body, context).await,
        Format::Npm => npm::put(state, subject, route, uri, headers, body, context).await,
        Format::Maven => maven::put(state, subject, route, uri, headers, body, context).await,
    }
}

/// 中央分发 DELETE 请求。
pub async fn delete(
    state: &AppState,
    subject: Subject,
    route: ArtifactRoute,
    uri: &Uri,
) -> Result<Response, ApiError> {
    let context =
        repository::protocol_repository_context(&state.pool, subject, &route.repository).await?;
    match context.repository.format {
        Format::Raw => raw::delete(state, subject, route, uri, context).await,
        Format::Npm => npm::delete(state, subject, route, uri, context).await,
        Format::Maven => maven::delete(state, subject, route, uri, context).await,
    }
}
