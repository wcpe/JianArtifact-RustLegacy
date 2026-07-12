// 仓库列表：分页表格 + 名称直达详情。管理员可经对话框新建仓库。
import { useCallback, useState, type FormEvent } from 'react';
import { Link } from 'react-router-dom';
import { Plus } from 'lucide-react';
import type { components } from '@jianartifact/devmock/schema';
import {
  Alert,
  AlertDescription,
  AlertTitle,
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
import { formatDateTime } from '../lib/format';

type RepoInfo = components['schemas']['RepoInfo'];
type Format = components['schemas']['Format'];
type RepoType = components['schemas']['RepoType'];
type Visibility = components['schemas']['Visibility'];

// 列定义：名称直达详情，类型/可见性用语义徽标，创建时间格式化
const columns: ColumnDef<RepoInfo>[] = [
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
  {
    accessorKey: 'created_at',
    header: '创建时间',
    cell: ({ row }) => formatDateTime(row.original.created_at),
  },
];

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

  const loader = useCallback(() => repoApi.list({ page, page_size: pageSize }), [page, pageSize]);
  const { data, error, loading, reload } = useAsync(loader);

  const changePageSize = (size: number) => {
    setPageSize(size);
    setPage(1);
  };

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="仓库"
        description="管理全部仓库的可见性与权限"
        actions={isAdmin ? <CreateRepoDialog onCreated={reload} /> : undefined}
      />
      {error ? (
        <ErrorState message={error.message} onRetry={reload} />
      ) : (
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
      )}
    </div>
  );
}
