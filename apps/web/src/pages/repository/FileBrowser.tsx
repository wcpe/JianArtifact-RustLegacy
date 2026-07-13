// 文件浏览（容器）：左侧懒加载文件树 + 右侧详情面板（M1-FR-019）。
// 面板化布局：左树固定宽 + 浅底 + 右分隔线，与右栏同容器等高、各自内部滚动；
// 目录仅在树内展开/折叠，右栏只在选中文件时展示文件详情（坐标卡 / 元数据 / 校验和 / 仓库接入）。
import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { Check, Copy, Download } from 'lucide-react';
import type { components } from '@jianartifact/devmock/schema';
import { Badge, ErrorState, Skeleton, cn } from '@jianartifact/ui';
import { ApiClientError } from '../../api/client';
import { repoApi } from '../../api/endpoints';
import { formatBytes, formatDateTime } from '../../lib/format';
import { FileTree, type TreeNode } from './FileTree';
import { UsagePanel } from './UsagePanel';
import {
  deriveMavenCoords,
  deriveNpmCoords,
  mavenDependencySnippets,
  npmInstallSnippets,
  type Snippet,
} from './coordinates';

type FileEntry = components['schemas']['FileEntry'];
type RepoFormat = components['schemas']['RepoInfo']['format'];

// 树的单目录拉取上限：取契约允许的最大每页条数，避免树内分页的复杂度
const TREE_PAGE_SIZE = 200;

// 格式协议端点基址（客户端片段占位，真实地址由部署环境决定）
function formatEndpointBase(name: string): string {
  return `https://<服务器地址>/repository/${name}`;
}

// 选中的文件：右侧展示元数据卡与校验和卡
interface SelectedFile {
  kind: 'file';
  name: string;
  path: string;
  size: number | null;
  sha256: string | null;
  sha1: string | null;
  md5: string | null;
  sha512: string | null;
  contentType: string | null;
  updatedAt: string | null;
}

// 把接口条目转成树节点（子目录未加载态）
function entryToNode(parentPath: string, entry: FileEntry): TreeNode {
  return {
    name: entry.name,
    path: parentPath ? `${parentPath}/${entry.name}` : entry.name,
    isDir: entry.is_dir,
    size: entry.size ?? null,
    sha256: entry.sha256 ?? null,
    sha1: entry.sha1 ?? null,
    md5: entry.md5 ?? null,
    sha512: entry.sha512 ?? null,
    contentType: entry.content_type ?? null,
    updatedAt: entry.updated_at ?? null,
    children: null,
    expanded: false,
    loading: false,
  };
}

// 不可变更新：按路径找到节点并应用补丁
function patchNode(nodes: TreeNode[], path: string, patch: Partial<TreeNode>): TreeNode[] {
  return nodes.map((node) => {
    if (node.path === path) return { ...node, ...patch };
    // 仅在目标位于该子树内时递归，避免全树遍历
    if (node.isDir && node.children && path.startsWith(`${node.path}/`)) {
      return { ...node, children: patchNode(node.children, path, patch) };
    }
    return node;
  });
}

// 按路径查找节点（供 URL 选中文件恢复用）
function findNode(nodes: TreeNode[], path: string): TreeNode | null {
  for (const node of nodes) {
    if (node.path === path) return node;
    if (node.isDir && node.children && path.startsWith(`${node.path}/`)) {
      const found = findNode(node.children, path);
      if (found) return found;
    }
  }
  return null;
}

// 树节点转右侧选中态（文件元数据）
function nodeToSelected(node: TreeNode): SelectedFile {
  return {
    kind: 'file',
    name: node.name,
    path: node.path,
    size: node.size,
    sha256: node.sha256,
    sha1: node.sha1,
    md5: node.md5,
    sha512: node.sha512,
    contentType: node.contentType,
    updatedAt: node.updatedAt,
  };
}

// 复制按钮：写入剪贴板并短暂反馈；label 供无障碍标注复制对象
function CopyButton({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      // 剪贴板不可用（权限/环境限制）时静默忽略，用户仍可手动选中复制
    }
  };
  return (
    <button
      type="button"
      aria-label={`复制 ${label}`}
      onClick={copy}
      className="text-muted-foreground hover:text-foreground shrink-0"
    >
      {copied ? <Check className="size-4" aria-hidden /> : <Copy className="size-4" aria-hidden />}
    </button>
  );
}

// 分区卡：统一的标题 + 内容容器（右栏各区块共用）
function Section({
  title,
  highlight = false,
  children,
}: {
  title: string;
  highlight?: boolean;
  children: ReactNode;
}) {
  return (
    <section
      className={cn(
        'flex flex-col gap-3 rounded-lg border p-4',
        highlight && 'border-primary/30 bg-primary/5',
      )}
    >
      <h4 className={cn('text-sm font-semibold', highlight && 'text-primary')}>{title}</h4>
      {children}
    </section>
  );
}

// 单条只读键值行（88px 标签 + 值）
function MetaRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="grid grid-cols-[88px_1fr] gap-x-4">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="text-sm break-all">{value}</dd>
    </div>
  );
}

// 片段页签：按工具切换可复制代码块（Maven/Gradle、npm/yarn/pnpm 等）
function SnippetTabs({ snippets }: { snippets: Snippet[] }) {
  const [active, setActive] = useState(0);
  const current = snippets[active] ?? snippets[0];
  if (!current) return null;
  return (
    <div className="flex flex-col gap-2">
      <div className="flex gap-4 border-b text-sm" role="tablist">
        {snippets.map((snippet, index) => (
          <button
            key={snippet.label}
            type="button"
            role="tab"
            aria-selected={index === active}
            onClick={() => setActive(index)}
            className={cn(
              'pb-2 transition-colors',
              index === active
                ? 'font-medium text-primary shadow-[inset_0_-2px_0] shadow-primary'
                : 'text-muted-foreground hover:text-foreground',
            )}
          >
            {snippet.label}
          </button>
        ))}
      </div>
      <div className="flex items-start gap-2">
        <pre className="min-w-0 flex-1 overflow-x-auto rounded-md border bg-muted p-3 font-mono text-xs">
          {current.content}
        </pre>
        <CopyButton label={current.label} value={current.content} />
      </div>
    </div>
  );
}

// 格式感知坐标卡：Maven → GAV + 依赖声明；npm → 包名版本 + 安装命令；其余格式或无法推导时不显示
function FormatCard({ format, name, path }: { format: RepoFormat; name: string; path: string }) {
  if (format === 'maven') {
    const coords = deriveMavenCoords(path);
    if (!coords) return null;
    return (
      <Section title="Maven 坐标" highlight>
        <dl className="flex flex-col gap-2">
          <MetaRow label="groupId" value={coords.groupId} />
          <MetaRow label="artifactId" value={coords.artifactId} />
          <MetaRow label="version" value={coords.version} />
          <MetaRow label="packaging" value={coords.packaging} />
        </dl>
        <SnippetTabs snippets={mavenDependencySnippets(coords)} />
      </Section>
    );
  }
  if (format === 'npm') {
    const coords = deriveNpmCoords(path);
    if (!coords) return null;
    return (
      <Section title="npm 包" highlight>
        <dl className="flex flex-col gap-2">
          <MetaRow label="包名" value={coords.name} />
          <MetaRow label="版本" value={coords.version} />
        </dl>
        <SnippetTabs snippets={npmInstallSnippets(coords, formatEndpointBase(name))} />
      </Section>
    );
  }
  return null;
}

// 文件详情头：路径标题 + 格式徽标 + 下载按钮（直连格式端点）
function FileHeader({
  format,
  name,
  file,
}: {
  format: RepoFormat;
  name: string;
  file: SelectedFile;
}) {
  return (
    <div className="flex flex-wrap items-center gap-3">
      <h3 className="min-w-0 flex-1 truncate text-lg font-semibold" title={file.path}>
        {file.path}
      </h3>
      <Badge variant="outline">{format.toUpperCase()}</Badge>
      <a
        href={`/repository/${name}/${file.path}`}
        download
        className="inline-flex items-center gap-1.5 rounded-md bg-primary px-3.5 py-1.5 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90"
      >
        <Download className="size-4" aria-hidden />
        下载
      </a>
    </div>
  );
}

// 校验和展示顺序：按格式惯例把生态常用算法排前（Maven → SHA-1/MD5；npm → SHA-512）
function checksumOrder(
  format: RepoFormat,
): Array<keyof Pick<SelectedFile, 'sha1' | 'md5' | 'sha256' | 'sha512'>> {
  if (format === 'maven') return ['sha1', 'md5', 'sha256', 'sha512'];
  if (format === 'npm') return ['sha512', 'sha1', 'sha256', 'md5'];
  return ['sha256', 'sha1', 'md5', 'sha512'];
}

const CHECKSUM_LABELS: Record<string, string> = {
  sha256: 'SHA-256',
  sha1: 'SHA-1',
  md5: 'MD5',
  sha512: 'SHA-512',
};

// 校验和卡：按格式惯例顺序逐行展示并可复制，缺失的算法行不显示
function ChecksumCard({ format, file }: { format: RepoFormat; file: SelectedFile }) {
  const rows = checksumOrder(format)
    .map((key) => ({ label: CHECKSUM_LABELS[key] ?? key, value: file[key] }))
    .filter((row): row is { label: string; value: string } => row.value !== null);
  if (rows.length === 0) return null;
  return (
    <Section title="校验和">
      <dl className="flex flex-col gap-2.5">
        {rows.map((row) => (
          <div key={row.label} className="flex items-center gap-3">
            <dt className="w-16 shrink-0 text-xs font-medium">{row.label}</dt>
            <dd className="flex min-w-0 flex-1 items-center gap-2">
              <code className="min-w-0 flex-1 truncate rounded-md border bg-muted px-2.5 py-1.5 font-mono text-xs">
                {row.value}
              </code>
              <CopyButton label={row.label} value={row.value} />
            </dd>
          </div>
        ))}
      </dl>
    </Section>
  );
}

// 文件浏览容器：左树右详情；选中文件受 URL（selectedPath）控制，点击经 onSelectFile 写 URL
export function FileBrowser({
  name,
  format,
  selectedPath,
  onSelectFile,
}: {
  name: string;
  format: RepoFormat;
  selectedPath: string | null;
  onSelectFile: (path: string | null) => void;
}) {
  const [roots, setRoots] = useState<TreeNode[] | null>(null);
  const [rootError, setRootError] = useState<string | null>(null);
  const [treeError, setTreeError] = useState<string | null>(null);
  const [selected, setSelected] = useState<SelectedFile | null>(null);
  const [rootNonce, setRootNonce] = useState(0);

  // 拉取一个目录的直接子项并转为树节点
  const loadChildren = useCallback(
    async (path: string): Promise<TreeNode[]> => {
      const result = await repoApi.files(name, { path, page_size: TREE_PAGE_SIZE });
      return result.items.map((entry) => entryToNode(path, entry));
    },
    [name],
  );

  // 加载根目录（仓库或重试变化时）
  useEffect(() => {
    let active = true;
    setRoots(null);
    setRootError(null);
    setSelected(null);
    loadChildren('')
      .then((nodes) => {
        if (active) setRoots(nodes);
      })
      .catch((err: unknown) => {
        if (active) setRootError(err instanceof ApiClientError ? err.message : '加载文件树失败');
      });
    return () => {
      active = false;
    };
  }, [loadChildren, rootNonce]);

  // 对树状态套用一次节点补丁（根未加载时不动）
  const patchRoots = useCallback((path: string, patch: Partial<TreeNode>) => {
    setRoots((current) => (current ? patchNode(current, path, patch) : current));
  }, []);

  // 展开目录节点：未加载先懒加载子项，失败提示且可重试（再次点击）
  const expandNode = useCallback(
    (node: TreeNode) => {
      if (node.children) {
        patchRoots(node.path, { expanded: true });
        return;
      }
      setTreeError(null);
      patchRoots(node.path, { loading: true });
      loadChildren(node.path)
        .then((children) => {
          patchRoots(node.path, { children, expanded: true, loading: false });
        })
        .catch((err: unknown) => {
          patchRoots(node.path, { loading: false });
          setTreeError(err instanceof ApiClientError ? err.message : '加载目录失败');
        });
    },
    [loadChildren, patchRoots],
  );

  // 从 URL 选中文件恢复：逐级展开路径上的目录，加载到位后选中文件节点（前进/后退与刷新可恢复）
  useEffect(() => {
    if (!selectedPath) {
      setSelected(null);
      return;
    }
    if (selected?.path === selectedPath) return;
    if (!roots) return;
    const target = findNode(roots, selectedPath);
    if (target && !target.isDir) {
      setSelected(nodeToSelected(target));
      return;
    }
    // 文件所在目录尚未加载：展开路径上第一个未展开的祖先目录，加载后本副作用重跑继续下钻
    const segments = selectedPath.split('/');
    let ancestor = '';
    for (let i = 0; i < segments.length - 1; i += 1) {
      ancestor = ancestor ? `${ancestor}/${segments[i]}` : (segments[i] ?? '');
      const dir = findNode(roots, ancestor);
      if (!dir) return;
      if (dir.isDir && !dir.expanded) {
        expandNode(dir);
        return;
      }
    }
  }, [selectedPath, roots, selected, expandNode]);

  // 树内点击目录：仅展开/折叠，不在右侧显示目录内容
  const handleDirClick = (node: TreeNode) => {
    if (node.expanded) {
      patchRoots(node.path, { expanded: false });
    } else {
      expandNode(node);
    }
  };

  // 树内点击文件：选中并写入 URL（selectedPath 由父级经 URL 回传）
  const handleFileClick = (node: TreeNode) => {
    setSelected(nodeToSelected(node));
    onSelectFile(node.path);
  };

  if (rootError) {
    return <ErrorState message={rootError} onRetry={() => setRootNonce((value) => value + 1)} />;
  }

  return (
    // 撑满浏览区（父级提供固定高度），左树与右详情各自内部滚动，页面本身不滚动
    <div className="flex h-full min-h-[420px] overflow-hidden rounded-lg border">
      {/* 左侧文件树：固定宽度 + 浅底 + 分隔线，独立滚动 */}
      <div className="w-[280px] shrink-0 overflow-y-auto border-r bg-muted/40 p-2">
        {treeError ? <p className="mb-2 px-2 text-sm text-destructive">{treeError}</p> : null}
        {roots ? (
          roots.length > 0 ? (
            <FileTree
              nodes={roots}
              selectedPath={selectedPath}
              onDirClick={handleDirClick}
              onFileClick={handleFileClick}
            />
          ) : (
            <p className="px-2 text-sm text-muted-foreground">仓库暂无条目</p>
          )
        ) : (
          <div className="flex flex-col gap-2 p-2">
            <Skeleton className="h-5 w-40" />
            <Skeleton className="h-5 w-32" />
            <Skeleton className="h-5 w-36" />
          </div>
        )}
      </div>

      {/* 右侧详情面板：仅在选中文件时展示文件详情，否则提示；独立滚动 */}
      <div className="min-w-0 flex-1 overflow-y-auto p-6">
        {selected ? (
          <div className="flex flex-col gap-5">
            <FileHeader format={format} name={name} file={selected} />
            <FormatCard format={format} name={name} path={selected.path} />
            <Section title="文件信息">
              <dl className="flex flex-col gap-2">
                <MetaRow label="所属仓库" value={name} />
                <MetaRow label="大小" value={formatBytes(selected.size)} />
                <MetaRow label="内容类型" value={selected.contentType ?? '—'} />
                <MetaRow label="更新时间" value={formatDateTime(selected.updatedAt)} />
              </dl>
            </Section>
            <ChecksumCard format={format} file={selected} />
            <Section title="仓库接入">
              <UsagePanel name={name} />
            </Section>
          </div>
        ) : (
          <div className="flex h-full items-center justify-center">
            <p className="text-sm text-muted-foreground">从左侧文件树选择一个文件查看详情</p>
          </div>
        )}
      </div>
    </div>
  );
}
