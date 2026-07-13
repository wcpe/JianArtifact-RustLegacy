// 迁移向导 · 步骤二：展示发现结果，勾选纳入的仓库并可改目标名，据此生成计划。
// 勾选/改名随操作缓存到 sessionStorage，刷新或步骤条回退时原样恢复。
import { useEffect, useState, type FormEvent } from 'react';
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
  Input,
  StatusBadge,
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
import { StepLoading } from './StepLoading';
import {
  SELECTIONS_KEY,
  isSelectionsCache,
  loadCache,
  saveCache,
  type RepoSelection,
} from './wizardCache';

type DiscoveryResult = components['schemas']['DiscoveryResult'];
type MigrationPlan = components['schemas']['MigrationPlan'];
type RepoMapping = components['schemas']['RepoMapping'];

export function DiscoveryStep({
  discovery,
  onBack,
  onPlanned,
}: {
  discovery: DiscoveryResult;
  onBack: () => void;
  onPlanned: (plan: MigrationPlan) => void;
}) {
  // 初始：优先取缓存（刷新/回退恢复），否则全部纳入、目标名默认同源名
  const [selections, setSelections] = useState<Record<string, RepoSelection>>(() => {
    const cached = loadCache(SELECTIONS_KEY, isSelectionsCache);
    return Object.fromEntries(
      discovery.repositories.map((repo): [string, RepoSelection] => [
        repo.name,
        cached?.[repo.name] ?? { included: true, target: repo.name },
      ]),
    );
  });
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // 勾选/改名变化即缓存
  useEffect(() => {
    saveCache(SELECTIONS_KEY, selections);
  }, [selections]);

  const patch = (name: string, next: Partial<RepoSelection>) => {
    setSelections((current) => {
      const base = current[name] ?? { included: true, target: name };
      return { ...current, [name]: { ...base, ...next } };
    });
  };

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setError(null);
    setSubmitting(true);
    const mappings: RepoMapping[] = discovery.repositories.map((repo) => {
      const sel = selections[repo.name] ?? { included: true, target: repo.name };
      return { source: repo.name, target: sel.target, excluded: !sel.included };
    });
    try {
      const plan = await migrationApi.plan({ discovery_id: discovery.discovery_id, mappings });
      onPlanned(plan);
    } catch (err) {
      setError(err instanceof ApiClientError ? err.message : '生成计划失败');
    } finally {
      setSubmitting(false);
    }
  };

  // 生成计划进行中：以处理态卡片替换发现表，明确告知正在计算映射与冲突
  if (submitting) {
    return <StepLoading message="正在计算映射与冲突、估算目标空间…" />;
  }

  return (
    <form className="flex flex-col gap-4" onSubmit={handleSubmit}>
      {error ? (
        <Alert variant="destructive">
          <AlertTitle>生成计划失败</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      {discovery.degraded_bare_blob ? (
        <Alert variant="destructive">
          <AlertTitle>裸 blob 降级模式</AlertTitle>
          <AlertDescription>源为裸 blob 目录，本次迁移无法达到全量成功，请知悉。</AlertDescription>
        </Alert>
      ) : null}

      <Card>
        <CardHeader>
          <CardTitle>发现的仓库（{discovery.repositories.length}）</CardTitle>
        </CardHeader>
        <CardContent>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead scope="col">纳入</TableHead>
                <TableHead scope="col">源仓库</TableHead>
                <TableHead scope="col">格式</TableHead>
                <TableHead scope="col">类型</TableHead>
                <TableHead scope="col">对象数</TableHead>
                <TableHead scope="col">大小</TableHead>
                <TableHead scope="col">目标仓库名</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {discovery.repositories.map((repo) => {
                const sel = selections[repo.name] ?? { included: true, target: repo.name };
                return (
                  <TableRow key={repo.name}>
                    <TableCell>
                      <input
                        type="checkbox"
                        aria-label={`纳入 ${repo.name}`}
                        checked={sel.included}
                        onChange={(event) => patch(repo.name, { included: event.target.checked })}
                      />
                    </TableCell>
                    <TableCell className="font-medium">{repo.name}</TableCell>
                    <TableCell>{repo.format}</TableCell>
                    <TableCell>
                      <StatusBadge kind="repo-type" value={repo.repo_type} />
                    </TableCell>
                    <TableCell>{repo.object_count}</TableCell>
                    <TableCell>{formatBytes(repo.total_bytes)}</TableCell>
                    <TableCell>
                      <Input
                        aria-label={`${repo.name} 目标仓库名`}
                        value={sel.target}
                        disabled={!sel.included}
                        onChange={(event) => patch(repo.name, { target: event.target.value })}
                      />
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      {discovery.unsupported.length > 0 ? (
        <Card>
          <CardHeader>
            <CardTitle>不支持项（{discovery.unsupported.length}）</CardTitle>
          </CardHeader>
          <CardContent>
            <ul className="flex flex-col gap-2 text-sm">
              {discovery.unsupported.map((item) => (
                <li key={`${item.category}-${item.name}`}>
                  <span className="font-medium">{item.name}</span>
                  <span className="text-muted-foreground">
                    （{item.category}）— {item.reason}
                  </span>
                </li>
              ))}
            </ul>
          </CardContent>
        </Card>
      ) : null}

      <div className="flex justify-between">
        <Button type="button" variant="outline" onClick={onBack}>
          上一步
        </Button>
        <Button type="submit">生成计划</Button>
      </div>
    </form>
  );
}
