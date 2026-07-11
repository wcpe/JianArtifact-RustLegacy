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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:8080".to_string(),
            database_path: PathBuf::from("data/jianartifact.db"),
            data_dir: PathBuf::from("data/blobs"),
            log_level: "info".to_string(),
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
