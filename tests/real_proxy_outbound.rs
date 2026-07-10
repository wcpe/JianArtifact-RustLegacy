//! 在线更新经真实代理的实机复现测试（fix：更新经代理下载资产失败）。
//!
//! 默认 `#[ignore]`：依赖外网 GitHub 与本机真实代理，不进常规验证门；实机验收时显式跑：
//!
//! ```text
//! JIANARTIFACT_TEST_PROXY=socks5://127.0.0.1:7890 cargo test --test real_proxy_outbound -- --ignored --nocapture
//! ```
//!
//! 断言链路与生产一致：`NetworkState`（热替换槽）→ `GithubReleaseSource` → 版本检查 + 资产流式下载。
//! 「版本检查成功而资产下载失败」即复现目标缺陷。

use std::sync::Arc;
use std::time::Duration;

use jianartifact::config::{NetworkProxyConfig, NetworkState, UpdateChannel};
use jianartifact::meta::MetaStore;
use jianartifact::proxy::{HttpUpstream, Upstream};
use jianartifact::update::{GithubReleaseSource, ReleaseSource};
use jianartifact::vuln::{HttpMirrorSource, VulnMirror};
use tokio::io::AsyncReadExt;

/// 从环境变量取测试代理 URL；未设置则跳过（测试本身已 `#[ignore]`，此处再兜底）。
fn 测试代理() -> Option<String> {
    std::env::var("JIANARTIFACT_TEST_PROXY")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

fn 真实代理配置() -> Option<NetworkProxyConfig> {
    测试代理().map(|proxy_url| NetworkProxyConfig {
        url: Some(proxy_url),
        http: None,
        https: None,
        all: None,
        no_proxy: None,
    })
}

#[tokio::test]
#[ignore = "依赖外网 GitHub 与本机真实代理，实机验收时显式跑"]
async fn 经真实代理_版本检查与资产下载均成功() {
    let Some(proxy) = 真实代理配置() else {
        eprintln!("未设置 JIANARTIFACT_TEST_PROXY，跳过");
        return;
    };
    let network = Arc::new(
        NetworkState::new(proxy, Duration::from_secs(30)).expect("代理配置应可构造 client"),
    );
    let source = GithubReleaseSource::with_network_state(
        network,
        "https://api.github.com".to_string(),
        "wcpe/JianArtifact".to_string(),
        None,
        Duration::from_secs(300),
    );

    // 第一步：版本检查（用户报告此步成功）
    let release = source
        .fetch_latest_release(UpdateChannel::Stable)
        .await
        .expect("版本检查应经代理成功");
    eprintln!(
        "版本检查成功：{}，资产 {} 个",
        release.tag_name,
        release.assets.len()
    );

    // 第二步：小资产下载（.sha256 sidecar，验证 302 重定向到 objects.githubusercontent.com 链路）
    let sha_asset = release
        .assets
        .iter()
        .find(|a| a.name.ends_with(".zip.sha256"))
        .expect("release 应含 .zip.sha256 资产");
    let mut reader = source
        .download_asset(&sha_asset.download_url)
        .await
        .expect("小资产下载应经代理成功（用户报告此步失败）");
    let mut buf = String::new();
    reader
        .read_to_string(&mut buf)
        .await
        .expect("读取小资产内容应成功");
    assert!(!buf.trim().is_empty(), "sha256 sidecar 内容不应为空");
    eprintln!("小资产下载成功：{}（{} 字节）", sha_asset.name, buf.len());

    // 第三步：大资产流式下载（zip 压缩包，验证长时间流式传输不被中途掐断）
    let zip_asset = release
        .assets
        .iter()
        .find(|a| a.name.ends_with("x86_64-pc-windows-msvc.zip"))
        .expect("release 应含 windows zip 资产");
    let mut reader = source
        .download_asset(&zip_asset.download_url)
        .await
        .expect("大资产下载应经代理成功");
    let mut total: u64 = 0;
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut chunk).await.expect("流式读取不应中途报错");
        if n == 0 {
            break;
        }
        total += n as u64;
    }
    assert!(total > 1024 * 1024, "zip 资产应大于 1MB，实际 {total} 字节");
    eprintln!("大资产流式下载成功：{}（{total} 字节）", zip_asset.name);
}

#[tokio::test]
#[ignore = "依赖外网公开源与本机真实代理，实机验收时显式跑"]
async fn 经真实代理_proxy回源与漏洞镜像刷新均成功() {
    let Some(proxy) = 真实代理配置() else {
        eprintln!("未设置 JIANARTIFACT_TEST_PROXY，跳过");
        return;
    };
    let network = Arc::new(
        NetworkState::new(proxy, Duration::from_secs(120)).expect("代理配置应可构造 client"),
    );

    let mut upstream = HttpUpstream::with_network_state(Arc::clone(&network))
        .fetch(
            "https://raw.githubusercontent.com/wcpe/JianArtifact/master",
            "Cargo.toml",
        )
        .await
        .expect("proxy 回源应经真实代理成功");
    let mut upstream_body = Vec::new();
    upstream
        .read_to_end(&mut upstream_body)
        .await
        .expect("proxy 回源响应应可读取");
    assert!(!upstream_body.is_empty(), "proxy 回源内容不应为空");

    let dir = tempfile::tempdir().unwrap();
    let store = MetaStore::open(&dir.path().join("meta.db"))
        .await
        .expect("测试元数据库应可打开");
    let source = HttpMirrorSource::with_network_state(
        "https://osv-vulnerabilities.storage.googleapis.com".to_string(),
        network,
    );
    let mirror_dir = dir.path().join("mirror");
    let mirror = VulnMirror::new(store.clone(), source, &mirror_dir);
    let count = mirror
        .refresh_ecosystem("GHC")
        .await
        .expect("漏洞镜像刷新应经真实代理成功");
    assert!(count > 0, "GHC 漏洞镜像应解析出公告");
    assert_eq!(store.count_advisories().await.unwrap() as usize, count);
}
