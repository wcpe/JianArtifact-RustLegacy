// 迁移向导 · 步骤三：展示计划预览（对象/字节/磁盘估算、映射明细与冲突），确认后创建执行任务。
import { useState } from 'react';
import type { components } from '@jianartifact/devmock/schema';
import {
  Alert,
  AlertDescription,
  AlertTitle,
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  StatusBadge,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  cn,
} from '@jianartifact/ui';
import { ApiClientError } from '../../api/client';
import { migrationApi } from '../../api/endpoints';
import { formatBytes } from '../../lib/format';
import { StepLoading } from './StepLoading';

type MigrationPlan = components['schemas']['MigrationPlan'];
type TaskInfo = components['schemas']['TaskInfo'];
type DiscoveredRepo = components['schemas']['DiscoveredRepo'];
type RepoMapping = components['schemas']['RepoMapping'];

// 单行映射明细：mapping 与其源仓库的发现信息（按源名 join，缓存缺失时为 null）
interface MappingRow {
  mapping: RepoMapping;
  repo: DiscoveredRepo | null;
}

// 仓库映射明细表：源/目标/格式/类型/对象数/字节数/凭据状态/是否排除，被排除行淡化
function MappingTable({ rows }: { rows: MappingRow[] }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>仓库映射明细（{rows.length}）</CardTitle>
      </CardHeader>
      <CardContent>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead scope="col">源仓库</TableHead>
              <TableHead scope="col">目标仓库</TableHead>
              <TableHead scope="col">格式</TableHead>
              <TableHead scope="col">类型</TableHead>
              <TableHead scope="col">对象数</TableHead>
              <TableHead scope="col">字节数</TableHead>
              <TableHead scope="col">凭据状态</TableHead>
              <TableHead scope="col">是否排除</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map(({ mapping, repo }) => (
              <TableRow key={mapping.source} className={cn(mapping.excluded && 'opacity-60')}>
                <TableCell className="font-medium">{mapping.source}</TableCell>
                <TableCell>{mapping.target ?? mapping.source}</TableCell>
                <TableCell>{repo?.format ?? '—'}</TableCell>
                <TableCell>
                  {repo ? <StatusBadge kind="repo-type" value={repo.repo_type} /> : '—'}
                </TableCell>
                <TableCell>{repo ? repo.object_count : '—'}</TableCell>
                <TableCell>{repo ? formatBytes(repo.total_bytes) : '—'}</TableCell>
                <TableCell>
                  {repo ? (
                    <Badge variant={repo.has_credential_ref ? 'outline' : 'secondary'}>
                      {repo.has_credential_ref ? '已映射' : '未映射'}
                    </Badge>
                  ) : (
                    '—'
                  )}
                </TableCell>
                <TableCell>
                  {mapping.excluded ? (
                    <Badge variant="destructive">排除</Badge>
                  ) : (
                    <Badge variant="outline">纳入</Badge>
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </CardContent>
    </Card>
  );
}

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
  repositories,
  onBack,
  onExecuted,
}: {
  plan: MigrationPlan;
  // 发现结果中的仓库信息（供映射明细 join；缓存缺失时可为空数组）
  repositories: DiscoveredRepo[];
  onBack: () => void;
  onExecuted: (task: TaskInfo) => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // 按源仓库名 join 发现信息，构造明细行
  const repoByName = new Map(repositories.map((repo) => [repo.name, repo]));
  const mappingRows: MappingRow[] = plan.mappings.map((mapping) => ({
    mapping,
    repo: repoByName.get(mapping.source) ?? null,
  }));

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

  // 创建任务进行中：以处理态卡片替换计划预览，明确告知正在创建
  if (submitting) {
    return <StepLoading message="正在创建迁移任务…" />;
  }

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

      <MappingTable rows={mappingRows} />

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
        <Button type="button" disabled={plan.blocked} onClick={execute}>
          开始迁移
        </Button>
      </div>
    </div>
  );
}
