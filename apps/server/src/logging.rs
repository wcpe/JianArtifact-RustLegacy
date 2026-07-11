//! 中文分级日志初始化（tracing）。生产代码一律经此输出，禁止临时打印。

use tracing_subscriber::EnvFilter;

/// 按配置的级别初始化全局日志订阅者。重复初始化会被忽略。
pub fn init(level: &str) {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));
    // try_init 在已初始化时返回错误，此处忽略以支持测试重复调用
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
