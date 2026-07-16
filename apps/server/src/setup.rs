//! 首启初始化令牌生命周期与安全比较原语。

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::io::AsyncWriteExt;

use crate::error::StartupError;
use crate::persistence;

/// 首启初始化令牌文件名，固定放在 SQLite 数据库父目录。
pub(crate) const INITIAL_ADMIN_TOKEN_FILE: &str = "initial-admin.token";

/// 计算首启初始化令牌文件路径。
pub(crate) fn token_path(database_path: &Path) -> PathBuf {
    database_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(INITIAL_ADMIN_TOKEN_FILE)
}

/// 启动时优先执行环境变量管理员自举；仍为空库时创建或校验本地令牌。
pub(crate) async fn initialize(
    pool: &SqlitePool,
    database_path: &Path,
    username: Option<String>,
    password: Option<String>,
) -> Result<(), StartupError> {
    crate::domain::bootstrap_admin_with(pool, username, password).await?;
    let users = persistence::count_users(pool)
        .await
        .map_err(|_| auth_config("首启初始化读取用户数失败"))?;
    let path = token_path(database_path);
    if users > 0 {
        cleanup_token_file(&path).await;
        return Ok(());
    }
    ensure_token_file(&path).await
}

fn auth_config(message: &str) -> StartupError {
    StartupError::AuthConfig(message.to_string())
}

/// 创建令牌文件；已存在时校验格式并修正 Unix 权限。
async fn ensure_token_file(path: &Path) -> Result<(), StartupError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(_) => validate_existing_token(path).await,
        Err(error) if error.kind() == ErrorKind::NotFound => create_token_file(path).await,
        Err(_) => Err(auth_config("首启初始化令牌文件读取失败")),
    }
}

/// 先完整写入同目录临时文件，再以硬链接原子发布，正式路径不会出现半写内容。
async fn create_token_file(path: &Path) -> Result<(), StartupError> {
    let temp_path = temporary_token_path(path);
    let token = crate::auth::random_hex(32);
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temp_path)
        .await
        .map_err(|_| auth_config("首启初始化令牌临时文件创建失败"))?;
    if write_new_token(&mut file, token.as_bytes()).await.is_err() {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(auth_config("首启初始化令牌文件写入失败"));
    }
    drop(file);
    if let Err(error) = enforce_token_permissions(&temp_path).await {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(error);
    }
    publish_token_file(path, &temp_path).await
}

fn temporary_token_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        ".{INITIAL_ADMIN_TOKEN_FILE}.{}.tmp",
        crate::auth::random_hex(8)
    ))
}

async fn publish_token_file(path: &Path, temp_path: &Path) -> Result<(), StartupError> {
    match tokio::fs::hard_link(temp_path, path).await {
        Ok(()) => {
            let _ = tokio::fs::remove_file(temp_path).await;
            sync_parent_directory(path).await?;
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            let _ = tokio::fs::remove_file(temp_path).await;
            validate_existing_token(path).await
        }
        Err(_) => {
            let _ = tokio::fs::remove_file(temp_path).await;
            Err(auth_config("首启初始化令牌文件发布失败"))
        }
    }
}

async fn write_new_token(file: &mut tokio::fs::File, token: &[u8]) -> Result<(), std::io::Error> {
    file.write_all(token).await?;
    file.sync_data().await
}

#[cfg(unix)]
async fn sync_parent_directory(path: &Path) -> Result<(), StartupError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::File::open(parent)?.sync_all())
        .await
        .map_err(|_| auth_config("首启初始化令牌目录同步失败"))?
        .map_err(|_| auth_config("首启初始化令牌目录同步失败"))
}

#[cfg(not(unix))]
async fn sync_parent_directory(_path: &Path) -> Result<(), StartupError> {
    Ok(())
}

async fn validate_existing_token(path: &Path) -> Result<(), StartupError> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|_| auth_config("首启初始化令牌文件读取失败"))?;
    if !metadata.file_type().is_file() {
        return Err(auth_config("首启初始化令牌文件无效"));
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| auth_config("首启初始化令牌文件读取失败"))?;
    if !token_bytes_are_valid(&bytes) {
        return Err(auth_config("首启初始化令牌文件无效"));
    }
    enforce_token_permissions(path).await
}

#[cfg(unix)]
async fn enforce_token_permissions(path: &Path) -> Result<(), StartupError> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = std::fs::Permissions::from_mode(0o600);
    tokio::fs::set_permissions(path, permissions)
        .await
        .map_err(|_| auth_config("首启初始化令牌文件权限修正失败"))?;
    if token_permissions_are_secure(path).await {
        Ok(())
    } else {
        Err(auth_config("首启初始化令牌文件权限修正失败"))
    }
}

#[cfg(not(unix))]
async fn enforce_token_permissions(_path: &Path) -> Result<(), StartupError> {
    Ok(())
}

#[cfg(unix)]
async fn token_permissions_are_secure(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.permissions().mode() & 0o777 == 0o600)
}

#[cfg(not(unix))]
async fn token_permissions_are_secure(_path: &Path) -> bool {
    true
}

/// 删除已失效令牌；不存在视为已清理，其他失败只告警。
pub(crate) async fn cleanup_token_file(path: &Path) {
    match tokio::fs::remove_file(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(_) => tracing::warn!("首启初始化令牌文件删除失败；初始化入口仍保持关闭"),
    }
}

/// 读取格式有效的令牌文件；缺失、非普通文件或格式错误均返回 None。
pub(crate) async fn read_valid_token_file(path: &Path) -> Option<Vec<u8>> {
    let metadata = tokio::fs::symlink_metadata(path).await.ok()?;
    if !metadata.file_type().is_file() || !token_permissions_are_secure(path).await {
        return None;
    }
    let bytes = tokio::fs::read(path).await.ok()?;
    token_bytes_are_valid(&bytes).then_some(bytes)
}

/// 判定令牌是否恰为 64 位小写十六进制。
fn token_bytes_are_valid(token: &[u8]) -> bool {
    token.len() == 64
        && token
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

/// 对两侧先做 SHA-256，再以手写常量时间字节比较判定是否一致。
pub(crate) fn token_matches(expected: &[u8], provided: &str) -> bool {
    let expected_hash = Sha256::digest(expected);
    let provided_hash = Sha256::digest(provided.as_bytes());
    let mut difference = 0_u8;
    for index in 0..expected_hash.len() {
        difference |= expected_hash[index] ^ provided_hash[index];
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 令牌格式只接受六十四位小写十六进制() {
        assert!(token_bytes_are_valid("a0".repeat(32).as_bytes()));
        assert!(!token_bytes_are_valid("A0".repeat(32).as_bytes()));
        assert!(!token_bytes_are_valid("a".repeat(63).as_bytes()));
        assert!(!token_bytes_are_valid("g".repeat(64).as_bytes()));
    }

    #[test]
    fn 令牌比较正确拒绝不同值() {
        assert!(token_matches(b"same-token", "same-token"));
        assert!(!token_matches(b"same-token", "other-token"));
    }
}
