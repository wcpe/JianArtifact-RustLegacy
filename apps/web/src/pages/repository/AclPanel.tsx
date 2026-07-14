// 仓库权限面板（M1-FR-008）：ACL 授权列表 + 行内添加/移除授权。
// 权限判定以后端为准：非仓库管理员访问 ACL 端点被 403 时展示无权占位，不做前端预判。
import { useCallback, useState, type FormEvent } from 'react';
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
  ErrorState,
  ForbiddenState,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@jianartifact/ui';
import { ApiClientError } from '../../api/client';
import { repoApi, userApi } from '../../api/endpoints';
import { useAsync } from '../../lib/useAsync';

type AclEntry = components['schemas']['AclEntry'];
type Permission = components['schemas']['Permission'];

// 一次取满候选用户（契约每页上限 200；M1 规模下授权对象不分页）
const USER_PAGE_SIZE = 200;

// 权限徽标语义：读为中性、写为信息蓝、管理为琥珀，与状态徽标色系一致
const PERMISSION_META: Record<Permission, { label: string; className: string }> = {
  read: { label: '读', className: 'border-transparent bg-muted text-muted-foreground' },
  write: {
    label: '写',
    className: 'border-transparent bg-blue-100 text-blue-700 dark:bg-blue-950 dark:text-blue-300',
  },
  admin: {
    label: '管理',
    className:
      'border-transparent bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-300',
  },
};

// 权限徽标：read/write/admin → 读/写/管理
function PermissionBadge({ permission }: { permission: Permission }) {
  const meta = PERMISSION_META[permission];
  return (
    <Badge variant="outline" className={meta.className}>
      {meta.label}
    </Badge>
  );
}

// 添加授权行内表单：候选用户（过滤已授权者）+ 权限档位 + 添加
function AddAclForm({
  candidates,
  busy,
  onAdd,
}: {
  candidates: { id: number; username: string }[];
  busy: boolean;
  onAdd: (username: string, permission: Permission) => Promise<void>;
}) {
  const [username, setUsername] = useState('');
  const [permission, setPermission] = useState<Permission>('read');

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (username === '') return;
    await onAdd(username, permission);
    setUsername('');
  };

  return (
    <form className="flex flex-wrap items-end gap-3" onSubmit={handleSubmit}>
      <div className="flex flex-col gap-2">
        <Label htmlFor="acl-user">用户</Label>
        <Select value={username} onValueChange={setUsername}>
          <SelectTrigger id="acl-user" className="w-44">
            <SelectValue placeholder="选择用户" />
          </SelectTrigger>
          <SelectContent>
            {candidates.map((user) => (
              <SelectItem key={user.id} value={user.username}>
                {user.username}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="flex flex-col gap-2">
        <Label htmlFor="acl-permission">权限</Label>
        <Select value={permission} onValueChange={(value) => setPermission(value as Permission)}>
          <SelectTrigger id="acl-permission" className="w-32">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="read">读</SelectItem>
            <SelectItem value="write">写</SelectItem>
            <SelectItem value="admin">管理</SelectItem>
          </SelectContent>
        </Select>
      </div>
      <Button type="submit" disabled={busy || username === ''}>
        添加
      </Button>
    </form>
  );
}

// ACL 条目表：用户名 / 权限徽标 / 移除操作
function AclTable({
  entries,
  busy,
  onRemove,
}: {
  entries: AclEntry[];
  busy: boolean;
  onRemove: (entry: AclEntry) => void;
}) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead scope="col">用户名</TableHead>
          <TableHead scope="col">权限</TableHead>
          <TableHead scope="col">操作</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {entries.map((entry) => (
          <TableRow key={entry.user_id}>
            <TableCell className="font-medium">{entry.username}</TableCell>
            <TableCell>
              <PermissionBadge permission={entry.permission} />
            </TableCell>
            <TableCell>
              <Button variant="outline" size="sm" disabled={busy} onClick={() => onRemove(entry)}>
                移除
              </Button>
            </TableCell>
          </TableRow>
        ))}
        {entries.length === 0 ? (
          <TableRow>
            <TableCell colSpan={3} className="text-center text-muted-foreground">
              暂无授权；管理员对全部仓库拥有隐式权限
            </TableCell>
          </TableRow>
        ) : null}
      </TableBody>
    </Table>
  );
}

// 仓库权限面板
export function AclPanel({ repoName }: { repoName: string }) {
  const aclLoader = useCallback(() => repoApi.aclList(repoName), [repoName]);
  const acl = useAsync(aclLoader);
  const usersLoader = useCallback(() => userApi.list({ page_size: USER_PAGE_SIZE }), []);
  const users = useAsync(usersLoader);
  const [actionError, setActionError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // 统一执行一次 ACL 变更：成功后刷新列表，失败在面板顶部提示后端消息
  const runAction = async (action: () => Promise<unknown>) => {
    setActionError(null);
    setBusy(true);
    try {
      await action();
      acl.reload();
    } catch (err) {
      setActionError(err instanceof ApiClientError ? err.message : '操作失败');
    } finally {
      setBusy(false);
    }
  };

  // 后端 403：非仓库管理员，展示无权占位（不复制授权判定）
  if (acl.error?.status === 403) {
    return <ForbiddenState message="仅仓库管理员可查看与管理访问授权" />;
  }
  if (acl.error) {
    return <ErrorState message={acl.error.message} onRetry={acl.reload} />;
  }
  if (!acl.data) {
    return <Skeleton className="h-40 w-full" />;
  }

  const entries = acl.data;
  // 候选用户：过滤掉已有授权条目的用户
  const candidates = (users.data?.items ?? []).filter(
    (user) => !entries.some((entry) => entry.user_id === user.id),
  );

  return (
    <Card>
      <CardHeader>
        <CardTitle>访问授权</CardTitle>
        <CardDescription>
          仓库级 ACL：读 ⊂ 写 ⊂ 管理（向下包含）；私有仓库对未授权用户零存在性泄露。
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        {actionError ? (
          <Alert variant="destructive">
            <AlertTitle>操作被拒绝</AlertTitle>
            <AlertDescription>{actionError}</AlertDescription>
          </Alert>
        ) : null}
        <AddAclForm
          candidates={candidates}
          busy={busy}
          onAdd={(username, permission) =>
            runAction(() => repoApi.aclSet(repoName, username, permission))
          }
        />
        <AclTable
          entries={entries}
          busy={busy}
          onRemove={(entry) => void runAction(() => repoApi.aclRemove(repoName, entry.username))}
        />
      </CardContent>
    </Card>
  );
}
