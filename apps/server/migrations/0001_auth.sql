-- 认证与用户管理表（M1-FR-001~006）。迁移只向前追加，禁止改写已发布迁移。

-- 本地用户：口令只存 Argon2 PHC 串，角色为 admin/user，enabled 控制启用状态
CREATE TABLE users (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    username      TEXT    NOT NULL UNIQUE,
    password_hash TEXT    NOT NULL,
    role          TEXT    NOT NULL CHECK (role IN ('admin', 'user')),
    enabled       INTEGER NOT NULL DEFAULT 1,
    created_at    TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- API Token：明文只在签发时显示一次，持久化只存域分隔哈希
CREATE TABLE api_tokens (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    token_hash TEXT    NOT NULL UNIQUE,
    created_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_api_tokens_user ON api_tokens(user_id);

-- 会话撤销表：只存域分隔哈希与过期时间，不存原始会话标识
CREATE TABLE revoked_sessions (
    jti_hash   TEXT NOT NULL PRIMARY KEY,
    expires_at TEXT NOT NULL
);
