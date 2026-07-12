// 用户管理（Admin）：用户列表 + 新增用户 + 启停切换与角色调整。
// 后端拒绝（如最后一名管理员保护 403）以其返回消息在页首提示。
import { useCallback, useState, type FormEvent } from 'react';
import { Plus } from 'lucide-react';
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
  ErrorState,
  Input,
  Label,
  PageHeader,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  type ColumnDef,
} from '@jianartifact/ui';
import { ApiClientError } from '../api/client';
import { userApi } from '../api/endpoints';
import { useAsync } from '../lib/useAsync';
import { formatDateTime } from '../lib/format';

type UserInfo = components['schemas']['UserInfo'];
type Role = components['schemas']['Role'];
type UpdateUserRequest = components['schemas']['UpdateUserRequest'];

// 行内用户操作：启停切换与角色调整；失败上抛消息由页首统一提示
function UserActions({
  user,
  onChanged,
  onError,
}: {
  user: UserInfo;
  onChanged: () => void;
  onError: (message: string) => void;
}) {
  const [busy, setBusy] = useState(false);

  const run = async (body: UpdateUserRequest) => {
    setBusy(true);
    try {
      await userApi.update(user.id, body);
      onChanged();
    } catch (err) {
      onError(err instanceof ApiClientError ? err.message : '操作失败');
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-wrap gap-2">
      <Button
        variant="outline"
        size="sm"
        disabled={busy}
        onClick={() => run({ enabled: !user.enabled })}
      >
        {user.enabled ? '禁用' : '启用'}
      </Button>
      <Button
        variant="outline"
        size="sm"
        disabled={busy}
        onClick={() => run({ role: user.role === 'admin' ? 'user' : 'admin' })}
      >
        {user.role === 'admin' ? '降为普通' : '升为管理员'}
      </Button>
    </div>
  );
}

// 新增用户对话框：填写用户名/口令/角色并提交
function CreateUserDialog({ onCreated }: { onCreated: () => void }) {
  const [open, setOpen] = useState(false);
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [role, setRole] = useState<Role>('user');
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const handleOpenChange = (next: boolean) => {
    setOpen(next);
    if (!next) {
      setUsername('');
      setPassword('');
      setRole('user');
      setError(null);
    }
  };

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      await userApi.create({ username, password, role });
      handleOpenChange(false);
      onCreated();
    } catch (err) {
      setError(err instanceof ApiClientError ? err.message : '创建用户失败');
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogTrigger asChild>
        <Button size="sm">
          <Plus className="size-4" aria-hidden />
          新增用户
        </Button>
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>新增用户</DialogTitle>
          <DialogDescription>初始口令仅用于传输，落库为哈希，不可找回。</DialogDescription>
        </DialogHeader>
        <form className="flex flex-col gap-4" onSubmit={handleSubmit}>
          {error ? (
            <Alert variant="destructive">
              <AlertTitle>创建失败</AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          ) : null}
          <div className="flex flex-col gap-2">
            <Label htmlFor="user-username">用户名</Label>
            <Input
              id="user-username"
              value={username}
              onChange={(event) => setUsername(event.target.value)}
              required
            />
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="user-password">初始口令</Label>
            <Input
              id="user-password"
              type="password"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              required
            />
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="user-role">角色</Label>
            <Select value={role} onValueChange={(value) => setRole(value as Role)}>
              <SelectTrigger id="user-role" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="user">普通用户</SelectItem>
                <SelectItem value="admin">管理员</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <DialogFooter>
            <Button
              type="submit"
              disabled={submitting || username.trim() === '' || password === ''}
            >
              {submitting ? '创建中…' : '创建'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function UsersPage() {
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);
  const [actionError, setActionError] = useState<string | null>(null);

  const loader = useCallback(() => userApi.list({ page, page_size: pageSize }), [page, pageSize]);
  const { data, error, loading, reload } = useAsync(loader);

  const columns: ColumnDef<UserInfo>[] = [
    { accessorKey: 'username', header: '用户名' },
    {
      accessorKey: 'role',
      header: '角色',
      cell: ({ row }) =>
        row.original.role === 'admin' ? (
          <Badge>管理员</Badge>
        ) : (
          <Badge variant="secondary">普通用户</Badge>
        ),
    },
    {
      accessorKey: 'enabled',
      header: '状态',
      cell: ({ row }) =>
        row.original.enabled ? (
          <Badge variant="outline">启用</Badge>
        ) : (
          <Badge variant="destructive">已禁用</Badge>
        ),
    },
    {
      accessorKey: 'created_at',
      header: '创建时间',
      cell: ({ row }) => formatDateTime(row.original.created_at),
    },
    {
      id: 'actions',
      header: '操作',
      cell: ({ row }) => (
        <UserActions
          user={row.original}
          onChanged={() => {
            setActionError(null);
            reload();
          }}
          onError={setActionError}
        />
      ),
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="用户管理"
        description="管理用户角色与启停状态"
        actions={<CreateUserDialog onCreated={reload} />}
      />
      {actionError ? (
        <Alert variant="destructive">
          <AlertTitle>操作被拒绝</AlertTitle>
          <AlertDescription>{actionError}</AlertDescription>
        </Alert>
      ) : null}
      {error ? (
        <ErrorState message={error.message} onRetry={reload} />
      ) : (
        <DataTable
          columns={columns}
          data={data?.items ?? []}
          isLoading={loading}
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
