//! 口令哈希与校验：Argon2id，参数钉死（OWASP 推荐档），盐取自 OsRng。
//! 计算开销高，调用方须保证在任何数据库事务与锁之外执行。

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};

use crate::auth::{ARGON2_ITERATIONS, ARGON2_MEMORY_KIB, ARGON2_PARALLELISM};
use crate::error::ApiError;

/// setup 与本地重置口令要求的最少 Unicode 字符数。
pub const MIN_PASSWORD_CHARS: usize = 12;

/// 按 Unicode 标量值计数判定口令是否达到最小长度。
pub fn password_is_long_enough(password: &str) -> bool {
    password.chars().count() >= MIN_PASSWORD_CHARS
}

/// 以钉死参数构造 Argon2id 哈希器。
fn hasher() -> Result<Argon2<'static>, ApiError> {
    let params = Params::new(
        ARGON2_MEMORY_KIB,
        ARGON2_ITERATIONS,
        ARGON2_PARALLELISM,
        None,
    )
    .map_err(|_| ApiError::internal("口令哈希参数无效"))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// 生成明文口令的 Argon2id PHC 串（含随机盐与钉死参数）。
pub fn hash_password(plain: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut OsRng);
    let phc = hasher()?
        .hash_password(plain.as_bytes(), &salt)
        .map_err(|_| ApiError::internal("口令哈希失败"))?
        .to_string();
    Ok(phc)
}

/// 校验明文口令与 PHC 串是否匹配；PHC 解析或校验失败一律返回 false，绝不 panic。
/// 校验参数来自 PHC 串本身，因此使用默认哈希器即可。
pub fn verify_password(plain: &str, phc: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return false;
    };
    Argon2::default()
        .verify_password(plain.as_bytes(), &parsed)
        .is_ok()
}

/// 用于不存在用户路径的固定 Argon2 校验输入，参数与生产口令哈希一致。
pub const DUMMY_PASSWORD_HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// 将高开销口令计算移交 Tokio 阻塞线程池。
async fn run_blocking<T, F>(operation: F, error_message: &'static str) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| ApiError::internal(error_message))
}

/// 在阻塞线程池中生成明文口令的 Argon2id PHC 串。
pub async fn hash_password_async(plain: &str) -> Result<String, ApiError> {
    let plain = plain.to_owned();
    run_blocking(move || hash_password(&plain), "口令哈希任务失败").await?
}

/// 在阻塞线程池中校验明文口令与 PHC 串。
pub async fn verify_password_async(plain: &str, phc: &str) -> Result<bool, ApiError> {
    let plain = plain.to_owned();
    let phc = phc.to_owned();
    run_blocking(move || verify_password(&plain, &phc), "口令校验任务失败").await
}

/// 校验真实哈希；用户不存在时改用同参数固定哈希并始终返回失败。
pub async fn verify_password_or_dummy_async(
    plain: &str,
    phc: Option<&str>,
) -> Result<bool, ApiError> {
    let hash = match phc {
        Some(value) => value,
        None => DUMMY_PASSWORD_HASH,
    };
    let valid = verify_password_async(plain, hash).await?;
    Ok(phc.is_some() && valid)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn 最小长度按unicode字符计数() {
        assert!(!password_is_long_enough(&"密".repeat(11)));
        assert!(password_is_long_enough(&"密".repeat(12)));
    }

    #[test]
    fn 哈希后可用原口令校验通过() {
        let phc = hash_password("correct horse battery").unwrap();
        assert!(verify_password("correct horse battery", &phc));
    }

    #[test]
    fn 错误口令校验失败() {
        let phc = hash_password("correct horse battery").unwrap();
        assert!(!verify_password("wrong password", &phc));
    }

    #[test]
    fn 非法phc串校验返回false而非panic() {
        assert!(!verify_password("任意口令", "这不是一个合法的-phc-串"));
        assert!(!verify_password("任意口令", ""));
    }

    #[test]
    fn phc串内嵌钉死的argon2参数() {
        let phc = hash_password("pw").unwrap();
        assert!(phc.starts_with("$argon2id$"), "应为 argon2id PHC 串：{phc}");
        assert!(phc.contains("m=19456"), "内存参数应为 19456：{phc}");
        assert!(phc.contains("t=2"), "迭代参数应为 2：{phc}");
        assert!(phc.contains("p=1"), "并行参数应为 1：{phc}");
    }

    #[test]
    fn 相同口令两次哈希因随机盐而不同() {
        let a = hash_password("same").unwrap();
        let b = hash_password("same").unwrap();
        assert_ne!(a, b, "随机盐应使两次 PHC 串不同");
    }
}
