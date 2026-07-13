// 文件树（展示层）：递归渲染目录/文件节点，目录支持展开/折叠，文件支持选中。
// 懒加载由容器（FileBrowser）负责，本组件只按节点状态渲染并回调交互。
import { ChevronDown, ChevronRight, FileText, Folder, FolderOpen } from 'lucide-react';
import { Skeleton, cn } from '@jianartifact/ui';

// 树节点：目录的 children 为 null 表示尚未加载（懒加载占位）
export interface TreeNode {
  // 条目名（不含路径）
  name: string;
  // 相对仓库根的完整路径
  path: string;
  // 是否目录
  isDir: boolean;
  // 文件大小（目录为 null）
  size: number | null;
  // 文件 sha256（目录为 null）
  sha256: string | null;
  // 文件 sha1（Maven 生态，缺失为 null）
  sha1: string | null;
  // 文件 md5（Maven 生态，缺失为 null）
  md5: string | null;
  // 文件 sha512（npm integrity，缺失为 null）
  sha512: string | null;
  // 内容类型（文件，缺失为 null）
  contentType: string | null;
  // 更新时间（目录为 null）
  updatedAt: string | null;
  // 子节点：null = 未加载，[] = 已加载且为空
  children: TreeNode[] | null;
  // 是否展开（仅目录有效）
  expanded: boolean;
  // 是否正在加载子节点
  loading: boolean;
}

interface FileTreeProps {
  // 根层节点
  nodes: TreeNode[];
  // 当前选中节点的完整路径（目录或文件）
  selectedPath: string | null;
  // 点击目录：由容器负责选中 + 展开/折叠（含懒加载）
  onDirClick: (node: TreeNode) => void;
  // 点击文件：由容器负责选中
  onFileClick: (node: TreeNode) => void;
}

// 单个节点行：目录带展开语义，文件可选中；按层级缩进
function TreeNodeRow({
  node,
  depth,
  selectedPath,
  onDirClick,
  onFileClick,
}: {
  node: TreeNode;
  depth: number;
} & Omit<FileTreeProps, 'nodes'>) {
  const selected = selectedPath === node.path;
  const Icon = node.isDir ? (node.expanded ? FolderOpen : Folder) : FileText;
  const Chevron = node.expanded ? ChevronDown : ChevronRight;
  return (
    <li
      role="treeitem"
      aria-expanded={node.isDir ? node.expanded : undefined}
      aria-selected={selected}
    >
      <button
        type="button"
        onClick={() => (node.isDir ? onDirClick(node) : onFileClick(node))}
        style={{ paddingLeft: `${depth * 16 + 4}px` }}
        className={cn(
          'flex w-full items-center gap-1.5 rounded-md py-1.5 pr-2 text-sm transition-colors',
          selected
            ? 'bg-accent font-medium text-accent-foreground'
            : 'text-foreground hover:bg-accent hover:text-accent-foreground',
        )}
      >
        {/* 目录带展开指示箭头；文件留同宽占位保证图标对齐 */}
        {node.isDir ? (
          <Chevron className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
        ) : (
          <span className="size-3.5 shrink-0" aria-hidden />
        )}
        <Icon
          className={cn(
            'size-4 shrink-0',
            node.isDir ? 'text-primary/70' : 'text-muted-foreground',
          )}
          aria-hidden
        />
        <span className="truncate">{node.name}</span>
      </button>
      {/* 加载中：展示占位骨架 */}
      {node.loading ? (
        <Skeleton className="my-1 h-4 w-24" style={{ marginLeft: `${(depth + 1) * 16 + 8}px` }} />
      ) : null}
      {/* 已展开且已加载：递归渲染子层 */}
      {node.isDir && node.expanded && node.children ? (
        <ul role="group">
          {node.children.map((child) => (
            <TreeNodeRow
              key={child.path}
              node={child}
              depth={depth + 1}
              selectedPath={selectedPath}
              onDirClick={onDirClick}
              onFileClick={onFileClick}
            />
          ))}
        </ul>
      ) : null}
    </li>
  );
}

// 文件树根：role="tree" 语义容器
export function FileTree({ nodes, selectedPath, onDirClick, onFileClick }: FileTreeProps) {
  return (
    <ul role="tree" aria-label="仓库文件树" className="flex flex-col gap-0.5">
      {nodes.map((node) => (
        <TreeNodeRow
          key={node.path}
          node={node}
          depth={0}
          selectedPath={selectedPath}
          onDirClick={onDirClick}
          onFileClick={onFileClick}
        />
      ))}
    </ul>
  );
}
