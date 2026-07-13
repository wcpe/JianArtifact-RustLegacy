// 仓库列表：分页表格 + 名称直达详情 + 操作列（设置/删除）。管理员可经对话框新建仓库。
import { useCallback, useState, type FormEvent } from 'react';
import { Link } from 'react-router-dom';
import { Plus, Settings, Trash2 } from 'lucide-react';
import type { components } from '@jianartifact/devmock/schema';
import {
  Alert,
  AlertDescription,
  AlertTitle,
  Badge,
  Button,
  DataTable,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
  EmptyState,
  ErrorState,
  Input,
  Label,
  PageHeader,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  StatusBadge,
  type ColumnDef,
} from '@jianartifact/ui';
import { ApiClientError } from '../api/client';
import { repoApi } from '../api/endpoints';
import { useAuth } from '../auth/AuthContext';
import { useAsync } from '../lib/useAsync';
import { useRegisterRefresh } from '../routes/RefreshContext';
import { formatBytes } from '../lib/format';
import { NameFilterInput } from '../components/NameFilterInput';
import { DeleteRepoDialog } from './repository/DeleteRepoDialog';

type RepoInfo = components['schemas']['RepoInfo'];
type Format = components['schemas']['Format'];
type RepoType = components['schemas']['RepoType'];
type Visibility = components['schemas']['Visibility'];

// 启停状态徽标：启用为绿色软底，已停用为中性灰
function RepoStateBadge({ disabled }: { disabled: boolean }) {
  if (disabled) {
    return <Badge variant="secondary">已停用</Badge>;
  }
  return (
    <Badge
      variant="outline"
      className="border-transparent bg-emerald-100 text-emerald-700 dark:bg-emerald-950 dark:text-emerald-300"
    >
      启用
    </Badge>
  );
}

// 行内操作：设置（直达详情配置 tab）与删除（确认对话框）
function RepoRowActions({
  repo,
  onDeleted,
}: {
  repo: RepoInfo;
  onDeleted: (name: string) => void;
}) {
  return (
    <div className="flex items-center gap-1">
      <Button asChild variant="ghost" size="icon-sm" aria-label={`仓库设置 ${repo.name}`}>
        <Link to={`/repos/${repo.name}#config`}>
          <Settings className="size-4" aria-hidden />
        </Link>
      </Button>
      <DeleteRepoDialog
        repoName={repo.name}
        onDeleted={() => onDeleted(repo.name)}
        trigger={
          <Button variant="ghost" size="icon-sm" aria-label={`删除仓库 ${repo.name}`}>
            <Trash2 className="size-4 text-destructive" aria-hidden />
          </Button>
        }
      />
    </div>
  );
}

// 列定义：名称直达详情，类型/可见性用语义徽标，统计列人类可读，proxy 展示上游
function buildColumns(onDeleted: (name: string) => void): ColumnDef<RepoInfo>[] {
  return [
    {
      accessorKey: 'name',
      header: '名称',
      cell: ({ row }) => (
        <Link
          to={`/repos/${row.original.name}`}
          className="font-medium text-primary underline-offset-4 hover:underline"
        >
          {row.original.name}
        </Link>
      ),
    },
    { accessorKey: 'format', header: '格式' },
    {
      accessorKey: 'repo_type',
      header: '类型',
      cell: ({ row }) => <StatusBadge kind="repo-type" value={row.original.repo_type} />,
    },
    {
      accessorKey: 'visibility',
      header: '可见性',
      cell: ({ row }) => <StatusBadge kind="visibility" value={row.original.visibility} />,
    },
    { accessorKey: 'object_count', header: '制品数' },
    {
      accessorKey: 'total_bytes',
      header: '总大小',
      cell: ({ row }) => formatBytes(row.original.total_bytes),
    },
    {
      accessorKey: 'disabled',
      header: '状态',
      cell: ({ row }) => <RepoStateBadge disabled={row.original.disabled} />,
    },
    {
      accessorKey: 'upstream_url',
      header: '上游',
      cell: ({ row }) =>
        row.original.repo_type === 'proxy' ? (row.original.upstream_url ?? '—') : '—',
    },
    {
      id: 'actions',
      header: '操作',
      cell: ({ row }) => <RepoRowActions repo={row.original} onDeleted={onDeleted} />,
    },
  ];
}

// 新建仓库对话框：填写名称/格式/类型/可见性并提交
function CreateRepoDialog({ onCreated }: { onCreated: () => void }) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState('');
  const [format, setFormat] = useState<Format>('raw');
  const [repoType, setRepoType] = useState<RepoType>('hosted');
  const [visibility, setVisibility] = useState<Visibility>('private');
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // 关闭时重置表单，避免下次打开残留上次输入
  const handleOpenChange = (next: boolean) => {
    setOpen(next);
    if (!next) {
      setName('');
      setFormat('raw');
      setRepoType('hosted');
      setVisibility('private');
      setError(null);
    }
  };

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      await repoApi.create({ name, format, repo_type: repoType, visibility });
      handleOpenChange(false);
      onCreated();
    } catch (err) {
      setError(err instanceof ApiClientError ? err.message : '创建仓库失败');
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogTrigger asChild>
        <Button size="sm">
          <Plus className="size-4" aria-hidden />
          新建仓库
        </Button>
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>新建仓库</DialogTitle>
          <DialogDescription>格式与类型在创建后不可更改，请谨慎选择。</DialogDescription>
        </DialogHeader>
        <form className="flex flex-col gap-4" onSubmit={handleSubmit}>
          {error ? (
            <Alert variant="destructive">
              <AlertTitle>创建失败</AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          ) : null}
          <div className="flex flex-col gap-2">
            <Label htmlFor="repo-name">仓库名</Label>
            <Input
              id="repo-name"
              value={name}
              placeholder="小写字母、数字与连字符"
              onChange={(event) => setName(event.target.value)}
              required
            />
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="repo-format">格式</Label>
            <Select value={format} onValueChange={(value) => setFormat(value as Format)}>
              <SelectTrigger id="repo-format" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="raw">Raw</SelectItem>
                <SelectItem value="maven">Maven</SelectItem>
                <SelectItem value="npm">npm</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="repo-type">类型</Label>
            <Select value={repoType} onValueChange={(value) => setRepoType(value as RepoType)}>
              <SelectTrigger id="repo-type" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="hosted">宿主</SelectItem>
                <SelectItem value="proxy">代理</SelectItem>
                <SelectItem value="group">聚合组</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="repo-visibility">可见性</Label>
            <Select
              value={visibility}
              onValueChange={(value) => setVisibility(value as Visibility)}
            >
              <SelectTrigger id="repo-visibility" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="private">私有</SelectItem>
                <SelectItem value="public">公开</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <DialogFooter>
            <Button type="submit" disabled={submitting || name.trim() === ''}>
              {submitting ? '创建中…' : '创建'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function RepositoriesPage() {
  const { isAdmin } = useAuth();
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);
  const [q, setQ] = useState('');
  const [notice, setNotice] = useState<string | null>(null);

  const loader = useCallback(
    () => repoApi.list({ page, page_size: pageSize, q: q === '' ? undefined : q }),
    [page, pageSize, q],
  );
  const { data, error, loading, reload } = useAsync(loader);
  useRegisterRefresh(reload);

  // 筛选值落定后传 q 并回到第 1 页，避免停留在越界页码
  const handleFilter = useCallback((value: string) => {
    setQ(value.trim());
    setPage(1);
  }, []);

  const changePageSize = (size: number) => {
    setPageSize(size);
    setPage(1);
  };

  // 删除成功：页首提示并刷新列表
  const handleDeleted = (name: string) => {
    setNotice(`已删除仓库 ${name}`);
    reload();
  };

  const columns = buildColumns(handleDeleted);

  return (
    <div className="flex h-full flex-col gap-6">
      <PageHeader
        title="仓库"
        description="管理全部仓库的可见性与权限"
        actions={isAdmin ? <CreateRepoDialog onCreated={reload} /> : undefined}
      />
      <NameFilterInput onDebouncedChange={handleFilter} />
      {notice ? (
        <Alert>
          <AlertTitle>操作成功</AlertTitle>
          <AlertDescription>{notice}</AlertDescription>
        </Alert>
      ) : null}
      {error ? (
        <ErrorState message={error.message} onRetry={reload} />
      ) : (
        // 表格区固定高度、由 DataTable 内部管理滚动：表格内滚、分页固定底部
        <div className="min-h-0 flex-1 overflow-hidden">
          <DataTable
            columns={columns}
            data={data?.items ?? []}
            isLoading={loading}
            emptyState={<EmptyState title="暂无仓库" description="当前没有你可访问的仓库" />}
            page={page}
            pageSize={pageSize}
            total={data?.total ?? 0}
            onPageChange={setPage}
            onPageSizeChange={changePageSize}
          />
        </div>
      )}
    </div>
  );
}
