-- 会话版本号（M1-FR-045 决策：改口令 / 重置口令后撤销该用户所有既有会话）。
-- 迁移只向前追加，禁止改写已发布迁移。

-- users 增加会话版本号：登录签发的 JWT 内嵌此值，改口令 / 重置口令时递增，
-- 使该用户所有既有会话的令牌版本失配、下一次请求即失效。
ALTER TABLE users ADD COLUMN token_version INTEGER NOT NULL DEFAULT 0;
