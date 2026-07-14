//! 服务端自身配置：默认值内置于代码，可由 config.toml 与 `JIANARTIFACT_` 前缀环境变量覆盖。
//! 只包含当前真正使用的字段，不为后续分期预留占位（架构不变量）。

use std::path::PathBuf;

use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use serde::{Deserialize, Serialize};

/// 服务端运行配置。TOML 字段名一律 kebab-case（config-files 规范）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    /// 服务监听地址（形如 `0.0.0.0:8080`）。
    pub bind: String,
    /// SQLite 元数据库文件路径。
    pub database_path: PathBuf,
    /// blob 数据目录，就绪自检会在此写探针文件。
    pub data_dir: PathBuf,
    /// 日志级别过滤（tracing EnvFilter 语法，如 `info`）。
    pub log_level: String,
    /// JWT HS256 会话令牌签名密钥（机密，仅经环境变量注入，缺失即拒绝启动，不落库不进日志）。
    pub jwt_secret: String,
    /// 会话令牌有效期（小时），默认 8。
    pub session_ttl_hours: u64,
    /// 单个制品允许写入的最大实际字节数，默认 5 GiB。
    pub max_artifact_bytes: u64,
    /// blob 写入前要求的最小可用空间字节数，默认 5 GiB。
    pub min_free_bytes: u64,
    /// blob 写入前要求的最小可用空间百分比，默认 5。
    pub min_free_percent: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:8080".to_string(),
            database_path: PathBuf::from("data/jianartifact.db"),
            data_dir: PathBuf::from("data/blobs"),
            log_level: "info".to_string(),
            // 密钥无内置默认值，须经环境变量注入；空值在 build_app 处被拒绝启动
            jwt_secret: String::new(),
            session_ttl_hours: 8,
            max_artifact_bytes: 5 * 1024 * 1024 * 1024,
            min_free_bytes: 5 * 1024 * 1024 * 1024,
            min_free_percent: 5,
        }
    }
}

impl Config {
    /// 按「代码默认值 → config.toml（若存在）→ 环境变量」的优先级加载配置。
    /// 错误装箱以避免 Result 错误分支过大（clippy::result_large_err）。
    pub fn load() -> Result<Self, Box<figment::Error>> {
        Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file("config.toml"))
            // 环境变量下划线映射为 kebab 键：JIANARTIFACT_DATABASE_PATH → database-path
            .merge(Env::prefixed("JIANARTIFACT_").map(|key| key.as_str().replace('_', "-").into()))
            .extract()
            .map_err(Box::new)
    }

    /// 组装 sqlx 使用的 SQLite 连接串（缺库文件时自动创建）。
    pub fn database_url(&self) -> String {
        format!("sqlite://{}?mode=rwc", self.database_path.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 制品与磁盘阈值默认值符合nfr() -> Result<(), serde_json::Error> {
        let config = Config::default();
        assert_eq!(config.max_artifact_bytes, 5 * 1024 * 1024 * 1024);
        assert_eq!(config.min_free_bytes, 5 * 1024 * 1024 * 1024);
        assert_eq!(config.min_free_percent, 5);
        let value = serde_json::to_value(config)?;
        assert!(value.get("max-artifact-bytes").is_some());
        assert!(value.get("min-free-bytes").is_some());
        assert!(value.get("min-free-percent").is_some());
        Ok(())
    }
}
