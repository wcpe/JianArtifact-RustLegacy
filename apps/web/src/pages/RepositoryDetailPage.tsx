// 仓库详情：信息卡 + 文件浏览 / 使用方式两个标签页。404 视为不存在或无权访问。
import { useCallback, useState, type ReactNode } from 'react';
import { useParams } from 'react-router-dom';
import { ChevronLeft, File, Folder } from 'lucide-react';
import type { components } from '@jianartifact/devmock/schema';
import {
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  DataTable,
  EmptyState,
  ErrorState,
  PageHeader,
  Skeleton,
  StatusBadge,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
  type ColumnDef,
} from '@jianartifact/ui';
import { repoApi } from '../api/endpoints';
import { useAsync } from '../lib/useAsync';
import { formatBytes, formatDateTime } from '../lib/format';

type RepoInfo = components['schemas']['RepoInfo'];
type FileEntry = components['schemas']['FileEntry'];

// 信息卡的一行「标签 - 值」
function InfoRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="text-sm">{children}</dd>
    </div>
  );
}

// 仓库信息卡：格式、类型、可见性、状态与（按类型）上游/成员
function RepoInfoCard({ repo }: { repo: RepoInfo }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>仓库信息</CardTitle>
      </CardHeader>
      <CardContent>
        <dl className="grid grid-cols-2 gap-4 sm:grid-cols-3">
          <InfoRow label="格式">{repo.format}</InfoRow>
          <InfoRow label="类型">
            <StatusBadge kind="repo-type" value={repo.repo_type} />
          </InfoRow>
          <InfoRow label="可见性">
            <StatusBadge kind="visibility" value={repo.visibility} />
          </InfoRow>
          <InfoRow label="状态">{repo.disabled ? '已停用' : '正常'}</InfoRow>
          <InfoRow label="创建时间">{formatDateTime(repo.created_at)}</InfoRow>
          {repo.repo_type === 'proxy' ? (
            <InfoRow label="上游地址">{repo.upstream_url ?? '—'}</InfoRow>
          ) : null}
          {repo.repo_type === 'group' ? (
            <InfoRow label="成员">{repo.members?.join('、') ?? '—'}</InfoRow>
          ) : null}
        </dl>
      </CardContent>
    </Card>
  );
}

// 文件浏览：目录逐级下钻，分页展示条目
function FilesBrowser({ name }: { name: string }) {
  const [path, setPath] = useState('');
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);

  const loader = useCallback(
    () => repoApi.files(name, { path, page, page_size: pageSize }),
    [name, path, page, pageSize],
  );
  const { data, error, loading, reload } = useAsync(loader);

  const enterDir = (dirName: string) => {
    setPath(path ? `${path}/${dirName}` : dirName);
    setPage(1);
  };
  const goUp = () => {
    const parts = path.split('/');
    parts.pop();
    setPath(parts.join('/'));
    setPage(1);
  };

  const columns: ColumnDef<FileEntry>[] = [
    {
      accessorKey: 'name',
      header: '名称',
      cell: ({ row }) =>
        row.original.is_dir ? (
          <button
            type="button"
            onClick={() => enterDir(row.original.name)}
            className="flex items-center gap-2 font-medium text-primary underline-offset-4 hover:underline"
          >
            <Folder className="size-4" aria-hidden />
            {row.original.name}
          </button>
        ) : (
          <span className="flex items-center gap-2">
            <File className="size-4 text-muted-foreground" aria-hidden />
            {row.original.name}
          </span>
        ),
    },
    {
      accessorKey: 'size',
      header: '大小',
      cell: ({ row }) => (row.original.is_dir ? '—' : formatBytes(row.original.size ?? 0)),
    },
    {
      accessorKey: 'updated_at',
      header: '更新时间',
      cell: ({ row }) => formatDateTime(row.original.updated_at),
    },
  ];

  if (error) {
    return <ErrorState message={error.message} onRetry={reload} />;
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center gap-2 text-sm text-muted-foreground">
        <Button variant="ghost" size="sm" disabled={path === ''} onClick={goUp}>
          <ChevronLeft className="size-4" aria-hidden />
          上级
        </Button>
        <span>当前路径：/{path}</span>
      </div>
      <DataTable
        columns={columns}
        data={data?.items ?? []}
        isLoading={loading}
        emptyState={<EmptyState title="空目录" description="该目录下暂无条目" />}
        page={page}
        pageSize={pageSize}
        total={data?.total ?? 0}
        onPageChange={setPage}
        onPageSizeChange={(size) => {
          setPageSize(size);
          setPage(1);
        }}
      />
    </div>
  );
}

// 使用方式：按格式罗列可复制的客户端配置片段
function UsagePanel({ name }: { name: string }) {
  const loader = useCallback(() => repoApi.usage(name), [name]);
  const { data, error, loading, reload } = useAsync(loader);

  if (loading) {
    return <Skeleton className="h-40 w-full" />;
  }
  if (error) {
    return <ErrorState message={error.message} onRetry={reload} />;
  }
  return (
    <div className="flex flex-col gap-4">
      {(data ?? []).map((snippet) => (
        <div key={snippet.title} className="flex flex-col gap-2">
          <h3 className="text-sm font-medium">{snippet.title}</h3>
          <pre className="overflow-x-auto rounded-md border bg-muted p-3 text-xs">
            {snippet.content}
          </pre>
        </div>
      ))}
    </div>
  );
}

export function RepositoryDetailPage() {
  const { name = '' } = useParams();
  const loader = useCallback(() => repoApi.detail(name), [name]);
  const { data: repo, error, loading, reload } = useAsync(loader);

  if (loading) {
    return <Skeleton className="h-64 w-full" />;
  }
  // 404：仓库不存在或对当前用户不可见（private 零存在性泄露）
  if (error?.status === 404) {
    return (
      <EmptyState title="仓库不存在或无权访问" description="请确认仓库名，或联系管理员授予权限" />
    );
  }
  if (error || !repo) {
    return <ErrorState message={error?.message ?? '加载仓库详情失败'} onRetry={reload} />;
  }

  return (
    <div className="flex flex-col gap-6">
      <PageHeader title={repo.name} description="仓库详情与文件浏览" />
      <RepoInfoCard repo={repo} />
      <Tabs defaultValue="files">
        <TabsList>
          <TabsTrigger value="files">文件浏览</TabsTrigger>
          <TabsTrigger value="usage">使用方式</TabsTrigger>
        </TabsList>
        <TabsContent value="files">
          <FilesBrowser name={repo.name} />
        </TabsContent>
        <TabsContent value="usage">
          <UsagePanel name={repo.name} />
        </TabsContent>
      </Tabs>
    </div>
  );
}
