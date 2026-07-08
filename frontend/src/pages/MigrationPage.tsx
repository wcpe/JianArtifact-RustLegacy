// Nexus 迁移向导（FR-142，重构 FR-81/82；消费 FR-140 持久化 + FR-141 索引双通道，仅 Admin）。
//
// 统一向导（Mantine Stepper）：
//   ① 源配置：在线填源地址 / 离线填本地路径（离线可选填源地址走权威通道）→ 加载仓库索引 / 预览。
//   ② 勾选仓库：多选、目标改名（在线）、推断项补填类型与上游（离线 FR-141）。
//   ③ 确认执行计划：核对源→目标 / 方式 / 类型 / 上游后开始迁移。
//   ④ 执行与报告：轮询进度队列（暂停 / 继续 / 取消），终态展示分仓库报告。
//
// 执行中刷新 / 离开回来：经 URL job_id / 本地存档 / 服务端活跃任务恢复到进度视图。
// 完成后历史报告可回看：消费 GET /migrate/tasks（含 interrupted）与详情，支持一键幂等重跑。
//
// 凭据脱敏（红线）：源 Nexus 凭据仅以「引用名 auth_ref」形式输入，用口令型输入框承载、
// 真值在后端 env 解析，前端绝不回显明文、不持久化。

import { useCallback, useEffect, useRef, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import {
  Stack,
  Title,
  Text,
  Stepper,
  SegmentedControl,
  TextInput,
  PasswordInput,
  Select,
  Button,
  Group,
  Table,
  Checkbox,
  Badge,
  Card,
  Loader,
  Center,
  Alert,
  Progress,
} from '@mantine/core';
import * as api from '../api/endpoints';
import { ApiError } from '../api/client';
import type {
  FormatSource,
  NexusRepoSummary,
  OnlinePullJob,
  RepoIndexEntry,
  TaskDetailDto,
  TaskRecord,
} from '../api/types';
import { errorMessage, formatBytes } from '../lib/format';
import { ErrorAlert } from '../components/ErrorAlert';
import { notifySuccess } from '../lib/notify';
import { MigrationHistoryPanel } from './MigrationHistory';

/** 迁移形态：在线 REST 入口 / 离线 blob store 入口。 */
type SourceMode = 'online' | 'offline';

/** 离线目录搬运目标类型：proxy 仓库 / hosted 仓库。 */
type MigrateKind = 'proxy' | 'hosted';

/**
 * 归一后的可选择仓库行（在线预览与离线索引统一为一种结构，供勾选 / 计划 / 搬运消费）。
 */
interface PreviewRow {
  /** 仓库名（在线取 name，离线取索引 name）。 */
  name: string;
  /** 本系统格式名（maven / npm / docker / raw…）。 */
  format: string;
  /** 格式来源（FR-141；在线一律 authoritative）。 */
  formatSource: FormatSource;
  /** 仓库类型（hosted / proxy）；离线推断项为 null，待补填。 */
  type: MigrateKind | null;
  /** proxy 上游地址（可空）。 */
  upstreamUrl: string | null;
  /** 离线 blob 数（在线为 null）。 */
  blobCount: number | null;
  /** 离线总大小字节（在线为 null）。 */
  totalSize: number | null;
}

/** 把在线预览结果归一为展示行（在线一律权威、类型取 Nexus 原值）。 */
function fromOnline(repos: NexusRepoSummary[]): PreviewRow[] {
  return repos.map((r) => ({
    name: r.name,
    format: r.format,
    formatSource: 'authoritative',
    type: r.type === 'proxy' ? 'proxy' : r.type === 'hosted' ? 'hosted' : null,
    upstreamUrl: r.upstream_url,
    blobCount: null,
    totalSize: null,
  }));
}

/** 把离线索引双通道结果归一为展示行（FR-141）。 */
function fromIndex(entries: RepoIndexEntry[]): PreviewRow[] {
  return entries.map((e) => ({
    name: e.name,
    format: e.format,
    formatSource: e.format_source,
    type: e.type === 'proxy' ? 'proxy' : e.type === 'hosted' ? 'hosted' : null,
    upstreamUrl: e.upstream_url,
    blobCount: e.blob_count,
    totalSize: e.total_size,
  }));
}

/** 在线拉取任务轮询间隔（毫秒）。 */
const ONLINE_POLL_INTERVAL_MS = 1500;

/** 在线拉取任务 job_id 的 localStorage 键名（供客户端重连续看）。 */
const ONLINE_JOB_STORAGE_KEY = 'jian.migrate.online.jobId';

/** 任务是否处于终态（done / failed / cancelled）：终态即停止轮询并展示结果。 */
function isTerminalPhase(phase: OnlinePullJob['phase']): boolean {
  return phase === 'done' || phase === 'failed' || phase === 'cancelled';
}

/** 统一任务注册表中的离线枚举任务（预览 / 索引）：对外 kind 仍是 migration，但不参与搬运单飞。 */
function isOfflineEnumTask(task: TaskRecord): boolean {
  return task.kind === 'migration' && (task.label === '离线预览' || task.label === '离线仓库索引');
}

/** 统一任务注册表中仍在途的迁移搬运任务：运行中或暂停中均需前端单飞。 */
function isActiveMigrationTask(task: TaskRecord): boolean {
  return (
    task.kind === 'migration' &&
    !isOfflineEnumTask(task) &&
    (task.state === 'running' || task.state === 'paused')
  );
}

/** 查询统一任务注册表，返回首个仍在途迁移任务。 */
async function findActiveMigrationTask(): Promise<TaskRecord | null> {
  const tasks = await api.listTasks();
  return tasks.find(isActiveMigrationTask) ?? null;
}

/** 统一任务详情中的迁移进度不带 job_id，这里补回同一任务 id 供进度面板控制使用。 */
function migrationJobFromTask(detail: TaskDetailDto): OnlinePullJob | null {
  if (detail.kind !== 'migration' || detail.migration === undefined) {
    return null;
  }
  return { ...detail.migration, job_id: detail.id };
}

/** 后端启动任务响应必须带有效 job_id，缺失时阻止继续轮询 undefined。 */
function requireJobId(resp: { job_id?: unknown }, message: string): string {
  const jobId = typeof resp.job_id === 'string' ? resp.job_id.trim() : '';
  if (!jobId) {
    throw new Error(message);
  }
  return jobId;
}

/** 从统一任务详情读取迁移快照；任务详情缺少迁移明细时返回空，交由调用方兜底。 */
async function loadTaskMigration(jobId: string): Promise<OnlinePullJob | null> {
  const detail = await api.getTask(jobId);
  return migrationJobFromTask(detail);
}

/** 统一读取迁移进度：优先任务中心详情，缺失时兜底专用迁移快照。 */
async function loadMigrationProgressJob(jobId: string): Promise<OnlinePullJob> {
  try {
    const job = await loadTaskMigration(jobId);
    if (job !== null) {
      return job;
    }
  } catch {
    // 任务中心在旧服务端或瞬时故障下可能不可用，继续尝试迁移专用端点。
  }
  return api.getMigrationJob(jobId);
}

/**
 * 轮询离线枚举任务（预览 / 索引）直到终态。返回终态快照；超时返回 null。
 * 间隔 800ms、上限约 5 分钟（上万 blob 枚举可能较久）。
 */
async function pollEnumJob(jobId: string): Promise<OnlinePullJob | null> {
  for (let i = 0; i < 375; i++) {
    const job = await loadMigrationProgressJob(jobId);
    if (isTerminalPhase(job.phase)) {
      return job;
    }
    await new Promise((resolve) => setTimeout(resolve, 800));
  }
  return null;
}

/** 阶段中文标签（t 由调用方组件传入，模块级函数自身无法使用 hook）。 */
function phaseLabel(phase: OnlinePullJob['phase'], t: TFunction): string {
  switch (phase) {
    case 'enumerating':
      return t('phase.enumerating');
    case 'downloading':
      return t('phase.downloading');
    case 'paused':
      return t('phase.paused');
    case 'cancelled':
      return t('phase.cancelled');
    case 'done':
      return t('phase.done');
    case 'failed':
      return t('phase.failed');
  }
}

/** 阶段对应的 Badge 颜色。 */
function phaseColor(phase: OnlinePullJob['phase']): string {
  switch (phase) {
    case 'failed':
      return 'red';
    case 'done':
      return 'green';
    case 'paused':
      return 'yellow';
    case 'cancelled':
      return 'gray';
    default:
      return 'blue';
  }
}

/** 汇总仓库明细中的已存在跳过数，兼容旧后端未返回批量字段的快照。 */
function skippedExistingTotal(job: OnlinePullJob): number {
  if (job.skipped_existing !== undefined) {
    return job.skipped_existing;
  }
  const repoTotal = job.repos.reduce(
    (sum, repo) => sum + (repo.skipped_existing_artifacts ?? 0),
    0,
  );
  if (repoTotal > 0) {
    return repoTotal;
  }
  return Math.max(0, job.done_assets - job.migrated - job.skipped);
}

/** 生成终态报告的关键计数：总数 / 新搬 / 已存在跳过 / 失败跳过。 */
function migrationSummary(job: OnlinePullJob) {
  return {
    total: job.total_assets,
    migrated: job.migrated,
    skippedExisting: skippedExistingTotal(job),
    skippedFailed: job.skipped,
  };
}

/** Nexus 迁移向导页。 */
export function MigrationPage() {
  const { t } = useTranslation('migration');
  const [searchParams] = useSearchParams();
  const requestedJobId = searchParams.get('job_id');
  const [active, setActive] = useState(0);

  // —— 源配置（步骤 ①）——
  const [mode, setMode] = useState<SourceMode>('online');
  const [baseUrl, setBaseUrl] = useState('');
  const [authRef, setAuthRef] = useState('');
  const [offlinePath, setOfflinePath] = useState('');
  // 离线入口的可选源地址 / 凭据引用（走权威通道，FR-141）。
  const [offlineBaseUrl, setOfflineBaseUrl] = useState('');
  const [offlineAuthRef, setOfflineAuthRef] = useState('');

  // —— 预览 / 索引结果（步骤 ①→②）——
  const [rows, setRows] = useState<PreviewRow[]>([]);
  const [previewing, setPreviewing] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);

  // —— 勾选与补填（步骤 ②）——
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [migratePath, setMigratePath] = useState('');
  // 在线拉取：源仓库名 → 目标仓库名改名映射（空 / 缺省即与源同名）。
  const [renames, setRenames] = useState<Record<string, string>>({});
  // 离线推断项补填：仓库名 → 类型（proxy / hosted）。
  const [typeFills, setTypeFills] = useState<Record<string, MigrateKind>>({});
  // 离线推断项补填：仓库名 → 上游 URL（仅展示 / 运维参考，不下发）。
  const [upstreamFills, setUpstreamFills] = useState<Record<string, string>>({});
  const [migrating, setMigrating] = useState(false);
  const [migrateError, setMigrateError] = useState<string | null>(null);

  // —— 报告（步骤 ④）——
  // 迁移任务的当前进度快照（轮询刷新；null 表示尚无任务）。
  const [onlineJob, setOnlineJob] = useState<OnlinePullJob | null>(null);
  // 正在轮询的任务 id（非 null 即开启轮询；终态或清理时置 null）。
  const [pollingJobId, setPollingJobId] = useState<string | null>(null);
  // 已拿到 job_id、但首次进度快照尚未返回时，用占位卡避免报告页空态。
  const [pendingJobId, setPendingJobId] = useState<string | null>(null);
  // 任务标题，用于区分在线拉取 / 离线 proxy / 离线 hosted。
  const [jobTitle, setJobTitle] = useState<string | null>(null);

  /** 某仓库的有效类型：权威 / 在线取行内类型，推断项取补填。 */
  const effectiveType = useCallback(
    (row: PreviewRow): MigrateKind | null => row.type ?? typeFills[row.name] ?? null,
    [typeFills],
  );

  /** 记录并进入某个迁移任务的续看状态，等待轮询填充真实快照。 */
  const watchMigrationJob = (jobId: string, title?: string) => {
    localStorage.setItem(ONLINE_JOB_STORAGE_KEY, jobId);
    setOnlineJob(null);
    setJobTitle(title ?? null);
    setPendingJobId(jobId);
    setPollingJobId(jobId);
    setActive(3);
  };

  /** 发起新迁移前做前端单飞检查，避免已有迁移在途时继续 POST。 */
  const canStartMigration = async () => {
    let activeTask: TaskRecord | null = null;
    try {
      activeTask = await findActiveMigrationTask();
    } catch (err) {
      if (!(err instanceof ApiError && err.status === 404)) {
        throw err;
      }
    }
    if (!activeTask) {
      return true;
    }
    setMigrateError(t('select.activeMigrationBlocked'));
    return false;
  };

  /** 切换源入口时只切换入口本身，清空跨入口的选择与补填，避免在线与离线串台。 */
  const handleModeChange = (nextMode: SourceMode) => {
    setMode(nextMode);
    setRows([]);
    setSelected(new Set());
    setRenames({});
    setTypeFills({});
    setUpstreamFills({});
    setPreviewError(null);
    setMigrateError(null);
  };

  /** 执行预览（在线）/ 加载索引（离线双通道）：据形态调用对应端点，归一展示行。 */
  const handleLoad = async () => {
    setPreviewError(null);
    setPreviewing(true);
    try {
      if (mode === 'online') {
        const repos = await api.previewNexusRepositories({
          base_url: baseUrl.trim(),
          auth_ref: authRef.trim() === '' ? undefined : authRef.trim(),
        });
        setRows(fromOnline(repos));
      } else {
        // FR-141：离线索引双通道异步化——发起任务后轮询，读 repo_index 回填。
        const created = await api.indexNexusOffline({
          path: offlinePath.trim(),
          base_url: offlineBaseUrl.trim() === '' ? undefined : offlineBaseUrl.trim(),
          auth_ref: offlineAuthRef.trim() === '' ? undefined : offlineAuthRef.trim(),
        });
        const jobId = requireJobId(created, t('job.missingJobId'));
        const job = await pollEnumJob(jobId);
        if (!job) {
          throw new Error(t('preview.indexTimeout'));
        }
        if (job.phase === 'failed') {
          throw new Error(job.error ?? t('preview.indexFailed'));
        }
        setRows(fromIndex(job.repo_index ?? []));
        if (migratePath.trim() === '') {
          setMigratePath(offlinePath.trim());
        }
      }
      setSelected(new Set());
      setRenames({});
      setTypeFills({});
      setUpstreamFills({});
    } catch (err) {
      setPreviewError(errorMessage(err));
    } finally {
      setPreviewing(false);
    }
  };

  /** 切换单个仓库勾选。 */
  const toggleSelect = (name: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(name)) {
        next.delete(name);
      } else {
        next.add(name);
      }
      return next;
    });
  };

  /** 设置某源仓库的目标改名。 */
  const setRename = (source: string, target: string) => {
    setRenames((prev) => ({ ...prev, [source]: target }));
  };

  /** 设置某推断仓库补填的类型。 */
  const setTypeFill = (name: string, type: MigrateKind) => {
    setTypeFills((prev) => ({ ...prev, [name]: type }));
  };

  /** 设置某推断仓库补填的上游。 */
  const setUpstreamFill = (name: string, url: string) => {
    setUpstreamFills((prev) => ({ ...prev, [name]: url }));
  };

  /** 勾选的推断项是否仍有未补填类型的：有则不允许进入执行计划。 */
  const hasUnfilledInferred = rows.some(
    (row) =>
      selected.has(row.name) && row.formatSource === 'inferred' && effectiveType(row) === null,
  );

  /** 发起离线搬运（FR-125 异步化）：按类型调用 proxy / hosted 端点。 */
  const startOfflineMigration = async (kind: MigrateKind, repos: string[]) => {
    const req = { offline_path: migratePath.trim(), repositories: repos };
    const created =
      kind === 'proxy' ? await api.migrateNexusProxy(req) : await api.migrateNexusHosted(req);
    const jobId = requireJobId(created, t('job.missingJobId'));
    watchMigrationJob(jobId, kind === 'proxy' ? '离线 proxy 搬运' : '离线 hosted 搬运');
    notifySuccess(t('toast.offlineStarted'));
  };

  /** 发起在线拉取搬运。 */
  const startOnlineMigration = async (repos: string[]) => {
    const repositories = repos.map((name) => {
      const target = (renames[name] ?? '').trim();
      return target === '' ? { source: name } : { source: name, target };
    });
    const created = await api.migrateNexusOnline({
      base_url: baseUrl.trim(),
      auth_ref: authRef.trim() === '' ? undefined : authRef.trim(),
      repositories,
    });
    const jobId = requireJobId(created, t('job.missingJobId'));
    watchMigrationJob(jobId, t('job.queueTitle'));
    notifySuccess(t('toast.onlineStarted'));
  };

  /**
   * 从执行计划确认发起迁移：单飞检查通过后按入口 / 类型分派。
   * 离线混合 proxy / hosted 时，因迁移单飞只允许一个在途，本批次优先发起 proxy 一类；
   * hosted 待前一批完成后由用户在历史 / 计划再发（P1 单飞取舍）。
   */
  const handleStartMigration = async () => {
    setMigrateError(null);
    setMigrating(true);
    try {
      if (!(await canStartMigration())) {
        return;
      }
      const chosen = rows.filter((row) => selected.has(row.name));
      if (mode === 'online') {
        await startOnlineMigration(chosen.map((row) => row.name));
        return;
      }
      const proxyRepos = chosen
        .filter((row) => effectiveType(row) === 'proxy')
        .map((row) => row.name);
      const hostedRepos = chosen
        .filter((row) => effectiveType(row) === 'hosted')
        .map((row) => row.name);
      if (proxyRepos.length > 0) {
        await startOfflineMigration('proxy', proxyRepos);
      } else if (hostedRepos.length > 0) {
        await startOfflineMigration('hosted', hostedRepos);
      }
    } catch (err) {
      setMigrateError(errorMessage(err));
    } finally {
      setMigrating(false);
    }
  };

  // 拉取一次任务进度快照：写入状态；终态则停止轮询并清存档；404 视为存档失效，清理。
  const fetchJobRef = useRef<(id: string) => Promise<void>>(async () => {});
  fetchJobRef.current = async (id: string) => {
    try {
      const job = await loadMigrationProgressJob(id);
      setOnlineJob(job);
      setPendingJobId(null);
      if (isTerminalPhase(job.phase)) {
        setPollingJobId(null);
        localStorage.removeItem(ONLINE_JOB_STORAGE_KEY);
      }
    } catch (err) {
      // 未知 job_id（任务已过期 / 被清理）：停止轮询并清存档，不再续看。
      if (err instanceof ApiError && err.status === 404) {
        setPollingJobId(null);
        setOnlineJob(null);
        setPendingJobId(null);
        localStorage.removeItem(ONLINE_JOB_STORAGE_KEY);
        return;
      }
      // 其他瞬时错误（如网络抖动）：保留轮询，下个周期重试。
      setMigrateError(errorMessage(err));
    }
  };

  // 轮询任务进度：pollingJobId 非空即立即拉一次并定时刷新；终态 / 卸载时清理定时器。
  useEffect(() => {
    if (pollingJobId === null) {
      return;
    }
    void fetchJobRef.current(pollingJobId);
    const timer = setInterval(() => {
      void fetchJobRef.current(pollingJobId);
    }, ONLINE_POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [pollingJobId]);

  // 客户端重连：页面加载时优先从服务器任务表恢复在途迁移；无在途任务时再用本地存档兜底。
  useEffect(() => {
    let mounted = true;

    const restoreJob = async (
      jobId: string,
      persist: boolean,
      title?: string | null,
    ): Promise<boolean> => {
      try {
        const job = await loadMigrationProgressJob(jobId);
        if (!mounted) {
          return false;
        }
        setOnlineJob(job);
        setJobTitle(title ?? null);
        setPendingJobId(null);
        setActive(3);
        if (isTerminalPhase(job.phase)) {
          if (persist) {
            localStorage.removeItem(ONLINE_JOB_STORAGE_KEY);
          }
        } else {
          if (persist) {
            localStorage.setItem(ONLINE_JOB_STORAGE_KEY, jobId);
          }
          setPollingJobId(jobId);
        }
        return true;
      } catch (err) {
        if (persist && err instanceof ApiError && err.status === 404) {
          localStorage.removeItem(ONLINE_JOB_STORAGE_KEY);
        }
        return false;
      }
    };

    void (async () => {
      if (requestedJobId) {
        try {
          const detail = await api.getTask(requestedJobId);
          const job = migrationJobFromTask(detail);
          if (!mounted) {
            return;
          }
          if (job !== null) {
            const title = detail.label ?? null;
            localStorage.setItem(ONLINE_JOB_STORAGE_KEY, requestedJobId);
            setOnlineJob(job);
            setJobTitle(title);
            setPendingJobId(null);
            setActive(3);
            if (isTerminalPhase(job.phase)) {
              localStorage.removeItem(ONLINE_JOB_STORAGE_KEY);
            } else {
              setPollingJobId(requestedJobId);
            }
            return;
          }
        } catch {
          // 指定任务详情暂时不可用时，再尝试专用迁移任务端点。
        }

        const restored = await restoreJob(requestedJobId, true);
        if (restored) {
          try {
            const detail = await api.getTask(requestedJobId);
            if (mounted && detail.kind === 'migration') {
              setJobTitle(detail.label ?? null);
            }
          } catch {
            // 任务登记表暂不可用时保留专用迁移任务快照。
          }
          return;
        }
      }

      try {
        const activeTask = await findActiveMigrationTask();
        if (!mounted) {
          return;
        }
        if (activeTask) {
          localStorage.setItem(ONLINE_JOB_STORAGE_KEY, activeTask.id);
          setJobTitle(activeTask.label ?? null);
          setPendingJobId(activeTask.id);
          setPollingJobId(activeTask.id);
          setActive(3);
          await restoreJob(activeTask.id, true, activeTask.label);
          return;
        }
      } catch {
        // 服务器任务表暂时不可用时，继续尝试本地存档兜底。
      }

      const archived = localStorage.getItem(ONLINE_JOB_STORAGE_KEY);
      if (archived) {
        await restoreJob(archived, true);
      }
    })();

    return () => {
      mounted = false;
    };
  }, [requestedJobId]);

  // 任务控制（FR-91）：取消 / 暂停 / 继续。控制端点幂等返回 200，调用后立即拉一次快照刷新按钮态。
  const [controlling, setControlling] = useState(false);
  const runControl = async (action: (id: string) => Promise<void>, id: string) => {
    setControlling(true);
    setMigrateError(null);
    try {
      await action(id);
      await fetchJobRef.current(id);
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) {
        setPollingJobId(null);
        setOnlineJob(null);
        setPendingJobId(null);
        localStorage.removeItem(ONLINE_JOB_STORAGE_KEY);
      } else {
        setMigrateError(errorMessage(err));
      }
    } finally {
      setControlling(false);
    }
  };

  /** 历史区一键重跑：发起重跑任务并进入进度续看。 */
  const handleRerun = async (taskId: string) => {
    const created = await api.rerunMigrationTask(taskId);
    const jobId = requireJobId(created, t('job.missingJobId'));
    watchMigrationJob(jobId, t('job.queueTitle'));
    notifySuccess(t('history.rerunStarted'));
  };

  const chosenRows = rows.filter((row) => selected.has(row.name));

  // 索引 / 预览加载：在线要源地址、离线要本地路径。
  const canLoad =
    (mode === 'online' ? baseUrl.trim() !== '' : offlinePath.trim() !== '') && !previewing;
  // 进入执行计划：选了仓库、离线需路径、推断项已补填类型。
  const canGoPlan =
    selected.size > 0 &&
    !hasUnfilledInferred &&
    (mode === 'online' ? baseUrl.trim() !== '' : migratePath.trim() !== '');

  return (
    <Stack>
      <Title order={2}>{t('title')}</Title>
      <Text c="dimmed">{t('intro')}</Text>

      <Stepper active={active} onStepClick={setActive}>
        {/* ① 源配置 */}
        <Stepper.Step label={t('step.sourceLabel')} description={t('step.sourceDesc')}>
          <Stack mt="md">
            <SegmentedControl
              value={mode}
              onChange={(v) => handleModeChange(v as SourceMode)}
              data={[
                { label: t('mode.online'), value: 'online' },
                { label: t('mode.offline'), value: 'offline' },
              ]}
            />

            {mode === 'online' ? (
              <>
                <TextInput
                  label={t('source.baseUrlLabel')}
                  placeholder="https://nexus.example"
                  value={baseUrl}
                  onChange={(e) => setBaseUrl(e.currentTarget.value)}
                  required
                />
                <PasswordInput
                  label={t('source.authRefLabel')}
                  description={t('source.authRefDesc')}
                  placeholder={t('source.authRefPlaceholder')}
                  value={authRef}
                  onChange={(e) => setAuthRef(e.currentTarget.value)}
                />
              </>
            ) : (
              <>
                <TextInput
                  label={t('source.offlinePathLabel')}
                  description={t('source.offlinePathDesc')}
                  placeholder="/data/nexus/blobs/default"
                  value={offlinePath}
                  onChange={(e) => setOfflinePath(e.currentTarget.value)}
                  required
                />
                <TextInput
                  label={t('source.offlineBaseUrlLabel')}
                  description={t('source.offlineBaseUrlDesc')}
                  placeholder={t('source.offlineBaseUrlPlaceholder')}
                  value={offlineBaseUrl}
                  onChange={(e) => setOfflineBaseUrl(e.currentTarget.value)}
                />
                <PasswordInput
                  label={t('source.authRefLabel')}
                  description={t('source.authRefDesc')}
                  placeholder={t('source.authRefPlaceholder')}
                  value={offlineAuthRef}
                  onChange={(e) => setOfflineAuthRef(e.currentTarget.value)}
                />
              </>
            )}

            {previewError && <ErrorAlert message={previewError} />}

            <Group>
              <Button onClick={handleLoad} disabled={!canLoad} loading={previewing}>
                {mode === 'online' ? t('preview.button') : t('preview.indexButton')}
              </Button>
              {rows.length > 0 && (
                <Button variant="default" onClick={() => setActive(1)}>
                  {t('preview.next')}
                </Button>
              )}
            </Group>

            {previewing ? (
              <Center h={120}>
                <Loader />
              </Center>
            ) : (
              rows.length > 0 && <PreviewTable rows={rows} />
            )}
          </Stack>
        </Stepper.Step>

        {/* ② 勾选仓库（含推断补填） */}
        <Stepper.Step label={t('step.selectLabel')} description={t('step.selectDesc')}>
          <Stack mt="md">
            {rows.length === 0 ? (
              <Text c="dimmed">{t('select.empty')}</Text>
            ) : (
              <>
                <Card withBorder padding="md" radius="md">
                  <Text fw={600} mb="sm">
                    {t('select.chosen', { count: selected.size })}
                  </Text>
                  {mode === 'online' ? (
                    <Stack gap="sm">
                      {rows.map((row) => (
                        <Group key={row.name} align="center" wrap="nowrap">
                          <Checkbox
                            label={`${row.name}（${row.format} / ${row.type ?? '-'}）`}
                            checked={selected.has(row.name)}
                            onChange={() => toggleSelect(row.name)}
                            style={{ flex: 1 }}
                          />
                          <TextInput
                            aria-label={t('source.targetRepoAria', { name: row.name })}
                            placeholder={t('source.targetRepoPlaceholder', { name: row.name })}
                            value={renames[row.name] ?? ''}
                            onChange={(e) => setRename(row.name, e.currentTarget.value)}
                            disabled={!selected.has(row.name)}
                            w={220}
                          />
                        </Group>
                      ))}
                    </Stack>
                  ) : (
                    <Stack gap="sm">
                      {rows.map((row) => (
                        <OfflineSelectRow
                          key={row.name}
                          row={row}
                          checked={selected.has(row.name)}
                          typeFill={typeFills[row.name]}
                          upstreamFill={upstreamFills[row.name] ?? ''}
                          onToggle={() => toggleSelect(row.name)}
                          onTypeChange={(type) => setTypeFill(row.name, type)}
                          onUpstreamChange={(url) => setUpstreamFill(row.name, url)}
                        />
                      ))}
                    </Stack>
                  )}
                </Card>

                {mode === 'online' ? (
                  <Alert color="blue" variant="light">
                    {t('select.onlineHint')}
                  </Alert>
                ) : (
                  <>
                    <TextInput
                      label={t('source.migratePathLabel')}
                      description={t('source.migratePathDesc')}
                      placeholder="/data/nexus/blobs/default"
                      value={migratePath}
                      onChange={(e) => setMigratePath(e.currentTarget.value)}
                      required
                    />
                    <Alert color="orange" variant="light">
                      {t('select.inferredHint')}
                    </Alert>
                  </>
                )}

                {hasUnfilledInferred && <ErrorAlert message={t('select.inferredNeedsType')} />}
                {migrateError && <ErrorAlert message={migrateError} />}

                <Group>
                  <Button onClick={() => setActive(0)} variant="default">
                    {t('select.prevStep')}
                  </Button>
                  <Button onClick={() => setActive(2)} disabled={!canGoPlan}>
                    {t('select.nextPlan')}
                  </Button>
                </Group>
              </>
            )}
          </Stack>
        </Stepper.Step>

        {/* ③ 确认执行计划 */}
        <Stepper.Step label={t('step.planLabel')} description={t('step.planDesc')}>
          <Stack mt="md">
            {chosenRows.length === 0 ? (
              <Text c="dimmed">{t('plan.empty')}</Text>
            ) : (
              <ExecutionPlan
                mode={mode}
                rows={chosenRows}
                renames={renames}
                effectiveType={effectiveType}
                upstreamOf={(row) => row.upstreamUrl ?? upstreamFills[row.name] ?? ''}
              />
            )}
            {migrateError && <ErrorAlert message={migrateError} />}
            <Group>
              <Button onClick={() => setActive(1)} variant="default">
                {t('plan.prevStep')}
              </Button>
              <Button
                onClick={handleStartMigration}
                disabled={chosenRows.length === 0 || migrating}
                loading={migrating}
              >
                {t('plan.start')}
              </Button>
            </Group>
          </Stack>
        </Stepper.Step>

        {/* ④ 执行与报告 */}
        <Stepper.Step label={t('step.reportLabel')} description={t('step.reportDesc')}>
          <Stack mt="md">
            {onlineJob ? (
              <OnlineJobPanel
                job={onlineJob}
                title={jobTitle}
                polling={pollingJobId !== null}
                controlling={controlling}
                onCancel={() => runControl(api.cancelMigrationJob, onlineJob.job_id)}
                onPause={() => runControl(api.pauseMigrationJob, onlineJob.job_id)}
                onResume={() => runControl(api.resumeMigrationJob, onlineJob.job_id)}
              />
            ) : pendingJobId ? (
              <PendingMigrationPanel jobId={pendingJobId} title={jobTitle} />
            ) : (
              <Text c="dimmed">{t('report.empty')}</Text>
            )}
            <Group>
              <Button variant="default" onClick={() => setActive(2)}>
                {t('report.backToSelect')}
              </Button>
            </Group>
          </Stack>
        </Stepper.Step>
      </Stepper>

      {/* 迁移历史（FR-140 持久化，含 interrupted，可回看报告 + 重跑） */}
      <MigrationHistoryPanel onRerun={handleRerun} />
    </Stack>
  );
}

/** 归一后的仓库索引 / 预览表（含格式来源、类型、blob 数、大小、上游）。 */
function PreviewTable({ rows }: { rows: PreviewRow[] }) {
  const { t } = useTranslation('migration');
  return (
    <Card withBorder padding="md" radius="md">
      <Text fw={600} mb="sm">
        {t('preview.count', { count: rows.length })}
      </Text>
      <Table.ScrollContainer minWidth={640}>
        <Table striped highlightOnHover>
          <Table.Thead>
            <Table.Tr>
              <Table.Th>{t('preview.thRepo')}</Table.Th>
              <Table.Th>{t('preview.thFormat')}</Table.Th>
              <Table.Th>{t('preview.thSource')}</Table.Th>
              <Table.Th>{t('preview.thType')}</Table.Th>
              <Table.Th ta="right">{t('preview.thBlobCount')}</Table.Th>
              <Table.Th ta="right">{t('preview.thSize')}</Table.Th>
              <Table.Th>{t('preview.thUpstream')}</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {rows.map((row) => (
              <Table.Tr key={row.name}>
                <Table.Td>{row.name}</Table.Td>
                <Table.Td>{row.format}</Table.Td>
                <Table.Td>
                  <FormatSourceBadge source={row.formatSource} />
                </Table.Td>
                <Table.Td>{row.type ?? '-'}</Table.Td>
                <Table.Td ta="right">
                  {row.blobCount !== null ? t('preview.blobCount', { count: row.blobCount }) : '-'}
                </Table.Td>
                <Table.Td ta="right">
                  {row.totalSize !== null ? formatBytes(row.totalSize) : '-'}
                </Table.Td>
                <Table.Td>{row.upstreamUrl ?? '-'}</Table.Td>
              </Table.Tr>
            ))}
          </Table.Tbody>
        </Table>
      </Table.ScrollContainer>
    </Card>
  );
}

/** 格式来源徽章（权威 / 推断）。 */
function FormatSourceBadge({ source }: { source: FormatSource }) {
  const { t } = useTranslation('migration');
  return source === 'authoritative' ? (
    <Badge color="green" variant="light">
      {t('preview.sourceAuthoritative')}
    </Badge>
  ) : (
    <Badge color="orange" variant="light">
      {t('preview.sourceInferred')}
    </Badge>
  );
}

/** 离线勾选行：权威项类型只读，推断项要求补填类型（proxy 可填上游）。 */
function OfflineSelectRow({
  row,
  checked,
  typeFill,
  upstreamFill,
  onToggle,
  onTypeChange,
  onUpstreamChange,
}: {
  row: PreviewRow;
  checked: boolean;
  typeFill: MigrateKind | undefined;
  upstreamFill: string;
  onToggle: () => void;
  onTypeChange: (type: MigrateKind) => void;
  onUpstreamChange: (url: string) => void;
}) {
  const { t } = useTranslation('migration');
  const inferred = row.formatSource === 'inferred';
  const currentType = row.type ?? typeFill ?? null;

  return (
    <Group align="center" wrap="nowrap">
      <Checkbox
        label={`${row.name}（${row.format}）`}
        checked={checked}
        onChange={onToggle}
        style={{ flex: 1 }}
      />
      <FormatSourceBadge source={row.formatSource} />
      {inferred ? (
        <Select
          aria-label={t('select.typeAria', { name: row.name })}
          placeholder={t('select.typePlaceholder')}
          data={[
            { value: 'proxy', label: t('select.typeProxy') },
            { value: 'hosted', label: t('select.typeHosted') },
          ]}
          value={typeFill ?? null}
          onChange={(v) => v && onTypeChange(v as MigrateKind)}
          disabled={!checked}
          w={130}
        />
      ) : (
        <Text size="sm" w={130}>
          {row.type ?? '-'}
        </Text>
      )}
      {inferred && currentType === 'proxy' && (
        <TextInput
          aria-label={t('select.upstreamAria', { name: row.name })}
          placeholder={t('select.upstreamPlaceholder')}
          value={upstreamFill}
          onChange={(e) => onUpstreamChange(e.currentTarget.value)}
          disabled={!checked}
          w={220}
        />
      )}
    </Group>
  );
}

/** 执行计划只读汇总表。 */
function ExecutionPlan({
  mode,
  rows,
  renames,
  effectiveType,
  upstreamOf,
}: {
  mode: SourceMode;
  rows: PreviewRow[];
  renames: Record<string, string>;
  effectiveType: (row: PreviewRow) => MigrateKind | null;
  upstreamOf: (row: PreviewRow) => string;
}) {
  const { t } = useTranslation('migration');
  const methodLabel = (row: PreviewRow): string => {
    if (mode === 'online') {
      return t('plan.methodOnline');
    }
    return effectiveType(row) === 'proxy' ? t('plan.methodProxy') : t('plan.methodHosted');
  };

  return (
    <Card withBorder padding="md" radius="md">
      <Text fw={600} mb="xs">
        {t('plan.title')}
      </Text>
      <Text size="sm" c="dimmed" mb="sm">
        {t('plan.intro')}
      </Text>
      <Table.ScrollContainer minWidth={640}>
        <Table striped>
          <Table.Thead>
            <Table.Tr>
              <Table.Th>{t('plan.thSource')}</Table.Th>
              <Table.Th>{t('plan.thTarget')}</Table.Th>
              <Table.Th>{t('plan.thMethod')}</Table.Th>
              <Table.Th>{t('plan.thType')}</Table.Th>
              <Table.Th>{t('plan.thUpstream')}</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {rows.map((row) => {
              const target =
                mode === 'online' && (renames[row.name] ?? '').trim() !== ''
                  ? renames[row.name].trim()
                  : row.name;
              return (
                <Table.Tr key={row.name}>
                  <Table.Td>{row.name}</Table.Td>
                  <Table.Td>{target}</Table.Td>
                  <Table.Td>{methodLabel(row)}</Table.Td>
                  <Table.Td>{effectiveType(row) ?? '-'}</Table.Td>
                  <Table.Td>{upstreamOf(row) || '-'}</Table.Td>
                </Table.Tr>
              );
            })}
          </Table.Tbody>
        </Table>
      </Table.ScrollContainer>
      {mode === 'offline' && (
        <Text size="xs" c="dimmed" mt="sm">
          {t('plan.upstreamNote')}
        </Text>
      )}
    </Card>
  );
}

/** 已发起迁移但首次快照尚未返回时的占位进度面板。 */
function PendingMigrationPanel({ jobId, title }: { jobId: string; title: string | null }) {
  const { t } = useTranslation('migration');
  return (
    <Card withBorder padding="md" radius="md">
      <Group justify="space-between" mb="sm">
        <Text fw={600}>{title ?? t('job.pendingTitle')}</Text>
        <Loader size="xs" />
      </Group>
      <Progress value={0} aria-label={t('job.progressAria')} animated />
      <Text size="sm" c="dimmed" mt="xs">
        {t('job.pendingDesc')}
      </Text>
      <Text size="xs" c="dimmed" mt={4}>
        {t('job.pendingJobId', { id: jobId })}
      </Text>
    </Card>
  );
}

/** 迁移任务进度面板：进行中展示导入队列进度，终态展示最终报告，并提供取消 / 暂停 / 继续。 */
function OnlineJobPanel({
  job,
  title,
  polling,
  controlling,
  onCancel,
  onPause,
  onResume,
}: {
  job: OnlinePullJob;
  title: string | null;
  polling: boolean;
  controlling: boolean;
  onCancel: () => void;
  onPause: () => void;
  onResume: () => void;
}) {
  const { t } = useTranslation('migration');
  // 进度百分比：优先使用字节级统计；无字节统计时退回资产数。
  const hasByteProgress = (job.total_bytes ?? 0) > 0;
  const percent = hasByteProgress
    ? Math.round(((job.done_bytes ?? 0) / (job.total_bytes ?? 1)) * 100)
    : job.total_assets > 0
      ? Math.round((job.done_assets / job.total_assets) * 100)
      : 0;
  const progressText = hasByteProgress
    ? t('job.byteProgress', {
        done: formatBytes(job.done_bytes ?? 0),
        total: formatBytes(job.total_bytes ?? 0),
        percent,
      })
    : t('job.progress', { done: job.done_assets, total: job.total_assets, percent });
  const terminal = isTerminalPhase(job.phase);
  const summary = migrationSummary(job);
  // 按钮可用性（FR-91）：仅进行中（未暂停的活动态）可暂停 / 取消；仅已暂停可继续；终态全禁用。
  const canPause = !terminal && !job.paused && !controlling;
  const canResume = !terminal && job.paused && !controlling;
  const canCancel = !terminal && !controlling;

  return (
    <Card withBorder padding="md" radius="md">
      <Group justify="space-between" mb="sm">
        <Text fw={600}>{title ?? t('job.queueTitle')}</Text>
        <Group gap="xs">
          {polling && <Loader size="xs" />}
          <Badge color={phaseColor(job.phase)} variant="light">
            {phaseLabel(job.phase, t)}
          </Badge>
        </Group>
      </Group>

      {/* 任务控制：取消 / 暂停 / 继续（按任务态启停） */}
      <Group gap="xs" mb="sm">
        {job.paused ? (
          <Button size="xs" variant="light" onClick={onResume} disabled={!canResume}>
            {t('job.resume')}
          </Button>
        ) : (
          <Button size="xs" variant="light" onClick={onPause} disabled={!canPause}>
            {t('job.pause')}
          </Button>
        )}
        <Button size="xs" variant="light" color="red" onClick={onCancel} disabled={!canCancel}>
          {t('job.cancel')}
        </Button>
      </Group>

      {/* 进度条 + 资产计数 */}
      <Progress
        value={percent}
        aria-label={t('job.progressAria')}
        animated={!terminal && !job.paused}
      />
      <Group justify="space-between" mt="xs">
        <Text size="sm" c="dimmed">
          {progressText}
        </Text>
        <Group gap="md">
          <Text size="sm">{t('job.migrated', { count: summary.migrated })}</Text>
          <Text size="sm" c="dimmed">
            {t('job.skippedExisting', { count: summary.skippedExisting })}
          </Text>
          <Text size="sm" c="dimmed">
            {t('job.skippedFailed', { count: summary.skippedFailed })}
          </Text>
        </Group>
      </Group>

      {/* 当前仓库 / 当前文件（进行中显示） */}
      {!terminal && (job.current_repo || job.current_path) && (
        <Stack gap={2} mt="sm">
          {job.current_repo && (
            <Text size="sm">
              {t('job.currentRepo')}
              <Text span fw={500}>
                {job.current_repo}
              </Text>
            </Text>
          )}
          {job.current_path && (
            <Text size="xs" c="dimmed" style={{ wordBreak: 'break-all' }}>
              {t('job.currentPath')}
              {job.current_path}
            </Text>
          )}
        </Stack>
      )}

      {/* 失败：展示错误文案 */}
      {job.phase === 'failed' && job.error && (
        <Alert color="red" variant="light" mt="sm" title={t('job.failedTitle')}>
          {job.error}
        </Alert>
      )}

      {/* 终态：展示完成报告和各仓库明细 */}
      {terminal && (
        <>
          <Group gap="md" mt="sm">
            <Text size="sm">{t('job.summaryTotal', { count: summary.total })}</Text>
            <Text size="sm">{t('job.summaryMigrated', { count: summary.migrated })}</Text>
            <Text size="sm" c="dimmed">
              {t('job.summarySkippedExisting', { count: summary.skippedExisting })}
            </Text>
            <Text size="sm" c="dimmed">
              {t('job.summarySkippedFailed', { count: summary.skippedFailed })}
            </Text>
          </Group>

          {job.repos.length === 0 ? (
            <Text c="dimmed" size="sm" mt="sm">
              {t('job.noRepos')}
            </Text>
          ) : (
            <Table.ScrollContainer minWidth={640}>
              <Table striped mt="sm">
                <Table.Thead>
                  <Table.Tr>
                    <Table.Th>{t('job.thSourceRepo')}</Table.Th>
                    <Table.Th>{t('job.thTargetRepo')}</Table.Th>
                    <Table.Th>{t('job.thFormat')}</Table.Th>
                    <Table.Th>{t('job.thCreated')}</Table.Th>
                    <Table.Th ta="right">{t('job.thMigrated')}</Table.Th>
                    <Table.Th ta="right">{t('job.thSkippedExisting')}</Table.Th>
                    <Table.Th ta="right">{t('job.thSkippedFailed')}</Table.Th>
                  </Table.Tr>
                </Table.Thead>
                <Table.Tbody>
                  {job.repos.map((r) => (
                    <Table.Tr key={`${r.source_repo}->${r.target_repo}`}>
                      <Table.Td>{r.source_repo}</Table.Td>
                      <Table.Td>{r.target_repo}</Table.Td>
                      <Table.Td>{r.format}</Table.Td>
                      <Table.Td>
                        <Badge color={r.created ? 'green' : 'gray'} variant="light">
                          {r.created ? t('job.createdYes') : t('job.createdExisting')}
                        </Badge>
                      </Table.Td>
                      <Table.Td ta="right">{r.migrated_artifacts}</Table.Td>
                      <Table.Td ta="right">{r.skipped_existing_artifacts ?? 0}</Table.Td>
                      <Table.Td ta="right">{r.skipped_artifacts}</Table.Td>
                    </Table.Tr>
                  ))}
                </Table.Tbody>
              </Table>
            </Table.ScrollContainer>
          )}

          {job.skipped_repos.length > 0 && (
            <Group mt="sm" gap="xs">
              <Text size="sm" c="dimmed">
                {t('job.skippedRepos')}
              </Text>
              {job.skipped_repos.map((name) => (
                <Badge key={name} color="orange" variant="light">
                  {name}
                </Badge>
              ))}
            </Group>
          )}
        </>
      )}
    </Card>
  );
}
