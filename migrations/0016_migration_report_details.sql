-- 迁移报告明细与分仓库字节指标（FR-145）。
ALTER TABLE migration_tasks ADD COLUMN omitted_asset_failures INTEGER NOT NULL DEFAULT 0;

ALTER TABLE migration_task_repos ADD COLUMN total_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE migration_task_repos ADD COLUMN done_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE migration_task_repos ADD COLUMN migrated_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE migration_task_repos ADD COLUMN skipped_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE migration_task_repos ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT 0;

CREATE TABLE migration_task_asset_failures (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     TEXT NOT NULL,
    position    INTEGER NOT NULL,
    source_repo TEXT NOT NULL,
    asset       TEXT NOT NULL,
    reason      TEXT NOT NULL,
    FOREIGN KEY (task_id) REFERENCES migration_tasks (id) ON DELETE CASCADE
);

CREATE INDEX idx_migration_task_asset_failures_task
    ON migration_task_asset_failures (task_id, position);
