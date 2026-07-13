// 仓库详情：返回链接 + 标题徽标组 + 浏览/配置/权限三标签页。
// 标签页与选中文件均以 URL 查询参数（?tab=&file=）为真源，支持前进/后退与刷新恢复；
// 404 视为不存在或无权访问（private 零存在性泄露）。
import { useCallback } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import {
  Badge,
  EmptyState,
  ErrorState,
  Skeleton,
  StatusBadge,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from '@jianartifact/ui';
import { repoApi } from '../api/endpoints';
import { useAsync } from '../lib/useAsync';
import { useRegisterRefresh } from '../routes/RefreshContext';
import { FileBrowser } from './repository/FileBrowser';
import { RepoConfigPanel } from './repository/RepoConfigPanel';
import { AclPanel } from './repository/AclPanel';
import type { components } from '@jianartifact/devmock/schema';

type RepoInfo = components['schemas']['RepoInfo'];

// 三个标签页标识，与 URL hash 一一对应（#browse/#config/#acl）
const TAB_IDS = ['browse', 'config', 'acl'] as const;
type TabId = (typeof TAB_IDS)[number];

// 标签页标识类型守卫
function isTabId(value: string): value is TabId {
  return (TAB_IDS as readonly string[]).includes(value);
}

// 详情页头：返回列表链接 + 仓库名大标题 + 徽标组（格式/类型/可见性/停用态）
function DetailHeader({ repo }: { repo: RepoInfo }) {
  return (
    <div className="flex flex-col gap-3">
      <Link
        to="/repos"
        className="flex w-fit items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
      >
        <ArrowLeft className="size-4" aria-hidden />
        返回仓库列表
      </Link>
      <div className="flex flex-wrap items-center gap-3">
        <h1 className="text-2xl font-semibold tracking-tight text-foreground">{repo.name}</h1>
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant="outline">{repo.format.toUpperCase()}</Badge>
          <StatusBadge kind="repo-type" value={repo.repo_type} />
          <StatusBadge kind="visibility" value={repo.visibility} />
          {repo.disabled ? <Badge variant="destructive">已停用</Badge> : null}
        </div>
      </div>
    </div>
  );
}

export function RepositoryDetailPage() {
  const { name = '' } = useParams();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const loader = useCallback(() => repoApi.detail(name), [name]);
  const { data: repo, error, reload } = useAsync(loader);
  useRegisterRefresh(reload);
  // 标签页与选中文件以 URL 查询参数为真源：切换/选中即入历史，前进后退与刷新可恢复
  const tabParam = searchParams.get('tab');
  const tab: TabId = tabParam && isTabId(tabParam) ? tabParam : 'browse';
  const filePath = searchParams.get('file');

  const changeTab = (value: string) => {
    if (!isTabId(value)) return;
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      next.set('tab', value);
      return next;
    });
  };

  // 选中文件：写入 URL 并切到浏览页签，push 历史以支持前进/后退在文件间导航
  const selectFile = (path: string | null) => {
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      next.set('tab', 'browse');
      if (path) next.set('file', path);
      else next.delete('file');
      return next;
    });
  };

  // 404：仓库不存在或对当前用户不可见（private 零存在性泄露）
  if (error?.status === 404) {
    return (
      <EmptyState title="仓库不存在或无权访问" description="请确认仓库名，或联系管理员授予权限" />
    );
  }
  if (error) {
    return <ErrorState message={error.message} onRetry={reload} />;
  }
  // 仅首次加载展示骨架；保存后的刷新沿用已有数据，避免整页闪烁与标签页状态丢失
  if (!repo) {
    return <Skeleton className="h-64 w-full" />;
  }

  return (
    <div className="flex h-full flex-col gap-5">
      <DetailHeader repo={repo} />
      <Tabs value={tab} onValueChange={changeTab} className="flex min-h-0 flex-1 flex-col gap-4">
        <TabsList className="shrink-0 self-start">
          <TabsTrigger value="browse">浏览</TabsTrigger>
          <TabsTrigger value="config">配置</TabsTrigger>
          <TabsTrigger value="acl">权限</TabsTrigger>
        </TabsList>
        {/* 浏览页签内部滚动（左树右详情各自滚），页面不整体滚 */}
        <TabsContent value="browse" className="min-h-0 flex-1 overflow-hidden">
          <FileBrowser
            name={repo.name}
            format={repo.format}
            selectedPath={filePath}
            onSelectFile={selectFile}
          />
        </TabsContent>
        <TabsContent value="config" className="min-h-0 flex-1 overflow-y-auto">
          <RepoConfigPanel repo={repo} onSaved={reload} onDeleted={() => navigate('/repos')} />
        </TabsContent>
        <TabsContent value="acl" className="min-h-0 flex-1 overflow-y-auto">
          <AclPanel repoName={repo.name} />
        </TabsContent>
      </Tabs>
    </div>
  );
}
