//! 本地管理员 CLI：严格解析参数、从安全输入读取口令并直接更新既有 SQLite。

use std::ffi::OsString;
use std::io::Read;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::auth::password::{hash_password_async, password_is_long_enough};
use crate::persistence;

/// 顶层命令分派结果。
#[derive(Debug, PartialEq, Eq)]
pub enum CliCommand {
    /// 不带参数时启动服务。
    Serve,
    /// 输出 OpenAPI JSON。
    OpenApi,
    /// 重置指定本地用户的口令。
    ResetPassword {
        /// 精确用户名。
        username: String,
        /// 是否从标准输入读取单行口令。
        password_stdin: bool,
    },
}

/// CLI 参数、输入或本地数据库操作错误。
#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    #[error("命令参数无效：{0}")]
    InvalidArguments(&'static str),
    #[error("命令参数必须是有效 UTF-8")]
    InvalidArgumentEncoding,
    #[error("命令参数无效：拒绝通过命令行参数提供口令，请使用交互输入或 --password-stdin")]
    PasswordArgumentRejected,
    #[error("无法读取新口令")]
    PasswordInput,
    #[error("无法使用交互终端，请改用 --password-stdin")]
    InteractiveUnavailable,
    #[error("标准输入必须只包含一个非空 UTF-8 行")]
    InvalidStdin,
    #[error("两次输入的新口令不一致")]
    PasswordMismatch,
    #[error("新口令至少需要 12 个字符")]
    PasswordTooShort,
    #[error("无法打开既有 SQLite 数据库")]
    DatabaseOpen,
    #[error("新口令哈希失败")]
    PasswordHash,
    #[error("重置口令失败")]
    DatabaseUpdate,
    #[error("指定用户不存在")]
    UserNotFound,
}

/// 严格解析顶层命令；未知、重复和额外参数全部拒绝。
pub fn parse_args<I>(args: I) -> Result<CliCommand, AdminError>
where
    I: IntoIterator<Item = OsString>,
{
    let values = decode_args(args)?;
    match values.as_slice() {
        [] => Ok(CliCommand::Serve),
        [command] if command == "openapi" => Ok(CliCommand::OpenApi),
        [first, second, rest @ ..] if first == "admin" && second == "reset-password" => {
            parse_reset_password(rest)
        }
        _ => Err(AdminError::InvalidArguments(
            "支持 openapi 或 admin reset-password <用户名> [--password-stdin]",
        )),
    }
}

fn decode_args<I>(args: I) -> Result<Vec<String>, AdminError>
where
    I: IntoIterator<Item = OsString>,
{
    args.into_iter()
        .map(|value| {
            value
                .into_string()
                .map_err(|_| AdminError::InvalidArgumentEncoding)
        })
        .collect()
}

fn parse_reset_password(args: &[String]) -> Result<CliCommand, AdminError> {
    let mut username = None;
    let mut password_stdin = false;
    for value in args {
        if value == "--password" || value.starts_with("--password=") {
            return Err(AdminError::PasswordArgumentRejected);
        }
        if value == "--password-stdin" && !password_stdin {
            password_stdin = true;
        } else if value.starts_with('-') {
            return Err(AdminError::InvalidArguments("存在未知或重复选项"));
        } else if username.is_none() {
            username = Some(value.clone());
        } else {
            return Err(AdminError::InvalidArguments("存在额外参数"));
        }
    }
    let username = username
        .filter(|value| !value.is_empty())
        .ok_or(AdminError::InvalidArguments("缺少 reset-password 用户名"))?;
    Ok(CliCommand::ResetPassword {
        username,
        password_stdin,
    })
}

/// 打开既有数据库、读取新口令并按用户名执行单条更新。
pub async fn reset_password(
    database_path: &Path,
    username: &str,
    password_stdin: bool,
) -> Result<(), AdminError> {
    let password = read_password_async(password_stdin).await?;
    if !password_is_long_enough(&password) {
        return Err(AdminError::PasswordTooShort);
    }
    let password_hash = hash_password_async(&password)
        .await
        .map_err(|_| AdminError::PasswordHash)?;
    let pool = open_existing_database(database_path).await?;
    let updated = persistence::reset_password_by_username(&pool, username, &password_hash)
        .await
        .map_err(|_| AdminError::DatabaseUpdate)?;
    pool.close().await;
    if updated {
        Ok(())
    } else {
        Err(AdminError::UserNotFound)
    }
}

async fn open_existing_database(database_path: &Path) -> Result<sqlx::SqlitePool, AdminError> {
    let options = SqliteConnectOptions::new()
        .filename(database_path)
        .create_if_missing(false)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|_| AdminError::DatabaseOpen)
}

async fn read_password_async(password_stdin: bool) -> Result<String, AdminError> {
    tokio::task::spawn_blocking(move || read_password(password_stdin))
        .await
        .map_err(|_| AdminError::PasswordInput)?
}

fn read_password(password_stdin: bool) -> Result<String, AdminError> {
    if password_stdin {
        return read_password_from_stdin();
    }
    #[cfg(unix)]
    {
        read_password_from_tty()?.ok_or(AdminError::InteractiveUnavailable)
    }
    #[cfg(not(unix))]
    {
        Err(AdminError::InteractiveUnavailable)
    }
}

fn read_password_from_stdin() -> Result<String, AdminError> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .read_to_end(&mut bytes)
        .map_err(|_| AdminError::PasswordInput)?;
    parse_single_line(bytes)
}

fn parse_single_line(bytes: Vec<u8>) -> Result<String, AdminError> {
    let text = String::from_utf8(bytes).map_err(|_| AdminError::InvalidStdin)?;
    let line = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(&text);
    if line.is_empty() || line.contains(['\r', '\n']) {
        return Err(AdminError::InvalidStdin);
    }
    Ok(line.to_string())
}

#[cfg(unix)]
fn read_password_from_tty() -> Result<Option<String>, AdminError> {
    let reader = match std::fs::OpenOptions::new().read(true).open("/dev/tty") {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let mut writer = match std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let mut echo = match EchoGuard::disable() {
        Ok(guard) => guard,
        Err(_) => return Ok(None),
    };
    let result = read_password_pair(BufReader::new(reader), &mut writer);
    echo.restore().map_err(|_| AdminError::PasswordInput)?;
    result.map(Some)
}

#[cfg(unix)]
fn read_password_pair(
    mut reader: BufReader<std::fs::File>,
    writer: &mut std::fs::File,
) -> Result<String, AdminError> {
    let first = read_tty_line(&mut reader, writer, "请输入新口令：")?;
    let second = read_tty_line(&mut reader, writer, "请再次输入新口令：")?;
    if first == second {
        Ok(first)
    } else {
        Err(AdminError::PasswordMismatch)
    }
}

#[cfg(unix)]
fn read_tty_line(
    reader: &mut BufReader<std::fs::File>,
    writer: &mut std::fs::File,
    prompt: &str,
) -> Result<String, AdminError> {
    writer
        .write_all(prompt.as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|_| AdminError::PasswordInput)?;
    let mut bytes = Vec::new();
    reader
        .read_until(b'\n', &mut bytes)
        .map_err(|_| AdminError::PasswordInput)?;
    writer
        .write_all(b"\n")
        .and_then(|()| writer.flush())
        .map_err(|_| AdminError::PasswordInput)?;
    parse_single_line(bytes)
}

#[cfg(unix)]
struct EchoGuard {
    active: bool,
}

#[cfg(unix)]
impl EchoGuard {
    fn disable() -> Result<Self, std::io::Error> {
        if let Err(error) = set_tty_echo(false) {
            let _ = set_tty_echo(true);
            return Err(error);
        }
        Ok(Self { active: true })
    }

    fn restore(&mut self) -> Result<(), std::io::Error> {
        if !self.active {
            return Ok(());
        }
        set_tty_echo(true)?;
        self.active = false;
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for EchoGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = set_tty_echo(true);
        }
    }
}

#[cfg(unix)]
fn set_tty_echo(enabled: bool) -> Result<(), std::io::Error> {
    let mode = if enabled { "echo" } else { "-echo" };
    let status = std::process::Command::new("stty")
        .args(["-F", "/dev/tty", mode])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("stty 执行失败"))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn parse(values: &[&str]) -> Result<CliCommand, AdminError> {
        parse_args(values.iter().map(OsString::from))
    }

    #[test]
    fn parser接受两种password_stdin位置() {
        let expected = CliCommand::ResetPassword {
            username: "alice".to_string(),
            password_stdin: true,
        };
        assert_eq!(
            parse(&["admin", "reset-password", "alice", "--password-stdin"]).unwrap(),
            expected
        );
        assert_eq!(
            parse(&["admin", "reset-password", "--password-stdin", "alice"]).unwrap(),
            expected
        );
    }

    #[test]
    fn parser拒绝明文口令未知重复与额外参数() {
        for values in [
            vec!["admin", "reset-password", "alice", "--password", "secret"],
            vec!["admin", "reset-password", "alice", "--unknown"],
            vec!["admin", "reset-password", "alice", "extra"],
            vec![
                "admin",
                "reset-password",
                "alice",
                "--password-stdin",
                "--password-stdin",
            ],
        ] {
            assert!(parse(&values).is_err());
        }
    }

    #[test]
    fn stdin仅去除一个lf或crlf并拒绝空行多行与非法utf8() {
        assert_eq!(parse_single_line(b"secret\n".to_vec()).unwrap(), "secret");
        assert_eq!(parse_single_line(b"secret\r\n".to_vec()).unwrap(), "secret");
        assert!(parse_single_line(b"\n".to_vec()).is_err());
        assert!(parse_single_line(b"first\nsecond\n".to_vec()).is_err());
        assert!(parse_single_line(vec![0xff, b'\n']).is_err());
    }
}
