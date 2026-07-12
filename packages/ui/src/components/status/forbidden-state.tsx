// 无权限占位：提示当前用户无权访问该资源
import { LockIcon } from 'lucide-react';

import { cn } from '../../lib/utils';

export interface ForbiddenStateProps {
  // 提示文案：默认“无权访问此资源”
  message?: string;
  // 追加类名
  className?: string;
}

// 无权限态：语义 role="status"，含锁图标与说明文案
export function ForbiddenState({ message = '无权访问此资源', className }: ForbiddenStateProps) {
  return (
    <div
      role="status"
      className={cn(
        'flex flex-col items-center justify-center gap-3 rounded-lg border border-dashed bg-card p-8 text-center',
        className,
      )}
    >
      <LockIcon className="size-10 text-muted-foreground" aria-hidden="true" />
      <p className="text-sm font-medium text-foreground">{message}</p>
    </div>
  );
}
