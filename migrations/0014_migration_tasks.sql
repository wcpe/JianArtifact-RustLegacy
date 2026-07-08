-- 迁移任务与报告持久化（FR-140，ADR-0040）：把迁移任务记录 / 节流进度快照 / 分仓库明细报告
-- 落本地 SQLite，使进程重启后历史 / 进度 / 报告可查、残留在途任务标「已中断」、可一键幂等重跑。
-- 运行态真源仍是进程内 TaskRegistry / MigrationJobs；本表是历史与恢复视图（ADR-0040）。
-- 凭据红线：params_snapshot 仅存 auth_ref 引用与非敏感配置，凭据真值绝不入库。

-- 迁移任务表：一行 = 一个迁移搬运任务（id = job_id），含参数快照 + 节流进度快照 + 终态。
CREATE TABLE migration_tasks (
    id                TEXT PRIMARY KEY,                       -- 任务 id（与进程内 MigrationJobs 的 job_id 一致）
    kind              TEXT NOT NULL,                          -- 迁移类别：online | offline_proxy | offline_hosted
    state             TEXT NOT NULL,                          -- 统一状态：running | paused | succeeded | failed | cancelled | interrupted
    label             TEXT,                                   -- 人类可读标签（如「在线拉取迁移」）
    params_snapshot   TEXT NOT NULL,                          -- 重跑所需非敏感参数 JSON；仅存 auth_ref 引用，绝不含凭据真值
    phase             TEXT NOT NULL,                          -- 引擎进度阶段：enumerating | downloading | paused | cancelled | done | failed
    total_assets      INTEGER NOT NULL DEFAULT 0,             -- 已枚举待搬运 asset 总数
    done_assets       INTEGER NOT NULL DEFAULT 0,             -- 已处理 asset 数
    migrated          INTEGER NOT NULL DEFAULT 0,             -- 成功新写入数
    skipped_existing  INTEGER NOT NULL DEFAULT 0,             -- 增量跳过数（目标已存在且 sha256 一致）
    skipped           INTEGER NOT NULL DEFAULT 0,             -- 失败跳过数
    total_bytes       INTEGER NOT NULL DEFAULT 0,             -- 待搬运逻辑总字节数
    done_bytes        INTEGER NOT NULL DEFAULT 0,             -- 已处理逻辑字节数
    migrated_bytes    INTEGER NOT NULL DEFAULT 0,             -- 成功新写入逻辑字节数
    skipped_bytes     INTEGER NOT NULL DEFAULT 0,             -- 跳过 / 失败逻辑字节数
    error             TEXT,                                   -- 失败原因（state / phase == failed 时）
    started_at        TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, -- 起始时刻（UTC）
    updated_at        TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, -- 最近一次落库时刻（UTC）
    finished_at       TEXT                                    -- 终态时刻（UTC，未结束为空）；与 started_at 之差即耗时
);

-- 历史列表按起始时刻倒序（最新在前）取近期任务。
CREATE INDEX idx_migration_tasks_started ON migration_tasks (started_at DESC);

-- 分仓库明细报告表：一行 = 一个任务下某源仓库的搬运结果明细。
CREATE TABLE migration_task_repos (
    id                          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id                     TEXT NOT NULL,                 -- 所属迁移任务 id
    position                    INTEGER NOT NULL,              -- 报告内稳定序（按引擎产出顺序）
    source_repo                 TEXT NOT NULL,                 -- 源仓库名
    target_repo                 TEXT NOT NULL,                 -- 本系统目标仓库名
    format                      TEXT NOT NULL,                 -- 本系统格式名
    created                     INTEGER NOT NULL DEFAULT 0,    -- 目标仓库是否新建（0/1）
    migrated_artifacts          INTEGER NOT NULL DEFAULT 0,    -- 成功新写入的制品数
    skipped_existing_artifacts  INTEGER NOT NULL DEFAULT 0,    -- 增量跳过数（目标已存在且 sha256 一致）
    skipped_artifacts           INTEGER NOT NULL DEFAULT 0,    -- 失败跳过数
    FOREIGN KEY (task_id) REFERENCES migration_tasks (id) ON DELETE CASCADE
);

-- 明细按任务 id + 序号查回。
CREATE INDEX idx_migration_task_repos_task ON migration_task_repos (task_id, position);
