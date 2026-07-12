// 迁移向导 · 步骤三：展示计划预览（对象/字节/磁盘估算与冲突），确认后创建执行任务。
import { useState } from 'react';
import type { components } from '@jianartifact/devmock/schema';
import {
  Alert,
  AlertDescription,
  AlertTitle,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@jianartifact/ui';
import { ApiClientError } from '../../api/client';
import { migrationApi } from '../../api/endpoints';
import { formatBytes } from '../../lib/format';

type MigrationPlan = components['schemas']['MigrationPlan'];
type TaskInfo = components['schemas']['TaskInfo'];

// 计划概览的一张统计卡
function SummaryCard({ title, value }: { title: string; value: string }) {
  return (
    <Card>
      <CardHeader>
        <CardDescription>{title}</CardDescription>
        <CardTitle className="text-xl">{value}</CardTitle>
      </CardHeader>
    </Card>
  );
}

export function PlanStep({
  plan,
  onBack,
  onExecuted,
}: {
  plan: MigrationPlan;
  onBack: () => void;
  onExecuted: (task: TaskInfo) => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const execute = async () => {
    setError(null);
    setSubmitting(true);
    try {
      const task = await migrationApi.createTask(plan.plan_id);
      onExecuted(task);
    } catch (err) {
      setError(err instanceof ApiClientError ? err.message : '创建任务失败');
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="flex flex-col gap-4">
      {error ? (
        <Alert variant="destructive">
          <AlertTitle>创建任务失败</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}

      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <SummaryCard title="对象总数" value={String(plan.object_count)} />
        <SummaryCard title="字节总数" value={formatBytes(plan.total_bytes)} />
        <SummaryCard title="目标磁盘估算" value={formatBytes(plan.estimated_disk_bytes)} />
        <SummaryCard title="磁盘空间" value={plan.disk_sufficient ? '充足' : '不足'} />
      </div>

      {plan.blocked ? (
        <Alert variant="destructive">
          <AlertTitle>存在阻断性冲突</AlertTitle>
          <AlertDescription>请返回上一步调整目标名或排除冲突仓库后再执行。</AlertDescription>
        </Alert>
      ) : null}

      {plan.conflicts.length > 0 ? (
        <Card>
          <CardHeader>
            <CardTitle>冲突（{plan.conflicts.length}）</CardTitle>
          </CardHeader>
          <CardContent>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead scope="col">类别</TableHead>
                  <TableHead scope="col">目标仓库</TableHead>
                  <TableHead scope="col">路径</TableHead>
                  <TableHead scope="col">说明</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {plan.conflicts.map((conflict, index) => (
                  <TableRow key={`${conflict.target}-${conflict.path}-${index}`}>
                    <TableCell>{conflict.category}</TableCell>
                    <TableCell>{conflict.target}</TableCell>
                    <TableCell>{conflict.path || '—'}</TableCell>
                    <TableCell className="text-muted-foreground">{conflict.detail}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </CardContent>
        </Card>
      ) : null}

      <div className="flex justify-between">
        <Button type="button" variant="outline" onClick={onBack}>
          上一步
        </Button>
        <Button type="button" disabled={submitting || plan.blocked} onClick={execute}>
          {submitting ? '创建任务中…' : '开始迁移'}
        </Button>
      </div>
    </div>
  );
}
