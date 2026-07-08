//! 离线迁移仓库索引双通道（FR-141，ADR-0041）。
//!
//! 离线迁移在搬运前需先获取**仓库索引**（仓库列表及其格式 / 类型 / 上游配置）展示给管理员选择。
//! 源 Nexus 3.70.x 的仓库配置存于嵌入式 OrientDB（Rust 无法读取），blob store 的 `.properties`
//! 只含仓库名 / 内容路径 / sha1 / 大小、不含权威配置。为此按 ADR-0041 采用**双通道**：
//!
//! - **通道一（权威，优先）**：给定源 Nexus REST 地址且可达时，经 `/service/rest/v1/repositories`
//!   取权威仓库配置（格式 / 类型 / 上游 URL），与离线扫描结果**按仓库名合并**——配置取 REST、
//!   blob 数与总大小取扫描。
//! - **通道二（推断，回退）**：未填地址或探测不可达时，纯离线扫描并按内容路径**启发式推断格式**
//!   （标注来源为「推断」，类型 / 上游留空，由管理员在 FR-142 向导补填）。
//!
//! 两通道输出同一份 [`RepoIndexEntry`]。本模块只产出索引、不搬运制品；不解析 OrientDB / H2。
//!
//! 关键约束：
//! - 格式推断做成无副作用纯函数（[`infer_repo_format`]），便于穷举各布局边界。
//! - 凭据经 `auth_ref` → 环境变量解析，绝不入库 / 进日志。
//! - 依赖方向：本模块属 `migrate`，复用 `offline` 扫描与 `online`/`discover_repositories` REST 客户端；
//!   api 层薄编排调用之。

use std::collections::BTreeMap;

use super::{
    map_nexus_format, NexusClient, NexusRepoSummary, OfflineBlobSummary, OfflineRepoSummary,
};

/// 索引项中格式的来源：权威（REST 取回）或推断（离线启发式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FormatSource {
    /// 权威通道：格式取自源 Nexus REST 仓库配置。
    Authoritative,
    /// 推断通道：格式由离线 blob 内容路径启发式推断，可能误判、需人工确认。
    Inferred,
}

/// 统一仓库索引项（FR-141）：两通道输出同一结构，供 FR-142 向导多选与后续搬运消费。
///
/// `name` / `blob_count` / `total_size` 两通道均取自离线 blob store 扫描（本地实有可迁内容为准）；
/// `format` / `format_source` / `r#type` / `upstream_url` 视通道而定——权威通道取 REST 配置、
/// `format_source = Authoritative`、类型 / 上游有值；推断通道 `format_source = Inferred`、
/// 类型与上游留空（`None`），由管理员在向导补填。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RepoIndexEntry {
    /// 仓库名（取自离线扫描的 `@Bucket.repo-name`）。
    pub name: String,
    /// 本系统格式名（`maven` / `npm` / `docker` / `raw` 等）。
    pub format: String,
    /// 格式来源（权威 / 推断）。
    pub format_source: FormatSource,
    /// 仓库类型（`hosted` / `proxy`）；仅权威通道有值，推断通道为 `None`。
    pub r#type: Option<String>,
    /// 该仓库离线扫描到的 blob 数量。
    pub blob_count: usize,
    /// 该仓库 blob 逻辑大小之和（`.properties` 声明 size 之和；缺失按 0 计）。
    pub total_size: u64,
    /// proxy 仓库上游地址；仅权威通道的 proxy 仓库有值，其余为 `None`。
    pub upstream_url: Option<String>,
}

/// 各推断格式命中一次内容特征的计分权重（大于 0 即视为命中该格式）。
const FORMAT_HIT_SCORE: usize = 1;

/// 据某仓库全部 blob 逻辑名路径**启发式推断本系统格式**（纯函数，便于穷举，ADR-0041）。
///
/// 对每个 blob 路径按内容特征给候选格式计分，取最高分者；全无命中兜底 `raw`。仅覆盖 ADR-0041
/// 约定的四格式（maven / npm / docker）+ raw 兜底：
/// - maven：含 `maven-metadata.xml`，或 group/artifact/version/file 布局且文件名以常见 Maven 扩展结尾；
/// - npm：含 `/-/` 且文件名以 `.tgz` 结尾（npm tarball 布局），或路径含 `package.json`；
/// - docker：docker v2 内容寻址布局（路径含 `blobs/sha256` / `manifests/` / `_uploads/`）；
/// - raw：以上均不命中的兜底。
pub fn infer_repo_format(blob_names: &[&str]) -> &'static str {
    let mut scores = BTreeMap::<&'static str, usize>::new();
    for blob_name in blob_names {
        let path = blob_name.trim_start_matches('/').to_ascii_lowercase();
        let file = path.rsplit('/').next().unwrap_or(&path);

        // npm：scoped/unscoped tarball 布局 `<pkg>/-/<pkg>-<ver>.tgz`，或含 package.json 元数据
        if (path.contains("/-/") && file.ends_with(".tgz")) || path.contains("package.json") {
            *scores.entry("npm").or_default() += FORMAT_HIT_SCORE;
        }
        // maven：maven-metadata.xml 或 g/a/v/file 坐标布局 + 常见构件扩展
        if path.contains("maven-metadata.xml") || is_maven_artifact_path(&path) {
            *scores.entry("maven").or_default() += FORMAT_HIT_SCORE;
        }
        // docker：v2 内容寻址布局（层 blob / manifest / 上传分块）
        if path.contains("blobs/sha256")
            || path.contains("manifests/")
            || path.contains("_uploads/")
        {
            *scores.entry("docker").or_default() += FORMAT_HIT_SCORE;
        }
    }

    // 取最高分格式；`max_by_key` 同分保留**最后**一个，故按优先级升序排列候选
    //（docker < npm < maven），使同分时优先级最高的 maven 胜出，结果确定。
    ["docker", "npm", "maven"]
        .into_iter()
        .max_by_key(|format| scores.get(format).copied().unwrap_or(0))
        .filter(|format| scores.get(format).copied().unwrap_or(0) > 0)
        // 全无命中兜底 raw
        .unwrap_or("raw")
}

/// Maven 常见构件路径：至少 group/artifact/version/file 四段，且文件名以常见 Maven 扩展结尾。
fn is_maven_artifact_path(path: &str) -> bool {
    let segments = path.split('/').collect::<Vec<_>>();
    if segments.len() < 4 {
        return false;
    }
    let file = segments.last().copied().unwrap_or_default();
    [".pom", ".jar", ".war", ".ear", ".module"]
        .iter()
        .any(|ext| file.ends_with(ext))
}

/// 从单个离线仓库摘要（含 blob 明细）构造**推断通道**索引项：格式推断、类型 / 上游留空。
fn inferred_entry(repo: &OfflineRepoSummary) -> RepoIndexEntry {
    let blob_names: Vec<&str> = repo.blobs.iter().map(|b| b.blob_name.as_str()).collect();
    RepoIndexEntry {
        name: repo.repo_name.clone(),
        format: infer_repo_format(&blob_names).to_string(),
        format_source: FormatSource::Inferred,
        // 推断通道无权威配置：类型与上游留空，由管理员补填
        r#type: None,
        blob_count: repo.blob_count,
        total_size: total_size_of(&repo.blobs),
        upstream_url: None,
    }
}

/// 累加一组 blob 预览项的逻辑大小（`size` 缺失按 0 计）。
fn total_size_of(blobs: &[OfflineBlobSummary]) -> u64 {
    blobs.iter().map(|b| b.size.unwrap_or(0)).sum()
}

/// 把离线扫描结果与权威 REST 仓库配置**按仓库名合并**为统一索引（纯函数，便于穷举）。
///
/// 以离线扫描仓库集合为基准（本地实有可迁内容为准）：
/// - 扫描仓库在 REST 配置中命中 → 权威项：`format`（经 [`map_nexus_format`] 归一，未实现格式回退推断）/
///   `type` / `upstream_url` 取 REST，`format_source = Authoritative`；`blob_count` / `total_size` 取扫描。
/// - 扫描仓库在 REST 中缺失 → 走推断项（该仓库无权威配置）。
/// - REST 有而扫描无的仓库不纳入索引（本地无 blob 可迁）。
///
/// 结果按仓库名字典序稳定排序（离线扫描已按字典序，此处保持）。
fn merge_authoritative(
    offline: &[OfflineRepoSummary],
    rest: &[NexusRepoSummary],
) -> Vec<RepoIndexEntry> {
    // REST 配置按仓库名建索引，供离线仓库按名对齐
    let rest_by_name: BTreeMap<&str, &NexusRepoSummary> =
        rest.iter().map(|r| (r.name.as_str(), r)).collect();

    offline
        .iter()
        .map(|repo| match rest_by_name.get(repo.repo_name.as_str()) {
            Some(cfg) => merged_entry(repo, cfg),
            // 扫描有而 REST 无该仓库配置：回退推断，类型 / 上游留空
            None => inferred_entry(repo),
        })
        .collect()
}

/// 由离线扫描项 + 命中的 REST 配置构造**权威通道**索引项。
///
/// 格式经 [`map_nexus_format`] 归一为本系统格式名；REST 报告未实现格式（无法映射）时回退按内容推断，
/// 且来源标「推断」（不冒充权威）。类型与上游取 REST 权威值。
fn merged_entry(repo: &OfflineRepoSummary, cfg: &NexusRepoSummary) -> RepoIndexEntry {
    match map_nexus_format(&cfg.format) {
        Some(format) => RepoIndexEntry {
            name: repo.repo_name.clone(),
            format: format.to_string(),
            format_source: FormatSource::Authoritative,
            r#type: Some(cfg.r#type.clone()),
            blob_count: repo.blob_count,
            total_size: total_size_of(&repo.blobs),
            upstream_url: cfg.upstream_url.clone(),
        },
        // REST 报告本系统未实现的格式：不冒充权威，回退按内容推断（来源仍标「推断」）
        None => inferred_entry(repo),
    }
}

/// 双通道离线仓库索引发现（FR-141，ADR-0041）。
///
/// `offline` 为离线 blob store 扫描结果（含各仓库 blob 明细，由 [`super::enumerate_blob_store`] 得）。
/// `rest` 给定时（`Some((client, base_url, auth_ref))`）尝试**权威通道**：探测源 Nexus REST 可达性——
/// 可达则取权威配置按名合并；**任何** REST 错误（不可达 / 鉴权失败 / 解析失败）记 WARN 后**回退推断通道**，
/// 不使整次索引失败。`rest` 为 `None` 时直接走推断通道。
///
/// 凭据经 `auth_ref` → 环境变量解析（在 [`super::discover_repositories`] 内），绝不入库 / 进日志。
pub async fn discover_repo_index<C: NexusClient>(
    offline: &[OfflineRepoSummary],
    rest: Option<(&C, &str, Option<&str>)>,
) -> Vec<RepoIndexEntry> {
    match rest {
        Some((client, base_url, auth_ref)) => {
            // 可达性探测即调用 discover_repositories：成功=可达走权威合并；任何错误=不可达回退推断
            match super::discover_repositories(client, base_url, auth_ref).await {
                Ok(rest_repos) => {
                    tracing::info!(
                        权威仓库数 = rest_repos.len(),
                        扫描仓库数 = offline.len(),
                        "离线索引：REST 可达，走权威通道合并"
                    );
                    merge_authoritative(offline, &rest_repos)
                }
                Err(e) => {
                    // 不向调用方泄露源系统内部细节，仅记 WARN 后回退纯离线推断
                    tracing::warn!(错误 = %e, "离线索引：REST 探测失败，回退纯离线推断通道");
                    offline.iter().map(inferred_entry).collect()
                }
            }
        }
        None => {
            tracing::info!(
                扫描仓库数 = offline.len(),
                "离线索引：未提供 REST 地址，走纯离线推断通道"
            );
            offline.iter().map(inferred_entry).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::MigrateError;
    use std::sync::Mutex;

    /// 构造一个离线仓库摘要（含指定 blob 名 + 大小）。
    fn repo(name: &str, blobs: &[(&str, u64)]) -> OfflineRepoSummary {
        let blobs: Vec<OfflineBlobSummary> = blobs
            .iter()
            .map(|(bn, size)| OfflineBlobSummary {
                blob_name: bn.to_string(),
                sha1: None,
                size: Some(*size),
            })
            .collect();
        OfflineRepoSummary {
            repo_name: name.to_string(),
            blob_count: blobs.len(),
            blobs,
        }
    }

    // ---- 格式推断纯函数穷举 ----

    #[test]
    fn 推断_maven_坐标布局与元数据() {
        // maven-metadata.xml 命中
        assert_eq!(
            infer_repo_format(&["/org/example/app/maven-metadata.xml"]),
            "maven"
        );
        // g/a/v/file + .jar 命中
        assert_eq!(
            infer_repo_format(&["/com/foo/lib/1.0/lib-1.0.jar"]),
            "maven"
        );
        // .pom 亦命中
        assert_eq!(
            infer_repo_format(&["/com/foo/lib/1.0/lib-1.0.pom"]),
            "maven"
        );
    }

    #[test]
    fn 推断_npm_tarball_与_package_json() {
        // <pkg>/-/<pkg>-<ver>.tgz 命中
        assert_eq!(infer_repo_format(&["/lodash/-/lodash-4.17.21.tgz"]), "npm");
        // scoped 包 tarball
        assert_eq!(infer_repo_format(&["/@babel/core/-/core-7.0.0.tgz"]), "npm");
        // package.json 元数据路径
        assert_eq!(infer_repo_format(&["/lodash/package.json"]), "npm");
    }

    #[test]
    fn 推断_docker_v2_内容寻址布局() {
        assert_eq!(
            infer_repo_format(&["/v2/library/nginx/blobs/sha256:abcd"]),
            "docker"
        );
        assert_eq!(
            infer_repo_format(&["/v2/library/nginx/manifests/latest"]),
            "docker"
        );
        assert_eq!(
            infer_repo_format(&["/v2/library/nginx/_uploads/uuid"]),
            "docker"
        );
    }

    #[test]
    fn 推断_raw_兜底() {
        // 无任何格式特征 → raw
        assert_eq!(infer_repo_format(&["/files/readme.txt"]), "raw");
        assert_eq!(infer_repo_format(&["/data/backup.bin"]), "raw");
        // 空仓库亦兜底 raw
        assert_eq!(infer_repo_format(&[]), "raw");
    }

    #[test]
    fn 推断_多_blob_取最高分格式() {
        // 三个 maven 构件 + 一个杂项 → maven 胜出
        let blobs = [
            "/com/a/x/1.0/x-1.0.jar",
            "/com/a/x/1.0/x-1.0.pom",
            "/com/a/maven-metadata.xml",
            "/misc/note.txt",
        ];
        assert_eq!(infer_repo_format(&blobs), "maven");
    }

    #[test]
    fn 推断_同分时按固定候选顺序确定() {
        // 各一个 maven / npm / docker 命中，同分 → 按 maven > npm > docker 取 maven（结果确定）
        let blobs = [
            "/com/a/x/1.0/x-1.0.jar",
            "/lodash/-/lodash-1.0.0.tgz",
            "/v2/img/manifests/latest",
        ];
        assert_eq!(infer_repo_format(&blobs), "maven");
    }

    // ---- 推断通道索引项 ----

    #[test]
    fn 推断通道_类型与上游留空且标推断来源() {
        let offline = vec![repo(
            "maven-releases",
            &[
                ("/com/a/x/1.0/x-1.0.jar", 100),
                ("/com/a/x/1.0/x-1.0.pom", 20),
            ],
        )];
        let idx = pollster_block(discover_repo_index::<NoopClient>(&offline, None));
        assert_eq!(idx.len(), 1);
        let e = &idx[0];
        assert_eq!(e.name, "maven-releases");
        assert_eq!(e.format, "maven");
        assert_eq!(e.format_source, FormatSource::Inferred);
        // 推断通道：类型与上游留空
        assert_eq!(e.r#type, None);
        assert_eq!(e.upstream_url, None);
        // blob 数与总大小取扫描
        assert_eq!(e.blob_count, 2);
        assert_eq!(e.total_size, 120);
    }

    // ---- 权威通道合并 ----

    #[test]
    fn 权威通道_按名合并配置取rest数量取扫描() {
        let offline = vec![
            repo("maven-releases", &[("/com/a/x/1.0/x-1.0.jar", 100)]),
            repo("npm-proxy", &[("/lodash/-/lodash-1.0.0.tgz", 50)]),
        ];
        let rest = vec![
            NexusRepoSummary {
                name: "maven-releases".to_string(),
                format: "maven2".to_string(),
                r#type: "hosted".to_string(),
                upstream_url: None,
                group_members: vec![],
            },
            NexusRepoSummary {
                name: "npm-proxy".to_string(),
                format: "npm".to_string(),
                r#type: "proxy".to_string(),
                upstream_url: Some("https://registry.npmjs.org".to_string()),
                group_members: vec![],
            },
        ];
        let client = OkClient::new(&rest);
        let idx = pollster_block(discover_repo_index(
            &offline,
            Some((&client, "https://nexus.example", None)),
        ));
        assert_eq!(idx.len(), 2);
        // maven-releases：格式经 maven2→maven 归一、类型 hosted 取 REST、来源权威
        let m = idx.iter().find(|e| e.name == "maven-releases").unwrap();
        assert_eq!(m.format, "maven");
        assert_eq!(m.format_source, FormatSource::Authoritative);
        assert_eq!(m.r#type.as_deref(), Some("hosted"));
        assert_eq!(m.upstream_url, None);
        // blob 数 / 大小取扫描
        assert_eq!(m.blob_count, 1);
        assert_eq!(m.total_size, 100);
        // npm-proxy：上游取 REST
        let n = idx.iter().find(|e| e.name == "npm-proxy").unwrap();
        assert_eq!(n.r#type.as_deref(), Some("proxy"));
        assert_eq!(
            n.upstream_url.as_deref(),
            Some("https://registry.npmjs.org")
        );
        assert_eq!(n.format_source, FormatSource::Authoritative);
    }

    #[test]
    fn 权威通道_扫描有而rest无的仓库回退推断() {
        let offline = vec![
            repo("maven-releases", &[("/com/a/x/1.0/x-1.0.jar", 100)]),
            // REST 未报告该仓库（如新建未同步）
            repo("raw-hosted", &[("/files/a.bin", 10)]),
        ];
        let rest = vec![NexusRepoSummary {
            name: "maven-releases".to_string(),
            format: "maven2".to_string(),
            r#type: "hosted".to_string(),
            upstream_url: None,
            group_members: vec![],
        }];
        let client = OkClient::new(&rest);
        let idx = pollster_block(discover_repo_index(
            &offline,
            Some((&client, "https://nexus.example", None)),
        ));
        let r = idx.iter().find(|e| e.name == "raw-hosted").unwrap();
        // 扫描有 REST 无 → 推断项：类型 / 上游留空、来源推断
        assert_eq!(r.format_source, FormatSource::Inferred);
        assert_eq!(r.format, "raw");
        assert_eq!(r.r#type, None);
    }

    #[test]
    fn 权威通道_rest多出的仓库不纳入索引() {
        let offline = vec![repo("maven-releases", &[("/com/a/x/1.0/x-1.0.jar", 100)])];
        let rest = vec![
            NexusRepoSummary {
                name: "maven-releases".to_string(),
                format: "maven2".to_string(),
                r#type: "hosted".to_string(),
                upstream_url: None,
                group_members: vec![],
            },
            // REST 报告但本地无 blob 的仓库：不纳入（无可迁内容）
            NexusRepoSummary {
                name: "empty-in-local".to_string(),
                format: "npm".to_string(),
                r#type: "hosted".to_string(),
                upstream_url: None,
                group_members: vec![],
            },
        ];
        let client = OkClient::new(&rest);
        let idx = pollster_block(discover_repo_index(
            &offline,
            Some((&client, "https://nexus.example", None)),
        ));
        assert_eq!(idx.len(), 1);
        assert_eq!(idx[0].name, "maven-releases");
    }

    #[test]
    fn 权威通道_未实现格式回退推断不冒充权威() {
        // REST 报告 rubygems（本系统未实现），不冒充权威 → 回退推断、来源标推断
        let offline = vec![repo("gems-proxy", &[("/gems/foo.gem", 10)])];
        let rest = vec![NexusRepoSummary {
            name: "gems-proxy".to_string(),
            format: "rubygems".to_string(),
            r#type: "proxy".to_string(),
            upstream_url: Some("https://rubygems.org".to_string()),
            group_members: vec![],
        }];
        let client = OkClient::new(&rest);
        let idx = pollster_block(discover_repo_index(
            &offline,
            Some((&client, "https://nexus.example", None)),
        ));
        assert_eq!(idx[0].format_source, FormatSource::Inferred);
        // 无 maven/npm/docker 特征 → raw 兜底；上游不取（推断项留空）
        assert_eq!(idx[0].format, "raw");
        assert_eq!(idx[0].upstream_url, None);
    }

    // ---- 不可达回退 ----

    #[test]
    fn 探测不可达时回退纯离线推断不报错() {
        let offline = vec![repo("maven-releases", &[("/com/a/x/1.0/x-1.0.jar", 100)])];
        // 客户端返回传输失败（不可达）
        let client = ErrClient::new(MigrateError::Transport("连接被拒".to_string()));
        let idx = pollster_block(discover_repo_index(
            &offline,
            Some((&client, "https://nexus.example", None)),
        ));
        // 回退推断：不报错、仍产出索引、来源推断
        assert_eq!(idx.len(), 1);
        assert_eq!(idx[0].format_source, FormatSource::Inferred);
        assert_eq!(idx[0].format, "maven");
        assert_eq!(idx[0].r#type, None);
    }

    #[test]
    fn 探测鉴权失败时同样回退推断() {
        let offline = vec![repo("r", &[("/files/a.bin", 1)])];
        let client = ErrClient::new(MigrateError::Status(401));
        let idx = pollster_block(discover_repo_index(
            &offline,
            Some((&client, "https://nexus.example", None)),
        ));
        assert_eq!(idx[0].format_source, FormatSource::Inferred);
    }

    #[test]
    fn 空离线扫描得空索引() {
        let idx = pollster_block(discover_repo_index::<NoopClient>(&[], None));
        assert!(idx.is_empty());
    }

    #[test]
    fn format_source_序列化为小写字符串() {
        assert_eq!(
            serde_json::to_string(&FormatSource::Authoritative).unwrap(),
            "\"authoritative\""
        );
        assert_eq!(
            serde_json::to_string(&FormatSource::Inferred).unwrap(),
            "\"inferred\""
        );
    }

    // ---- 测试用 mock 客户端 ----

    /// 用当前线程 tokio 运行时驱动被测 async（mock 客户端无真实 IO，立即就绪）。
    /// 守 `#![forbid(unsafe_code)]`：不手写执行器，复用 tokio。
    fn pollster_block<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(fut)
    }

    /// 从不被调用的占位客户端（用于纯推断通道 `rest = None` 的用例）。
    struct NoopClient;
    impl NexusClient for NoopClient {
        async fn fetch_repositories(
            &self,
            _base_url: &str,
            _credential: Option<&super::super::NexusCredential>,
        ) -> Result<String, MigrateError> {
            unimplemented!("纯推断通道不调用 REST")
        }
        async fn fetch_components(
            &self,
            _base_url: &str,
            _repository: &str,
            _continuation_token: Option<&str>,
            _credential: Option<&super::super::NexusCredential>,
        ) -> Result<String, MigrateError> {
            unimplemented!()
        }
        async fn download_asset(
            &self,
            _download_url: &str,
            _credential: Option<&super::super::NexusCredential>,
        ) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>, MigrateError> {
            unimplemented!()
        }
    }

    /// 返回预置仓库列表 JSON 的客户端（权威通道用例）。
    struct OkClient {
        body: String,
    }
    impl OkClient {
        fn new(repos: &[NexusRepoSummary]) -> Self {
            // 复用 Nexus REST 响应形态，交由 discover_repositories 内的 parse_repositories 解析
            let items: Vec<serde_json::Value> = repos
                .iter()
                .map(|r| {
                    let mut attrs = serde_json::Map::new();
                    if let Some(url) = &r.upstream_url {
                        attrs.insert("proxy".to_string(), serde_json::json!({ "remoteUrl": url }));
                    }
                    serde_json::json!({
                        "name": r.name,
                        "format": r.format,
                        "type": r.r#type,
                        "attributes": attrs,
                    })
                })
                .collect();
            Self {
                body: serde_json::to_string(&items).unwrap(),
            }
        }
    }
    impl NexusClient for OkClient {
        async fn fetch_repositories(
            &self,
            _base_url: &str,
            _credential: Option<&super::super::NexusCredential>,
        ) -> Result<String, MigrateError> {
            Ok(self.body.clone())
        }
        async fn fetch_components(
            &self,
            _base_url: &str,
            _repository: &str,
            _continuation_token: Option<&str>,
            _credential: Option<&super::super::NexusCredential>,
        ) -> Result<String, MigrateError> {
            unimplemented!()
        }
        async fn download_asset(
            &self,
            _download_url: &str,
            _credential: Option<&super::super::NexusCredential>,
        ) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>, MigrateError> {
            unimplemented!()
        }
    }

    /// 返回预置错误的客户端（不可达 / 鉴权失败回退用例）。
    struct ErrClient {
        err: Mutex<Option<MigrateError>>,
    }
    impl ErrClient {
        fn new(err: MigrateError) -> Self {
            Self {
                err: Mutex::new(Some(err)),
            }
        }
    }
    impl NexusClient for ErrClient {
        async fn fetch_repositories(
            &self,
            _base_url: &str,
            _credential: Option<&super::super::NexusCredential>,
        ) -> Result<String, MigrateError> {
            // 取出预置错误返回（用例仅调用一次）
            Err(self
                .err
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| MigrateError::Transport("已消费".to_string())))
        }
        async fn fetch_components(
            &self,
            _base_url: &str,
            _repository: &str,
            _continuation_token: Option<&str>,
            _credential: Option<&super::super::NexusCredential>,
        ) -> Result<String, MigrateError> {
            unimplemented!()
        }
        async fn download_asset(
            &self,
            _download_url: &str,
            _credential: Option<&super::super::NexusCredential>,
        ) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>, MigrateError> {
            unimplemented!()
        }
    }
}
