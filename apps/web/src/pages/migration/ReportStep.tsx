// 迁移向导 · 步骤六：展示任务报告——分仓统计、失败样本、不支持项与被排除仓库。
import { useCallback } from 'react';
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
  Skeleton,
  StatusBadge,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@jianartifact/ui';
import { migrationApi } from '../../api/endpoints';
import { useAsync } from '../../lib/useAsync';
import { formatBytes } from '../../lib/format';

export function ReportStep({ taskId, onRestart }: { taskId: string; onRestart: () => void }) {
  const loader = useCallback(() => migrationApi.report(taskId), [taskId]);
  const { data: report, error, loading, reload } = useAsync(loader);

  if (loading) {
    return <Skeleton className="h-64 w-full" />;
  }
  if (error || !report) {
    return <ErrorState message={error?.message ?? '加载报告失败'} onRetry={reload} />;
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center gap-3">
        <span className="text-sm text-muted-foreground">最终状态</span>
        <StatusBadge kind="task-status" value={report.status} />
      </div>

      <Card>
        <CardHeader>
          <CardTitle>分仓库报告（{report.repositories.length}）</CardTitle>
        </CardHeader>
        <CardContent>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead scope="col">源 → 目标</TableHead>
                <TableHead scope="col">迁移</TableHead>
                <TableHead scope="col">跳过</TableHead>
                <TableHead scope="col">失败</TableHead>
                <TableHead scope="col">总数</TableHead>
                <TableHead scope="col">大小</TableHead>
                <TableHead scope="col">凭据</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {report.repositories.map((repo) => (
                <TableRow key={`${repo.source}-${repo.target}`}>
                  <TableCell className="font-medium">
                    {repo.source} → {repo.target}
                  </TableCell>
                  <TableCell>{repo.migrated}</TableCell>
                  <TableCell>{repo.skipped}</TableCell>
                  <TableCell>{repo.failed}</TableCell>
                  <TableCell>{repo.total}</TableCell>
                  <TableCell>{formatBytes(repo.total_bytes)}</TableCell>
                  <TableCell>{repo.credential_status}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      {report.failures.length > 0 ? (
        <Card>
          <CardHeader>
            <CardTitle>失败样本（{report.failures.length}）</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead scope="col">仓库</TableHead>
                  <TableHead scope="col">对象</TableHead>
                  <TableHead scope="col">类别</TableHead>
                  <TableHead scope="col">说明</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {report.failures.map((failure) => (
                  <TableRow key={`${failure.repo}-${failure.object_key}`}>
                    <TableCell>{failure.repo}</TableCell>
                    <TableCell className="break-all">{failure.object_key}</TableCell>
                    <TableCell>{failure.category}</TableCell>
                    <TableCell className="text-muted-foreground">{failure.message}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
            {report.failures_omitted > 0 ? (
              <p className="text-sm text-muted-foreground">
                另有 {report.failures_omitted} 条失败因样本上限被省略。
              </p>
            ) : null}
          </CardContent>
        </Card>
      ) : null}

      {report.unsupported.length > 0 ? (
        <Alert>
          <AlertTitle>不支持项（{report.unsupported.length}）</AlertTitle>
          <AlertDescription>
            <ul className="flex flex-col gap-1">
              {report.unsupported.map((item) => (
                <li key={`${item.category}-${item.name}`}>
                  {item.name}（{item.category}）— {item.reason}
                </li>
              ))}
            </ul>
          </AlertDescription>
        </Alert>
      ) : null}

      {report.excluded.length > 0 ? (
        <Alert>
          <AlertTitle>已排除仓库（{report.excluded.length}）</AlertTitle>
          <AlertDescription>{report.excluded.join('、')}</AlertDescription>
        </Alert>
      ) : null}

      <div className="flex justify-end">
        <Button type="button" onClick={onRestart}>
          新的迁移
        </Button>
      </div>
    </div>
  );
}
