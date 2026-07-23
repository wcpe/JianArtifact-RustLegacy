-- JWT 当前会话撤销表：仅保存域分隔撤销键的 sha256 哈希与原会话过期时间，不保存原 JWT / jti 明文。
-- 遵 ADR-0031 向前兼容：只新增表与索引，不修改既有认证与 Token 表。
CREATE TABLE revoked_jwt_sessions (
    session_key_hash BLOB PRIMARY KEY,
    expires_at INTEGER NOT NULL
);

-- 过期撤销记录按 expires_at 批量清理。
CREATE INDEX idx_revoked_jwt_sessions_expires_at ON revoked_jwt_sessions (expires_at);
