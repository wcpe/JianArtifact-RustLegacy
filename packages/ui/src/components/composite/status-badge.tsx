// 状态徽标：把可见性、仓库类型、任务状态映射为带语义色的 Badge
import { Badge } from '../ui/badge';
import { cn } from '../../lib/utils';

// 徽标语义色调；warning（琥珀）用于“部分完成”这类既非成功也非纯失败的中间态
type Tone = 'success' | 'error' | 'progress' | 'warning' | 'neutral';

export type StatusBadgeKind = 'visibility' | 'repo-type' | 'task-status';

export interface StatusBadgeProps {
  // 取值域：可见性 / 仓库类型 / 任务状态
  kind: StatusBadgeKind;
  // 原始英文值（如 public、hosted、running）
  value: string;
  // 追加类名
  className?: string;
}

// 各色调对应的软色背景与文字（含暗色变体），中性色复用主题语义令牌
const toneClass: Record<Tone, string> = {
  success:
    'border-transparent bg-emerald-100 text-emerald-700 dark:bg-emerald-950 dark:text-emerald-300',
  error: 'border-transparent bg-red-100 text-red-700 dark:bg-red-950 dark:text-red-300',
  progress: 'border-transparent bg-blue-100 text-blue-700 dark:bg-blue-950 dark:text-blue-300',
  warning: 'border-transparent bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-300',
  neutral: 'border-transparent bg-muted text-muted-foreground',
};

interface StatusMeta {
  // 中文标签
  label: string;
  // 语义色调
  tone: Tone;
}

// 可见性映射：公开可见走信息色，私有走中性色
const visibilityMap: Record<string, StatusMeta> = {
  public: { label: '公开', tone: 'progress' },
  private: { label: '私有', tone: 'neutral' },
};

// 仓库类型映射：三类以不同色调区分，便于扫读
const repoTypeMap: Record<string, StatusMeta> = {
  hosted: { label: '宿主', tone: 'progress' },
  proxy: { label: '代理', tone: 'warning' },
  group: { label: '聚合组', tone: 'neutral' },
};

// 任务状态映射（迁移任务十态全覆盖）
const taskStatusMap: Record<string, StatusMeta> = {
  queued: { label: '排队中', tone: 'neutral' },
  running: { label: '运行中', tone: 'progress' },
  pausing: { label: '暂停中', tone: 'progress' },
  paused: { label: '已暂停', tone: 'neutral' },
  cancelling: { label: '取消中', tone: 'progress' },
  cancelled: { label: '已取消', tone: 'neutral' },
  succeeded: { label: '全量成功', tone: 'success' },
  partial: { label: '部分完成', tone: 'warning' },
  failed: { label: '失败', tone: 'error' },
  interrupted: { label: '已中断', tone: 'error' },
};

// 按 kind 选择映射表
const mapByKind: Record<StatusBadgeKind, Record<string, StatusMeta>> = {
  visibility: visibilityMap,
  'repo-type': repoTypeMap,
  'task-status': taskStatusMap,
};

// 未知值统一回退为中性色，标签直接展示原始值
function resolveMeta(kind: StatusBadgeKind, value: string): StatusMeta {
  return mapByKind[kind][value] ?? { label: value, tone: 'neutral' };
}

// 状态徽标
export function StatusBadge({ kind, value, className }: StatusBadgeProps) {
  const meta = resolveMeta(kind, value);
  return (
    <Badge variant="outline" data-value={value} className={cn(toneClass[meta.tone], className)}>
      {meta.label}
    </Badge>
  );
}
