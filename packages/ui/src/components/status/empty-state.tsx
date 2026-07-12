// 空态占位：以居中卡片形式提示当前区域暂无数据，可选图标与操作入口
import type { ReactNode } from 'react';
import { InboxIcon } from 'lucide-react';

import { cn } from '../../lib/utils';

export interface EmptyStateProps {
  // 主标题：简述“空”的含义（如“暂无数据”）
  title: string;
  // 补充说明：可选，进一步解释或引导下一步
  description?: string;
  // 自定义图标：不传时用默认收件箱图标
  icon?: ReactNode;
  // 操作槽：可选，放置引导按钮等
  action?: ReactNode;
  // 追加类名
  className?: string;
}

// 空态：语义 role="status"，居中卡片式呈现
export function EmptyState({ title, description, icon, action, className }: EmptyStateProps) {
  return (
    <div
      role="status"
      className={cn(
        'flex flex-col items-center justify-center gap-3 rounded-lg border border-dashed bg-card p-8 text-center',
        className,
      )}
    >
      {/* 图标仅作装饰，对辅助技术隐藏 */}
      <span className="text-muted-foreground [&_svg]:size-10" aria-hidden="true">
        {icon ?? <InboxIcon />}
      </span>
      <div className="flex flex-col gap-1">
        <p className="text-sm font-medium text-foreground">{title}</p>
        {description ? <p className="text-sm text-muted-foreground">{description}</p> : null}
      </div>
      {action ? <div className="mt-1">{action}</div> : null}
    </div>
  );
}
