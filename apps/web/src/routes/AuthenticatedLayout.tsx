// 登录后布局（对齐旧项目控制台外壳）：左上 logo（点击折叠）+ 分段导航 + 左下 footer（开源许可 + 折叠按钮）
// + 顶栏（搜索框 + 用户下拉菜单）+ 固定宽内容区。折叠时文字淡出（不逐项重排），Ctrl/Cmd+K 聚焦搜索。
import { useEffect, useRef, useState } from 'react';
import {
  Boxes,
  ChevronDown,
  KeyRound,
  LayoutDashboard,
  LogOut,
  PanelLeftClose,
  PanelLeftOpen,
  RotateCw,
  Scale,
  Search,
  Users,
  Workflow,
} from 'lucide-react';
import { NavLink, Outlet, useNavigate } from 'react-router-dom';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  cn,
} from '@jianartifact/ui';
import { useAuth } from '../auth/AuthContext';
import { useRefresh } from './RefreshContext';
import { ChangePasswordDialog } from './ChangePasswordDialog';

// 单个导航项定义
interface NavItem {
  to: string;
  label: string;
  icon: typeof Boxes;
  adminOnly?: boolean;
}

// 导航按语义分组
interface NavGroup {
  label: string;
  items: NavItem[];
}

const NAV_GROUPS: NavGroup[] = [
  {
    label: '浏览',
    items: [
      { to: '/dashboard', label: '仪表盘', icon: LayoutDashboard },
      { to: '/repos', label: '仓库', icon: Boxes },
    ],
  },
  {
    label: '管理',
    items: [
      { to: '/tokens', label: 'API Token', icon: KeyRound },
      { to: '/users', label: '用户管理', icon: Users, adminOnly: true },
      { to: '/migration', label: 'Nexus 迁移', icon: Workflow, adminOnly: true },
    ],
  },
];

// 品牌标记：Mantine 蓝方块 + 浅蓝盒子线框（制品盒隐喻，沿用旧项目标识）
function BrandMark({ size = 30 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden
      className="shrink-0"
    >
      <rect x="1.5" y="1.5" width="21" height="21" rx="5" fill="#228be6" />
      <path
        d="M12 5.5 L17.5 8.5 L12 11.5 L6.5 8.5 Z"
        stroke="#a5d8ff"
        strokeWidth="1.4"
        strokeLinejoin="round"
        fill="none"
      />
      <path
        d="M6.5 8.5 L6.5 15 L12 18 L17.5 15 L17.5 8.5"
        stroke="#a5d8ff"
        strokeWidth="1.4"
        strokeLinejoin="round"
        fill="none"
      />
      <path d="M12 11.5 L12 18" stroke="#a5d8ff" strokeWidth="1.4" />
    </svg>
  );
}

// 折叠动画共用：文字随折叠平滑收起并淡出（max-width + opacity，不触发逐项重排）
function collapsibleText(collapsed: boolean): string {
  return cn(
    'overflow-hidden whitespace-nowrap transition-[max-width,opacity] duration-200',
    collapsed ? 'max-w-0 opacity-0' : 'max-w-[160px] opacity-100',
  );
}

export function AuthenticatedLayout() {
  const { user, isAdmin, logout } = useAuth();
  const { refresh, refreshing, hasRefresh } = useRefresh();
  const navigate = useNavigate();
  // 侧栏折叠态：折叠后仅图标 rail（宽 64px），展开为完整宽（256px）
  const [collapsed, setCollapsed] = useState(false);
  // 修改口令模态框开合（由用户下拉菜单触发）
  const [pwdOpen, setPwdOpen] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);

  // Ctrl/Cmd+K 聚焦搜索框、Esc 失焦（全局搜索能力属后续里程碑，此处仅版式与聚焦）
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        searchRef.current?.focus();
      } else if (e.key === 'Escape' && document.activeElement === searchRef.current) {
        searchRef.current?.blur();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  const handleLogout = async () => {
    await logout();
    navigate('/login', { replace: true });
  };

  // 逐组过滤管理员专属项，过滤后为空的组不渲染
  const groups = NAV_GROUPS.map((group) => ({
    ...group,
    items: group.items.filter((item) => !item.adminOnly || isAdmin),
  })).filter((group) => group.items.length > 0);

  return (
    // 固定视口高、整体不滚动；导航与内容区各自内部滚动
    <div className="flex h-screen overflow-hidden">
      {/* 侧栏：满高，logo 顶 + 分组导航 + 左下 footer；折叠切换宽度、文字淡出 */}
      <aside
        aria-label="侧边导航"
        className={cn(
          'hidden shrink-0 flex-col overflow-hidden border-r bg-muted/30 transition-[width] duration-200 md:flex',
          collapsed ? 'w-16' : 'w-64',
        )}
      >
        {/* 品牌区：点击整体切换导航展开/收起 */}
        <button
          type="button"
          aria-label={collapsed ? '展开导航' : '收起导航'}
          onClick={() => setCollapsed((c) => !c)}
          className="flex items-center gap-2.5 px-4 py-4 text-left transition-colors hover:bg-muted"
        >
          <BrandMark />
          <div className={cn('leading-tight', collapsibleText(collapsed))}>
            <p className="text-[15px] font-semibold tracking-tight">JianArtifact</p>
            <p className="font-mono text-[10px] text-muted-foreground">v0.1.0</p>
          </div>
        </button>

        {/* 分组导航：可滚动 */}
        <nav aria-label="主导航" className="flex-1 overflow-y-auto px-2.5 py-1">
          {groups.map((group) => (
            <div key={group.label} className="mb-1">
              <p
                className={cn(
                  'px-3 pb-1 pt-3 font-mono text-[10px] uppercase tracking-wider text-muted-foreground/70 transition-opacity duration-200',
                  collapsed && 'opacity-0',
                )}
              >
                {group.label}
              </p>
              {group.items.map((item) => {
                const Icon = item.icon;
                return (
                  <NavLink
                    key={item.to}
                    to={item.to}
                    title={collapsed ? item.label : undefined}
                    className={({ isActive }) =>
                      cn(
                        'flex items-center gap-3 rounded-md px-3 py-2 text-sm transition-colors',
                        isActive
                          ? 'bg-primary/10 font-medium text-primary'
                          : 'text-foreground hover:bg-muted',
                      )
                    }
                  >
                    {({ isActive }) => (
                      <>
                        <Icon
                          className={cn(
                            'size-[18px] shrink-0',
                            isActive ? 'text-primary' : 'text-muted-foreground',
                          )}
                          aria-hidden
                        />
                        <span className={collapsibleText(collapsed)}>{item.label}</span>
                      </>
                    )}
                  </NavLink>
                );
              })}
            </div>
          ))}
        </nav>

        {/* 左下 footer：开源许可 + 折叠按钮；折叠态许可淡出、仅留切换按钮 */}
        <div className="flex items-center justify-between gap-2 border-t px-3 py-3">
          <a
            href="https://opensource.org/license/mit"
            target="_blank"
            rel="noreferrer"
            className={cn(
              'flex items-center gap-1.5 text-xs text-muted-foreground transition-colors hover:text-foreground',
              collapsibleText(collapsed),
            )}
          >
            <Scale className="size-3.5 shrink-0" aria-hidden />
            开源许可
          </a>
          <button
            type="button"
            aria-label={collapsed ? '展开导航' : '收起导航'}
            title={collapsed ? '展开导航' : '收起导航'}
            onClick={() => setCollapsed((c) => !c)}
            className={cn(
              'flex shrink-0 text-muted-foreground transition-colors hover:text-foreground',
              collapsed && 'mx-auto',
            )}
          >
            {collapsed ? (
              <PanelLeftOpen className="size-[18px]" aria-hidden />
            ) : (
              <PanelLeftClose className="size-[18px]" aria-hidden />
            )}
          </button>
        </div>
      </aside>

      {/* 右侧：顶栏 + 内容区 */}
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-14 shrink-0 items-center gap-4 border-b bg-card px-6">
          {/* 搜索框：Ctrl K 聚焦；全局搜索为后续里程碑能力，此处为版式与聚焦位 */}
          <div className="relative hidden w-full max-w-sm md:block">
            <Search
              className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground"
              aria-hidden
            />
            <input
              ref={searchRef}
              type="search"
              placeholder="搜索制品…"
              aria-label="搜索制品"
              className="h-9 w-full rounded-md border bg-muted/40 pr-16 pl-9 text-sm outline-none focus:border-ring focus:bg-background"
            />
            <kbd className="absolute top-1/2 right-2.5 -translate-y-1/2 rounded border bg-background px-1.5 font-mono text-[10px] text-muted-foreground">
              Ctrl K
            </kbd>
          </div>
          <div className="flex-1" />
          {/* 刷新当前内容页：无可刷新内容时禁用，刷新中图标旋转 */}
          <button
            type="button"
            aria-label="刷新当前页"
            title="刷新当前页"
            disabled={!hasRefresh || refreshing}
            onClick={refresh}
            className="flex size-9 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:opacity-40 disabled:hover:bg-transparent"
          >
            <RotateCw className={cn('size-4', refreshing && 'animate-spin')} aria-hidden />
          </button>
          <div className="mx-1 h-6 w-px bg-border" />
          {/* 用户下拉菜单：头像 + 用户名/角色 → 修改口令 / 退出登录 */}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                className="flex items-center gap-2.5 rounded-md px-2 py-1.5 transition-colors hover:bg-muted"
              >
                <div className="flex size-8 shrink-0 items-center justify-center rounded-full bg-primary text-sm font-semibold text-primary-foreground">
                  {user?.username?.[0]?.toUpperCase() ?? '?'}
                </div>
                <div className="hidden text-left leading-tight sm:block">
                  <p className="text-sm font-medium">{user?.username}</p>
                  <p className="text-[11px] text-muted-foreground">
                    {isAdmin ? '管理员' : '普通用户'}
                  </p>
                </div>
                <ChevronDown className="size-4 text-muted-foreground" aria-hidden />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-52">
              <DropdownMenuLabel>
                {user?.username}
                <span className="text-muted-foreground">（{isAdmin ? '管理员' : '普通用户'}）</span>
              </DropdownMenuLabel>
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={() => setPwdOpen(true)}>
                <KeyRound aria-hidden />
                修改口令
              </DropdownMenuItem>
              <DropdownMenuItem variant="destructive" onSelect={handleLogout}>
                <LogOut aria-hidden />
                退出登录
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </header>
        {/* 内容区：整体不滚动，页面在固定高度容器内各自内部滚动；内容最大宽居中 */}
        <main className="min-w-0 flex-1 overflow-hidden">
          <div className="mx-auto flex h-full max-w-[1280px] flex-col p-6 lg:p-8">
            <Outlet />
          </div>
        </main>
      </div>

      {/* 修改口令模态框（受控，由用户菜单触发） */}
      <ChangePasswordDialog open={pwdOpen} onOpenChange={setPwdOpen} />
    </div>
  );
}
