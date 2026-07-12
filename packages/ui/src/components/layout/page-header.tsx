// 页面标题区：一级标题 + 可选描述 + 右侧操作槽
import type { ReactNode } from 'react';

export interface PageHeaderProps {
  // 页面标题
  title: string;
  // 描述：可选
  description?: ReactNode;
  // 操作槽：可选，通常放主操作按钮
  actions?: ReactNode;
}

// 页面标题区
export function PageHeader({ title, description, actions }: PageHeaderProps) {
  return (
    <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
      <div className="flex flex-col gap-1">
        <h1 className="text-2xl font-semibold tracking-tight text-foreground">{title}</h1>
        {description ? <p className="text-sm text-muted-foreground">{description}</p> : null}
      </div>
      {actions ? <div className="flex shrink-0 items-center gap-2">{actions}</div> : null}
    </div>
  );
}
