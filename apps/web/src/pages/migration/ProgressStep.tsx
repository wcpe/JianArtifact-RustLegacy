// 迁移向导 · 步骤五：轮询任务详情展示进度，终态停止轮询并放开查看报告入口。
// 按状态提供任务控制：running→暂停/取消，paused→继续/取消，终态失败类→重试（续传）。
import { useEffect, useState } from 'react';
import { Loader2 } from 'lucide-react';
import type { components } from '@jianartifact/devmock/schema';
import {
  Alert,
  AlertDescription,
  AlertTitle,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  ErrorState,
  Input,
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

// 控制动作标识
type ControlKey = 'pause' | 'resume' | 'cancel' | 'retry' | 'finalize';

// 控制动作的中文按钮文案
const CONTROL_LABEL: Record<ControlKey, string> = {
  pause: '暂停',
  resume: '继续',
  cancel: '取消',
  retry: '重试',
  finalize: '执行最终增量扫描',
};

// 状态 → 可用控制动作映射（对齐后端十态状态机的合法转移，未列出的状态不提供控制按钮）：
// queued/running/pausing/paused 可取消，running 可暂停，paused 可继续，失败类终态可重试续传。
const CONTROLS_BY_STATUS: Partial<Record<string, ControlKey[]>> = {
  queued: ['cancel'],
  running: ['pause', 'cancel'],
  pausing: ['cancel'],
  paused: ['resume', 'cancel'],
  failed: ['retry'],
  partial: ['retry'],
  interrupted: ['retry'],
  cancelled: ['retry'],
};

// 各状态的中文阶段说明（进度条附近醒目展示，比状态徽标更完整地陈述当前处境与可用操作）。
// 措辞刻意避开与对应状态徽标标签相同的子串，避免测试/选择器按文本命中时与徽标歧义。
const STATUS_PHASE: Record<string, string> = {
  queued: '正在排队，等待执行器空闲',
  running: '迁移进行中，正在传输对象与字节',
  pausing: '正在暂停，等待当前对象收尾',
  paused: '任务暂停，可继续或取消',
  cancelling: '正在取消，等待安全停止',
  cancelled: '任务取消，迁移未完成',
  succeeded: '迁移完成，全部对象成功',
  partial: '部分对象未成功迁移，可重试续传',
  failed: '迁移未成功，可重试续传',
  interrupted: '任务因服务中断而暂停，可重试续传',
};

// 活动态（后台仍在推进的状态）：阶段说明附旋转指示，强化“处理中”感知
const ACTIVE_STATUSES: ReadonlySet<string> = new Set([
  'queued',
  'running',
  'pausing',
  'cancelling',
]);

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
  const [controlError, setControlError] = useState<string | null>(null);
  const [controlBusy, setControlBusy] = useState(false);
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
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

  // 执行控制动作：成功以响应体立即刷新任务详情，并重启轮询；在线任务重启后可在此重新提交临时凭据。
  const runControl = async (key: ControlKey) => {
    setControlError(null);
    if ((username || password) && (!username || !password)) {
      setControlError('用户名与密码必须同时填写');
      return;
    }
    const credentials = username && password ? { username, password } : undefined;
    setControlBusy(true);
    try {
      let info: TaskInfo;
      if (key === 'pause') info = await migrationApi.pause(taskId);
      else if (key === 'resume') info = await migrationApi.resume(taskId);
      else if (key === 'cancel') info = await migrationApi.cancel(taskId);
      else if (key === 'retry') info = await migrationApi.retry(taskId, credentials);
      else info = await migrationApi.finalize(taskId, credentials);
      setPassword('');
      setTask(info);
      setReloadToken((value) => value + 1);
    } catch (err) {
      setControlError(err instanceof ApiClientError ? err.message : '任务控制操作失败');
    } finally {
      setControlBusy(false);
    }
  };

  // 轮询失败（含刷新恢复时在途任务已不存在）：除重试外提供“重新开始”逃生出口，避免卡在死任务上
  if (error) {
    return (
      <div className="flex flex-col gap-3">
        <ErrorState message={error} onRetry={() => setReloadToken((value) => value + 1)} />
        <div>
          <Button type="button" variant="outline" onClick={onRestart}>
            重新开始
          </Button>
        </div>
      </div>
    );
  }
  if (!task) {
    return <Skeleton className="h-40 w-full" />;
  }

  const percent =
    task.total_objects > 0
      ? Math.min(100, Math.round((task.done_objects / task.total_objects) * 100))
      : 0;
  const terminal = isTerminalStatus(task.status);
  const controls: ControlKey[] =
    task.status === 'paused' && task.phase === 'awaiting-cutover'
      ? ['finalize', 'cancel']
      : (CONTROLS_BY_STATUS[task.status] ?? []);
  const phase =
    task.phase === 'awaiting-cutover'
      ? '初次迁移已完成，请停写 Nexus 后执行最终增量扫描'
      : task.phase === 'finalizing'
        ? '正在执行最终增量扫描并核对源端稳定性'
        : (STATUS_PHASE[task.status] ?? task.phase);
  const active = ACTIVE_STATUSES.has(task.status);

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center justify-between">
          <span>迁移进度</span>
          <StatusBadge kind="task-status" value={task.status} />
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        {controlError ? (
          <Alert variant="destructive">
            <AlertTitle>操作被拒绝</AlertTitle>
            <AlertDescription>{controlError}</AlertDescription>
          </Alert>
        ) : null}
        {task.status === 'interrupted' ? (
          <Alert>
            <AlertTitle>任务已中断</AlertTitle>
            <AlertDescription>
              任务因服务中断而暂停，可点击重试续传，已完成对象将幂等跳过
            </AlertDescription>
          </Alert>
        ) : null}

        <div className="flex flex-col gap-2">
          {/* 阶段说明：醒目陈述当前处境与可用操作，活动态附旋转指示 */}
          <div className="flex items-center gap-2 text-sm font-medium text-foreground">
            {active ? <Loader2 className="size-4 animate-spin text-primary" aria-hidden /> : null}
            <span>{phase}</span>
          </div>
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

        {controls.includes('retry') || controls.includes('finalize') ? (
          <div className="grid gap-3 rounded-lg border border-border/70 bg-muted/30 p-3 md:grid-cols-2">
            <div className="md:col-span-2 text-sm text-muted-foreground">
              若服务已重启，请为在线 Nexus 任务重新填写临时凭据；凭据仅驻留当前进程内存。
            </div>
            <Input
              aria-label="Nexus 用户名"
              autoComplete="username"
              placeholder="Nexus 用户名（可选）"
              value={username}
              onChange={(event) => setUsername(event.target.value)}
            />
            <Input
              aria-label="Nexus 密码"
              autoComplete="current-password"
              placeholder="Nexus 密码（可选）"
              type="password"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
            />
          </div>
        ) : null}

        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex gap-2">
            <Button type="button" variant="outline" onClick={onRestart}>
              重新开始
            </Button>
            {controls.map((key) => (
              <Button
                key={key}
                type="button"
                variant="outline"
                disabled={controlBusy}
                onClick={() => runControl(key)}
              >
                {CONTROL_LABEL[key]}
              </Button>
            ))}
          </div>
          <Button type="button" disabled={!terminal} onClick={onViewReport}>
            查看报告
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}
