// 迁移向导 · 步骤五：轮询任务详情展示进度，终态停止轮询并放开查看报告入口。
import { useEffect, useState } from 'react';
import type { components } from '@jianartifact/devmock/schema';
import {
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  ErrorState,
  Skeleton,
  StatusBadge,
} from '@jianartifact/ui';
import { ApiClientError } from '../../api/client';
import { migrationApi } from '../../api/endpoints';
import { formatBytes } from '../../lib/format';
import { isTerminalStatus } from './types';

type TaskInfo = components['schemas']['TaskInfo'];

// 轮询间隔（毫秒）
const POLL_INTERVAL_MS = 1500;

export function ProgressStep({
  taskId,
  onViewReport,
  onRestart,
}: {
  taskId: string;
  onViewReport: () => void;
  onRestart: () => void;
}) {
  const [task, setTask] = useState<TaskInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [reloadToken, setReloadToken] = useState(0);

  useEffect(() => {
    let active = true;
    let timer = 0;
    const poll = () => {
      migrationApi
        .taskDetail(taskId)
        .then((info) => {
          if (!active) return;
          setTask(info);
          setError(null);
          // 进入终态即停止轮询
          if (isTerminalStatus(info.status)) window.clearInterval(timer);
        })
        .catch((err: unknown) => {
          if (!active) return;
          setError(err instanceof ApiClientError ? err.message : '获取任务进度失败');
          window.clearInterval(timer);
        });
    };
    poll();
    timer = window.setInterval(poll, POLL_INTERVAL_MS);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [taskId, reloadToken]);

  if (error) {
    return <ErrorState message={error} onRetry={() => setReloadToken((value) => value + 1)} />;
  }
  if (!task) {
    return <Skeleton className="h-40 w-full" />;
  }

  const percent =
    task.total_objects > 0
      ? Math.min(100, Math.round((task.done_objects / task.total_objects) * 100))
      : 0;
  const terminal = isTerminalStatus(task.status);

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center justify-between">
          <span>迁移进度</span>
          <StatusBadge kind="task-status" value={task.status} />
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <div className="flex flex-col gap-2">
          <div className="h-2 w-full overflow-hidden rounded-full bg-muted">
            <div
              className="h-full rounded-full bg-primary transition-all"
              style={{ width: `${percent}%` }}
            />
          </div>
          <div className="flex justify-between text-sm text-muted-foreground">
            <span>
              对象 {task.done_objects} / {task.total_objects}（{percent}%）
            </span>
            <span>
              字节 {formatBytes(task.done_bytes)} / {formatBytes(task.total_bytes)}
            </span>
          </div>
        </div>

        <div className="flex justify-between">
          <Button type="button" variant="outline" onClick={onRestart}>
            重新开始
          </Button>
          <Button type="button" disabled={!terminal} onClick={onViewReport}>
            查看报告
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}
