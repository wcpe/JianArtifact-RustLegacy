-- 仓库核心元数据（M1-FR-007~009）：仓库、用户 ACL 与有序 group 成员关系。
-- 本批次只保存仓库配置，不建立制品索引或 blob 引用。

CREATE TABLE repositories (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    name                    TEXT    NOT NULL UNIQUE
                                    CHECK (length(name) BETWEEN 1 AND 64)
                                    CHECK (name NOT GLOB '*[^a-z0-9-]*'),
    format                  TEXT    NOT NULL CHECK (format IN ('raw', 'maven', 'npm')),
    repo_type               TEXT    NOT NULL CHECK (repo_type IN ('hosted', 'proxy', 'group')),
    visibility              TEXT    NOT NULL CHECK (visibility IN ('public', 'private')),
    disabled                INTEGER NOT NULL DEFAULT 0 CHECK (disabled IN (0, 1)),
    overwrite_policy        TEXT             CHECK (overwrite_policy IN ('allow', 'deny')),
    version_policy          TEXT             CHECK (version_policy IN ('release', 'snapshot', 'mixed')),
    upstream_url            TEXT,
    upstream_credential_ref TEXT,
    created_at              TEXT    NOT NULL DEFAULT (datetime('now')),
    CHECK (
        (
            repo_type = 'hosted'
            AND overwrite_policy IS NOT NULL
            AND upstream_url IS NULL
            AND upstream_credential_ref IS NULL
            AND (
                (format = 'maven' AND version_policy IS NOT NULL)
                OR (format <> 'maven' AND version_policy IS NULL)
            )
        )
        OR (
            repo_type = 'proxy'
            AND overwrite_policy IS NULL
            AND version_policy IS NULL
            AND upstream_url IS NOT NULL
            AND length(trim(upstream_url)) > 0
            AND (
                upstream_credential_ref IS NULL
                OR length(trim(upstream_credential_ref)) > 0
            )
        )
        OR (
            repo_type = 'group'
            AND overwrite_policy IS NULL
            AND version_policy IS NULL
            AND upstream_url IS NULL
            AND upstream_credential_ref IS NULL
        )
    )
);

CREATE TABLE repository_acl (
    repo_id     INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    permission  TEXT    NOT NULL CHECK (permission IN ('read', 'write', 'admin')),
    PRIMARY KEY (repo_id, user_id)
);

CREATE INDEX idx_repository_acl_user ON repository_acl(user_id, repo_id);

CREATE TABLE repository_group_members (
    group_repo_id  INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    member_repo_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE RESTRICT,
    position       INTEGER NOT NULL CHECK (position >= 0),
    PRIMARY KEY (group_repo_id, position),
    UNIQUE (group_repo_id, member_repo_id),
    CHECK (group_repo_id <> member_repo_id)
);

CREATE INDEX idx_repository_group_member ON repository_group_members(member_repo_id);

-- group 关系由数据库兜底校验，避免绕过服务层的直接 SQL 写入破坏不变量。
CREATE TRIGGER validate_repository_group_member_insert
BEFORE INSERT ON repository_group_members
BEGIN
    SELECT RAISE(ABORT, 'group_repo 必须引用 group 仓库')
    WHERE (SELECT repo_type FROM repositories WHERE id = NEW.group_repo_id) <> 'group';

    SELECT RAISE(ABORT, 'group 成员不能引用 group 仓库')
    WHERE (SELECT repo_type FROM repositories WHERE id = NEW.member_repo_id) = 'group';

    SELECT RAISE(ABORT, 'group 与成员的格式必须一致')
    WHERE (SELECT format FROM repositories WHERE id = NEW.group_repo_id)
        <> (SELECT format FROM repositories WHERE id = NEW.member_repo_id);
END;

CREATE TRIGGER validate_repository_group_member_update
BEFORE UPDATE OF group_repo_id, member_repo_id ON repository_group_members
BEGIN
    SELECT RAISE(ABORT, 'group_repo 必须引用 group 仓库')
    WHERE (SELECT repo_type FROM repositories WHERE id = NEW.group_repo_id) <> 'group';

    SELECT RAISE(ABORT, 'group 成员不能引用 group 仓库')
    WHERE (SELECT repo_type FROM repositories WHERE id = NEW.member_repo_id) = 'group';

    SELECT RAISE(ABORT, 'group 与成员的格式必须一致')
    WHERE (SELECT format FROM repositories WHERE id = NEW.group_repo_id)
        <> (SELECT format FROM repositories WHERE id = NEW.member_repo_id);
END;

-- 格式与仓库类型属于创建后不可变的公开契约。
CREATE TRIGGER prevent_repository_contract_update
BEFORE UPDATE OF format, repo_type ON repositories
WHEN NEW.format <> OLD.format OR NEW.repo_type <> OLD.repo_type
BEGIN
    SELECT RAISE(ABORT, '仓库格式与类型不可更新');
END;
