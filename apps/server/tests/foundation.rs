//! Wave 1 持久化共享接口集成测试。
#![allow(clippy::unwrap_used)]

use jianartifact_server::artifact::ArtifactService;
use jianartifact_server::persistence::{
    self, ArtifactWriteInput, NpmHostedVersionInput, NpmProxyPackumentInput, NpmProxyTarballInput,
};
use jianartifact_server::storage::BlobStore;

async fn pool() -> (sqlx::SqlitePool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let url = format!("sqlite://{}?mode=rwc", path.display());
    (persistence::init(&url, &path).await.unwrap(), dir)
}

async fn repository(pool: &sqlx::SqlitePool, name: &str, repo_type: &str) -> i64 {
    let overwrite = (repo_type == "hosted").then_some("allow");
    let upstream = (repo_type == "proxy").then_some("https://registry.example.invalid");
    sqlx::query_as::<_, (i64,)>("INSERT INTO repositories (name, format, repo_type, visibility, overwrite_policy, upstream_url) VALUES (?, 'npm', ?, 'private', ?, ?) RETURNING id")
        .bind(name).bind(repo_type).bind(overwrite).bind(upstream)
        .fetch_one(pool).await.unwrap().0
}

async fn service(pool: &sqlx::SqlitePool, dir: &tempfile::TempDir) -> ArtifactService {
    let blobs = BlobStore::new(dir.path().join("blobs"), 1024, 0, 0);
    blobs.initialize().await.unwrap();
    ArtifactService::new(pool.clone(), blobs)
}

async fn hosted_version(
    pool: &sqlx::SqlitePool,
    repo_id: i64,
    package_name: &str,
    version: &str,
    tag: &str,
    marker: char,
) {
    persistence::commit_npm_hosted_version(
        pool,
        &NpmHostedVersionInput {
            artifact: artifact(
                repo_id,
                &format!("{package_name}/-/{package_name}-{version}.tgz"),
                marker,
            ),
            package_name: package_name.to_string(),
            version: version.to_string(),
            manifest_json: format!(r#"{{"name":"{package_name}","version":"{version}"}}"#),
            dist_tags: vec![tag.to_string()],
        },
    )
    .await
    .unwrap();
}

async fn proxy_packument(pool: &sqlx::SqlitePool, repo_id: i64, package_name: &str, version: &str) {
    persistence::upsert_npm_proxy_packument(
        pool,
        &NpmProxyPackumentInput {
            repo_id,
            package_name: package_name.to_string(),
            rewritten_json: format!(r#"{{"name":"{package_name}"}}"#),
            upstream_url_snapshot: "https://registry.example.invalid".to_string(),
            ttl_secs: 1800,
            tarballs: vec![NpmProxyTarballInput {
                version: version.to_string(),
                tarball_url: format!("https://registry.example.invalid/{package_name}.tgz"),
                integrity: Some(format!("sha512-{version}")),
                shasum: None,
                artifact_path: Some(format!("{package_name}/-/{package_name}-{version}.tgz")),
            }],
        },
    )
    .await
    .unwrap();
}

fn artifact(repo_id: i64, path: &str, marker: char) -> ArtifactWriteInput {
    ArtifactWriteInput {
        repo_id,
        path: path.to_string(),
        sha256: marker.to_string().repeat(64),
        sha1: marker.to_string().repeat(40),
        md5: marker.to_string().repeat(32),
        sha512: marker.to_string().repeat(128),
        size: 3,
        cached: false,
    }
}

#[tokio::test]
async fn 前缀与缓存新鲜度接口稳定排序() {
    let (pool, _dir) = pool().await;
    let repo_id = repository(&pool, "prefix", "hosted").await;
    persistence::commit_artifact(&pool, &artifact(repo_id, "g/a/2", 'b'), false)
        .await
        .unwrap();
    persistence::commit_artifact(&pool, &artifact(repo_id, "g/a/1", 'a'), false)
        .await
        .unwrap();
    let rows = persistence::list_artifacts_by_prefix(&pool, repo_id, "g/a/")
        .await
        .unwrap();
    let status = persistence::find_artifact_cache_status(&pool, repo_id, "g/a/1", 1800)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rows.iter().map(|row| row.path.as_str()).collect::<Vec<_>>(),
        vec!["g/a/1", "g/a/2"]
    );
    assert!(status.fresh);
}

#[tokio::test]
async fn npm_hosted提交冲突且proxy缓存原子替换() {
    let (pool, _dir) = pool().await;
    let hosted = repository(&pool, "hosted", "hosted").await;
    let input = NpmHostedVersionInput {
        artifact: artifact(hosted, "pkg/-/pkg-1.0.0.tgz", 'a'),
        package_name: "pkg".to_string(),
        version: "1.0.0".to_string(),
        manifest_json: r#"{"name":"pkg","version":"1.0.0"}"#.to_string(),
        dist_tags: vec!["latest".to_string()],
    };
    persistence::commit_npm_hosted_version(&pool, &input)
        .await
        .unwrap();
    let conflict = persistence::commit_npm_hosted_version(&pool, &input)
        .await
        .unwrap_err();
    assert!(matches!(
        conflict,
        persistence::NpmVersionCommitError::Conflict
    ));
    let package = persistence::find_npm_package(&pool, hosted, "pkg")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(package.dist_tags.get("latest"), Some(&"1.0.0".to_string()));

    let proxy = repository(&pool, "proxy", "proxy").await;
    persistence::upsert_npm_proxy_packument(
        &pool,
        &NpmProxyPackumentInput {
            repo_id: proxy,
            package_name: "pkg".to_string(),
            rewritten_json: r#"{"name":"pkg"}"#.to_string(),
            upstream_url_snapshot: "https://registry.example.invalid".to_string(),
            ttl_secs: 1800,
            tarballs: vec![NpmProxyTarballInput {
                version: "1.0.0".to_string(),
                tarball_url: "https://registry.example.invalid/pkg.tgz".to_string(),
                integrity: Some("sha512-test".to_string()),
                shasum: Some("a".repeat(40)),
                artifact_path: Some("pkg/-/pkg.tgz".to_string()),
            }],
        },
    )
    .await
    .unwrap();
    let cached = persistence::find_npm_proxy_packument(&pool, proxy, "pkg")
        .await
        .unwrap()
        .unwrap();
    assert!(cached.fresh);
    assert_eq!(cached.tarballs.len(), 1);
}

#[tokio::test]
async fn 制品批量读取覆盖顺序空输入大批量与缺失仓库() {
    let (pool, dir) = pool().await;
    let first = repository(&pool, "artifact-first", "hosted").await;
    let second = repository(&pool, "artifact-second", "hosted").await;
    persistence::commit_artifact(&pool, &artifact(first, "g/a/1", 'a'), false)
        .await
        .unwrap();
    persistence::commit_artifact(&pool, &artifact(second, "g/a/1", 'b'), false)
        .await
        .unwrap();
    persistence::commit_artifact(&pool, &artifact(second, "g/a/2", 'c'), false)
        .await
        .unwrap();

    let direct = persistence::find_artifacts_by_path(&pool, &[second, 99_999, first], "g/a/1")
        .await
        .unwrap();
    assert_eq!(
        direct.keys().copied().collect::<Vec<_>>(),
        vec![first, second]
    );
    assert_eq!(direct[&first].sha256, "a".repeat(64));
    assert_eq!(direct[&second].sha256, "b".repeat(64));
    assert!(
        persistence::find_artifacts_by_path(&pool, &[], "g/a/1")
            .await
            .unwrap()
            .is_empty()
    );

    let service = service(&pool, &dir).await;
    let prefixed = service
        .list_by_prefix_batch(&[second, 99_999, first], "g/a/")
        .await
        .unwrap();
    assert_eq!(
        prefixed[&second]
            .iter()
            .map(|record| record.path.as_str())
            .collect::<Vec<_>>(),
        vec!["g/a/1", "g/a/2"]
    );
    assert_eq!(prefixed[&first][0].path, "g/a/1");
    assert!(
        service
            .list_by_prefix_batch(&[], "g/a/")
            .await
            .unwrap()
            .is_empty()
    );

    let mut large_ids: Vec<i64> = (100_000..116_385).collect();
    large_ids.push(second);
    let large = service
        .find_by_path_batch(&large_ids, "g/a/1")
        .await
        .unwrap();
    assert_eq!(large.len(), 1);
    assert_eq!(large[&second].path, "g/a/1");
}

#[tokio::test]
async fn npm批量读取固定查询并覆盖空输入大批量与缺失仓库() {
    let (pool, dir) = pool().await;
    let first = repository(&pool, "npm-first", "hosted").await;
    let second = repository(&pool, "npm-second", "hosted").await;
    let proxy_first = repository(&pool, "proxy-first", "proxy").await;
    let proxy_second = repository(&pool, "proxy-second", "proxy").await;
    hosted_version(&pool, first, "pkg", "1.0.0", "latest", 'a').await;
    hosted_version(&pool, second, "pkg", "2.0.0", "next", 'b').await;
    proxy_packument(&pool, proxy_first, "pkg", "3.0.0").await;
    proxy_packument(&pool, proxy_second, "pkg", "4.0.0").await;

    let hosted = persistence::find_npm_packages(&pool, &[second, 99_999, first], "pkg")
        .await
        .unwrap();
    assert_eq!(
        hosted.keys().copied().collect::<Vec<_>>(),
        vec![first, second]
    );
    assert_eq!(hosted[&first].versions[0].version, "1.0.0");
    assert_eq!(hosted[&second].dist_tags["next"], "2.0.0");
    assert!(
        persistence::find_npm_packages(&pool, &[], "pkg")
            .await
            .unwrap()
            .is_empty()
    );

    let service = service(&pool, &dir).await;
    let proxy = service
        .npm_proxy_packuments_batch(&[proxy_second, 99_999, proxy_first], "pkg")
        .await
        .unwrap();
    assert_eq!(
        proxy.keys().copied().collect::<Vec<_>>(),
        vec![proxy_first, proxy_second]
    );
    assert_eq!(proxy[&proxy_first].tarballs[0].version, "3.0.0");
    assert_eq!(proxy[&proxy_second].tarballs[0].version, "4.0.0");
    assert!(
        service
            .npm_proxy_packuments_batch(&[], "pkg")
            .await
            .unwrap()
            .is_empty()
    );

    let mut large_ids: Vec<i64> = (100_000..116_385).collect();
    large_ids.extend([first, proxy_first]);
    assert_eq!(
        service
            .npm_packages_batch(&large_ids, "pkg")
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        service
            .npm_proxy_packuments_batch(&large_ids, "pkg")
            .await
            .unwrap()
            .len(),
        1
    );
}
