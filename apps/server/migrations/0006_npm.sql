-- npm 最小元数据：hosted 版本、dist-tag 与 proxy 重写 packument 缓存。
CREATE TABLE npm_versions (
    repo_id        INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    package_name   TEXT    NOT NULL CHECK (length(package_name) BETWEEN 1 AND 214),
    version         TEXT    NOT NULL CHECK (length(version) BETWEEN 1 AND 128),
    manifest_json   TEXT    NOT NULL CHECK (json_valid(manifest_json)),
    tarball_path    TEXT    NOT NULL CHECK (length(CAST(tarball_path AS BLOB)) BETWEEN 1 AND 4096),
    artifact_id     INTEGER NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
    sha256          TEXT    NOT NULL CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
    sha1            TEXT    NOT NULL CHECK (length(sha1) = 40 AND sha1 NOT GLOB '*[^0-9a-f]*'),
    md5             TEXT    NOT NULL CHECK (length(md5) = 32 AND md5 NOT GLOB '*[^0-9a-f]*'),
    sha512          TEXT    NOT NULL CHECK (length(sha512) = 128 AND sha512 NOT GLOB '*[^0-9a-f]*'),
    created_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (repo_id, package_name, version),
    UNIQUE (repo_id, tarball_path),
    UNIQUE (artifact_id)
);

CREATE INDEX idx_npm_versions_package ON npm_versions(repo_id, package_name, version);
CREATE INDEX idx_npm_versions_artifact ON npm_versions(artifact_id);

CREATE TABLE npm_dist_tags (
    repo_id       INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    package_name  TEXT    NOT NULL CHECK (length(package_name) BETWEEN 1 AND 214),
    tag            TEXT    NOT NULL CHECK (length(tag) BETWEEN 1 AND 128),
    version        TEXT    NOT NULL CHECK (length(version) BETWEEN 1 AND 128),
    updated_at     TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (repo_id, package_name, tag),
    FOREIGN KEY (repo_id, package_name, version)
        REFERENCES npm_versions(repo_id, package_name, version) ON DELETE CASCADE
);

CREATE INDEX idx_npm_dist_tags_version ON npm_dist_tags(repo_id, package_name, version);

CREATE TABLE npm_proxy_packuments (
    repo_id                INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    package_name           TEXT    NOT NULL CHECK (length(package_name) BETWEEN 1 AND 214),
    rewritten_json         TEXT    NOT NULL CHECK (json_valid(rewritten_json)),
    upstream_url_snapshot  TEXT    NOT NULL CHECK (length(trim(upstream_url_snapshot)) > 0),
    ttl_secs               INTEGER NOT NULL CHECK (ttl_secs > 0),
    fetched_at             TEXT    NOT NULL DEFAULT (datetime('now')),
    expires_at             TEXT    NOT NULL,
    PRIMARY KEY (repo_id, package_name)
);

CREATE INDEX idx_npm_proxy_packuments_expiry ON npm_proxy_packuments(repo_id, expires_at);

CREATE TABLE npm_proxy_tarballs (
    repo_id        INTEGER NOT NULL,
    package_name   TEXT    NOT NULL,
    version         TEXT    NOT NULL CHECK (length(version) BETWEEN 1 AND 128),
    tarball_url     TEXT    NOT NULL CHECK (length(trim(tarball_url)) > 0),
    integrity       TEXT,
    shasum          TEXT             CHECK (shasum IS NULL OR (length(shasum) = 40 AND shasum NOT GLOB '*[^0-9a-f]*')),
    artifact_path   TEXT             CHECK (artifact_path IS NULL OR length(CAST(artifact_path AS BLOB)) BETWEEN 1 AND 4096),
    PRIMARY KEY (repo_id, package_name, version),
    FOREIGN KEY (repo_id, package_name)
        REFERENCES npm_proxy_packuments(repo_id, package_name) ON DELETE CASCADE
);

CREATE INDEX idx_npm_proxy_tarballs_path ON npm_proxy_tarballs(repo_id, artifact_path);
