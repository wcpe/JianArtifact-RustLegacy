// 任务中心页（FR-132，仅 Admin）：展示统一任务注册表的活跃+近期任务队列。
// 依赖 FR-131 后端：GET /api/v1/tasks（TaskRecord[]）与 GET /api/v1/tasks/{id}（详情）。
// 轮询间隔 3s；卸载时立即 clearInterval；无任务显空状态；点击行展开详情，详情内提供跳转续看与迁移控制。

import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  Stack,
  Title,
  Text,
  Badge,
  Card,
  Group,
  Loader,
  Center,
  Alert,
  ThemeIcon,
  Button,
  Progress,
} from '@mantine/core';
import {
  IconArrowsExchange,
  IconArrowUpCircle,
  IconShield,
  IconListCheck,
  IconAlertCircle,
} from '@tabler/icons-react';
import { useNavigate } from 'react-router-dom';
import * as api from '../api/endpoints';
import type { OnlinePullJob, TaskDetailDto, TaskKind, TaskRecord, TaskState } from '../api/types';
import { errorMessage } from '../lib/format';
import { useGlobalProgress } from '../hooks/useGlobalProgress';

/** 轮询间隔（毫秒）。 */
const POLL_INTERVAL_MS = 3000;

/** 任务类型 → 中文名与图标。 */
function kindMeta(kind: TaskKind): { label: string; icon: React.ReactNode } {
  switch (kind) {
    case 'migration':
      return { label: 'Nexus 迁移', icon: <IconArrowsExchange size={16} /> };
    case 'update':
      return { label: '在线更新', icon: <IconArrowUpCircle size={16} /> };
    case 'vuln':
      return { label: '漏洞库刷新', icon: <IconShield size={16} /> };
  }
}

/** 任务状态 → Badge 文案与颜色。 */
function stateMeta(state: TaskState): { label: string; color: string } {
  switch (state) {
    case 'running':
      return { label: '运行中', color: 'blue' };
    case 'paused':
      return { label: '已暂停', color: 'yellow' };
    case 'succeeded':
      return { label: '已完成', color: 'green' };
    case 'failed':
      return { label: '失败', color: 'red' };
    case 'cancelled':
      return { label: '已取消', color: 'gray' };
  }
}

/** 任务是否为活跃态（running / paused）。 */
function isActive(state: TaskState): boolean {
  return state === 'running' || state === 'paused';
}

/** 迁移阶段是否为终态。 */
function isTerminalMigration(phase: OnlinePullJob['phase']): boolean {
  return phase === 'done' || phase === 'failed' || phase === 'cancelled';
}

/** 格式化 Unix 秒为本地时间串。 */
function fmtUnixSecs(secs: number): string {
  return new Date(secs * 1000).toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
}

/** 点击详情内按钮跳转对应续看页路由（按 kind）。 */
function kindRoute(task: TaskRecord): string {
  switch (task.kind) {
    case 'migration':
      return `/migration?job_id=${encodeURIComponent(task.id)}`;
    case 'update':
      return '/system';
    case 'vuln':
      return '/settings';
  }
}

/** 单条任务行组件。 */
function TaskRow({ task, onClick }: { task: TaskRecord; onClick: () => void }) {
  const { icon } = kindMeta(task.kind);
  const { label: stateLabel, color: stateColor } = stateMeta(task.state);
  const displayLabel = task.label ?? kindMeta(task.kind).label;

  return (
    <Card
      withBorder
      padding="sm"
      style={{ cursor: 'pointer' }}
      onClick={onClick}
      role="row"
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onClick();
        }
      }}
    >
      <Group justify="space-between" wrap="nowrap">
        <Group gap="sm" wrap="nowrap" style={{ minWidth: 0 }}>
          <ThemeIcon variant="light" size="sm" color="blue">
            {icon}
          </ThemeIcon>
          <Text fw={500} size="sm" truncate="end">
            {displayLabel}
          </Text>
        </Group>
        <Group gap="xs" wrap="nowrap" style={{ flexShrink: 0 }}>
          <Badge color={stateColor} variant="light" size="sm">
            {stateLabel}
          </Badge>
        </Group>
      </Group>
      <Group gap="md" mt={4}>
        <Text size="xs" c="dimmed">
          开始：{fmtUnixSecs(task.started_at)}
        </Text>
        {task.finished_at !== undefined && (
          <Text size="xs" c="dimmed">
            完成：{fmtUnixSecs(task.finished_at)}
          </Text>
        )}
        {task.error !== undefined && (
          <Text size="xs" c="red" truncate="end" style={{ maxWidth: 300 }}>
            {task.error}
          </Text>
        )}
      </Group>
    </Card>
  );
}

/** 迁移任务进度详情。 */
function MigrationDetail({
  job,
  isOfflinePreview,
}: {
  job: OnlinePullJob;
  isOfflinePreview: boolean;
}) {
  const percent = job.total_assets > 0 ? Math.round((job.done_assets / job.total_assets) * 100) : 0;
  const terminal = isTerminalMigration(job.phase);
  const previewRepos = job.offline_preview ?? [];
  const previewBlobCount = previewRepos.reduce((sum, repo) => sum + repo.blob_count, 0);

  return (
    <Stack gap="xs">
      <Progress value={percent} animated={!terminal && !job.paused} aria-label="迁移进度" />
      <Group gap="md">
        <Text size="sm" c="dimmed">
          进度 {job.done_assets} / {job.total_assets}（{percent}%）
        </Text>
        {isOfflinePreview ? (
          <Text size="sm" c="dimmed">
            离线预览不搬运制品
          </Text>
        ) : (
          <>
            <Text size="sm">已迁 {job.migrated}</Text>
            <Text size="sm" c="dimmed">
              已跳过 {job.skipped}
            </Text>
          </>
        )}
      </Group>
      {isOfflinePreview && terminal && (
        <Stack gap={4}>
          <Text size="sm">
            离线预览已完成：共 {previewRepos.length} 个仓库、{previewBlobCount} 个 blob。
          </Text>
          {previewRepos.map((repo) => (
            <Text key={repo.repo_name} size="xs" c="dimmed">
              {repo.repo_name}：{repo.blob_count} 个 blob
            </Text>
          ))}
        </Stack>
      )}
      {job.current_repo && (
        <Text size="sm">
          当前仓库：
          <Text span fw={500}>
            {job.current_repo}
          </Text>
        </Text>
      )}
      {job.current_path && (
        <Text size="xs" c="dimmed" style={{ wordBreak: 'break-all' }}>
          当前文件：{job.current_path}
        </Text>
      )}
      {job.error && (
        <Alert color="red" variant="light">
          {job.error}
        </Alert>
      )}
    </Stack>
  );
}

/** 任务详情面板。 */
function TaskDetailPanel({
  detail,
  loading,
  error,
  controlling,
  onPause,
  onResume,
  onCancel,
  onJump,
}: {
  detail: TaskDetailDto | null;
  loading: boolean;
  error: string | null;
  controlling: boolean;
  onPause: () => void;
  onResume: () => void;
  onCancel: () => void;
  onJump: () => void;
}) {
  if (loading) {
    return (
      <Card withBorder padding="sm">
        <Group gap="xs">
          <Loader size="xs" />
          <Text size="sm" c="dimmed">
            正在加载任务详情…
          </Text>
        </Group>
      </Card>
    );
  }

  if (error !== null) {
    return (
      <Alert icon={<IconAlertCircle size={16} />} color="red">
        {error}
      </Alert>
    );
  }

  if (detail === null) {
    return null;
  }

  const migration = detail.migration;
  const terminal = migration ? isTerminalMigration(migration.phase) : !isActive(detail.state);
  const paused = migration?.paused ?? detail.state === 'paused';
  const isOfflinePreview = detail.label === '离线预览' || migration?.offline_preview !== undefined;
  // 离线预览是单次阻塞枚举，目前后端控制信号不参与中断；避免展示不可生效的控制按钮。
  const canControlMigration = detail.kind === 'migration' && !isOfflinePreview;
  const canPause = canControlMigration && migration !== undefined && !terminal && !paused;
  const canResume = canControlMigration && migration !== undefined && !terminal && paused;
  const canCancel = canControlMigration && migration !== undefined && !terminal;

  return (
    <Card withBorder padding="sm" bg="gray.0">
      <Stack gap="sm">
        <Group justify="space-between" wrap="nowrap">
          <Text fw={600}>任务详情</Text>
          <Button size="xs" variant="default" onClick={onJump}>
            跳转查看
          </Button>
        </Group>

        <Group gap="md">
          <Text size="sm">ID：{detail.id}</Text>
          <Text size="sm" c="dimmed">
            更新时间：{fmtUnixSecs(detail.updated_at)}
          </Text>
        </Group>

        {migration && <MigrationDetail job={migration} isOfflinePreview={isOfflinePreview} />}
        {detail.update && (
          <Stack gap={2}>
            <Text size="sm">更新阶段：{detail.update.phase}</Text>
            <Text size="sm" c="dimmed">
              当前版本：{detail.update.current_version}
            </Text>
            {detail.update.latest_version && (
              <Text size="sm" c="dimmed">
                最新版本：{detail.update.latest_version}
              </Text>
            )}
          </Stack>
        )}
        {!migration && !detail.update && (
          <Text size="sm" c="dimmed">
            此任务暂无更细进度。
          </Text>
        )}

        {canControlMigration && migration && (
          <Group gap="xs">
            {paused ? (
              <Button
                size="xs"
                variant="light"
                onClick={onResume}
                disabled={!canResume || controlling}
              >
                继续
              </Button>
            ) : (
              <Button
                size="xs"
                variant="light"
                onClick={onPause}
                disabled={!canPause || controlling}
              >
                暂停
              </Button>
            )}
            <Button
              size="xs"
              variant="light"
              color="red"
              onClick={onCancel}
              disabled={!canCancel || controlling}
            >
              结束
            </Button>
          </Group>
        )}
      </Stack>
    </Card>
  );
}

/**
 * 任务中心页（FR-132，仅 Admin）：展示统一任务注册表的活跃+近期任务，
 * 点击行展开详情；详情内跳转对应续看页；轮询 3s 自动刷新；离开页面仍可找回已完成历史。
 */
export function TaskCenterPage() {
  const { t } = useTranslation('taskCenter');
  const navigate = useNavigate();
  const { registerPageRefreshCallback } = useGlobalProgress();
  const [tasks, setTasks] = useState<TaskRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<TaskDetailDto | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [controlling, setControlling] = useState(false);
  const selectedIdRef = useRef<string | null>(null);
  const detailRequestSeqRef = useRef(0);

  const refreshTasks = useCallback(async () => {
    const result = await api.listTasks();
    setTasks(result);
  }, []);

  const loadDetail = async (id: string) => {
    const requestSeq = detailRequestSeqRef.current + 1;
    detailRequestSeqRef.current = requestSeq;

    if (selectedIdRef.current === id) {
      selectedIdRef.current = null;
      setSelectedId(null);
      setDetail(null);
      setDetailError(null);
      setDetailLoading(false);
      return;
    }

    selectedIdRef.current = id;
    setSelectedId(id);
    setDetailLoading(true);
    setDetailError(null);
    try {
      const nextDetail = await api.getTask(id);
      if (detailRequestSeqRef.current === requestSeq) {
        setDetail(nextDetail);
      }
    } catch (err) {
      if (detailRequestSeqRef.current === requestSeq) {
        setDetail(null);
        setDetailError(errorMessage(err));
      }
    } finally {
      if (detailRequestSeqRef.current === requestSeq) {
        setDetailLoading(false);
      }
    }
  };

  const runMigrationControl = async (action: (id: string) => Promise<void>) => {
    const controlId = selectedIdRef.current;
    if (controlId === null) {
      return;
    }
    setControlling(true);
    setDetailError(null);
    try {
      await action(controlId);
      const nextDetail = await api.getTask(controlId);
      if (selectedIdRef.current === controlId) {
        setDetail(nextDetail);
      }
      await refreshTasks();
    } catch (err) {
      if (selectedIdRef.current === controlId) {
        setDetailError(errorMessage(err));
      }
    } finally {
      setControlling(false);
    }
  };

  useEffect(
    () => registerPageRefreshCallback(refreshTasks),
    [registerPageRefreshCallback, refreshTasks],
  );

  useEffect(() => {
    let cancelled = false;

    // 首次加载
    api
      .listTasks()
      .then((result) => {
        if (!cancelled) {
          setTasks(result);
          setLoading(false);
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setError(errorMessage(err));
          setLoading(false);
        }
      });

    // 轮询刷新（3s 间隔）
    const timer = setInterval(() => {
      api
        .listTasks()
        .then((result) => {
          if (!cancelled) setTasks(result);
        })
        .catch(() => {
          // 轮询失败静默忽略，不覆盖现有列表
        });
    }, POLL_INTERVAL_MS);

    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, []);

  // 按活跃 / 近期历史分组
  const activeTasks = tasks.filter((task) => isActive(task.state));
  const recentTasks = tasks.filter((task) => !isActive(task.state));

  const renderTask = (task: TaskRecord) => (
    <Stack key={task.id} gap={4}>
      <TaskRow task={task} onClick={() => void loadDetail(task.id)} />
      {selectedId === task.id && (
        <TaskDetailPanel
          detail={detail}
          loading={detailLoading}
          error={detailError}
          controlling={controlling}
          onPause={() => void runMigrationControl(api.pauseMigrationJob)}
          onResume={() => void runMigrationControl(api.resumeMigrationJob)}
          onCancel={() => void runMigrationControl(api.cancelMigrationJob)}
          onJump={() => navigate(kindRoute(task))}
        />
      )}
    </Stack>
  );

  if (loading) {
    return (
      <Center h={200}>
        <Loader />
      </Center>
    );
  }

  if (error !== null) {
    return (
      <Alert icon={<IconAlertCircle size={16} />} color="red" mt="md">
        {error}
      </Alert>
    );
  }

  return (
    <Stack gap="lg">
      <Group gap="sm">
        <IconListCheck size={24} />
        <Title order={2}>{t('title')}</Title>
      </Group>

      {tasks.length === 0 ? (
        <Text c="dimmed" ta="center" py="xl">
          {t('empty')}
        </Text>
      ) : (
        <Stack gap="md">
          {activeTasks.length > 0 && (
            <Stack gap="xs">
              <Text fw={600} size="sm" c="dimmed">
                {t('sectionActive')}
              </Text>
              {activeTasks.map(renderTask)}
            </Stack>
          )}
          {recentTasks.length > 0 && (
            <Stack gap="xs">
              <Text fw={600} size="sm" c="dimmed">
                {t('sectionRecent')}
              </Text>
              {recentTasks.map(renderTask)}
            </Stack>
          )}
        </Stack>
      )}
    </Stack>
  );
}
