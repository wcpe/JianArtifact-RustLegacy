// API Token：列出本人 Token、签发新 Token（明文仅展示一次）、吊销 Token。
import { useCallback, useState, type FormEvent } from 'react';
import { Plus } from 'lucide-react';
import type { components } from '@jianartifact/devmock/schema';
import {
  Alert,
  AlertDescription,
  AlertTitle,
  Button,
  DataTable,
  Dialog,
  DialogClose,
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
  type ColumnDef,
} from '@jianartifact/ui';
import { ApiClientError } from '../api/client';
import { tokenApi } from '../api/endpoints';
import { useAsync } from '../lib/useAsync';
import { formatDateTime } from '../lib/format';

type TokenInfo = components['schemas']['TokenInfo'];
type CreatedToken = components['schemas']['CreatedToken'];

// 吊销确认按钮：二次确认后调用吊销并刷新列表
function RevokeTokenButton({ token, onRevoked }: { token: TokenInfo; onRevoked: () => void }) {
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const handleRevoke = async () => {
    setError(null);
    setSubmitting(true);
    try {
      await tokenApi.revoke(token.id);
      setOpen(false);
      onRevoked();
    } catch (err) {
      setError(err instanceof ApiClientError ? err.message : '吊销失败');
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button variant="outline" size="sm">
          吊销
        </Button>
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>吊销 Token</DialogTitle>
          <DialogDescription>
            吊销后使用该 Token 的客户端将立即失效，此操作不可撤销。
          </DialogDescription>
        </DialogHeader>
        {error ? (
          <Alert variant="destructive">
            <AlertTitle>吊销失败</AlertTitle>
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        ) : null}
        <DialogFooter>
          <DialogClose asChild>
            <Button variant="outline">取消</Button>
          </DialogClose>
          <Button variant="destructive" disabled={submitting} onClick={handleRevoke}>
            {submitting ? '吊销中…' : `吊销「${token.name}」`}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// 新建 Token 对话框：提交后展示一次性明文，关闭即不可再见
function CreateTokenDialog({ onCreated }: { onCreated: () => void }) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState('');
  const [created, setCreated] = useState<CreatedToken | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // 关闭时清空明文与输入，保证明文不再可见
  const handleOpenChange = (next: boolean) => {
    setOpen(next);
    if (!next) {
      setName('');
      setCreated(null);
      setError(null);
    }
  };

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      const result = await tokenApi.create({ name });
      setCreated(result);
      onCreated();
    } catch (err) {
      setError(err instanceof ApiClientError ? err.message : '签发失败');
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogTrigger asChild>
        <Button size="sm">
          <Plus className="size-4" aria-hidden />
          新建 Token
        </Button>
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>新建 API Token</DialogTitle>
          <DialogDescription>Token 明文仅在签发后展示一次，请立即复制保存。</DialogDescription>
        </DialogHeader>
        {created ? (
          <div className="flex flex-col gap-4">
            <Alert>
              <AlertTitle>请立即保存 Token 明文（仅此一次）</AlertTitle>
              <AlertDescription>
                <code className="mt-1 block break-all rounded bg-muted px-2 py-1 font-mono text-sm">
                  {created.token}
                </code>
                <span className="mt-2 block">关闭对话框后将无法再次查看此明文。</span>
              </AlertDescription>
            </Alert>
            <DialogFooter>
              <DialogClose asChild>
                <Button>我已保存，关闭</Button>
              </DialogClose>
            </DialogFooter>
          </div>
        ) : (
          <form className="flex flex-col gap-4" onSubmit={handleSubmit}>
            {error ? (
              <Alert variant="destructive">
                <AlertTitle>签发失败</AlertTitle>
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            ) : null}
            <div className="flex flex-col gap-2">
              <Label htmlFor="token-name">Token 名称</Label>
              <Input
                id="token-name"
                value={name}
                placeholder="便于区分用途，如 ci 发布"
                onChange={(event) => setName(event.target.value)}
                required
              />
            </div>
            <DialogFooter>
              <Button type="submit" disabled={submitting || name.trim() === ''}>
                {submitting ? '签发中…' : '创建'}
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

export function TokensPage() {
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);

  const loader = useCallback(() => tokenApi.list({ page, page_size: pageSize }), [page, pageSize]);
  const { data, error, loading, reload } = useAsync(loader);

  const columns: ColumnDef<TokenInfo>[] = [
    { accessorKey: 'name', header: '名称' },
    {
      accessorKey: 'created_at',
      header: '创建时间',
      cell: ({ row }) => formatDateTime(row.original.created_at),
    },
    {
      id: 'actions',
      header: '操作',
      cell: ({ row }) => <RevokeTokenButton token={row.original} onRevoked={reload} />,
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="API Token"
        description="用于 CLI 与原生客户端鉴权"
        actions={<CreateTokenDialog onCreated={reload} />}
      />
      {error ? (
        <ErrorState message={error.message} onRetry={reload} />
      ) : (
        <DataTable
          columns={columns}
          data={data?.items ?? []}
          isLoading={loading}
          emptyState={
            <EmptyState title="暂无 Token" description="点击右上角新建你的第一个 Token" />
          }
          page={page}
          pageSize={pageSize}
          total={data?.total ?? 0}
          onPageChange={setPage}
          onPageSizeChange={(size) => {
            setPageSize(size);
            setPage(1);
          }}
        />
      )}
    </div>
  );
}
