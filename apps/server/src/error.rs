//! 统一错误类型与对外错误响应结构（API.md §3）。
//! 对外响应含稳定错误码与中文消息，不泄露内部路径、凭据或存在性；request_id 经响应头传播。

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

/// 服务启动期错误（配置、数据库、迁移等），用于 `main` 的错误传播。
#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("配置加载失败：{0}")]
    Config(#[from] Box<figment::Error>),
    #[error("监听地址无效：{0}")]
    BindAddr(String),
    #[error("数据库连接失败：{0}")]
    Database(#[from] sqlx::Error),
    #[error("数据库迁移失败：{0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),
}

/// 对外统一错误响应体。
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorBody {
    /// 稳定错误码（大写下划线，供客户端分支判断）。
    pub code: String,
    /// 面向用户的中文错误消息，不含内部细节。
    pub message: String,
}

/// 可转换为 HTTP 响应的 API 错误。
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    /// 构造一个 API 错误。
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    /// 契约骨架占位：功能待阶段 3 实现。
    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_IMPLEMENTED, "NOT_IMPLEMENTED", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            code: self.code.to_string(),
            message: self.message,
        };
        (self.status, Json(body)).into_response()
    }
}
