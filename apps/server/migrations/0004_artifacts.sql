-- 制品元数据与内容寻址 blob 引用；只建立当前分期实际使用的字段。
CREATE TABLE artifacts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    repo_id     INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    path        TEXT    NOT NULL CHECK (length(CAST(path AS BLOB)) BETWEEN 1 AND 4096),
    sha256      TEXT    NOT NULL CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
    sha1        TEXT    NOT NULL CHECK (length(sha1) = 40 AND sha1 NOT GLOB '*[^0-9a-f]*'),
    md5         TEXT    NOT NULL CHECK (length(md5) = 32 AND md5 NOT GLOB '*[^0-9a-f]*'),
    sha512      TEXT    NOT NULL CHECK (length(sha512) = 128 AND sha512 NOT GLOB '*[^0-9a-f]*'),
    size        INTEGER NOT NULL CHECK (size >= 0),
    cached      INTEGER NOT NULL DEFAULT 0 CHECK (cached IN (0, 1)),
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE UNIQUE INDEX idx_artifacts_repo_path ON artifacts(repo_id, path);
CREATE INDEX idx_artifacts_blob ON artifacts(sha256);
