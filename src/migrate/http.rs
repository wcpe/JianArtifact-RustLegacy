//! reqwest 实现的 Nexus REST 客户端（FR-36）：纯 rustls TLS，按 base URL 拉取仓库列表。
//!
//! 守 SECURITY：仅 rustls 校验源系统 HTTPS 证书，不引 native-tls / openssl。
//! 凭据经 HTTP Basic Auth 注入请求头，绝不写入日志 / 错误信息。

use std::collections::HashSet;
use std::sync::Arc;

use futures_util::TryStreamExt;
use tokio_util::io::StreamReader;

use crate::config::NetworkState;

use super::{
    DockerRegistryClient, FetchedManifest, MigrateError, NexusClient, NexusCredential,
    NEXUS_COMPONENTS_PATH, NEXUS_REPOSITORIES_PATH,
};

/// 拉取 docker manifest 时声明可接受的媒体类型（schema2 / OCI image manifest / manifest list / index）：
/// 让源 registry 返回原始 manifest（而非按旧客户端降级转换），保证 digest 与源一致。
const DOCKER_MANIFEST_ACCEPT: &str = "application/vnd.docker.distribution.manifest.v2+json, \
     application/vnd.docker.distribution.manifest.list.v2+json, \
     application/vnd.oci.image.manifest.v1+json, \
     application/vnd.oci.image.index.v1+json";

/// Registry v2 单次枚举请求的页容量。
const REGISTRY_PAGE_SIZE: &str = "100";
/// Registry v2 枚举允许的最大页数，防止异常源无限续页。
const MAX_REGISTRY_PAGES: usize = 10_000;

/// 基于 reqwest 的 Nexus REST 客户端。
///
/// 不再持有启动期固化的 client，改持出站网络热替换槽 [`NetworkState`]（FR-88，ADR-0022）：
/// 每次出站经 `network.client()` 取当前 client（含运行时 PATCH 后的新代理）。
#[derive(Clone)]
pub struct HttpNexusClient {
    /// 出站网络热替换槽（含当前 client，随 PATCH 即时换代理）。
    network: Arc<NetworkState>,
}

impl HttpNexusClient {
    /// 持出站网络热替换槽构造 Nexus 客户端（FR-88，ADR-0022）。
    ///
    /// 出站时经 `network.client()` 取当前 client；代理 / 超时 / rustls / stream 特性由槽统一注入。
    pub fn with_network_state(network: Arc<NetworkState>) -> Self {
        Self { network }
    }
}

impl NexusClient for HttpNexusClient {
    async fn fetch_repositories(
        &self,
        base_url: &str,
        credential: Option<&NexusCredential>,
    ) -> Result<String, MigrateError> {
        // 拼 URL：去 base 尾斜杠，避免双斜杠或缺斜杠
        let url = format!(
            "{}/{}",
            base_url.trim_end_matches('/'),
            NEXUS_REPOSITORIES_PATH
        );

        // 从热替换槽取当前 client（读锁极短、锁外发请求）
        let mut req = self.network.client().get(&url);
        // 带凭据时以 Basic Auth 注入；凭据不进日志
        if let Some(c) = credential {
            req = req.basic_auth(&c.username, Some(&c.password));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| MigrateError::Transport(e.to_string()))?;

        // 非 2xx 一律按源系统错误处理（如 401 鉴权失败 / 5xx）
        if !resp.status().is_success() {
            return Err(MigrateError::Status(resp.status().as_u16()));
        }

        // 仓库列表为有限规模的 JSON 元数据，整体读为文本后交纯函数解析
        resp.text()
            .await
            .map_err(|e| MigrateError::Transport(e.to_string()))
    }

    async fn fetch_components(
        &self,
        base_url: &str,
        repository: &str,
        continuation_token: Option<&str>,
        credential: Option<&NexusCredential>,
    ) -> Result<String, MigrateError> {
        // 拼 `{base}/service/rest/v1/components?repository=X[&continuationToken=T]`
        let mut url = reqwest::Url::parse(&format!(
            "{}/{}",
            base_url.trim_end_matches('/'),
            NEXUS_COMPONENTS_PATH
        ))
        .map_err(|e| MigrateError::Invalid(e.to_string()))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("repository", repository);
            if let Some(t) = continuation_token {
                q.append_pair("continuationToken", t);
            }
        }

        let mut req = self.network.client().get(url);
        if let Some(c) = credential {
            req = req.basic_auth(&c.username, Some(&c.password));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| MigrateError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(MigrateError::Status(resp.status().as_u16()));
        }
        resp.text()
            .await
            .map_err(|e| MigrateError::Transport(e.to_string()))
    }

    async fn download_asset(
        &self,
        download_url: &str,
        credential: Option<&NexusCredential>,
    ) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>, MigrateError> {
        let mut req = self.network.client().get(download_url);
        if let Some(c) = credential {
            req = req.basic_auth(&c.username, Some(&c.password));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| MigrateError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(MigrateError::Status(resp.status().as_u16()));
        }
        // 响应字节流适配为 AsyncRead：流式下载，不整体载入内存
        let stream = resp
            .bytes_stream()
            .map_err(|e| std::io::Error::other(e.to_string()));
        Ok(Box::new(StreamReader::new(stream)))
    }
}

/// 拼出源 Nexus docker 仓库的 registry v2 基址：`{base}/repository/{repo}/v2`（FR-144）。
///
/// Nexus OSS 的 docker hosted / proxy 仓库经 `{base}/repository/{repo}/v2/...` 暴露标准 registry v2
/// 端点（与其 REST 仓库同名子路径）。各镜像 / manifest / blob 端点相对此基址拼接。
fn docker_v2_base(base_url: &str, repository: &str) -> String {
    format!(
        "{}/repository/{}/v2",
        base_url.trim_end_matches('/'),
        repository
    )
}

/// 解析带尾斜杠的源 registry v2 前缀，供安全续页校验与相对 URL 拼接共用。
fn registry_prefix_url(base_url: &str, repository: &str) -> Result<reqwest::Url, MigrateError> {
    reqwest::Url::parse(&format!("{}/", docker_v2_base(base_url, repository)))
        .map_err(|e| MigrateError::Invalid(e.to_string()))
}

impl DockerRegistryClient for HttpNexusClient {
    async fn list_images(
        &self,
        base_url: &str,
        repository: &str,
        credential: Option<&NexusCredential>,
    ) -> Result<Vec<String>, MigrateError> {
        #[derive(serde::Deserialize)]
        struct Catalog {
            #[serde(default)]
            repositories: Vec<String>,
        }

        let prefix = registry_prefix_url(base_url, repository)?;
        let initial = prefix
            .join("_catalog")
            .map_err(|e| MigrateError::Invalid(e.to_string()))?;
        self.collect_registry_pages(initial, &prefix, credential, |body| {
            let page: Catalog =
                serde_json::from_str(body).map_err(|e| MigrateError::Parse(e.to_string()))?;
            Ok(page.repositories)
        })
        .await
    }

    async fn list_tags(
        &self,
        base_url: &str,
        repository: &str,
        image: &str,
        credential: Option<&NexusCredential>,
    ) -> Result<Vec<String>, MigrateError> {
        #[derive(serde::Deserialize)]
        struct TagList {
            #[serde(default)]
            tags: Option<Vec<String>>,
        }

        let prefix = registry_prefix_url(base_url, repository)?;
        let initial = prefix
            .join(&format!("{image}/tags/list"))
            .map_err(|e| MigrateError::Invalid(e.to_string()))?;
        self.collect_registry_pages(initial, &prefix, credential, |body| {
            let page: TagList =
                serde_json::from_str(body).map_err(|e| MigrateError::Parse(e.to_string()))?;
            Ok(page.tags.unwrap_or_default())
        })
        .await
    }

    async fn fetch_manifest(
        &self,
        base_url: &str,
        repository: &str,
        image: &str,
        reference: &str,
        credential: Option<&NexusCredential>,
    ) -> Result<FetchedManifest, MigrateError> {
        // `{v2}/{image}/manifests/{reference}`，带 Accept 头取原始 manifest
        let url = format!(
            "{}/{}/manifests/{}",
            docker_v2_base(base_url, repository),
            image,
            reference
        );
        let mut req = self
            .network
            .client()
            .get(&url)
            .header(reqwest::header::ACCEPT, DOCKER_MANIFEST_ACCEPT);
        if let Some(c) = credential {
            req = req.basic_auth(&c.username, Some(&c.password));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| MigrateError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(MigrateError::Status(resp.status().as_u16()));
        }
        // 媒体类型取自响应 Content-Type（缺失按 schema2 默认，与本系统落库默认一致）
        let media_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(';').next().unwrap_or(s).trim().to_string())
            .unwrap_or_else(|| "application/vnd.docker.distribution.manifest.v2+json".to_string());
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| MigrateError::Transport(e.to_string()))?
            .to_vec();
        Ok(FetchedManifest { bytes, media_type })
    }

    async fn download_blob(
        &self,
        base_url: &str,
        repository: &str,
        image: &str,
        digest: &str,
        credential: Option<&NexusCredential>,
    ) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>, MigrateError> {
        // `{v2}/{image}/blobs/{digest}`，流式下载（复用 download_asset 的流适配）
        let url = format!(
            "{}/{}/blobs/{}",
            docker_v2_base(base_url, repository),
            image,
            digest
        );
        self.download_asset(&url, credential).await
    }
}

impl HttpNexusClient {
    /// 完整枚举 Registry v2 分页，跨页去重并保留首次出现顺序。
    async fn collect_registry_pages<F>(
        &self,
        mut current: reqwest::Url,
        prefix: &reqwest::Url,
        credential: Option<&NexusCredential>,
        mut parse_items: F,
    ) -> Result<Vec<String>, MigrateError>
    where
        F: FnMut(&str) -> Result<Vec<String>, MigrateError>,
    {
        current
            .query_pairs_mut()
            .append_pair("n", REGISTRY_PAGE_SIZE);
        let mut visited = HashSet::new();
        let mut seen_items = HashSet::new();
        let mut items = Vec::new();

        let mut page_number = 1;
        loop {
            ensure_page_within_limit(page_number)?;
            register_page(&current, &mut visited)?;
            let page = self.get_registry_page(&current, credential).await?;
            extend_unique(&mut items, &mut seen_items, parse_items(&page.body)?);
            let Some(next) = parse_next_link(&page.links)? else {
                return Ok(items);
            };
            current = resolve_next_url(&current, prefix, &next)?;
            page_number += 1;
        }
    }

    /// 拉取一页 JSON 元数据，并在读取 body 前保存全部 Link 头。
    async fn get_registry_page(
        &self,
        url: &reqwest::Url,
        credential: Option<&NexusCredential>,
    ) -> Result<RegistryPage, MigrateError> {
        let mut req = self.network.client().get(url.clone());
        if let Some(c) = credential {
            req = req.basic_auth(&c.username, Some(&c.password));
        }
        let resp = req.send().await.map_err(|_| {
            MigrateError::Transport("Registry v2 分页请求失败（地址已脱敏）".to_string())
        })?;
        if !resp.status().is_success() {
            return Err(MigrateError::Status(resp.status().as_u16()));
        }
        let links = read_link_headers(resp.headers())?;
        let body = resp.text().await.map_err(|_| {
            MigrateError::Transport("Registry v2 分页响应读取失败（地址已脱敏）".to_string())
        })?;
        Ok(RegistryPage { body, links })
    }
}

/// Registry v2 元数据单页响应。
struct RegistryPage {
    body: String,
    /// `None` 表示响应完全没有 Link 头；`Some` 保留全部 Link 字段值供严格解析。
    links: Option<Vec<String>>,
}

fn read_link_headers(
    headers: &reqwest::header::HeaderMap,
) -> Result<Option<Vec<String>>, MigrateError> {
    let values = headers.get_all(reqwest::header::LINK);
    let mut links = Vec::new();
    for value in values.iter() {
        links.push(
            value
                .to_str()
                .map_err(|_| MigrateError::Parse("Registry v2 Link 头不是有效文本".to_string()))?
                .to_string(),
        );
    }
    Ok((!links.is_empty()).then_some(links))
}

fn ensure_page_within_limit(page_number: usize) -> Result<(), MigrateError> {
    if page_number <= MAX_REGISTRY_PAGES {
        return Ok(());
    }
    Err(MigrateError::Invalid(
        "Registry v2 分页超过 10000 页".to_string(),
    ))
}

fn register_page(url: &reqwest::Url, visited: &mut HashSet<String>) -> Result<(), MigrateError> {
    if visited.insert(url.as_str().to_string()) {
        return Ok(());
    }
    Err(MigrateError::Invalid(
        "Registry v2 分页 URL 重复或形成循环".to_string(),
    ))
}

fn extend_unique(items: &mut Vec<String>, seen: &mut HashSet<String>, page: Vec<String>) {
    for item in page {
        if seen.insert(item.clone()) {
            items.push(item);
        }
    }
}

/// 解析 Link 头中唯一的 `rel=next`；只有合法的非 next relation 时视为末页。
fn parse_next_link(headers: &Option<Vec<String>>) -> Result<Option<String>, MigrateError> {
    let Some(headers) = headers else {
        return Ok(None);
    };
    let mut next = None;
    for header in headers {
        for value in split_link_values(header)? {
            let parsed = parse_link_value(value)?;
            if parsed.is_next && next.replace(parsed.target).is_some() {
                return Err(MigrateError::Parse(
                    "Registry v2 Link 头包含多个 next 关系".to_string(),
                ));
            }
        }
    }
    Ok(next)
}

struct ParsedLink {
    target: String,
    is_next: bool,
}

fn parse_link_value(value: &str) -> Result<ParsedLink, MigrateError> {
    let value = value.trim();
    let close = value
        .strip_prefix('<')
        .and_then(|rest| rest.find('>').map(|i| i + 1))
        .ok_or_else(malformed_link)?;
    let target = value[1..close].trim();
    if target.is_empty() {
        return Err(malformed_link());
    }
    let remainder = value[close + 1..].trim();
    if !remainder.is_empty() && !remainder.starts_with(';') {
        return Err(malformed_link());
    }
    let relations = parse_relations(remainder)?;
    Ok(ParsedLink {
        target: target.to_string(),
        is_next: relations.iter().any(|r| r.eq_ignore_ascii_case("next")),
    })
}

fn parse_relations(parameters: &str) -> Result<Vec<String>, MigrateError> {
    let mut relations = Vec::new();
    for parameter in split_quoted(parameters.trim_start_matches(';'), ';')? {
        let parameter = parameter.trim();
        if parameter.is_empty() {
            continue;
        }
        let (name, value) = parameter.split_once('=').ok_or_else(malformed_link)?;
        if name.trim().eq_ignore_ascii_case("rel") {
            let value = unquote_parameter(value.trim())?;
            relations.extend(value.split_ascii_whitespace().map(str::to_string));
        }
    }
    Ok(relations)
}

fn split_link_values(header: &str) -> Result<Vec<&str>, MigrateError> {
    split_delimited(header, ',', true)
}

fn split_quoted(value: &str, delimiter: char) -> Result<Vec<&str>, MigrateError> {
    split_delimited(value, delimiter, false)
}

fn split_delimited(
    value: &str,
    delimiter: char,
    track_angles: bool,
) -> Result<Vec<&str>, MigrateError> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut angled = false;
    for (index, ch) in value.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '<' if track_angles && !quoted => angled = true,
            '>' if track_angles && !quoted => angled = false,
            _ if ch == delimiter && !quoted && !angled => {
                parts.push(&value[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if quoted || angled {
        return Err(malformed_link());
    }
    parts.push(&value[start..]);
    Ok(parts)
}

fn unquote_parameter(value: &str) -> Result<&str, MigrateError> {
    if let Some(inner) = value.strip_prefix('"') {
        return inner.strip_suffix('"').ok_or_else(malformed_link);
    }
    if value.contains('"') || value.is_empty() {
        return Err(malformed_link());
    }
    Ok(value)
}

fn resolve_next_url(
    current: &reqwest::Url,
    prefix: &reqwest::Url,
    target: &str,
) -> Result<reqwest::Url, MigrateError> {
    let next = current.join(target).map_err(|_| malformed_link())?;
    if !same_origin(&next, prefix) || !next.path().starts_with(prefix.path()) {
        return Err(MigrateError::Invalid(
            "Registry v2 续页 URL 越出允许的同源仓库前缀".to_string(),
        ));
    }
    Ok(next)
}

fn same_origin(left: &reqwest::Url, right: &reqwest::Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn malformed_link() -> MigrateError {
    MigrateError::Parse("Registry v2 Link 头格式损坏".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{NetworkProxyConfig, NetworkState};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    struct MockPage {
        expected_target: &'static str,
        body: &'static str,
        link: Option<&'static str>,
    }

    async fn 启动分页源(pages: Vec<MockPage>) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            for page in pages {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buf = [0u8; 1024];
                    let read = stream.read(&mut buf).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..read]);
                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8(request).unwrap();
                let target = request
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                assert_eq!(target, page.expected_target);

                let link = page
                    .link
                    .map(|value| format!("Link: {value}\r\n"))
                    .unwrap_or_default();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{link}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    page.body.len(),
                    page.body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (format!("http://{addr}"), handle)
    }

    fn 测试客户端() -> HttpNexusClient {
        let network =
            NetworkState::new(NetworkProxyConfig::default(), Duration::from_secs(5)).unwrap();
        HttpNexusClient::with_network_state(Arc::new(network))
    }

    #[tokio::test]
    async fn catalog_跟随两页并保留编码游标() {
        let (base, server) = 启动分页源(vec![
            MockPage {
                expected_target: "/repository/docker-hosted/v2/_catalog?n=100",
                body: r#"{"repositories":["ns/first","duplicate"]}"#,
                link: Some(
                    "</repository/docker-hosted/v2/_catalog?n=100&last=ns%2Ffirst>; ReL=NeXt",
                ),
            },
            MockPage {
                expected_target: "/repository/docker-hosted/v2/_catalog?n=100&last=ns%2Ffirst",
                body: r#"{"repositories":["duplicate","ns/second"]}"#,
                link: None,
            },
        ])
        .await;

        let images = 测试客户端()
            .list_images(&base, "docker-hosted", None)
            .await
            .unwrap();

        assert_eq!(images, vec!["ns/first", "duplicate", "ns/second"]);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn tags_跟随仅查询续页并保留编码游标() {
        let (base, server) = 启动分页源(vec![
            MockPage {
                expected_target: "/repository/docker-hosted/v2/app/tags/list?n=100",
                body: r#"{"name":"app","tags":["v1","same"]}"#,
                link: Some("<?n=100&last=v1%2Bbuild>; rel=\"next\"; TYPE=cursor"),
            },
            MockPage {
                expected_target: "/repository/docker-hosted/v2/app/tags/list?n=100&last=v1%2Bbuild",
                body: r#"{"name":"app","tags":["same","v2"]}"#,
                link: None,
            },
        ])
        .await;

        let tags = 测试客户端()
            .list_tags(&base, "docker-hosted", "app", None)
            .await
            .unwrap();

        assert_eq!(tags, vec!["v1", "same", "v2"]);
        server.await.unwrap();
    }

    #[test]
    fn link_只跟随_next_并兼容引号与大小写() {
        let links = Some(vec![
            r#"</ignored>; rel="prev", <next?p=1>; REL="alternate NeXt"; TYPE="x""#.to_string(),
        ]);
        assert_eq!(
            parse_next_link(&links).unwrap().as_deref(),
            Some("next?p=1")
        );

        let only_previous = Some(vec!["</previous>; rel=prev".to_string()]);
        assert_eq!(parse_next_link(&only_previous).unwrap(), None);
    }

    #[test]
    fn link_存在但格式损坏或多个_next_时报错() {
        for links in [
            Some(vec!["".to_string()]),
            Some(vec!["not-a-link".to_string()]),
            Some(vec!["<next; rel=next".to_string()]),
            Some(vec!["<next>; rel".to_string()]),
            Some(vec!["<a>; rel=next, <b>; rel=next".to_string()]),
        ] {
            assert!(matches!(
                parse_next_link(&links),
                Err(MigrateError::Parse(_))
            ));
        }
    }

    #[test]
    fn 续页_url_完整支持绝对_绝对路径_相对与仅查询() {
        let prefix = reqwest::Url::parse("https://registry.example/repository/r/v2/").unwrap();
        let current = prefix.join("path/_catalog?n=100&last=a").unwrap();
        let cases = [
            (
                "https://registry.example/repository/r/v2/absolute?n=100",
                "/repository/r/v2/absolute?n=100",
            ),
            ("/repository/r/v2/root?n=100", "/repository/r/v2/root?n=100"),
            ("relative?n=100", "/repository/r/v2/path/relative?n=100"),
            (
                "?n=100&last=b%2Fc",
                "/repository/r/v2/path/_catalog?n=100&last=b%2Fc",
            ),
        ];

        for (target, expected) in cases {
            let next = resolve_next_url(&current, &prefix, target).unwrap();
            let actual = match next.query() {
                Some(query) => format!("{}?{query}", next.path()),
                None => next.path().to_string(),
            };
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn 续页_url_拒绝跨源与越出_registry_前缀() {
        let prefix = reqwest::Url::parse("https://registry.example/repository/r/v2/").unwrap();
        let current = prefix.join("_catalog?n=100").unwrap();
        for target in [
            "https://other.example/repository/r/v2/_catalog?n=100",
            "http://registry.example/repository/r/v2/_catalog?n=100",
            "https://registry.example:444/repository/r/v2/_catalog?n=100",
            "/repository/other/v2/_catalog?n=100",
            "../../../outside?n=100",
        ] {
            assert!(matches!(
                resolve_next_url(&current, &prefix, target),
                Err(MigrateError::Invalid(_))
            ));
        }
    }

    #[tokio::test]
    async fn 重复续页_url_按循环失败() {
        let (base, server) = 启动分页源(vec![MockPage {
            expected_target: "/repository/docker-hosted/v2/_catalog?n=100",
            body: r#"{"repositories":["first"]}"#,
            link: Some("<?n=100>; rel=next"),
        }])
        .await;

        let result = 测试客户端().list_images(&base, "docker-hosted", None).await;
        assert!(matches!(result, Err(MigrateError::Invalid(_))));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn 分页传输错误不回显完整敏感查询() {
        let (base, server) = 启动分页源(vec![MockPage {
            expected_target: "/repository/docker-hosted/v2/_catalog?n=100",
            body: r#"{"repositories":["first"]}"#,
            link: Some("<?n=100&last=sensitive%2Fcursor>; rel=next"),
        }])
        .await;

        let error = 测试客户端()
            .list_images(&base, "docker-hosted", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("地址已脱敏"));
        assert!(!error.contains("sensitive"));
        server.await.unwrap();
    }

    #[test]
    fn registry_分页上限允许一万页并拒绝下一页() {
        assert!(ensure_page_within_limit(10_000).is_ok());
        assert!(matches!(
            ensure_page_within_limit(10_001),
            Err(MigrateError::Invalid(_))
        ));
    }
}
