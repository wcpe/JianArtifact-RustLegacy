// 仪表盘：实例概览。展示仓库总数、Token 数与（管理员）迁移任务数三张概览卡。
import { useCallback } from 'react';
import { Boxes, KeyRound, Workflow } from 'lucide-react';
import { Card, ErrorState, PageHeader, Skeleton, cn } from '@jianartifact/ui';
import { migrationApi, repoApi, tokenApi } from '../api/endpoints';
import { useAuth } from '../auth/AuthContext';
import { useAsync } from '../lib/useAsync';
import { useRegisterRefresh } from '../routes/RefreshContext';

// 概览卡图标块配色：不同指标以不同色相区分（明暗各一套）
const TONES = {
  blue: 'bg-blue-100 text-blue-600 dark:bg-blue-950/60 dark:text-blue-400',
  violet: 'bg-violet-100 text-violet-600 dark:bg-violet-950/60 dark:text-violet-400',
  teal: 'bg-teal-100 text-teal-600 dark:bg-teal-950/60 dark:text-teal-400',
} as const;

// 单张概览卡：左侧标题 + 大号数值，右侧彩色图标块
function StatCard({
  icon: Icon,
  title,
  value,
  tone,
}: {
  icon: typeof Boxes;
  title: string;
  value: number;
  tone: keyof typeof TONES;
}) {
  return (
    <Card className="p-5">
      <div className="flex items-start justify-between gap-3">
        <div className="flex flex-col gap-1">
          <span className="text-sm text-muted-foreground">{title}</span>
          <span className="text-3xl font-bold tracking-tight tabular-nums">{value}</span>
        </div>
        <div className={cn('flex size-10 items-center justify-center rounded-lg', TONES[tone])}>
          <Icon className="size-5" aria-hidden />
        </div>
      </div>
    </Card>
  );
}

export function DashboardPage() {
  const { isAdmin } = useAuth();
  // 仅取总数：请求最小分页即可
  const reposLoader = useCallback(() => repoApi.list({ page: 1, page_size: 1 }), []);
  const tokensLoader = useCallback(() => tokenApi.list({ page: 1, page_size: 1 }), []);
  // 迁移任务列表为管理员端点：非管理员不请求，避免触发 403
  const tasksLoader = useCallback(
    () => (isAdmin ? migrationApi.listTasks({ page: 1, page_size: 1 }) : Promise.resolve(null)),
    [isAdmin],
  );

  const repos = useAsync(reposLoader);
  const tokens = useAsync(tokensLoader);
  const tasks = useAsync(tasksLoader);

  const loading = repos.loading || tokens.loading || tasks.loading;
  const error = repos.error ?? tokens.error ?? tasks.error;

  const retry = () => {
    repos.reload();
    tokens.reload();
    tasks.reload();
  };

  // 顶栏刷新按钮触发本页三块数据重载
  useRegisterRefresh(retry);

  return (
    <div className="flex h-full flex-col gap-6 overflow-y-auto">
      <PageHeader title="仪表盘" description="实例整体概览" />
      {loading ? (
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          <Skeleton className="h-24 w-full" />
          <Skeleton className="h-24 w-full" />
          <Skeleton className="h-24 w-full" />
        </div>
      ) : error ? (
        <ErrorState message={error.message} onRetry={retry} />
      ) : (
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          <StatCard icon={Boxes} title="仓库总数" value={repos.data?.total ?? 0} tone="blue" />
          <StatCard
            icon={KeyRound}
            title="Token 数"
            value={tokens.data?.total ?? 0}
            tone="violet"
          />
          {isAdmin && tasks.data ? (
            <StatCard icon={Workflow} title="迁移任务数" value={tasks.data.total} tone="teal" />
          ) : null}
        </div>
      )}
    </div>
  );
}
