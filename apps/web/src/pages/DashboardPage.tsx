// 仪表盘：实例概览。展示仓库总数、Token 数与（管理员）迁移任务数三张概览卡。
import { useCallback } from 'react';
import { Boxes, KeyRound, Workflow } from 'lucide-react';
import {
  Card,
  CardDescription,
  CardHeader,
  CardTitle,
  ErrorState,
  PageHeader,
  Skeleton,
} from '@jianartifact/ui';
import { migrationApi, repoApi, tokenApi } from '../api/endpoints';
import { useAuth } from '../auth/AuthContext';
import { useAsync } from '../lib/useAsync';

// 单张概览卡：图标 + 标题 + 大号数值
function StatCard({
  icon: Icon,
  title,
  value,
}: {
  icon: typeof Boxes;
  title: string;
  value: number;
}) {
  return (
    <Card>
      <CardHeader>
        <CardDescription className="flex items-center gap-2">
          <Icon className="size-4" aria-hidden />
          {title}
        </CardDescription>
        <CardTitle className="text-3xl">{value}</CardTitle>
      </CardHeader>
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

  return (
    <div className="flex flex-col gap-6">
      <PageHeader title="仪表盘" description="实例整体概览" />
      {loading ? (
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          <Skeleton className="h-28 w-full" />
          <Skeleton className="h-28 w-full" />
          <Skeleton className="h-28 w-full" />
        </div>
      ) : error ? (
        <ErrorState message={error.message} onRetry={retry} />
      ) : (
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          <StatCard icon={Boxes} title="仓库总数" value={repos.data?.total ?? 0} />
          <StatCard icon={KeyRound} title="Token 数" value={tokens.data?.total ?? 0} />
          {isAdmin && tasks.data ? (
            <StatCard icon={Workflow} title="迁移任务数" value={tasks.data.total} />
          ) : null}
        </div>
      )}
    </div>
  );
}
