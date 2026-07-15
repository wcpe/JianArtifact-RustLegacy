//! 正式构建的内嵌前端静态资源服务。

use axum::Json;
use axum::body::Body;
use axum::http::{Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

use crate::error::ErrorBody;

#[derive(RustEmbed)]
#[folder = "../web/dist/"]
struct WebAssets;

/// 为未命中的请求提供静态资源或 SPA 回退，并保持 API/协议的 JSON 404 语义。
pub(crate) async fn fallback(method: Method, uri: Uri) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return not_found();
    }
    let path = uri.path().trim_start_matches('/');
    if reserved_path(path) {
        return not_found();
    }
    if path.is_empty() {
        return embedded_response("index.html", method == Method::HEAD);
    }
    if let Some(asset) = WebAssets::get(path) {
        return asset_response(path, asset.data.into_owned(), method == Method::HEAD);
    }
    if path
        .rsplit('/')
        .next()
        .is_some_and(|name| name.contains('.'))
    {
        return not_found();
    }
    embedded_response("index.html", method == Method::HEAD)
}

fn reserved_path(path: &str) -> bool {
    ["api", "repository", "healthz", "readyz"]
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
}

fn embedded_response(path: &str, head_only: bool) -> Response {
    match WebAssets::get(path) {
        Some(asset) => asset_response(path, asset.data.into_owned(), head_only),
        None => not_found(),
    }
}

fn asset_response(path: &str, bytes: Vec<u8>, head_only: bool) -> Response {
    let content_type = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();
    let body = if head_only {
        Body::empty()
    } else {
        Body::from(bytes)
    };
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorBody {
            code: "NOT_FOUND".to_string(),
            message: "请求的资源不存在".to_string(),
        }),
    )
        .into_response()
}
