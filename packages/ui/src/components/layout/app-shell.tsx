// 页面骨架：左侧固定宽侧栏 + 顶栏 + 主内容区，窄屏隐藏侧栏
import type { ReactNode } from 'react';

export interface AppShellProps {
  // 侧栏内容（导航等）
  sidebar: ReactNode;
  // 顶栏内容：可选
  header?: ReactNode;
  // 主内容区
  children: ReactNode;
}

// 应用外壳布局：语义地标 aside / header / main，窄屏（< md）折叠侧栏仅留顶栏
export function AppShell({ sidebar, header, children }: AppShellProps) {
  return (
    <div className="flex min-h-svh w-full">
      <aside aria-label="侧边导航" className="hidden w-64 shrink-0 border-r bg-card md:block">
        {sidebar}
      </aside>
      <div className="flex min-w-0 flex-1 flex-col">
        {header ? (
          <header className="flex h-14 items-center border-b bg-card px-6">{header}</header>
        ) : null}
        <main className="min-w-0 flex-1 p-6">{children}</main>
      </div>
    </div>
  );
}
