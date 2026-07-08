//! reqwest 实现的 Nexus REST 客户端（FR-36）：纯 rustls TLS，按 base URL 拉取仓库列表。
//!
//! 守 SECURITY：仅 rustls 校验源系统 HTTPS 证书，不引 native-tls / openssl。
//! 凭据经 HTTP Basic Auth 注入请求头，绝不写入日志 / 错误信息。

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

impl DockerRegistryClient for HttpNexusClient {
    async fn list_images(
        &self,
        base_url: &str,
        repository: &str,
        credential: Option<&NexusCredential>,
    ) -> Result<Vec<String>, MigrateError> {
        // registry v2 目录端点：`{v2}/_catalog` → { "repositories": ["img", "ns/img", ...] }
        let url = format!("{}/_catalog", docker_v2_base(base_url, repository));
        let body = self.get_text(&url, credential, None).await?;

        #[derive(serde::Deserialize)]
        struct Catalog {
            #[serde(default)]
            repositories: Vec<String>,
        }
        let catalog: Catalog =
            serde_json::from_str(&body).map_err(|e| MigrateError::Parse(e.to_string()))?;
        Ok(catalog.repositories)
    }

    async fn list_tags(
        &self,
        base_url: &str,
        repository: &str,
        image: &str,
        credential: Option<&NexusCredential>,
    ) -> Result<Vec<String>, MigrateError> {
        // `{v2}/{image}/tags/list` → { "name": "...", "tags": ["1.0", "latest", ...] }
        let url = format!(
            "{}/{}/tags/list",
            docker_v2_base(base_url, repository),
            image
        );
        let body = self.get_text(&url, credential, None).await?;

        #[derive(serde::Deserialize)]
        struct TagList {
            #[serde(default)]
            tags: Option<Vec<String>>,
        }
        let list: TagList =
            serde_json::from_str(&body).map_err(|e| MigrateError::Parse(e.to_string()))?;
        // Nexus 对无 tag 的镜像可能返回 `"tags": null`，归一化为空列表
        Ok(list.tags.unwrap_or_default())
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
    /// GET 一个 URL 取文本响应（Basic Auth 注入、非 2xx 归 Status、可选 Accept 头）。
    ///
    /// 供 registry v2 的 `_catalog` / `tags/list` 等 JSON 元数据端点复用。凭据不进日志。
    async fn get_text(
        &self,
        url: &str,
        credential: Option<&NexusCredential>,
        accept: Option<&str>,
    ) -> Result<String, MigrateError> {
        let mut req = self.network.client().get(url);
        if let Some(a) = accept {
            req = req.header(reqwest::header::ACCEPT, a);
        }
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
}
