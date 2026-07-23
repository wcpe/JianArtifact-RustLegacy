// 迁移历史面板（FR-142 消费 FR-140 持久化，仅 Admin）。
//
// 列出库中持久化的迁移任务（含进程重启后被标 interrupted 的在途任务），点开展示分仓库报告；
// interrupted / 终态任务提供「重跑续传」——调 rerun 端点得新 job_id，交由父页进入进度续看。

import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import {
  Stack,
  Card,
  Title,
  Text,
  Table,
  Badge,
  Button,
  Group,
  Loader,
  Center,
} from '@mantine/core';
import * as api from '../api/endpoints';
import type {
  MigrationHistoryState,
  MigrationTaskDto,
  MigrationTaskKind,
  MigrationTaskDetailDto,
} from '../api/types';
import { errorMessage, formatBytes, formatDuration } from '../lib/format';
import { ErrorAlert } from '../components/ErrorAlert';

/** 历史任务类别 → 中文标签。 */
function kindLabel(kind: MigrationTaskKind, t: TFunction): string {
  switch (kind) {
    case 'online':
      return t('history.kindOnline');
    case 'offline_proxy':
      return t('history.kindOfflineProxy');
    case 'offline_hosted':
      return t('history.kindOfflineHosted');
  }
}

/** 历史任务状态 → 中文标签 + Badge 颜色。 */
function stateMeta(state: MigrationHistoryState, t: TFunction): { label: string; color: string } {
  switch (state) {
    case 'running':
      return { label: t('history.stateRunning'), color: 'blue' };
    case 'paused':
      return { label: t('history.statePaused'), color: 'yellow' };
    case 'succeeded':
      return { label: t('history.stateSucceeded'), color: 'green' };
    case 'failed':
      return { label: t('history.stateFailed'), color: 'red' };
    case 'cancelled':
      return { label: t('history.stateCancelled'), color: 'gray' };
    case 'interrupted':
      return { label: t('history.stateInterrupted'), color: 'orange' };
  }
}

/** 终态 / 中断态才可重跑（运行中 / 暂停中不重复发起）。 */
function canRerun(state: MigrationHistoryState): boolean {
  return state !== 'running' && state !== 'paused';
}

/** 迁移历史面板。`onRerun` 由父页提供（发起重跑并进入进度续看）。 */
export function MigrationHistoryPanel({ onRerun }: { onRerun: (taskId: string) => Promise<void> }) {
  const { t } = useTranslation('migration');
  const [tasks, setTasks] = useState<MigrationTaskDto[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<MigrationTaskDetailDto | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [rerunningId, setRerunningId] = useState<string | null>(null);
  const detailSeqRef = useRef(0);

  const refresh = useCallback(async () => {
    setLoadError(null);
    try {
      const result = await api.listMigrationTasks();
      setTasks(result);
    } catch (err) {
      setLoadError(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  /** 点击某任务：收起已选 / 展开并加载详情报告。 */
  const toggleDetail = async (id: string) => {
    if (selectedId === id) {
      setSelectedId(null);
      setDetail(null);
      setDetailError(null);
      return;
    }
    const seq = detailSeqRef.current + 1;
    detailSeqRef.current = seq;
    setSelectedId(id);
    setDetail(null);
    setDetailError(null);
    try {
      const result = await api.getMigrationTask(id);
      if (detailSeqRef.current === seq) {
        setDetail(result);
      }
    } catch (err) {
      if (detailSeqRef.current === seq) {
        setDetailError(errorMessage(err));
      }
    }
  };

  /** 发起重跑：委托父页进入进度续看，成功后刷新历史列表。 */
  const handleRerun = async (id: string) => {
    setRerunningId(id);
    try {
      await onRerun(id);
      await refresh();
    } catch (err) {
      setLoadError(errorMessage(err));
    } finally {
      setRerunningId(null);
    }
  };

  return (
    <Card withBorder padding="md" radius="md" mt="lg">
      <Group justify="space-between" mb="sm">
        <Title order={4}>{t('history.title')}</Title>
        <Button size="xs" variant="default" onClick={() => void refresh()}>
          {t('history.refresh')}
        </Button>
      </Group>

      {loadError && <ErrorAlert message={loadError} />}

      {loading ? (
        <Center h={80}>
          <Loader size="sm" />
        </Center>
      ) : tasks.length === 0 ? (
        <Text c="dimmed" size="sm">
          {t('history.empty')}
        </Text>
      ) : (
        <Table.ScrollContainer minWidth={640}>
          <Table striped highlightOnHover>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>{t('history.thTask')}</Table.Th>
                <Table.Th>{t('history.thState')}</Table.Th>
                <Table.Th>{t('history.thStarted')}</Table.Th>
                <Table.Th>{t('history.thFinished')}</Table.Th>
                <Table.Th />
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {tasks.map((task) => {
                const meta = stateMeta(task.state, t);
                return (
                  <Table.Tr key={task.id}>
                    <Table.Td>
                      <Text size="sm" fw={500}>
                        {task.label ?? kindLabel(task.kind, t)}
                      </Text>
                      <Text size="xs" c="dimmed">
                        {kindLabel(task.kind, t)}
                      </Text>
                    </Table.Td>
                    <Table.Td>
                      <Badge color={meta.color} variant="light">
                        {meta.label}
                      </Badge>
                    </Table.Td>
                    <Table.Td>
                      <Text size="xs" c="dimmed">
                        {task.started_at}
                      </Text>
                    </Table.Td>
                    <Table.Td>
                      <Text size="xs" c="dimmed">
                        {task.finished_at ?? '-'}
                      </Text>
                    </Table.Td>
                    <Table.Td>
                      <Group gap="xs" wrap="nowrap" justify="flex-end">
                        <Button
                          size="xs"
                          variant="subtle"
                          onClick={() => void toggleDetail(task.id)}
                        >
                          {t('history.view')}
                        </Button>
                        {canRerun(task.state) && (
                          <Button
                            size="xs"
                            variant="light"
                            loading={rerunningId === task.id}
                            onClick={() => void handleRerun(task.id)}
                          >
                            {t('history.rerun')}
                          </Button>
                        )}
                      </Group>
                    </Table.Td>
                  </Table.Tr>
                );
              })}
            </Table.Tbody>
          </Table>
        </Table.ScrollContainer>
      )}

      {selectedId !== null && <HistoryDetail detail={detail} error={detailError} />}
    </Card>
  );
}

/** 单个历史任务的分仓库报告详情。 */
function HistoryDetail({
  detail,
  error,
}: {
  detail: MigrationTaskDetailDto | null;
  error: string | null;
}) {
  const { t } = useTranslation('migration');
  if (error !== null) {
    return <ErrorAlert message={error} />;
  }
  if (detail === null) {
    return (
      <Center h={60}>
        <Loader size="sm" />
      </Center>
    );
  }
  return (
    <Stack gap="xs" mt="md">
      <Text fw={600} size="sm">
        {t('history.detailTitle')}
      </Text>
      <Group gap="md">
        <Text size="sm">
          {t('history.summaryTotal', { value: formatBytes(detail.total_bytes) })}
        </Text>
        <Text size="sm">{t('history.summaryDone', { value: formatBytes(detail.done_bytes) })}</Text>
        <Text size="sm">
          {t('history.summaryMigrated', { value: formatBytes(detail.migrated_bytes) })}
        </Text>
        <Text size="sm">
          {t('history.summarySkipped', { value: formatBytes(detail.skipped_bytes) })}
        </Text>
      </Group>
      {detail.repos.length === 0 ? (
        <Text c="dimmed" size="sm">
          {t('report.noRepos')}
        </Text>
      ) : (
        <Table.ScrollContainer minWidth={640}>
          <Table striped>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>{t('job.thSourceRepo')}</Table.Th>
                <Table.Th>{t('job.thTargetRepo')}</Table.Th>
                <Table.Th>{t('job.thFormat')}</Table.Th>
                <Table.Th>{t('job.thCreated')}</Table.Th>
                <Table.Th ta="right">{t('job.thMigrated')}</Table.Th>
                <Table.Th ta="right">{t('job.thSkippedExisting')}</Table.Th>
                <Table.Th ta="right">{t('job.thSkippedFailed')}</Table.Th>
                <Table.Th ta="right">{t('history.thTotalBytes')}</Table.Th>
                <Table.Th ta="right">{t('history.thDoneBytes')}</Table.Th>
                <Table.Th ta="right">{t('history.thMigratedBytes')}</Table.Th>
                <Table.Th ta="right">{t('history.thSkippedBytes')}</Table.Th>
                <Table.Th ta="right">{t('history.thDuration')}</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {detail.repos.map((r) => (
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
                  <Table.Td ta="right">{r.skipped_existing_artifacts}</Table.Td>
                  <Table.Td ta="right">{r.skipped_artifacts}</Table.Td>
                  <Table.Td ta="right">{formatBytes(r.total_bytes)}</Table.Td>
                  <Table.Td ta="right">{formatBytes(r.done_bytes)}</Table.Td>
                  <Table.Td ta="right">{formatBytes(r.migrated_bytes)}</Table.Td>
                  <Table.Td ta="right">{formatBytes(r.skipped_bytes)}</Table.Td>
                  <Table.Td ta="right">{formatDuration(r.duration_ms)}</Table.Td>
                </Table.Tr>
              ))}
            </Table.Tbody>
          </Table>
        </Table.ScrollContainer>
      )}
      <Stack gap="xs" mt="sm">
        <Text fw={600} size="sm">
          {t('history.failureTitle')}
        </Text>
        {detail.asset_failures.length === 0 ? (
          <Text c="dimmed" size="sm">
            {t('history.failureEmpty')}
          </Text>
        ) : (
          <Table.ScrollContainer minWidth={560}>
            <Table striped>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>{t('history.thFailureRepo')}</Table.Th>
                  <Table.Th>{t('history.thFailureAsset')}</Table.Th>
                  <Table.Th>{t('history.thFailureReason')}</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {detail.asset_failures.map((failure, index) => (
                  <Table.Tr key={`${failure.source_repo}-${index}`}>
                    <Table.Td>{failure.source_repo}</Table.Td>
                    <Table.Td>{failure.asset}</Table.Td>
                    <Table.Td>{failure.reason}</Table.Td>
                  </Table.Tr>
                ))}
              </Table.Tbody>
            </Table>
          </Table.ScrollContainer>
        )}
      </Stack>
      {detail.omitted_asset_failures > 0 && (
        <Text c="orange" size="sm">
          {t('history.failuresOmitted', { count: detail.omitted_asset_failures })}
        </Text>
      )}
    </Stack>
  );
}
