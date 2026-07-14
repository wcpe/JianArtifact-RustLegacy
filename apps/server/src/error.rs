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
    #[error("blob 存储初始化失败：{0}")]
    Storage(crate::storage::StorageError),
    #[error("启动恢复 blob 回收意图失败")]
    BlobReclamationRecovery,
    #[error("认证配置无效：{0}")]
    AuthConfig(String),
}

impl From<crate::storage::StorageError> for StartupError {
    fn from(error: crate::storage::StorageError) -> Self {
        match error {
            crate::storage::StorageError::Io(error) => Self::Io(error),
            other => Self::Storage(other),
        }
    }
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
#[derive(Debug, Clone)]
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

    /// 401：凭据缺失或无效（登录失败统一此码，不区分用户不存在 / 口令错误，防枚举）。
    pub fn unauthorized(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, code, message)
    }

    /// 403：已认证但无权限，或违反最后一名启用管理员保护。
    pub fn forbidden(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, code, message)
    }

    /// 404：资源不存在（对无权私有资源返回一致响应，不泄露存在性）。
    pub fn not_found(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, message)
    }

    /// 409：唯一性冲突（如用户名已存在）。
    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    /// 400：请求参数无效。
    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    /// 500：服务端内部错误，细节只落中文日志、不外泄。
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL", message)
    }

    /// 稳定错误码（供调用方分支判断与测试断言）。
    pub fn code(&self) -> &'static str {
        self.code
    }

    /// HTTP 状态码。
    pub fn status(&self) -> StatusCode {
        self.status
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
