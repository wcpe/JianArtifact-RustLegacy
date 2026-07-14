-- 持久化 blob 回收意图；代次用于隔离同一摘要的新旧回收任务。
CREATE TABLE blob_reclamations (
    sha256      TEXT    PRIMARY KEY CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
    generation  INTEGER NOT NULL CHECK (generation > 0),
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);
