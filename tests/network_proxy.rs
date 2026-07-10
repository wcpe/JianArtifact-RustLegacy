//! FR-84 / FR-146 统一出站网络代理集成测试（单键 `url`，ADR-0042）。
//!
//! 用本地裸 TCP mock 正向代理断言：
//! ① 配置 `[network.proxy] url` 后出站请求经代理（代理收到 absolute-form 请求行）；
//! ② 旧键（http / https / all）经 `effective_url` 兜底仍生效（向后兼容）；
//! ③ `no_proxy` 命中的主机直连绕过代理（代理收不到该主机请求）；
//! ④ 不配置代理时行为与现状一致（请求直达源站、代理零参与）；
//! ⑤ 凭据型代理 URL 的凭据不出现在错误信息中。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jianartifact::config::{build_outbound_client, NetworkProxyConfig, NetworkState};
use jianartifact::proxy::{HttpUpstream, Upstream};
use jianartifact::vuln::{HttpMirrorSource, MirrorSource};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 起一个最小裸 TCP mock 正向代理：记录每个连接收到的首个请求行（HTTP 代理为 absolute-form），
/// 然后回一个最小 200 响应。返回 (代理地址, 收到的请求行列表)。
async fn start_mock_proxy() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_task = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let seen_conn = seen_task.clone();
            tokio::spawn(async move {
                // 只读首个请求行即可判定代理是否被使用，避免与客户端协议细节耦合
                let mut buf = [0u8; 1024];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                if n > 0 {
                    let text = String::from_utf8_lossy(&buf[..n]);
                    if let Some(line) = text.lines().next() {
                        seen_conn.lock().unwrap().push(line.to_string());
                    }
                }
                // 回最小响应，让客户端不至于因连接被重置而报传输层错误
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
                let _ = sock.flush().await;
            });
        }
    });
    wait_ready(&addr).await;
    (addr, seen)
}

async fn start_mock_socks5() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_task = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let seen_conn = Arc::clone(&seen_task);
            tokio::spawn(async move {
                let mut greeting = [0u8; 3];
                if sock.read_exact(&mut greeting).await.is_err() || greeting != [5, 1, 0] {
                    return;
                }
                if sock.write_all(&[5, 0]).await.is_err() {
                    return;
                }
                let mut header = [0u8; 4];
                if sock.read_exact(&mut header).await.is_err() || header[..3] != [5, 1, 0] {
                    return;
                }
                let host = match header[3] {
                    1 => {
                        let mut ip = [0u8; 4];
                        if sock.read_exact(&mut ip).await.is_err() {
                            return;
                        }
                        std::net::Ipv4Addr::from(ip).to_string()
                    }
                    3 => {
                        let mut len = [0u8; 1];
                        if sock.read_exact(&mut len).await.is_err() {
                            return;
                        }
                        let mut host = vec![0u8; len[0] as usize];
                        if sock.read_exact(&mut host).await.is_err() {
                            return;
                        }
                        String::from_utf8_lossy(&host).into_owned()
                    }
                    _ => return,
                };
                let mut port = [0u8; 2];
                if sock.read_exact(&mut port).await.is_err() {
                    return;
                }
                seen_conn
                    .lock()
                    .unwrap()
                    .push(format!("{host}:{}", u16::from_be_bytes(port)));
                let _ = sock.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80]).await;
                let mut request = [0u8; 1024];
                let _ = sock.read(&mut request).await;
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    wait_ready(&addr).await;
    (addr, seen)
}

/// 起一个最小裸 TCP 源站：记录是否被直连访问，回最小 200。返回 (源站地址, 被访问标志)。
async fn start_mock_origin() -> (SocketAddr, Arc<Mutex<bool>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hit: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    let hit_task = hit.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 512];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            if n > 0 {
                *hit_task.lock().unwrap() = true;
            }
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await;
            let _ = sock.flush().await;
        }
    });
    wait_ready(&addr).await;
    (addr, hit)
}

/// 轮询直到端口可连接。
async fn wait_ready(addr: &SocketAddr) {
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("mock 服务未就绪");
}

#[tokio::test]
async fn 配置通用代理后出站请求经代理() {
    let (proxy_addr, seen) = start_mock_proxy().await;
    let proxy = NetworkProxyConfig {
        url: Some(format!("http://{proxy_addr}")),
        http: None,
        https: None,
        all: None,
        no_proxy: None,
    };
    let client = build_outbound_client(Duration::from_secs(5), &proxy).unwrap();
    // 目标主机不真实存在也无妨：只要请求被送到代理即证明注入生效
    let _ = client.get("http://example.invalid/some/path").send().await;

    let lines = seen.lock().unwrap().clone();
    assert!(!lines.is_empty(), "代理应收到出站请求");
    // HTTP 正向代理收到 absolute-form 请求行，含完整目标 URL
    assert!(
        lines.iter().any(|l| l.contains("http://example.invalid")),
        "代理收到的请求行应为指向目标主机的 absolute-form，实际：{lines:?}"
    );
}

#[tokio::test]
async fn https目标经http代理发送connect() {
    let (proxy_addr, seen) = start_mock_proxy().await;
    let proxy = NetworkProxyConfig {
        url: Some(format!("http://{proxy_addr}")),
        no_proxy: None,
        ..Default::default()
    };
    let client = build_outbound_client(Duration::from_secs(2), &proxy).unwrap();
    let _ = client.get("https://example.invalid/secure").send().await;

    let lines = seen.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|line| line == "CONNECT example.invalid:443 HTTP/1.1"),
        "HTTPS 目标应经 HTTP 代理发送 CONNECT，实际：{lines:?}"
    );
}

#[tokio::test]
async fn socks5h代理收到远端dns域名与目标端口() {
    let (proxy_addr, seen) = start_mock_socks5().await;
    let proxy = NetworkProxyConfig {
        url: Some(format!("socks5h://{proxy_addr}")),
        no_proxy: None,
        ..Default::default()
    };
    let client = build_outbound_client(Duration::from_secs(2), &proxy).unwrap();
    let response = client
        .get("http://example.invalid/socks-path")
        .send()
        .await
        .expect("SOCKS5H mock 应完成握手并返回响应");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["example.invalid:80"],
        "SOCKS5H 必须把域名交给代理解析"
    );
}

#[tokio::test]
async fn proxy回源与漏洞镜像下载均读取共享代理槽() {
    let (proxy_addr, seen) = start_mock_proxy().await;
    let proxy = NetworkProxyConfig {
        url: Some(format!("http://{proxy_addr}")),
        no_proxy: None,
        ..Default::default()
    };
    let network = Arc::new(NetworkState::new(proxy, Duration::from_secs(2)).unwrap());

    HttpUpstream::with_network_state(Arc::clone(&network))
        .fetch("http://repo.example.invalid", "artifact.bin")
        .await
        .expect("proxy 回源应经共享代理槽成功");

    let dir = tempfile::tempdir().unwrap();
    HttpMirrorSource::with_network_state(
        "http://osv.example.invalid".to_string(),
        Arc::clone(&network),
    )
    .download("Maven", &dir.path().join("all.zip"))
    .await
    .expect("漏洞镜像下载应经共享代理槽成功");

    let lines = seen.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("repo.example.invalid/artifact.bin")),
        "proxy 回源请求应到达代理，实际：{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("osv.example.invalid/Maven/all.zip")),
        "漏洞镜像请求应到达代理，实际：{lines:?}"
    );
}

#[tokio::test]
async fn 旧键_all_经兜底仍生效() {
    // ADR-0042 向后兼容：未 normalize 的旧 all 键经 effective_url 兜底仍注入代理。
    let (proxy_addr, seen) = start_mock_proxy().await;
    let proxy = NetworkProxyConfig {
        url: None,
        http: None,
        https: None,
        all: Some(format!("http://{proxy_addr}")),
        no_proxy: None,
    };
    let client = build_outbound_client(Duration::from_secs(5), &proxy).unwrap();
    let _ = client.get("http://example.invalid/some/path").send().await;

    let lines = seen.lock().unwrap().clone();
    assert!(!lines.is_empty(), "仅配 all 时 http 请求也应经全局代理");
    assert!(
        lines.iter().any(|l| l.contains("http://example.invalid")),
        "all 代理收到的请求行应为指向目标主机的 absolute-form，实际：{lines:?}"
    );
}

#[tokio::test]
async fn 旧三键并存时按优先级走_all() {
    // ADR-0042：旧三键并存按 all > https > http 折叠，请求应命中 all 指向的代理。
    let (http_proxy_addr, http_seen) = start_mock_proxy().await;
    let (all_proxy_addr, all_seen) = start_mock_proxy().await;
    let proxy = NetworkProxyConfig {
        url: None,
        http: Some(format!("http://{http_proxy_addr}")),
        https: None,
        all: Some(format!("http://{all_proxy_addr}")),
        no_proxy: None,
    };
    let client = build_outbound_client(Duration::from_secs(5), &proxy).unwrap();
    let _ = client.get("http://example.invalid/some/path").send().await;

    // 单一通用代理：优先级最高的 all 生效，旧 http 键被忽略
    assert!(
        !all_seen.lock().unwrap().is_empty(),
        "请求应走优先级最高的 all 代理"
    );
    assert!(
        http_seen.lock().unwrap().is_empty(),
        "旧 http 键不应生效，却收到：{:?}",
        http_seen.lock().unwrap()
    );
}

#[tokio::test]
async fn no_proxy命中主机直连绕过代理() {
    let (proxy_addr, seen) = start_mock_proxy().await;
    let (origin_addr, origin_hit) = start_mock_origin().await;
    let origin_host = origin_addr.ip().to_string();

    let proxy = NetworkProxyConfig {
        url: Some(format!("http://{proxy_addr}")),
        http: None,
        https: None,
        all: None,
        // 把源站主机列入 no_proxy，应直连绕过代理
        no_proxy: Some(origin_host.clone()),
    };
    let client = build_outbound_client(Duration::from_secs(5), &proxy).unwrap();
    let _ = client
        .get(format!("http://{origin_addr}/ping"))
        .send()
        .await;

    assert!(
        *origin_hit.lock().unwrap(),
        "no_proxy 命中的主机应被直连访问"
    );
    let lines = seen.lock().unwrap().clone();
    assert!(
        lines.is_empty(),
        "no_proxy 命中的主机不应经代理，代理却收到：{lines:?}"
    );
}

#[tokio::test]
async fn 不配置代理时直达源站不经代理() {
    let (proxy_addr, seen) = start_mock_proxy().await;
    let (origin_addr, origin_hit) = start_mock_origin().await;

    // 三键全空：不显式注入代理，行为与现状一致（直达源站）
    let proxy = NetworkProxyConfig::default();
    let client = build_outbound_client(Duration::from_secs(5), &proxy).unwrap();
    let _ = client
        .get(format!("http://{origin_addr}/ping"))
        .send()
        .await;

    assert!(*origin_hit.lock().unwrap(), "不配置代理时应直达源站");
    let lines = seen.lock().unwrap().clone();
    assert!(
        lines.is_empty(),
        "不配置代理时不应有请求经过 mock 代理（地址 {proxy_addr}），实际：{lines:?}"
    );
}

#[tokio::test]
async fn 凭据型代理url的凭据不出现在错误信息() {
    // 非法代理 URL（含凭据）触发构造错误；断言错误信息不泄露用户名 / 口令
    let proxy = NetworkProxyConfig {
        url: Some("http://secretuser:secretpass@".to_string()),
        http: None,
        https: None,
        all: None,
        no_proxy: None,
    };
    let result = build_outbound_client(Duration::from_secs(5), &proxy);
    if let Err(msg) = result {
        assert!(
            !msg.contains("secretuser") && !msg.contains("secretpass"),
            "错误信息不得泄露代理凭据，实际：{msg}"
        );
    }
    // 构造成功也可接受：关键是失败路径不泄露凭据（上面已断言）
}
